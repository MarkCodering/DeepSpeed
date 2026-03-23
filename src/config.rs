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
    /// "anthropic" or "ollama" (default)
    pub provider: String,
    /// Anthropic API key — only required when provider = "anthropic"
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
    pub min_confidence: f64,
    pub action_cooldown_secs: u64,
    /// Ollama base URL — cloud: https://api.ollama.com, local: http://localhost:11434
    pub ollama_base_url: String,
    /// Ollama cloud API key (set OLLAMA_API_KEY env var or this field for cloud access)
    pub ollama_api_key: String,
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
                // Scale heavy process threshold to 6% of total memory by default;
                // users with more RAM may want to raise this in their config.
                process_heavy_memory_mb: 500,
            },
            actions: ActionsConfig {
                allow_purge_cache: false,
                allow_renice: true,
                renice_value: 10,
                protected_processes: vec![],
                min_process_age_secs: 120,
            },
            ai: AiConfig {
                provider: "ollama".to_string(),
                api_key: String::new(),
                model: "nemotron-3-super".to_string(),
                max_tokens: 1024,
                min_confidence: 0.75,
                action_cooldown_secs: 600,
                ollama_base_url: "https://api.ollama.com".to_string(),
                ollama_api_key: String::new(),
            },
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;

        if !config_path.exists() {
            tracing::info!("No config file at {:?} — using defaults", config_path);
            return Ok(Config::default());
        }

        let content = std::fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config from {:?}", config_path))?;

        let mut config: Config = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config at {:?}", config_path))?;

        // Environment variables always win over config file
        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            if !key.is_empty() {
                config.ai.api_key = key;
            }
        }
        if let Ok(key) = std::env::var("OLLAMA_API_KEY") {
            if !key.is_empty() {
                config.ai.ollama_api_key = key;
            }
        }

        Ok(config)
    }

    pub fn config_path() -> Result<PathBuf> {
        #[cfg(target_os = "windows")]
        {
            let base = dirs::config_dir()
                .context("Cannot find AppData\\Roaming")?;
            Ok(base.join("deepspeed").join("deepspeed.toml"))
        }
        #[cfg(not(target_os = "windows"))]
        {
            let home = dirs::home_dir().context("Cannot determine home directory")?;
            Ok(home.join(".config").join("deepspeed").join("deepspeed.toml"))
        }
    }

    pub fn log_path(&self) -> PathBuf {
        if !self.general.log_file.is_empty() {
            return PathBuf::from(&self.general.log_file);
        }

        #[cfg(target_os = "macos")]
        {
            dirs::home_dir()
                .unwrap_or_else(|| std::env::temp_dir())
                .join("Library")
                .join("Logs")
                .join("deepspeed.log")
        }
        #[cfg(target_os = "linux")]
        {
            dirs::data_local_dir()
                .unwrap_or_else(|| std::env::temp_dir())
                .join("deepspeed")
                .join("deepspeed.log")
        }
        #[cfg(target_os = "windows")]
        {
            dirs::data_local_dir()
                .unwrap_or_else(|| std::env::temp_dir())
                .join("DeepSpeed")
                .join("deepspeed.log")
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            std::env::temp_dir().join("deepspeed.log")
        }
    }

    pub fn ai_enabled(&self) -> bool {
        match self.ai.provider.as_str() {
            "ollama" => true, // Ollama runs locally — no API key required
            _ => !self.ai.api_key.is_empty(),
        }
    }
}
