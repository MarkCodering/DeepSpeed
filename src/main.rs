mod actions;
mod ai_engine;
mod config;
mod monitor;
mod optimizer;

use anyhow::Result;
use clap::{Parser, Subcommand};
use config::Config;
use monitor::SystemMonitor;
use optimizer::RuleOptimizer;
use ai_engine::AiEngine;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "deepspeed",
    about = "AI-powered macOS system optimizer for M2/8GB",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Path to config file (default: ~/.config/deepspeed/deepspeed.toml)
    #[arg(short, long)]
    config: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the background optimization daemon (default)
    Start,
    /// Print a one-shot system snapshot and exit
    Status,
    /// Print the effective config and exit
    Config,
    /// Install the launchd plist for auto-start on login
    Install,
    /// Uninstall the launchd plist
    Uninstall,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut cfg = Config::load()?;

    if let Some(path) = &cli.config {
        let content = std::fs::read_to_string(path)?;
        cfg = toml::from_str(&content)?;
    }

    // Bootstrap logging
    setup_logging(&cfg);

    match cli.command.unwrap_or(Commands::Start) {
        Commands::Start => run_daemon(cfg).await,
        Commands::Status => run_status(),
        Commands::Config => {
            println!("{}", toml::to_string_pretty(&cfg)?);
            Ok(())
        }
        Commands::Install => install_launchd(),
        Commands::Uninstall => uninstall_launchd(),
    }
}

// ── Daemon loop ────────────────────────────────────────────────────────────

async fn run_daemon(config: Config) -> Result<()> {
    info!(
        "DeepSpeed starting — monitor every {}s, AI enabled: {}",
        config.general.monitor_interval_secs,
        config.ai_enabled()
    );

    if config.ai_enabled() {
        info!(
            "AI engine active (model: {}, cooldown: {}s)",
            config.ai.model, config.ai.action_cooldown_secs
        );
    } else {
        info!("Running in rule-only mode (no API key set)");
    }

    let monitor = Arc::new(Mutex::new(SystemMonitor::new()));
    let optimizer = Arc::new(Mutex::new(RuleOptimizer::new(config.clone())));
    let ai_engine = Arc::new(Mutex::new(AiEngine::new(config.clone())));

    let monitor_interval = Duration::from_secs(config.general.monitor_interval_secs);
    let mut ticker = interval(monitor_interval);

    loop {
        ticker.tick().await;

        // Collect snapshot
        let snapshot = {
            let mut mon = monitor.lock().await;
            match mon.snapshot() {
                Ok(s) => s,
                Err(e) => {
                    error!("Failed to collect system snapshot: {}", e);
                    continue;
                }
            }
        };

        info!("{}", snapshot.summary());

        // Run rule-based optimizer
        let result = {
            let mut opt = optimizer.lock().await;
            opt.run(&snapshot)
        };

        for action in &result.actions_taken {
            info!("Rule action: {}", action);
        }

        // Escalate to AI only when rules signal it's needed
        if result.needs_ai {
            let ai_check = ai_engine.lock().await;
            if ai_check.can_call() {
                let snap_clone = snapshot.clone();
                drop(ai_check); // release lock before await

                let mut ai = ai_engine.lock().await;
                match ai.analyze(&snap_clone).await {
                    Ok(rec) => {
                        info!(
                            "AI (call #{}): {} [confidence={:.2}]",
                            ai.call_count(),
                            rec.summary,
                            rec.confidence
                        );
                        let opt = optimizer.lock().await;
                        let applied = ai.apply_recommendation(&rec, opt.executor(), &config);
                        for a in applied {
                            info!("{}", a);
                        }
                    }
                    Err(e) => {
                        warn!("AI analysis failed: {}", e);
                    }
                }
            } else {
                info!("AI escalation requested but engine is on cooldown — skipping");
            }
        }
    }
}

// ── Status subcommand ──────────────────────────────────────────────────────

