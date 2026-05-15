use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    /// LLM provider: "litellm", "ollama", "openai", "anthropic"
    #[serde(default = "default_ai_provider")]
    pub provider: String,
    /// Endpoint URL (for litellm/ollama/compatible APIs).
    pub endpoint: Option<String>,
    /// Model identifier.
    pub model: Option<String>,
    /// API key (or use ${secrets.ai_api_key}).
    pub api_key: Option<String>,
    /// Conversational alerting configuration.
    #[serde(default)]
    pub alerts: Option<AiAlertConfig>,
    /// Auto-remediation rules.
    #[serde(default)]
    pub auto_remediate: Option<AutoRemediateConfig>,
}

fn default_ai_provider() -> String {
    "ollama".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiAlertConfig {
    /// Enable conversational alerts (default: true when [ai] is configured).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// How often to analyze cluster health (seconds, default: 60).
    #[serde(default = "default_analysis_interval")]
    pub analysis_interval_secs: u64,
    /// Channels to deliver conversation updates.
    pub channels: Option<AlertDeliveryChannels>,
}

fn default_true() -> bool {
    true
}

fn default_analysis_interval() -> u64 {
    60
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AlertDeliveryChannels {
    /// Generic webhook URL — receives JSON POST per alert event.
    pub webhook: Option<String>,
    /// Slack incoming-webhook URL — receives a formatted block per alert event.
    pub slack: Option<String>,
    /// Email delivery via SMTP — sends one email per alert event.
    pub email: Option<EmailChannelConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailChannelConfig {
    pub smtp_host: String,
    #[serde(default = "default_smtp_port")]
    pub smtp_port: u16,
    pub username: String,
    /// SMTP password. Use `${secrets.SMTP_PASSWORD}` to resolve from the secrets store.
    pub password: String,
    pub from: String,
    pub to: Vec<String>,
    /// If true, use STARTTLS (port 587 default); else implicit TLS (port 465).
    #[serde(default = "default_smtp_starttls")]
    pub starttls: bool,
}

fn default_smtp_port() -> u16 {
    587
}

fn default_smtp_starttls() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoRemediateConfig {
    /// Auto-restart crashed services (default: true).
    #[serde(default = "default_true")]
    pub restart_crashed: bool,
    /// Auto-scale on resource pressure (default: false, suggest only).
    #[serde(default)]
    pub scale_on_pressure: bool,
    /// Auto-rollback on deploy failure (default: false, suggest only).
    #[serde(default)]
    pub rollback_on_failure: bool,
}
