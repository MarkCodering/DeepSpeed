use crate::config::Config;
use crate::monitor::ProcessInfo;
use anyhow::Result;
use std::process::Command;
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub enum Action {
    ReniceProcess { pid: u32, name: String, nice: i32 },
    PurgeCache,
    #[allow(dead_code)]
    KillProcess { pid: u32, name: String },
    Notify { title: String, message: String },
}

pub struct ActionExecutor {
    config: Config,
}

impl ActionExecutor {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub fn execute(&self, action: &Action) -> Result<bool> {
        match action {
            Action::ReniceProcess { pid, name, nice } => self.renice(*pid, name, *nice),
            Action::PurgeCache => self.purge_cache(),
            Action::KillProcess { pid, name } => self.kill_process(*pid, name),
            Action::Notify { title, message } => {
                self.send_notification(title, message);
                Ok(true)
            }
        }
    }

    fn renice(&self, pid: u32, name: &str, nice: i32) -> Result<bool> {
        if !self.config.actions.allow_renice {
            return Ok(false);
        }

        info!("Renicing process {} (pid {}) to nice={}", name, pid, nice);
        let status = Command::new("renice")
            .args([nice.to_string().as_str(), "-p", &pid.to_string()])
            .status()?;

        if status.success() {
            info!("Reniced {} (pid {}) to nice={}", name, pid, nice);
            Ok(true)
        } else {
            warn!("Failed to renice {} (pid {})", name, pid);
            Ok(false)
        }
    }

    fn purge_cache(&self) -> Result<bool> {
        if !self.config.actions.allow_purge_cache {
            warn!("Cache purge requested but disabled in config (allow_purge_cache = false)");
            return Ok(false);
        }
        info!("Purging disk cache");
        let status = Command::new("purge").status()?;
        Ok(status.success())
    }

    fn kill_process(&self, pid: u32, name: &str) -> Result<bool> {
        warn!("Kill requested for {} (pid {}), skipping — not auto-killing processes", name, pid);
        // We never auto-kill; only notify the user
        Ok(false)
    }

    pub fn send_notification(&self, title: &str, message: &str) {
        if !self.config.general.notifications_enabled {
            return;
        }
        let script = format!(
            "display notification \"{}\" with title \"DeepSpeed\" subtitle \"{}\"",
            message.replace('"', "'"),
            title.replace('"', "'"),
        );
        if let Err(e) = Command::new("osascript").args(["-e", &script]).status() {
            warn!("Failed to send notification: {}", e);
        }
    }
}

/// Determine if a process should be protected from any action
pub fn is_protected(name: &str, protected: &[String]) -> bool {
    // Always protect core system processes
    let always_protected = [
        "kernel_task",
        "launchd",
        "WindowServer",
        "loginwindow",
        "SystemUIServer",
        "Dock",
        "Finder",
        "coreaudiod",
        "bluetoothd",
        "configd",
        "notifyd",
        "deepspeed",
        "mds",
        "mds_stores",
    ];

    if always_protected.iter().any(|p| name.eq_ignore_ascii_case(p)) {
        return true;
    }
    protected.iter().any(|p| name.eq_ignore_ascii_case(p))
}

/// Pick the best candidate processes to renice
pub fn select_renice_candidates(
    processes: &[ProcessInfo],
    heavy_threshold_mb: u64,
    min_age_secs: u64,
    protected: &[String],
) -> Vec<ProcessInfo> {
    processes
        .iter()
        .filter(|p| {
            p.memory_mb >= heavy_threshold_mb
                && p.run_time_secs >= min_age_secs
                && p.nice == 0
                && !is_protected(&p.name, protected)
        })
        .cloned()
        .collect()
}
