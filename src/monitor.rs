use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Instant;
use sysinfo::{CpuRefreshKind, Networks, ProcessesToUpdate, System};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemSnapshot {
    pub timestamp: DateTime<Utc>,
    pub memory: MemoryInfo,
    pub cpu: CpuInfo,
    pub swap: SwapInfo,
    pub top_processes: Vec<ProcessInfo>,
    pub memory_pressure: MemoryPressureLevel,
    pub thermal_state: ThermalState,
    pub gpu: Option<GpuInfo>,
    pub network: NetworkInfo,
    pub load_avg: [f64; 3],
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
    pub per_core_pct: Vec<f64>,
    /// Current CPU frequency in MHz (0 = unavailable)
    pub freq_mhz: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuInfo {
    pub name: String,
    /// None when the platform cannot measure utilisation without root
    pub utilization_pct: Option<f64>,
    pub vram_total_mb: Option<u64>,
    pub vram_used_mb: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkInfo {
    /// Bytes received per second (all non-loopback interfaces)
    pub rx_bytes_sec: f64,
    /// Bytes transmitted per second (all non-loopback interfaces)
    pub tx_bytes_sec: f64,
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
    /// Full path to the executable (None for kernel threads or sandboxed processes)
    pub exe_path: Option<String>,
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
    networks: Networks,
    net_last_refresh: Instant,
    /// GPU info cached at startup; re-queried every 10 s on platforms with live utilisation
    gpu_cache: Option<GpuInfo>,
    gpu_last_refresh: Instant,
}

impl SystemMonitor {
    pub fn new() -> Self {
        let sys = System::new_all();
        let networks = Networks::new_with_refreshed_list();
        let gpu_cache = query_gpu_info();
        Self {
            sys,
            networks,
            net_last_refresh: Instant::now(),
            gpu_cache,
            gpu_last_refresh: Instant::now(),
        }
    }

    pub fn snapshot(&mut self) -> Result<SystemSnapshot> {
        let net_elapsed = self.net_last_refresh.elapsed().as_secs_f64().max(0.5);

        // Targeted refresh — skip disks, components
        self.sys.refresh_memory();
        self.sys.refresh_cpu_specifics(
            CpuRefreshKind::new().with_cpu_usage().with_frequency(),
        );
        self.sys.refresh_processes(ProcessesToUpdate::All, true);

        // Network counters: refresh gives bytes-since-last-refresh
        self.networks.refresh();
        self.net_last_refresh = Instant::now();

        // Re-query GPU every 10 s (picks up live utilisation on Linux/NVIDIA)
        if self.gpu_last_refresh.elapsed().as_secs() >= 10 {
            if let Some(fresh) = query_gpu_info() {
                self.gpu_cache = Some(fresh);
            }
            self.gpu_last_refresh = Instant::now();
        }

        let memory = self.collect_memory();
        let cpu = self.collect_cpu();
        let swap = self.collect_swap();
        let top_processes = self.collect_processes();
        let memory_pressure = query_memory_pressure(memory.used_pct);
        let thermal_state = query_thermal_state();
        let network = self.collect_network(net_elapsed);

        let la = System::load_average();
        let load_avg = [la.one, la.five, la.fifteen];

        Ok(SystemSnapshot {
            timestamp: Utc::now(),
            memory,
            cpu,
            swap,
            top_processes,
            memory_pressure,
            thermal_state,
            gpu: self.gpu_cache.clone(),
            network,
            load_avg,
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
        let per_core_pct: Vec<f64> = cpus.iter().map(|c| c.cpu_usage() as f64).collect();
        let usage_pct = if per_core_pct.is_empty() {
            0.0
        } else {
            per_core_pct.iter().sum::<f64>() / per_core_pct.len() as f64
        };
        // frequency() returns MHz; 0 means unavailable
        let freq_mhz = cpus.first().map(|c| c.frequency()).unwrap_or(0);
        CpuInfo {
            usage_pct,
            core_count: cpus.len(),
            per_core_pct,
            freq_mhz,
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
                exe_path: proc.exe().map(|p| p.to_string_lossy().into_owned()),
            })
            .collect()
    }

    fn collect_network(&self, elapsed_secs: f64) -> NetworkInfo {
        let (rx, tx) = self
            .networks
            .iter()
            .filter(|(name, _)| *name != "lo" && *name != "lo0")
            .map(|(_, n)| (n.received(), n.transmitted()))
            .fold((0u64, 0u64), |(ar, at), (r, t)| (ar + r, at + t));
        NetworkInfo {
            rx_bytes_sec: rx as f64 / elapsed_secs,
            tx_bytes_sec: tx as f64 / elapsed_secs,
        }
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

// ── GPU info ───────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn query_gpu_info() -> Option<GpuInfo> {
    use std::process::Command;
    let out = Command::new("system_profiler")
        .args(["SPDisplaysDataType", "-json"])
        .output()
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let entry = json.get("SPDisplaysDataType")?.as_array()?.first()?;
    let name = entry.get("_name")
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown GPU")
        .to_string();
    let vram_str = entry.get("spdisplays_vram")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    Some(GpuInfo {
        name,
        utilization_pct: None, // requires root (powermetrics)
        vram_total_mb: parse_vram_mb(vram_str),
        vram_used_mb: None,
    })
}

#[cfg(target_os = "linux")]
fn query_gpu_info() -> Option<GpuInfo> {
    query_nvidia_gpu().or_else(query_amd_gpu)
}

#[cfg(target_os = "linux")]
fn query_nvidia_gpu() -> Option<GpuInfo> {
    use std::process::Command;
    let out = Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,utilization.gpu,memory.used,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    if !out.status.success() { return None; }
    let text = String::from_utf8_lossy(&out.stdout);
    let parts: Vec<&str> = text.lines().next()?.split(',').map(str::trim).collect();
    if parts.len() < 4 { return None; }
    Some(GpuInfo {
        name: parts[0].to_string(),
        utilization_pct: parts[1].parse().ok(),
        vram_used_mb: parts[2].parse().ok(),
        vram_total_mb: parts[3].parse().ok(),
    })
}

#[cfg(target_os = "linux")]
fn query_amd_gpu() -> Option<GpuInfo> {
    use std::process::Command;
    let out = Command::new("rocm-smi")
        .args(["--showuse", "--showmeminfo", "vram", "--csv"])
        .output()
        .ok()?;
    if !out.status.success() { return None; }
    for line in String::from_utf8_lossy(&out.stdout).lines().skip(1) {
        let p: Vec<&str> = line.split(',').map(str::trim).collect();
        if p.len() >= 4 {
            return Some(GpuInfo {
                name: format!("AMD GPU ({})", p[0]),
                utilization_pct: p[1].trim_end_matches('%').parse().ok(),
                vram_used_mb: p[2].parse::<u64>().ok().map(|b| b / 1024 / 1024),
                vram_total_mb: p[3].parse::<u64>().ok().map(|b| b / 1024 / 1024),
            });
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn query_gpu_info() -> Option<GpuInfo> {
    use std::process::Command;
    let out = Command::new("wmic")
        .args(["path", "win32_VideoController", "get", "Name,AdapterRAM", "/format:csv"])
        .output()
        .ok()?;
    for line in String::from_utf8_lossy(&out.stdout).lines().skip(2) {
        let p: Vec<&str> = line.split(',').collect();
        if p.len() >= 3 {
            let name = p[1].trim().to_string();
            let bytes: u64 = p[2].trim().parse().unwrap_or(0);
            if !name.is_empty() {
                return Some(GpuInfo {
                    name,
                    utilization_pct: None,
                    vram_total_mb: Some(bytes / 1024 / 1024),
                    vram_used_mb: None,
                });
            }
        }
    }
    None
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn query_gpu_info() -> Option<GpuInfo> { None }

fn parse_vram_mb(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("shared") || s.is_empty() { return None; }
    let mut parts = s.splitn(2, ' ');
    let num: f64 = parts.next()?.parse().ok()?;
    match parts.next()?.trim().to_uppercase().as_str() {
        "GB" | "GIB" => Some((num * 1024.0) as u64),
        "MB" | "MIB" => Some(num as u64),
        _ => None,
    }
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
    line.split_whitespace().nth(1).unwrap_or("0").parse().unwrap_or(0)
}

// ── Snapshot display ───────────────────────────────────────────────────────

impl SystemSnapshot {
    pub fn summary(&self) -> String {
        format!(
            "Memory: {:.1}% ({}/{}MB) | Swap: {:.1}% | CPU: {:.1}% | Load: {:.2} | Net: ↓{}/s ↑{}/s",
            self.memory.used_pct,
            self.memory.used_mb,
            self.memory.total_mb,
            self.swap.used_pct,
            self.cpu.usage_pct,
            self.load_avg[0],
            fmt_bytes(self.network.rx_bytes_sec as u64),
            fmt_bytes(self.network.tx_bytes_sec as u64),
        )
    }
}

pub fn fmt_bytes(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1}MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.0}KB", bytes as f64 / 1024.0)
    } else {
        format!("{}B", bytes)
    }
}
