//! Email delivery via SMTP using `lettre`.
//!
//! Sends one multipart/alternative email per alert event — plain text plus
//! an HTML version with the LLM's markdown rendered (tables, code blocks,
//! bold). STARTTLS by default (port 587); set `tls = "implicit"` for port
//! 465, or `tls = "none"` for plain SMTP against local dev catchers
//! (mailpit). Credentials resolve through the secrets store via
//! `${secrets.SMTP_PASSWORD}` in cluster.toml.

use async_trait::async_trait;
use lettre::message::{Message, MultiPart};
use lettre::transport::smtp::AsyncSmtpTransport;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncTransport, Tokio1Executor};
use std::time::Duration;

use orca_core::config::{EmailChannelConfig, SmtpTls};
use orca_core::types::{AlertConversation, AlertSender, AlertSeverity};

use super::{AlertEvent, Channel};

pub struct EmailChannel {
    cfg: EmailChannelConfig,
    transport: AsyncSmtpTransport<Tokio1Executor>,
}

impl EmailChannel {
    pub fn new(cfg: EmailChannelConfig) -> Self {
        let mut builder = match cfg.tls {
            SmtpTls::Starttls => {
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.smtp_host)
                    .expect("build STARTTLS transport")
            }
            SmtpTls::Implicit => AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.smtp_host)
                .expect("build implicit-TLS transport"),
            // Plain SMTP — local dev only (e.g. mailpit catchers).
            SmtpTls::None => {
                AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&cfg.smtp_host)
            }
        };
        builder = builder
            .port(cfg.smtp_port)
            .timeout(Some(Duration::from_secs(15)));
        // Lettre refuses to send AUTH over a non-TLS channel by design (the
        // credentials would be cleartext). Skip credentials entirely on plain
        // SMTP — fine for catchers like mailpit, which accept anonymous mail.
        if !matches!(cfg.tls, SmtpTls::None) {
            builder =
                builder.credentials(Credentials::new(cfg.username.clone(), cfg.password.clone()));
        }
        let transport = builder.build();
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
        let plain = Self::body(conv, event);
        let html = Self::render_html(&plain);
        let mut builder = Message::builder()
            .from(self.cfg.from.parse()?)
            .subject(subject);
        for recipient in &self.cfg.to {
            builder = builder.to(recipient.parse()?);
        }
        // Multipart/alternative: clients that prefer HTML get the rendered
        // version, plain-text clients (mutt, ops scripts) fall back cleanly.
        let email = builder.multipart(MultiPart::alternative_plain_html(plain, html))?;
        self.transport.send(email).await?;
        Ok(())
    }
}

impl EmailChannel {
    /// Render the alert body's markdown to HTML (tables, code blocks, bold).
    /// Wraps in a minimal document with inline CSS so common clients render
    /// tables and code blocks without help.
    fn render_html(body: &str) -> String {
        let mut opts = comrak::Options::default();
        opts.extension.table = true;
        opts.extension.strikethrough = true;
        opts.extension.tagfilter = true;
        opts.extension.autolink = true;
        let rendered = comrak::markdown_to_html(body, &opts);
        format!(
            "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><style>\
             body{{font-family:-apple-system,system-ui,sans-serif;font-size:14px;color:#222;max-width:760px;margin:0 auto;padding:16px;line-height:1.5}}\
             code{{background:#f4f4f4;border-radius:3px;padding:1px 4px;font-family:ui-monospace,Menlo,monospace}}\
             pre{{background:#f4f4f4;border-radius:4px;padding:10px;overflow-x:auto;font-family:ui-monospace,Menlo,monospace;font-size:13px}}\
             pre code{{background:transparent;padding:0}}\
             table{{border-collapse:collapse;margin:8px 0}}\
             th,td{{border:1px solid #ccc;padding:6px 10px;text-align:left}}\
             th{{background:#f7f7f7}}\
             h1,h2,h3{{margin-top:1em;margin-bottom:0.4em}}\
             blockquote{{margin:0;padding-left:12px;border-left:3px solid #ddd;color:#666}}\
             </style></head><body>{rendered}</body></html>"
        )
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
