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

    // ── Renice / priority lowering ─────────────────────────────────────────

    fn renice(&self, pid: u32, name: &str, nice: i32) -> Result<bool> {
        if !self.config.actions.allow_renice {
            return Ok(false);
        }
        info!("Lowering priority of '{}' (pid {})", name, pid);
        renice_process(pid, name, nice)
    }

    // ── Cache purge ────────────────────────────────────────────────────────

    fn purge_cache(&self) -> Result<bool> {
        if !self.config.actions.allow_purge_cache {
            warn!("Cache purge requested but disabled in config (allow_purge_cache = false)");
            return Ok(false);
        }
        purge_cache_impl()
    }

    fn kill_process(&self, pid: u32, name: &str) -> Result<bool> {
        warn!("Kill requested for '{}' (pid {}) — skipping, DeepSpeed never auto-kills", name, pid);
        Ok(false)
    }

    // ── Notifications ──────────────────────────────────────────────────────

    pub fn send_notification(&self, title: &str, message: &str) {
        if !self.config.general.notifications_enabled {
            return;
        }
        send_notification_impl(title, message);
    }
}

// ── Platform-specific: renice ──────────────────────────────────────────────

#[cfg(unix)]
fn renice_process(pid: u32, name: &str, nice: i32) -> Result<bool> {
    let status = Command::new("renice")
        .args([nice.to_string().as_str(), "-p", &pid.to_string()])
        .status()?;
    if status.success() {
        info!("Reniced '{}' (pid {}) to nice={}", name, pid, nice);
        Ok(true)
    } else {
        warn!("renice failed for '{}' (pid {})", name, pid);
        Ok(false)
    }
}

#[cfg(target_os = "windows")]
fn renice_process(pid: u32, name: &str, nice: i32) -> Result<bool> {
    // Map Unix nice value to Windows priority class
    let priority_class = if nice >= 10 { "Idle" } else { "BelowNormal" };
    let script = format!(
        "(Get-Process -Id {pid}).PriorityClass = '{cls}'",
        pid = pid,
        cls = priority_class,
    );
    let status = Command::new("powershell")
        .args(["-NonInteractive", "-Command", &script])
        .status()?;
    if status.success() {
        info!("Set '{}' (pid {}) priority to {}", name, pid, priority_class);
        Ok(true)
    } else {
        warn!("Failed to set priority for '{}' (pid {})", name, pid);
        Ok(false)
    }
}

// ── Platform-specific: cache purge ────────────────────────────────────────

#[cfg(target_os = "macos")]
fn purge_cache_impl() -> Result<bool> {
    info!("Purging disk cache (macOS purge)");
    let status = Command::new("purge").status()?;
    Ok(status.success())
}

#[cfg(target_os = "linux")]
fn purge_cache_impl() -> Result<bool> {
    // Requires root or passwordless sudo for tee /proc/sys/vm/drop_caches
    info!("Dropping page cache (Linux)");
    let sync_ok = Command::new("sync").status().map(|s| s.success()).unwrap_or(false);
    if !sync_ok {
        warn!("sync failed before drop_caches");
    }
    let status = Command::new("sudo")
        .args(["tee", "/proc/sys/vm/drop_caches"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(stdin) = child.stdin.as_mut() {
                stdin.write_all(b"3")?;
            }
            child.wait()
        });
    match status {
        Ok(s) if s.success() => Ok(true),
        _ => {
            warn!("drop_caches failed — ensure passwordless sudo for tee /proc/sys/vm/drop_caches");
            Ok(false)
        }
    }
}

#[cfg(target_os = "windows")]
fn purge_cache_impl() -> Result<bool> {
    warn!("Cache purge not supported on Windows");
    Ok(false)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn purge_cache_impl() -> Result<bool> {
    warn!("Cache purge not supported on this platform");
    Ok(false)
}

// ── Platform-specific: notifications ──────────────────────────────────────

#[cfg(target_os = "macos")]
fn send_notification_impl(title: &str, message: &str) {
    let script = format!(
        "display notification \"{}\" with title \"DeepSpeed\" subtitle \"{}\"",
        message.replace('"', "'"),
        title.replace('"', "'"),
    );
    if let Err(e) = Command::new("osascript").args(["-e", &script]).status() {
        warn!("Failed to send macOS notification: {}", e);
    }
}

#[cfg(target_os = "linux")]
fn send_notification_impl(title: &str, message: &str) {
    // notify-send is part of libnotify-bin; falls back silently if not installed
    if let Err(e) = Command::new("notify-send")
        .args(["--app-name=DeepSpeed", "--urgency=normal", title, message])
        .status()
    {
        warn!("Failed to send Linux notification (is notify-send installed?): {}", e);
    }
}

#[cfg(target_os = "windows")]
fn send_notification_impl(title: &str, message: &str) {
    let title = title.replace('\'', "\"");
    let message = message.replace('\'', "\"");
    let script = format!(
        r#"
        Add-Type -AssemblyName System.Windows.Forms
        $n = New-Object System.Windows.Forms.NotifyIcon
        $n.Icon = [System.Drawing.SystemIcons]::Information
        $n.BalloonTipTitle = '{title}'
        $n.BalloonTipText  = '{message}'
        $n.Visible = $true
        $n.ShowBalloonTip(5000)
        Start-Sleep -Seconds 6
        $n.Dispose()
        "#,
        title = title,
        message = message,
    );
    if let Err(e) = Command::new("powershell")
        .args(["-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script])
        .spawn()
    {
        warn!("Failed to send Windows notification: {}", e);
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn send_notification_impl(title: &str, message: &str) {
    info!("Notification [{}]: {}", title, message);
}

// ── Protected process lists ────────────────────────────────────────────────

pub fn is_protected(name: &str, protected: &[String]) -> bool {
    if always_protected_system(name) {
        return true;
    }
    protected.iter().any(|p| name.eq_ignore_ascii_case(p))
}

fn always_protected_system(name: &str) -> bool {
    #[cfg(target_os = "macos")]
    let list = &[
        "kernel_task", "launchd", "WindowServer", "loginwindow",
        "SystemUIServer", "Dock", "Finder", "coreaudiod", "bluetoothd",
        "configd", "notifyd", "deepspeed", "mds", "mds_stores",
        "com.apple.WebKit.WebContent", "com.apple.WebKit.Networking",
    ];

    #[cfg(target_os = "linux")]
    let list = &[
        "systemd", "kthreadd", "init", "dbus-daemon", "NetworkManager",
        "polkitd", "deepspeed", "Xorg", "gnome-shell", "plasmashell",
        "pipewire", "wireplumber", "pulseaudio",
    ];

    #[cfg(target_os = "windows")]
    let list = &[
        "System", "smss.exe", "csrss.exe", "wininit.exe", "services.exe",
        "lsass.exe", "winlogon.exe", "explorer.exe", "deepspeed.exe",
        "svchost.exe", "dwm.exe", "audiodg.exe",
    ];

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let list: &[&str] = &["deepspeed"];

    list.iter().any(|p| name.eq_ignore_ascii_case(p))
}

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
