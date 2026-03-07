use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::process::Command;
use sysinfo::System;

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
    /// macOS wired (kernel) memory in MB
    pub wired_mb: u64,
    /// macOS compressed memory in MB
    pub compressed_mb: u64,
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
        let mut sys = System::new_all();
        sys.refresh_all();
        Self { sys }
    }

    pub fn snapshot(&mut self) -> Result<SystemSnapshot> {
        self.sys.refresh_all();

        let memory = self.collect_memory();
        let cpu = self.collect_cpu();
        let swap = self.collect_swap();
        let top_processes = self.collect_processes();
        let memory_pressure = self.query_memory_pressure();
        let thermal_state = self.query_thermal_state();

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
        let total_mb = total / 1024 / 1024;
        let used_mb = used / 1024 / 1024;
        let available_mb = available / 1024 / 1024;
        let used_pct = if total > 0 {
            (used as f64 / total as f64) * 100.0
        } else {
            0.0
        };

        // Query macOS-specific vm_stat for wired + compressed memory
        let (wired_mb, compressed_mb) = self.query_vm_stat();

        MemoryInfo {
            total_mb,
            used_mb,
            available_mb,
            used_pct,
            wired_mb,
            compressed_mb,
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
        let total_mb = total / 1024 / 1024;
        let used_mb = used / 1024 / 1024;
        let used_pct = if total > 0 {
            (used as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        SwapInfo {
            total_mb,
            used_mb,
            used_pct,
        }
    }

    fn collect_processes(&self) -> Vec<ProcessInfo> {
        let mut processes: Vec<ProcessInfo> = self
            .sys
            .processes()
            .iter()
            .map(|(pid, proc)| ProcessInfo {
                pid: pid.as_u32(),
                name: proc.name().to_string_lossy().to_string(),
                memory_mb: proc.memory() / 1024 / 1024,
                cpu_pct: proc.cpu_usage(),
                nice: 0, // sysinfo doesn't expose nice; we query separately
                run_time_secs: proc.run_time(),
            })
            .collect();

        // Sort by memory descending, take top 20
        processes.sort_by(|a, b| b.memory_mb.cmp(&a.memory_mb));
        processes.truncate(20);
        processes
    }

    /// Parse vm_stat output for wired and compressed memory pages
    fn query_vm_stat(&self) -> (u64, u64) {
        let output = Command::new("vm_stat").output();
        let Ok(out) = output else { return (0, 0) };
        let text = String::from_utf8_lossy(&out.stdout);

        let page_size: u64 = 16384; // M2 uses 16KB pages
        let mut wired_pages: u64 = 0;
        let mut compressed_pages: u64 = 0;

        for line in text.lines() {
            if line.starts_with("Pages wired down:") {
                wired_pages = parse_vm_stat_value(line);
            } else if line.starts_with("Pages occupied by compressor:") {
                compressed_pages = parse_vm_stat_value(line);
            }
        }

        let wired_mb = wired_pages * page_size / 1024 / 1024;
        let compressed_mb = compressed_pages * page_size / 1024 / 1024;
        (wired_mb, compressed_mb)
    }

    /// Query macOS memory pressure level via `memory_pressure` command
    fn query_memory_pressure(&self) -> MemoryPressureLevel {
        let output = Command::new("memory_pressure").output();
        let Ok(out) = output else {
            return MemoryPressureLevel::Unknown;
        };
        let text = String::from_utf8_lossy(&out.stdout).to_lowercase();

        if text.contains("critical") {
            MemoryPressureLevel::Critical
        } else if text.contains("warn") {
            MemoryPressureLevel::Warning
        } else if text.contains("normal") {
            MemoryPressureLevel::Normal
        } else {
            MemoryPressureLevel::Unknown
        }
    }

    /// Query macOS thermal state via pmset
    fn query_thermal_state(&self) -> ThermalState {
        let output = Command::new("pmset").args(["-g", "therm"]).output();
        let Ok(out) = output else {
            return ThermalState::Unknown;
        };
        let text = String::from_utf8_lossy(&out.stdout);

        // Look for CPU_Speed_Limit < 100 indicating throttling
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
}

fn parse_vm_stat_value(line: &str) -> u64 {
    line.split(':')
        .nth(1)
        .unwrap_or("0")
        .trim()
        .trim_end_matches('.')
        .replace(',', "")
        .parse()
        .unwrap_or(0)
}

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
