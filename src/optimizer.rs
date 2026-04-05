/// Rule-based optimizer: runs every monitor cycle, zero API cost.
/// AI engine is only escalated when rules alone can't resolve the situation.
use crate::actions::{is_protected, select_renice_candidates, Action, ActionExecutor};
use crate::config::Config;
use crate::monitor::{MemoryPressureLevel, SystemSnapshot};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::{info, warn};

pub struct RuleOptimizer {
    config: Config,
    executor: ActionExecutor,
    /// Tracks last time an action was taken, keyed by action type string
    action_cooldowns: HashMap<String, Instant>,
    /// Consecutive cycles where memory has been critical
    critical_cycles: u32,
    /// Whether we've already asked AI to escalate in the current critical window
    pub ai_escalated: bool,
}

#[derive(Debug)]
pub struct OptimizerResult {
    pub actions_taken: Vec<String>,
    /// True when the situation exceeds what rules can handle — caller should invoke AI
    pub needs_ai: bool,
}

impl RuleOptimizer {
    pub fn new(config: Config) -> Self {
        let executor = ActionExecutor::new(config.clone());
        Self {
            config,
            executor,
            action_cooldowns: HashMap::new(),
            critical_cycles: 0,
            ai_escalated: false,
        }
    }

    pub fn run(&mut self, snap: &SystemSnapshot) -> OptimizerResult {
        let mut actions_taken = Vec::new();
        let mut needs_ai = false;

        let mem_pct = snap.memory.used_pct;
        let swap_pct = snap.swap.used_pct;
        // Clone threshold values to avoid holding an immutable borrow across mutable calls
        let mem_warn = self.config.thresholds.memory_warning_pct;
        let mem_crit = self.config.thresholds.memory_critical_pct;
        let swap_warn = self.config.thresholds.swap_warning_pct;
        let swap_crit = self.config.thresholds.swap_critical_pct;
        let heavy_mb = self.config.thresholds.process_heavy_memory_mb;
        let _cpu_warn = self.config.thresholds.cpu_warning_pct;
        let min_age = self.config.actions.min_process_age_secs;
        let renice_val = self.config.actions.renice_value;
        let allow_purge = self.config.actions.allow_purge_cache;
        let protected = self.config.actions.protected_processes.clone();

        // ── Track critical cycles ──────────────────────────────────────────
        if snap.memory_pressure == MemoryPressureLevel::Critical
            || mem_pct >= mem_crit
        {
            self.critical_cycles += 1;
        } else {
            // Pressure resolved, reset escalation flag
            if self.critical_cycles > 0 {
                info!("Memory pressure resolved after {} cycles", self.critical_cycles);
            }
            self.critical_cycles = 0;
            self.ai_escalated = false;
        }

        // ── Memory warning: renice heavy processes ─────────────────────────
        if mem_pct >= mem_warn || swap_pct >= swap_warn {
            let candidates = select_renice_candidates(
                &snap.top_processes,
                heavy_mb,
                min_age,
                &protected,
            );

            for proc in candidates {
                let key = format!("renice:{}", proc.pid);
                if self.is_on_cooldown(&key, Duration::from_secs(600)) {
                    continue;
                }
                let action = Action::ReniceProcess {
                    pid: proc.pid,
                    name: proc.name.clone(),
                    nice: renice_val,
                };
                match self.executor.execute(&action) {
                    Ok(true) => {
                        let msg = format!("Reniced '{}' ({}MB, pid {})", proc.name, proc.memory_mb, proc.pid);
                        info!("{}", msg);
                        actions_taken.push(msg);
                        self.record_action(&key);
                    }
                    Ok(false) => {}
                    Err(e) => warn!("Renice error: {}", e),
                }
            }
        }

        // ── Memory critical: purge cache if allowed ────────────────────────
        if mem_pct >= mem_crit || snap.memory_pressure == MemoryPressureLevel::Critical {
            let key = "purge_cache".to_string();
            if allow_purge && !self.is_on_cooldown(&key, Duration::from_secs(300)) {
                match self.executor.execute(&Action::PurgeCache) {
                    Ok(true) => {
                        actions_taken.push("Purged disk cache".to_string());
                        self.record_action(&key);
                    }
                    _ => {}
                }
            }

            // Notify user on first critical cycle
            if self.critical_cycles == 1 {
                let msg = format!(
                    "Memory at {:.0}% ({} MB free). Swap: {:.0}% used.",
                    mem_pct,
                    snap.memory.available_mb,
                    swap_pct,
                );
                self.executor.execute(&Action::Notify {
                    title: "High Memory Pressure".to_string(),
                    message: msg.clone(),
                }).ok();
                actions_taken.push(format!("Notified: {}", msg));
            }

            // Escalate to AI after 3 consecutive critical cycles (90s) if not already done
            if self.critical_cycles >= 3 && !self.ai_escalated && self.config.ai_enabled() {
                warn!(
                    "Memory critical for {} cycles — escalating to AI",
                    self.critical_cycles
                );
                needs_ai = true;
                self.ai_escalated = true;
            }
        }

        // ── CPU sustained high: lower IO priority of CPU-heavy processes ───
        if snap.cpu.usage_pct >= _cpu_warn {
            for proc in &snap.top_processes {
                if proc.cpu_pct >= 80.0
                    && proc.run_time_secs >= min_age
                    && !is_protected(&proc.name, &protected)
                    && proc.io_priority != "idle"
                {
                    let key = format!("ionice:{}", proc.pid);
                    if self.is_on_cooldown(&key, Duration::from_secs(600)) {
                        continue;
                    }
                    let action = Action::IoNice {
                        pid: proc.pid,
                        name: proc.name.clone(),
                        class: "idle".to_string(),
                    };
                    match self.executor.execute(&action) {
                        Ok(true) => {
                            let msg = format!("Set IO priority idle for '{}' (cpu={:.1}%, pid {})", proc.name, proc.cpu_pct, proc.pid);
                            info!("{}", msg);
                            actions_taken.push(msg);
                            self.record_action(&key);
                        }
                        Ok(false) => {}
                        Err(e) => warn!("ionice error: {}", e),
                    }
                }
            }
        }

        // ── Swap critical: always escalate to AI (unusual situation) ────────
        if swap_pct >= swap_crit && !self.ai_escalated && self.config.ai_enabled() {
            warn!("Swap at {:.0}% — escalating to AI", swap_pct);
            needs_ai = true;
            self.ai_escalated = true;
        }

        if !actions_taken.is_empty() || needs_ai {
            info!(
                "Optimizer: {} action(s) taken, needs_ai={}",
                actions_taken.len(),
                needs_ai
            );
        }

        OptimizerResult { actions_taken, needs_ai }
    }

    fn is_on_cooldown(&self, key: &str, duration: Duration) -> bool {
        self.action_cooldowns
            .get(key)
            .map(|t| t.elapsed() < duration)
            .unwrap_or(false)
    }

    fn record_action(&mut self, key: &str) {
        self.action_cooldowns.insert(key.to_string(), Instant::now());
    }

    pub fn executor(&self) -> &ActionExecutor {
        &self.executor
    }
}
