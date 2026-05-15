//! Email delivery via SMTP using `lettre`.
//!
//! Sends one plain-text email per alert event. STARTTLS by default
//! (port 587); set `starttls = false` in cluster.toml for implicit TLS on
//! port 465. Credentials resolve through the secrets store via
//! `${secrets.SMTP_PASSWORD}` in cluster.toml.

use async_trait::async_trait;
use lettre::message::Message;
use lettre::transport::smtp::AsyncSmtpTransport;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncTransport, Tokio1Executor};
use std::time::Duration;

use orca_core::config::EmailChannelConfig;
use orca_core::types::{AlertConversation, AlertSender, AlertSeverity};

use super::{AlertEvent, Channel};

pub struct EmailChannel {
    cfg: EmailChannelConfig,
    transport: AsyncSmtpTransport<Tokio1Executor>,
}

impl EmailChannel {
    pub fn new(cfg: EmailChannelConfig) -> Self {
        let creds = Credentials::new(cfg.username.clone(), cfg.password.clone());
        let builder = if cfg.starttls {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.smtp_host)
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.smtp_host)
        };
        let transport = builder
            .expect("build SMTP transport")
            .port(cfg.smtp_port)
            .credentials(creds)
            .timeout(Some(Duration::from_secs(15)))
            .build();
        Self { cfg, transport }
    }

    fn subject(conv: &AlertConversation, event: AlertEvent) -> String {
        let sev = match conv.severity {
            AlertSeverity::Critical => "[CRITICAL]",
            AlertSeverity::Warning => "[WARN]",
            AlertSeverity::Info => "[INFO]",
        };
        format!("{sev} {} — {}", event.label(), conv.service)
    }

    fn body(conv: &AlertConversation, event: AlertEvent) -> String {
        let mut out = String::new();
        out.push_str(&format!("Service: {}\n", conv.service));
        out.push_str(&format!("Severity: {:?}\n", conv.severity));
        out.push_str(&format!("State: {:?}\n", conv.state));
        out.push_str(&format!("Event: {}\n", event.label()));
        out.push_str(&format!("Started: {}\n", conv.started_at.to_rfc3339()));
        if let Some(resolved) = conv.resolved_at {
            out.push_str(&format!("Resolved: {}\n", resolved.to_rfc3339()));
        }
        out.push_str("\n--- Conversation ---\n");
        for msg in &conv.messages {
            let who = match msg.sender {
                AlertSender::Orca => "orca",
                AlertSender::Operator => "operator",
                AlertSender::System => "system",
            };
            out.push_str(&format!(
                "[{}] {}: {}\n",
                msg.timestamp.format("%H:%M:%S"),
                who,
                msg.content
            ));
            if let Some(cmd) = &msg.suggested_command {
                out.push_str(&format!("  fix: {cmd}\n"));
            }
        }
        out
    }
}

#[async_trait]
impl Channel for EmailChannel {
    fn name(&self) -> &'static str {
        "email"
    }

    async fn deliver(&self, conv: &AlertConversation, event: AlertEvent) -> anyhow::Result<()> {
        let subject = Self::subject(conv, event);
        let body = Self::body(conv, event);
        let mut builder = Message::builder()
            .from(self.cfg.from.parse()?)
            .subject(subject);
        for recipient in &self.cfg.to {
            builder = builder.to(recipient.parse()?);
        }
        let email = builder.body(body)?;
        self.transport.send(email).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orca_core::types::{AlertMessage, AlertState};

    fn fake_conv() -> AlertConversation {
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
                content: "OOM detected".into(),
                suggested_command: Some("orca redeploy api".into()),
            }],
        }
    }

    #[test]
    fn subject_includes_severity_event_and_service() {
        let conv = fake_conv();
        let s = EmailChannel::subject(&conv, AlertEvent::Opened);
        assert!(s.contains("[CRITICAL]"));
        assert!(s.contains("Opened"));
        assert!(s.contains("api"));
    }

    #[test]
    fn body_renders_messages_and_suggested_fix() {
        let conv = fake_conv();
        let body = EmailChannel::body(&conv, AlertEvent::Opened);
        assert!(body.contains("Service: api"));
        assert!(body.contains("OOM detected"));
        assert!(body.contains("fix: orca redeploy api"));
    }
}
