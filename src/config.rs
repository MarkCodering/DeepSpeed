use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub general: GeneralConfig,
    pub thresholds: ThresholdConfig,
    pub actions: ActionsConfig,
    pub ai: AiConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GeneralConfig {
    pub monitor_interval_secs: u64,
    pub ai_interval_secs: u64,
    pub log_level: String,
    pub log_file: String,
    pub notifications_enabled: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ThresholdConfig {
    pub memory_warning_pct: f64,
    pub memory_critical_pct: f64,
    pub swap_warning_pct: f64,
    pub swap_critical_pct: f64,
    pub cpu_warning_pct: f64,
    pub cpu_sustained_secs: u64,
    pub process_heavy_memory_mb: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ActionsConfig {
    pub allow_purge_cache: bool,
    pub allow_renice: bool,
    pub renice_value: i32,
    pub protected_processes: Vec<String>,
    pub min_process_age_secs: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AiConfig {
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
    pub min_confidence: f64,
    pub action_cooldown_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            general: GeneralConfig {
                monitor_interval_secs: 30,
                ai_interval_secs: 300,
                log_level: "info".to_string(),
                log_file: String::new(),
                notifications_enabled: true,
            },
            thresholds: ThresholdConfig {
                memory_warning_pct: 80.0,
                memory_critical_pct: 90.0,
                swap_warning_pct: 40.0,
                swap_critical_pct: 70.0,
                cpu_warning_pct: 85.0,
                cpu_sustained_secs: 60,
                process_heavy_memory_mb: 500,
            },
            actions: ActionsConfig {
                allow_purge_cache: false,
                allow_renice: true,
                renice_value: 10,
                protected_processes: vec![
                    "Finder".to_string(),
                    "WindowServer".to_string(),
                    "loginwindow".to_string(),
                    "SystemUIServer".to_string(),
                    "Dock".to_string(),
                    "deepspeed".to_string(),
                ],
                min_process_age_secs: 120,
            },
            ai: AiConfig {
                api_key: String::new(),
                model: "claude-haiku-4-5-20251001".to_string(),
                max_tokens: 1024,
                min_confidence: 0.75,
                action_cooldown_secs: 600,
            },
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;

        if !config_path.exists() {
            tracing::info!("No config file found at {:?}, using defaults", config_path);
            return Ok(Config::default());
        }

        let content = std::fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config from {:?}", config_path))?;

        let mut config: Config = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config at {:?}", config_path))?;

        // Allow env var to override API key
        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            if !key.is_empty() {
                config.ai.api_key = key;
            }
        }

        Ok(config)
    }

    pub fn config_path() -> Result<PathBuf> {
        let home = dirs::home_dir().context("Cannot determine home directory")?;
        Ok(home.join(".config").join("deepspeed").join("deepspeed.toml"))
    }

    pub fn log_path(&self) -> PathBuf {
        if !self.general.log_file.is_empty() {
            return PathBuf::from(&self.general.log_file);
        }
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
        home.join("Library").join("Logs").join("deepspeed.log")
    }

    pub fn ai_enabled(&self) -> bool {
        !self.ai.api_key.is_empty()
    }
}
