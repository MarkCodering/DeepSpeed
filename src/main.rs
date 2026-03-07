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
    about = "AI-powered system optimizer — macOS, Linux, Windows",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Path to config file
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
    /// Install as a login daemon (launchd / systemd / Task Scheduler)
    Install,
    /// Uninstall the daemon
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

    setup_logging(&cfg);

    match cli.command.unwrap_or(Commands::Start) {
        Commands::Start => run_daemon(cfg).await,
        Commands::Status => run_status(),
        Commands::Config => {
            println!("{}", toml::to_string_pretty(&cfg)?);
            Ok(())
        }
        Commands::Install => install_daemon(),
        Commands::Uninstall => uninstall_daemon(),
    }
}

// ── Daemon loop ────────────────────────────────────────────────────────────

async fn run_daemon(config: Config) -> Result<()> {
    info!(
        "DeepSpeed starting — OS: {}, monitor every {}s, AI enabled: {}",
        std::env::consts::OS,
        config.general.monitor_interval_secs,
        config.ai_enabled()
    );

    if config.ai_enabled() {
        info!(
            "AI engine active (model: {}, cooldown: {}s)",
            config.ai.model, config.ai.action_cooldown_secs
        );
    } else {
        info!("Running in rule-only mode (set ANTHROPIC_API_KEY to enable AI)");
    }

    let monitor = Arc::new(Mutex::new(SystemMonitor::new()));
    let optimizer = Arc::new(Mutex::new(RuleOptimizer::new(config.clone())));
    let ai_engine = Arc::new(Mutex::new(AiEngine::new(config.clone())));

    let monitor_interval = Duration::from_secs(config.general.monitor_interval_secs);
    let mut ticker = interval(monitor_interval);

    loop {
        ticker.tick().await;

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

        let result = {
            let mut opt = optimizer.lock().await;
            opt.run(&snapshot)
        };

        for action in &result.actions_taken {
            info!("Rule action: {}", action);
        }

        if result.needs_ai {
            let ai_check = ai_engine.lock().await;
            if ai_check.can_call() {
                let snap_clone = snapshot.clone();
                drop(ai_check);

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
                    Err(e) => warn!("AI analysis failed: {}", e),
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
    println!("OS:       {} ({})", std::env::consts::OS, std::env::consts::ARCH);
    println!(
        "Memory:   {:.1}% used ({} MB used / {} MB total)",
        snap.memory.used_pct, snap.memory.used_mb, snap.memory.total_mb
    );
    println!("          {} MB available", snap.memory.available_mb);
    if snap.memory.kernel_mb > 0 {
        println!("          {} MB kernel/wired", snap.memory.kernel_mb);
    }
    if snap.memory.cached_mb > 0 {
        println!("          {} MB compressed/cached", snap.memory.cached_mb);
    }
    println!(
        "Swap:     {:.1}% used ({}/{} MB)",
        snap.swap.used_pct, snap.swap.used_mb, snap.swap.total_mb
    );
    println!("CPU:      {:.1}% ({} cores)", snap.cpu.usage_pct, snap.cpu.core_count);
    println!("Pressure: {:?}", snap.memory_pressure);
    println!("Thermal:  {:?}", snap.thermal_state);
    println!("\nTop processes by memory:");
    for (i, p) in snap.top_processes.iter().take(10).enumerate() {
        println!(
            "  {:2}. {:30} {:5} MB   {:5.1}% CPU   pid {}",
            i + 1, p.name, p.memory_mb, p.cpu_pct, p.pid,
        );
    }
    Ok(())
}

// ── Daemon install / uninstall (platform-specific) ────────────────────────

#[cfg(target_os = "macos")]
fn install_daemon() -> Result<()> {
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

    let status = std::process::Command::new("launchctl")
        .args(["load", "-w", plist_path.to_str().unwrap()])
        .status()?;

    if status.success() {
        println!("DeepSpeed installed and started (launchd).");
        println!("Plist: {}", plist_path.display());
        println!("Log:   {}", log_dir.join("deepspeed.log").display());
    } else {
        eprintln!("launchctl load failed — plist written but not loaded.");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn uninstall_daemon() -> Result<()> {
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
        println!("DeepSpeed uninstalled (launchd agent removed).");
    } else {
        println!("No launchd plist found — already uninstalled?");
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn install_daemon() -> Result<()> {
    let exe_path = std::env::current_exe()?;
    let service_dir = dirs::home_dir()
        .expect("Cannot find home dir")
        .join(".config")
        .join("systemd")
        .join("user");
    std::fs::create_dir_all(&service_dir)?;

    let service_content = format!(
        r#"[Unit]
Description=DeepSpeed AI System Optimizer
After=default.target

[Service]
Type=simple
ExecStart={exe} start
Restart=always
RestartSec=10
Nice=10
IOSchedulingClass=idle

[Install]
WantedBy=default.target
"#,
        exe = exe_path.display()
    );

    let service_path = service_dir.join("deepspeed.service");
    std::fs::write(&service_path, service_content)?;

    // Enable and start
    let enable = std::process::Command::new("systemctl")
        .args(["--user", "enable", "--now", "deepspeed"])
        .status()?;

    if enable.success() {
        println!("DeepSpeed installed and started (systemd user service).");
        println!("Service: {}", service_path.display());
        println!("Logs:    journalctl --user -u deepspeed -f");
    } else {
        eprintln!("systemctl enable failed — service file written but not enabled.");
        eprintln!("Run manually: systemctl --user enable --now deepspeed");
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn uninstall_daemon() -> Result<()> {
    std::process::Command::new("systemctl")
        .args(["--user", "disable", "--now", "deepspeed"])
        .status()
        .ok();

    let service_path = dirs::home_dir()
        .expect("Cannot find home dir")
        .join(".config")
        .join("systemd")
        .join("user")
        .join("deepspeed.service");

    if service_path.exists() {
        std::fs::remove_file(&service_path)?;
        println!("DeepSpeed uninstalled (systemd service removed).");
    } else {
        println!("No systemd service found — already uninstalled?");
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn install_daemon() -> Result<()> {
    let exe_path = std::env::current_exe()?;
    let exe_str = exe_path.to_string_lossy().replace('\'', "\"");

    let script = format!(
        r#"
        $action   = New-ScheduledTaskAction -Execute '{exe}' -Argument 'start'
        $trigger  = New-ScheduledTaskTrigger -AtLogOn
        $principal = New-ScheduledTaskPrincipal `
                        -UserId $env:USERNAME `
                        -LogonType Interactive `
                        -RunLevel Limited
        $settings = New-ScheduledTaskSettingsSet `
                        -ExecutionTimeLimit 0 `
                        -RestartCount 3 `
                        -RestartInterval (New-TimeSpan -Minutes 1) `
                        -Priority 7
        Register-ScheduledTask `
            -TaskName 'DeepSpeed' `
            -Action $action `
            -Trigger $trigger `
            -Principal $principal `
            -Settings $settings `
            -Force
        Start-ScheduledTask -TaskName 'DeepSpeed'
        Write-Host 'DeepSpeed installed and started (Task Scheduler).'
        "#,
        exe = exe_str
    );

    let status = std::process::Command::new("powershell")
        .args(["-NonInteractive", "-Command", &script])
        .status()?;

    if !status.success() {
        eprintln!("Task Scheduler registration failed.");
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn uninstall_daemon() -> Result<()> {
    let script = r#"
        Stop-ScheduledTask -TaskName 'DeepSpeed' -ErrorAction SilentlyContinue
        Unregister-ScheduledTask -TaskName 'DeepSpeed' -Confirm:$false -ErrorAction SilentlyContinue
        Write-Host 'DeepSpeed uninstalled (Task Scheduler task removed).'
    "#;
    std::process::Command::new("powershell")
        .args(["-NonInteractive", "-Command", script])
        .status()?;
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn install_daemon() -> Result<()> {
    eprintln!("Auto-install not supported on this platform. Run `deepspeed start` manually.");
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn uninstall_daemon() -> Result<()> {
    eprintln!("Auto-uninstall not supported on this platform.");
    Ok(())
}

// ── Logging setup ──────────────────────────────────────────────────────────

fn setup_logging(config: &Config) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&config.general.log_level));

    let log_path = config.log_path();
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let log_dir = log_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let log_file = log_path
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("deepspeed.log"));

    let appender = tracing_appender::rolling::never(log_dir, log_file);
    let (non_blocking, guard) = tracing_appender::non_blocking(appender);
    std::mem::forget(guard); // daemon runs forever

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(non_blocking)
        .with_ansi(false)
        .init();
}
