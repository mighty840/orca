//! Slack incoming-webhook delivery.
//!
//! Posts a Block Kit message per alert event. The webhook URL determines
//! which workspace + channel the message lands in; no API token needed.

use async_trait::async_trait;
use serde_json::{Value, json};
use std::time::Duration;

use orca_core::types::{AlertConversation, AlertSender, AlertSeverity};

use super::{AlertEvent, Channel};

pub struct SlackChannel {
    webhook_url: String,
    client: reqwest::Client,
}

impl SlackChannel {
    pub fn new(webhook_url: String) -> Self {
        Self {
            webhook_url,
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(15))
                .build()
                .expect("reqwest client"),
        }
    }

    fn payload(conv: &AlertConversation, event: AlertEvent) -> Value {
        let severity_emoji = match conv.severity {
            AlertSeverity::Critical => ":rotating_light:",
            AlertSeverity::Warning => ":warning:",
            AlertSeverity::Info => ":information_source:",
        };
        let header = format!(
            "{severity_emoji} {} — {} ({})",
            event.label(),
            conv.service,
            severity_label(conv.severity)
        );
        let latest = conv
            .messages
            .iter()
            .rev()
            .find(|m| !matches!(m.sender, AlertSender::Operator))
            .map(|m| m.content.clone())
            .unwrap_or_default();

        let mut blocks = vec![
            json!({
                "type": "header",
                "text": { "type": "plain_text", "text": header, "emoji": true }
            }),
            json!({
                "type": "section",
                "text": { "type": "mrkdwn", "text": truncate(&latest, 2900) }
            }),
        ];
        if let Some(cmd) = conv
            .messages
            .last()
            .and_then(|m| m.suggested_command.as_ref())
        {
            blocks.push(json!({
                "type": "section",
                "text": { "type": "mrkdwn", "text": format!("Suggested fix:\n```{cmd}```") }
            }));
        }
        json!({ "blocks": blocks, "text": header })
    }
}

fn severity_label(s: AlertSeverity) -> &'static str {
    match s {
        AlertSeverity::Critical => "critical",
        AlertSeverity::Warning => "warning",
        AlertSeverity::Info => "info",
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut out = s[..max].to_string();
        out.push('…');
        out
    }
}

#[async_trait]
impl Channel for SlackChannel {
    fn name(&self) -> &'static str {
        "slack"
    }

    async fn deliver(&self, conv: &AlertConversation, event: AlertEvent) -> anyhow::Result<()> {
        let payload = Self::payload(conv, event);
        let resp = self
            .client
            .post(&self.webhook_url)
            .json(&payload)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("slack webhook returned {status}: {body}");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orca_core::types::{AlertMessage, AlertSender, AlertState};

    fn conv_with(suggested: Option<&str>) -> AlertConversation {
        AlertConversation {
            id: uuid::Uuid::now_v7(),
            service: "api".into(),
            severity: AlertSeverity::Critical,
            state: AlertState::AwaitingAction,
            started_at: chrono::Utc::now(),
            resolved_at: None,
            messages: vec![AlertMessage {
                timestamp: chrono::Utc::now(),
                sender: AlertSender::Orca,
                content: "Service is crash-looping. OOM in last 5 minutes.".into(),
                suggested_command: suggested.map(String::from),
            }],
        }
    }

    #[test]
    fn payload_has_header_and_section_blocks() {
        let payload = SlackChannel::payload(&conv_with(None), AlertEvent::Opened);
        let blocks = payload["blocks"].as_array().expect("blocks array");
        assert_eq!(blocks[0]["type"], "header");
        assert_eq!(blocks[1]["type"], "section");
        let header_text = blocks[0]["text"]["text"].as_str().unwrap();
        assert!(header_text.contains("Opened"));
        assert!(header_text.contains("api"));
        assert!(header_text.contains("critical"));
    }

    #[test]
    fn payload_includes_suggested_command_when_present() {
        let payload =
            SlackChannel::payload(&conv_with(Some("orca redeploy api")), AlertEvent::Opened);
        let blocks = payload["blocks"].as_array().unwrap();
        let last = blocks.last().unwrap()["text"]["text"].as_str().unwrap();
        assert!(last.contains("orca redeploy api"));
        assert!(last.contains("Suggested fix"));
    }

    #[test]
    fn truncate_caps_long_strings() {
        let long = "x".repeat(5000);
        let out = truncate(&long, 100);
        assert!(out.ends_with("…"));
        // Char count check (truncate is byte-based + 3-byte ellipsis)
        assert!(out.len() < 200);
    }
}
