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
    /// Grace window (seconds) a service must stay down before a "service down"
    /// alert fires (default: 120). A deploy/rollout briefly drops replicas to 0;
    /// without this grace every webhook deploy would open + auto-remediate an
    /// alert, spamming the configured channels. Raise it for services whose
    /// image pulls/health checks routinely take longer than the default.
    #[serde(default = "default_alert_grace")]
    pub alert_grace_secs: u64,
    /// Channels to deliver conversation updates.
    pub channels: Option<AlertDeliveryChannels>,
}

fn default_true() -> bool {
    true
}

fn default_analysis_interval() -> u64 {
    60
}

fn default_alert_grace() -> u64 {
    120
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
    /// TLS handshake mode. Production should always use `starttls` (default).
    /// `none` is for local dev/test against catchers like mailpit and must
    /// not be used over an untrusted network — credentials go in cleartext.
    #[serde(default)]
    pub tls: SmtpTls,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SmtpTls {
    /// STARTTLS upgrade on a plain connection (port 587 default).
    #[default]
    Starttls,
    /// Implicit TLS from the start of the connection (port 465).
    Implicit,
    /// No TLS — plain SMTP. Local dev only.
    None,
}

fn default_smtp_port() -> u16 {
    587
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
