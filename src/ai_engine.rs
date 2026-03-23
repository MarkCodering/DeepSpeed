/// AI Engine — calls Claude (Anthropic) or a local Ollama model when the
/// rule-based optimizer escalates. Calls happen only during genuine crises.
use crate::actions::{Action, ActionExecutor};
use crate::config::Config;
use crate::monitor::SystemSnapshot;
use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tracing::{info, warn};

pub struct AiEngine {
    config: Config,
    client: Client,
    /// Tracks last successful AI call time to prevent rapid-fire calls
    last_call: Option<Instant>,
    /// Rolling count of calls made (for logging/awareness)
    call_count: u32,
}

// ── Anthropic API types ────────────────────────────────────────────────────

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    system: String,
    messages: Vec<ChatMessage>,
}

#[derive(Serialize, Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: Option<String>,
}

// ── Ollama cloud API types (OpenAI-compatible /v1/chat/completions) ────────

#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    messages: Vec<ChatMessage>,
}

#[derive(Deserialize)]
struct OllamaResponse {
    choices: Vec<OllamaChoice>,
}

#[derive(Deserialize)]
struct OllamaChoice {
    message: ChatMessage,
}

// ── AI recommendation types ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct AiRecommendation {
    pub summary: String,
    pub confidence: f64,
    pub actions: Vec<AiAction>,
    pub explanation: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AiAction {
    pub action_type: String, // "renice", "notify", "purge_cache", "none"
    pub pid: Option<u32>,
    pub process_name: Option<String>,
    pub nice_value: Option<i32>,
    pub message: Option<String>,
}