fn run_status() -> Result<()> {
    let mut monitor = SystemMonitor::new();
    let snap = monitor.snapshot()?;

    println!("=== DeepSpeed System Status ===\n");
    println!("Memory:   {:.1}% used ({} MB used / {} MB total)", snap.memory.used_pct, snap.memory.used_mb, snap.memory.total_mb);
    println!("          {} MB available", snap.memory.available_mb);
    println!("          {} MB wired (kernel)", snap.memory.wired_mb);
    println!("          {} MB compressed", snap.memory.compressed_mb);
    println!("Swap:     {:.1}% used ({}/{} MB)", snap.swap.used_pct, snap.swap.used_mb, snap.swap.total_mb);
    println!("CPU:      {:.1}% ({} cores)", snap.cpu.usage_pct, snap.cpu.core_count);
    println!("Pressure: {:?}", snap.memory_pressure);
    println!("Thermal:  {:?}", snap.thermal_state);
    println!("\nTop processes by memory:");
    for (i, p) in snap.top_processes.iter().take(10).enumerate() {
        println!(
            "  {:2}. {:30} {:5} MB   {:5.1}% CPU   pid {}",
            i + 1,
            p.name,
            p.memory_mb,
            p.cpu_pct,
            p.pid,
        );
    }
    Ok(())
}

// ── launchd helpers ────────────────────────────────────────────────────────

fn install_launchd() -> Result<()> {
    let home = dirs::home_dir().expect("Cannot find home dir");
    let plist_dir = home.join("Library").join("LaunchAgents");
    std::fs::create_dir_all(&plist_dir)?;

    let exe_path = std::env::current_exe()?;
    let log_dir = home.join("Library").join("Logs");

    let plist_content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.deepspeed.daemon</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>start</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{log}/deepspeed.log</string>
    <key>StandardErrorPath</key>
    <string>{log}/deepspeed.log</string>
    <key>ProcessType</key>
    <string>Background</string>
    <key>LowPriorityIO</key>
    <true/>
    <key>ThrottleInterval</key>
    <integer>10</integer>
</dict>
</plist>
"#,
        exe = exe_path.display(),
        log = log_dir.display(),
    );

    let plist_path = plist_dir.join("com.deepspeed.daemon.plist");
    std::fs::write(&plist_path, plist_content)?;

    // Load it now
    let status = std::process::Command::new("launchctl")
        .args(["load", "-w", plist_path.to_str().unwrap()])
        .status()?;

    if status.success() {
        println!("DeepSpeed installed and started.");
        println!("Plist: {:?}", plist_path);
        println!("Log:   {:?}", log_dir.join("deepspeed.log"));
    } else {
        eprintln!("launchctl load failed — plist written but not loaded.");
    }
    Ok(())
}

fn uninstall_launchd() -> Result<()> {
    let home = dirs::home_dir().expect("Cannot find home dir");
    let plist_path = home
        .join("Library")
        .join("LaunchAgents")
        .join("com.deepspeed.daemon.plist");

    if plist_path.exists() {
        std::process::Command::new("launchctl")
            .args(["unload", "-w", plist_path.to_str().unwrap()])
            .status()
            .ok();
        std::fs::remove_file(&plist_path)?;
        println!("DeepSpeed uninstalled.");
    } else {
        println!("DeepSpeed plist not found — already uninstalled?");
    }
    Ok(())
}

// ── Logging setup ──────────────────────────────────────────────────────────

fn setup_logging(config: &Config) {
    let log_level = &config.general.log_level;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(log_level));

    let log_path = config.log_path();
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    // Try file appender; fall back to stderr
    match tracing_appender::rolling::never(
        log_path.parent().unwrap_or_else(|| std::path::Path::new("/tmp")),
        log_path.file_name().unwrap_or_else(|| std::ffi::OsStr::new("deepspeed.log")),
    ) {
        appender => {
            let (non_blocking, _guard) = tracing_appender::non_blocking(appender);
            // Keep _guard alive by leaking it (daemon runs forever)
            std::mem::forget(_guard);
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(non_blocking)
                .with_ansi(false)
                .init();
        }
    }
}
