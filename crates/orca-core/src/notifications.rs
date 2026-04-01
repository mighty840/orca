//! Notification dispatch to Slack, Discord, and email channels.

use serde::Serialize;
use tracing::{info, warn};

use crate::config::ObservabilityConfig;

/// A notification channel target.
#[derive(Debug, Clone)]
pub enum NotificationChannel {
    /// Slack/Discord-compatible webhook.
    Webhook { url: String },
    /// Email notification (SMTP delivery deferred to M5).
    Email {
        smtp_host: String,
        smtp_port: u16,
        from: String,
        to: String,
    },
}

/// Dispatches notifications to all configured channels.
#[derive(Debug, Clone)]
pub struct Notifier {
    channels: Vec<NotificationChannel>,
    client: reqwest::Client,
}

#[derive(Serialize)]
struct WebhookPayload {
    text: String,
}

impl Notifier {
    /// Create a notifier with the given channels.
    pub fn new(channels: Vec<NotificationChannel>) -> Self {
        Self {
            channels,
            client: reqwest::Client::new(),
        }
    }

    /// Build a notifier from cluster observability config.
    ///
    /// Reads `observability.alerts.webhook` and `observability.alerts.email` fields.
    pub fn from_config(config: &ObservabilityConfig) -> Self {
        let mut channels = Vec::new();

        if let Some(ref alerts) = config.alerts {
            if let Some(ref url) = alerts.webhook {
                channels.push(NotificationChannel::Webhook { url: url.clone() });
            }
            if let Some(ref email) = alerts.email {
                channels.push(NotificationChannel::Email {
                    smtp_host: "localhost".into(),
                    smtp_port: 587,
                    from: "orca@localhost".into(),
                    to: email.clone(),
                });
            }
        }

        Self::new(channels)
    }

    /// Send a notification to all configured channels.
    ///
    /// `severity` is informational (e.g. "info", "warning", "critical").
    /// Failures on individual channels are logged but do not abort the remaining sends.
    pub async fn send(&self, title: &str, message: &str, severity: &str) {
        for channel in &self.channels {
            match channel {
                NotificationChannel::Webhook { url } => {
                    self.send_webhook(url, title, message, severity).await;
                }
                NotificationChannel::Email {
                    smtp_host,
                    smtp_port,
                    from,
                    to,
                } => {
                    self.send_email(smtp_host, *smtp_port, from, to, title, message, severity)
                        .await;
                }
            }
        }
    }

    async fn send_webhook(&self, url: &str, title: &str, message: &str, severity: &str) {
        let payload = WebhookPayload {
            text: format!("[{severity}] {title}: {message}"),
        };

        match self.client.post(url).json(&payload).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    info!(url = %url, "notification sent via webhook");
                } else {
                    warn!(
                        url = %url,
                        status = %resp.status(),
                        "webhook returned non-success status"
                    );
                }
            }
            Err(e) => {
                warn!(url = %url, error = %e, "failed to send webhook notification");
            }
        }
    }

    /// Send email notification via the `sendmail` command if available,
    /// falling back to raw SMTP over TCP for internal relays.
    #[allow(clippy::too_many_arguments)]
    async fn send_email(
        &self,
        smtp_host: &str,
        smtp_port: u16,
        from: &str,
        to: &str,
        title: &str,
        message: &str,
        severity: &str,
    ) {
        let body = format!(
            "Subject: [orca/{severity}] {title}\r\nFrom: {from}\r\nTo: {to}\r\n\r\n{message}\r\n"
        );

        // Try sendmail first (most reliable for self-hosted setups)
        if Self::try_sendmail(to, &body) {
            info!(to = %to, "email sent via sendmail");
            return;
        }

        // Fall back to raw SMTP
        match Self::try_raw_smtp(smtp_host, smtp_port, from, to, &body).await {
            Ok(()) => info!(to = %to, "email sent via SMTP to {smtp_host}:{smtp_port}"),
            Err(e) => warn!(to = %to, error = %e, "failed to send email"),
        }
    }

    /// Attempt to send via the local sendmail binary.
    fn try_sendmail(to: &str, body: &str) -> bool {
        use std::io::Write;
        let Ok(mut child) = std::process::Command::new("sendmail")
            .arg("-t")
            .arg(to)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        else {
            return false;
        };
        if let Some(ref mut stdin) = child.stdin {
            let _ = stdin.write_all(body.as_bytes());
        }
        child.wait().is_ok_and(|s| s.success())
    }

    /// Send via raw SMTP (no TLS, suitable for internal relays).
    async fn try_raw_smtp(
        host: &str,
        port: u16,
        from: &str,
        to: &str,
        body: &str,
    ) -> anyhow::Result<()> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio::net::TcpStream;

        let stream = TcpStream::connect((host, port)).await?;
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut line = String::new();

        // Read greeting
        reader.read_line(&mut line).await?;
        line.clear();

        writer.write_all(b"HELO orca\r\n").await?;
        reader.read_line(&mut line).await?;
        line.clear();

        writer
            .write_all(format!("MAIL FROM:<{from}>\r\n").as_bytes())
            .await?;
        reader.read_line(&mut line).await?;
        line.clear();

        writer
            .write_all(format!("RCPT TO:<{to}>\r\n").as_bytes())
            .await?;
        reader.read_line(&mut line).await?;
        line.clear();

        writer.write_all(b"DATA\r\n").await?;
        reader.read_line(&mut line).await?;
        line.clear();

        writer.write_all(body.as_bytes()).await?;
        writer.write_all(b"\r\n.\r\n").await?;
        reader.read_line(&mut line).await?;
        line.clear();

        writer.write_all(b"QUIT\r\n").await?;

        Ok(())
    }

    /// Returns the number of configured channels.
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_config_with_webhook() {
        use crate::config::{AlertChannelConfig, ObservabilityConfig};

        let config = ObservabilityConfig {
            otlp_endpoint: None,
            alerts: Some(AlertChannelConfig {
                webhook: Some("https://hooks.slack.com/test".into()),
                email: Some("ops@example.com".into()),
            }),
        };

        let notifier = Notifier::from_config(&config);
        assert_eq!(notifier.channel_count(), 2);
    }

    #[test]
    fn from_config_no_alerts() {
        let config = ObservabilityConfig {
            otlp_endpoint: None,
            alerts: None,
        };

        let notifier = Notifier::from_config(&config);
        assert_eq!(notifier.channel_count(), 0);
    }

    #[test]
    fn empty_notifier() {
        let notifier = Notifier::new(vec![]);
        assert_eq!(notifier.channel_count(), 0);
    }
}