impl AiEngine {
    pub fn new(config: Config) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            config,
            client,
            last_call: None,
            call_count: 0,
        }
    }

    /// Returns true if calling the AI is allowed (respects cooldown)
    pub fn can_call(&self) -> bool {
        if !self.config.ai_enabled() {
            return false;
        }
        match self.last_call {
            None => true,
            Some(t) => t.elapsed() >= Duration::from_secs(self.config.ai.action_cooldown_secs),
        }
    }

    pub fn call_count(&self) -> u32 {
        self.call_count
    }

    /// Analyze the snapshot and return recommendations.
    /// Only call this when `can_call()` returns true.
    pub async fn analyze(&mut self, snap: &SystemSnapshot) -> Result<AiRecommendation> {
        if !self.can_call() {
            anyhow::bail!("AI engine on cooldown — skipping call");
        }

        info!(
            "Calling {} AI (call #{}, model: {}) to analyze system state",
            self.config.ai.provider, self.call_count + 1, self.config.ai.model
        );

        let text = match self.config.ai.provider.as_str() {
            "ollama" => self.call_ollama(snap).await?,
            _ => self.call_anthropic(snap).await?,
        };

        self.last_call = Some(Instant::now());
        self.call_count += 1;

        let recommendation = Self::parse_recommendation(&text)?;
        info!(
            "AI recommendation (confidence={:.2}): {}",
            recommendation.confidence, recommendation.summary
        );

        Ok(recommendation)
    }

    // ── Provider: Ollama cloud ─────────────────────────────────────────────

    async fn call_ollama(&self, snap: &SystemSnapshot) -> Result<String> {
        let request = OllamaRequest {
            model: self.config.ai.model.clone(),
            messages: vec![
                ChatMessage { role: "system".to_string(), content: Self::build_system_prompt() },
                ChatMessage { role: "user".to_string(),   content: Self::build_user_message(snap) },
            ],
        };

        // Ollama cloud uses the OpenAI-compatible endpoint
        let url = format!("{}/v1/chat/completions", self.config.ai.ollama_base_url);
        let mut req = self.client.post(&url).json(&request);

        if !self.config.ai.ollama_api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.config.ai.ollama_api_key));
        }

        let response = req
            .send()
            .await
            .context("Failed to reach Ollama cloud API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Ollama API error {}: {}", status, body);
        }

        let ollama_resp: OllamaResponse = response
            .json()
            .await
            .context("Failed to parse Ollama response")?;

        ollama_resp
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .context("Ollama response contained no choices")
    }

    // ── Provider: Anthropic ────────────────────────────────────────────────

    async fn call_anthropic(&self, snap: &SystemSnapshot) -> Result<String> {
        let system_prompt = Self::build_system_prompt();
        let user_message = Self::build_user_message(snap);

        let request = AnthropicRequest {
            model: self.config.ai.model.clone(),
            max_tokens: self.config.ai.max_tokens,
            system: system_prompt,
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: user_message,
            }],
        };

        let response = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.config.ai.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .context("Failed to call Anthropic API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Anthropic API error {}: {}", status, body);
        }

        let api_response: AnthropicResponse = response
            .json()
            .await
            .context("Failed to parse Anthropic response")?;

        Ok(api_response
            .content
            .iter()
            .find(|b| b.block_type == "text")
            .and_then(|b| b.text.as_deref())
            .unwrap_or("")
            .to_string())
    }

    // ── Shared helpers ─────────────────────────────────────────────────────

    fn build_system_prompt() -> String {
        r#"You are DeepSpeed, an AI system optimizer daemon.
Your job is to analyze system metrics and recommend targeted, safe optimizations.

CRITICAL RULES:
1. Never recommend killing essential system processes (kernel_task, WindowServer, loginwindow, Dock, Finder, SystemUIServer, coreaudiod, systemd, init)
2. Only recommend renice (lowering priority) — never SIGKILL or SIGTERM
3. Prefer the least disruptive action possible
4. If memory pressure will resolve naturally, recommend "none"
5. Be conservative: an unhelpful recommendation is better than a harmful one

Respond ONLY with valid JSON matching this schema — no markdown, no extra text:
{
  "summary": "one-line summary",
  "confidence": 0.0-1.0,
  "explanation": "brief reasoning (2-3 sentences)",
  "actions": [
    {
      "action_type": "renice" | "notify" | "purge_cache" | "none",
      "pid": 1234,
      "process_name": "name",
      "nice_value": 10,
      "message": "user-visible message if action_type is notify"
    }
  ]
}"#
        .to_string()
    }

    fn build_user_message(snap: &SystemSnapshot) -> String {
        let processes: Vec<String> = snap
            .top_processes
            .iter()
            .take(5)
            .map(|p| {
                format!(
                    "  - {} (pid={}, mem={}MB, cpu={:.1}%, age={}s, nice={})",
                    p.name, p.pid, p.memory_mb, p.cpu_pct, p.run_time_secs, p.nice
                )
            })
            .collect();

        format!(
            "System state at {}:\n\
             Memory: {:.1}% used ({}/{}MB, {}MB free)\n\
             Kernel/wired: {}MB, Compressed/cached: {}MB\n\
             Swap: {:.1}% ({}/{}MB)\n\
             CPU: {:.1}% ({} cores)\n\
             Memory pressure: {:?}\n\
             Thermal state: {:?}\n\
             Top processes by memory:\n{}\n\n\
             Rule-based actions already attempted: renice of heavy processes.\n\
             What additional optimizations would you recommend?",
            snap.timestamp.format("%H:%M:%S"),
            snap.memory.used_pct,
            snap.memory.used_mb,
            snap.memory.total_mb,
            snap.memory.available_mb,
            snap.memory.kernel_mb,
            snap.memory.cached_mb,
            snap.swap.used_pct,
            snap.swap.used_mb,
            snap.swap.total_mb,
            snap.cpu.usage_pct,
            snap.cpu.core_count,
            snap.memory_pressure,
            snap.thermal_state,
            processes.join("\n"),
        )
    }

    fn parse_recommendation(text: &str) -> Result<AiRecommendation> {
        // Strip any accidental markdown code fences
        let cleaned = text
            .trim()
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();

        serde_json::from_str(cleaned).context("Failed to parse AI JSON response")
    }

    /// Execute safe actions from an AI recommendation
    pub fn apply_recommendation(
        &self,
        rec: &AiRecommendation,
        executor: &ActionExecutor,
        config: &Config,
    ) -> Vec<String> {
        let mut applied = Vec::new();

        if rec.confidence < config.ai.min_confidence {
            warn!(
                "AI confidence {:.2} below threshold {:.2} — skipping actions",
                rec.confidence, config.ai.min_confidence
            );
            return applied;
        }

        for ai_action in &rec.actions {
            let action = match ai_action.action_type.as_str() {
                "renice" => {
                    let Some(pid) = ai_action.pid else { continue };
                    let name = ai_action.process_name.clone().unwrap_or_default();
                    let nice = ai_action.nice_value.unwrap_or(config.actions.renice_value);
                    Action::ReniceProcess { pid, name, nice }
                }
                "purge_cache" => Action::PurgeCache,
                "notify" => {
                    let msg = ai_action.message.clone().unwrap_or_else(|| rec.summary.clone());
                    Action::Notify {
                        title: "DeepSpeed AI".to_string(),
                        message: msg,
                    }
                }
                "none" => {
                    info!("AI recommends no action: {}", rec.explanation);
                    continue;
                }
                other => {
                    warn!("Unknown AI action type: {}", other);
                    continue;
                }
            };

            match executor.execute(&action) {
                Ok(true) => applied.push(format!("AI action applied: {:?}", ai_action.action_type)),
                Ok(false) => {}
                Err(e) => warn!("AI action failed: {}", e),
            }
        }

        applied
    }
}
