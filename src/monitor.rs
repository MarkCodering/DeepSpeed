use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sysinfo::{ProcessesToUpdate, System};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemSnapshot {
    pub timestamp: DateTime<Utc>,
    pub memory: MemoryInfo,
    pub cpu: CpuInfo,
    pub swap: SwapInfo,
    pub top_processes: Vec<ProcessInfo>,
    pub memory_pressure: MemoryPressureLevel,
    pub thermal_state: ThermalState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryInfo {
    pub total_mb: u64,
    pub used_mb: u64,
    pub available_mb: u64,
    pub used_pct: f64,
    /// Kernel/wired memory in MB (macOS: wired, Linux: Slab, Windows: 0)
    pub kernel_mb: u64,
    /// Compressed/cached memory in MB (macOS: compressed, Linux: Cached, Windows: 0)
    pub cached_mb: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuInfo {
    pub usage_pct: f64,
    pub core_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapInfo {
    pub total_mb: u64,
    pub used_mb: u64,
    pub used_pct: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub memory_mb: u64,
    pub cpu_pct: f32,
    pub nice: i32,
    pub run_time_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MemoryPressureLevel {
    Normal,
    Warning,
    Critical,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ThermalState {
    Nominal,
    Fair,
    Serious,
    Critical,
    Unknown,
}

pub struct SystemMonitor {
    sys: System,
}

impl SystemMonitor {
    pub fn new() -> Self {
        // new_all() already does a full initial refresh — no second call needed
        let sys = System::new_all();
        Self { sys }
    }

    pub fn snapshot(&mut self) -> Result<SystemSnapshot> {
        // Targeted refresh: skip disks, networks, components — ~5–20ms faster per cycle
        self.sys.refresh_memory();
        self.sys.refresh_cpu_usage();
        self.sys.refresh_processes(ProcessesToUpdate::All, true);

        let memory = self.collect_memory();
        let cpu = self.collect_cpu();
        let swap = self.collect_swap();
        let top_processes = self.collect_processes();
        let memory_pressure = query_memory_pressure(memory.used_pct);
        let thermal_state = query_thermal_state();

        Ok(SystemSnapshot {
            timestamp: Utc::now(),
            memory,
            cpu,
            swap,
            top_processes,
            memory_pressure,
            thermal_state,
        })
    }

    fn collect_memory(&self) -> MemoryInfo {
        let total = self.sys.total_memory();
        let used = self.sys.used_memory();
        let available = self.sys.available_memory();
        let total_mb = total >> 20;
        let used_mb = used >> 20;
        let available_mb = available >> 20;
        let used_pct = if total > 0 {
            (used as f64 / total as f64) * 100.0
        } else {
            0.0
        };

        let (kernel_mb, cached_mb) = query_extra_mem();

        MemoryInfo {
            total_mb,
            used_mb,
            available_mb,
            used_pct,
            kernel_mb,
            cached_mb,
        }
    }

    fn collect_cpu(&self) -> CpuInfo {
        let cpus = self.sys.cpus();
        let usage_pct = if cpus.is_empty() {
            0.0
        } else {
            cpus.iter().map(|c| c.cpu_usage() as f64).sum::<f64>() / cpus.len() as f64
        };
        CpuInfo {
            usage_pct,
            core_count: cpus.len(),
        }
    }

    fn collect_swap(&self) -> SwapInfo {
        let total = self.sys.total_swap();
        let used = self.sys.used_swap();
        let total_mb = total >> 20;
        let used_mb = used >> 20;
        let used_pct = if total > 0 {
            (used as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        SwapInfo { total_mb, used_mb, used_pct }
    }

    fn collect_processes(&self) -> Vec<ProcessInfo> {
        const TOP_N: usize = 20;
        // Collect cheap (memory, pid, proc) refs, then partial-select top N — O(n) average
        let mut refs: Vec<_> = self.sys.processes().iter().collect();
        if refs.len() > TOP_N {
            refs.select_nth_unstable_by(TOP_N - 1, |a, b| b.1.memory().cmp(&a.1.memory()));
            refs.truncate(TOP_N);
        }
        refs.sort_unstable_by(|a, b| b.1.memory().cmp(&a.1.memory()));
        refs.into_iter()
            .map(|(pid, proc)| ProcessInfo {
                pid: pid.as_u32(),
                name: proc.name().to_string_lossy().into_owned(),
                memory_mb: proc.memory() >> 20,
                cpu_pct: proc.cpu_usage(),
                nice: 0,
                run_time_secs: proc.run_time(),
            })
            .collect()
    }
}

// ── Platform-specific: extra memory details ────────────────────────────────

#[cfg(target_os = "macos")]
fn query_extra_mem() -> (u64, u64) {
    use std::process::Command;
    let output = Command::new("vm_stat").output();
    let Ok(out) = output else { return (0, 0) };
    let text = String::from_utf8_lossy(&out.stdout);

    let page_size: u64 = 16384; // Apple Silicon uses 16 KB pages
    let mut wired_pages: u64 = 0;
    let mut compressed_pages: u64 = 0;

    for line in text.lines() {
        if line.starts_with("Pages wired down:") {
            wired_pages = parse_colon_u64(line);
        } else if line.starts_with("Pages occupied by compressor:") {
            compressed_pages = parse_colon_u64(line);
        }
    }

    let kernel_mb = wired_pages * page_size / 1024 / 1024;
    let cached_mb = compressed_pages * page_size / 1024 / 1024;
    (kernel_mb, cached_mb)
}

#[cfg(target_os = "linux")]
fn query_extra_mem() -> (u64, u64) {
    let Ok(content) = std::fs::read_to_string("/proc/meminfo") else {
        return (0, 0);
    };
    let mut cached_kb: u64 = 0;
    let mut slab_kb: u64 = 0;
    for line in content.lines() {
        if line.starts_with("Cached:") {
            cached_kb = parse_meminfo_kb(line);
        } else if line.starts_with("Slab:") {
            slab_kb = parse_meminfo_kb(line);
        }
    }
    (slab_kb / 1024, cached_kb / 1024)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn query_extra_mem() -> (u64, u64) {
    (0, 0)
}

// ── Platform-specific: memory pressure ────────────────────────────────────

#[cfg(target_os = "macos")]
fn query_memory_pressure(used_pct: f64) -> MemoryPressureLevel {
    // Fast-path: skip the subprocess spawn when clearly below warning threshold
    if used_pct < 70.0 {
        return MemoryPressureLevel::Normal;
    }
    use std::process::Command;
    let output = Command::new("memory_pressure").output();
    let Ok(out) = output else {
        return pressure_from_pct(used_pct);
    };
    let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
    if text.contains("critical") {
        MemoryPressureLevel::Critical
    } else if text.contains("warn") {
        MemoryPressureLevel::Warning
    } else if text.contains("normal") {
        MemoryPressureLevel::Normal
    } else {
        pressure_from_pct(used_pct)
    }
}

#[cfg(target_os = "linux")]
fn query_memory_pressure(used_pct: f64) -> MemoryPressureLevel {
    // Linux Pressure Stall Information (kernel 4.20+)
    let Ok(content) = std::fs::read_to_string("/proc/pressure/memory") else {
        return pressure_from_pct(used_pct);
    };
    for line in content.lines() {
        if line.starts_with("some ") {
            if let Some(part) = line.split("avg10=").nth(1) {
                let avg10: f64 = part.split_whitespace().next()
                    .unwrap_or("0").parse().unwrap_or(0.0);
                return if avg10 >= 30.0 {
                    MemoryPressureLevel::Critical
                } else if avg10 >= 5.0 {
                    MemoryPressureLevel::Warning
                } else {
                    MemoryPressureLevel::Normal
                };
            }
        }
    }
    pressure_from_pct(used_pct)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn query_memory_pressure(used_pct: f64) -> MemoryPressureLevel {
    pressure_from_pct(used_pct)
}

fn pressure_from_pct(used_pct: f64) -> MemoryPressureLevel {
    if used_pct >= 92.0 {
        MemoryPressureLevel::Critical
    } else if used_pct >= 80.0 {
        MemoryPressureLevel::Warning
    } else {
        MemoryPressureLevel::Normal
    }
}

// ── Platform-specific: thermal state ──────────────────────────────────────

#[cfg(target_os = "macos")]
fn query_thermal_state() -> ThermalState {
    use std::process::Command;
    let output = Command::new("pmset").args(["-g", "therm"]).output();
    let Ok(out) = output else { return ThermalState::Unknown };
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        if line.contains("CPU_Speed_Limit") {
            if let Some(val_str) = line.split('=').nth(1) {
                if let Ok(val) = val_str.trim().parse::<u32>() {
                    return match val {
                        100 => ThermalState::Nominal,
                        75..=99 => ThermalState::Fair,
                        50..=74 => ThermalState::Serious,
                        _ => ThermalState::Critical,
                    };
                }
            }
        }
    }
    ThermalState::Nominal
}

#[cfg(target_os = "linux")]
fn query_thermal_state() -> ThermalState {
    // Try the first available thermal zone
    for zone in 0..=4u32 {
        let path = format!("/sys/class/thermal/thermal_zone{}/temp", zone);
        if let Ok(s) = std::fs::read_to_string(&path) {
            if let Ok(millidegrees) = s.trim().parse::<u64>() {
                let celsius = millidegrees / 1000;
                return match celsius {
                    0..=59 => ThermalState::Nominal,
                    60..=74 => ThermalState::Fair,
                    75..=84 => ThermalState::Serious,
                    _ => ThermalState::Critical,
                };
            }
        }
    }
    ThermalState::Unknown
}

#[cfg(target_os = "windows")]
fn query_thermal_state() -> ThermalState {
    use std::process::Command;
    // MSAcpi_ThermalZoneTemperature returns tenths of Kelvin
    let output = Command::new("wmic")
        .args([
            "/namespace:\\\\root\\wmi",
            "PATH",
            "MSAcpi_ThermalZoneTemperature",
            "get",
            "CurrentTemperature",
        ])
        .output();
    let Ok(out) = output else { return ThermalState::Unknown };
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines().skip(1) {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            if let Ok(raw) = trimmed.parse::<u64>() {
                let celsius = raw.saturating_sub(2732) / 10;
                return match celsius {
                    0..=59 => ThermalState::Nominal,
                    60..=74 => ThermalState::Fair,
                    75..=84 => ThermalState::Serious,
                    _ => ThermalState::Critical,
                };
            }
        }
    }
    ThermalState::Unknown
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn query_thermal_state() -> ThermalState {
    ThermalState::Unknown
}

// ── Parse helpers ──────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn parse_colon_u64(line: &str) -> u64 {
    line.split(':')
        .nth(1)
        .unwrap_or("0")
        .trim()
        .trim_end_matches('.')
        .replace(',', "")
        .parse()
        .unwrap_or(0)
}

#[cfg(target_os = "linux")]
fn parse_meminfo_kb(line: &str) -> u64 {
    // "Cached:    123456 kB"
    line.split_whitespace().nth(1).unwrap_or("0").parse().unwrap_or(0)
}

// ── Snapshot display ───────────────────────────────────────────────────────

impl SystemSnapshot {
    pub fn summary(&self) -> String {
        format!(
            "Memory: {:.1}% ({}/{}MB) | Swap: {:.1}% ({}MB) | CPU: {:.1}% | Pressure: {:?}",
            self.memory.used_pct,
            self.memory.used_mb,
            self.memory.total_mb,
            self.swap.used_pct,
            self.swap.used_mb,
            self.cpu.usage_pct,
            self.memory_pressure,
        )
    }
}
