//! Alert delivery channels (Slack, generic webhook, email).
//!
//! `ConversationEngine` holds an optional [`Dispatcher`] that fans alert
//! events to every configured channel in parallel. A failure in one channel
//! does not block the others — each channel is awaited concurrently and
//! errors are logged but swallowed so the engine's mutation completes.

mod email;
mod slack;
mod webhook;

use async_trait::async_trait;
use futures_util::future::join_all;
use tracing::warn;

use orca_core::config::AlertDeliveryChannels;
use orca_core::types::AlertConversation;

pub use email::EmailChannel;
pub use slack::SlackChannel;
pub use webhook::WebhookChannel;

/// What just happened to a conversation. Lets channels render differently
/// for the opening shot vs ongoing updates vs the final state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertEvent {
    /// First message — initial diagnosis from the AI.
    Opened,
    /// Operator replied or system pushed new context.
    Updated,
    /// Auto-remediation applied; conversation moves to Remediated state.
    Remediated,
    /// Issue self-resolved or operator marked it resolved.
    Resolved,
    /// Operator dismissed the alert.
    Dismissed,
}

impl AlertEvent {
    pub fn label(self) -> &'static str {
        match self {
            AlertEvent::Opened => "Opened",
            AlertEvent::Updated => "Updated",
            AlertEvent::Remediated => "Remediated",
            AlertEvent::Resolved => "Resolved",
            AlertEvent::Dismissed => "Dismissed",
        }
    }
}

#[async_trait]
pub trait Channel: Send + Sync {
    fn name(&self) -> &'static str;
    async fn deliver(&self, conv: &AlertConversation, event: AlertEvent) -> anyhow::Result<()>;
}

/// Holds the configured delivery channels and fans events out in parallel.
/// Construct via [`Dispatcher::from_config`]; an empty dispatcher is a no-op
/// so callers don't need to special-case the unconfigured case.
pub struct Dispatcher {
    channels: Vec<Box<dyn Channel>>,
}

impl Dispatcher {
    pub fn new(channels: Vec<Box<dyn Channel>>) -> Self {
        Self { channels }
    }

    pub fn empty() -> Self {
        Self {
            channels: Vec::new(),
        }
    }

    /// Build a dispatcher from the cluster.toml `[ai.alerts.channels]` block.
    /// Channels with `None` config entries are skipped silently.
    pub fn from_config(cfg: &AlertDeliveryChannels) -> Self {
        let mut channels: Vec<Box<dyn Channel>> = Vec::new();
        if let Some(url) = &cfg.slack {
            channels.push(Box::new(SlackChannel::new(url.clone())));
        }
        if let Some(url) = &cfg.webhook {
            channels.push(Box::new(WebhookChannel::new(url.clone())));
        }
        if let Some(email) = &cfg.email {
            channels.push(Box::new(EmailChannel::new(email.clone())));
        }
        Self { channels }
    }

    pub fn is_empty(&self) -> bool {
        self.channels.is_empty()
    }

    pub fn channel_names(&self) -> Vec<&'static str> {
        self.channels.iter().map(|c| c.name()).collect()
    }

    /// Fan the event out to every channel in parallel. Failures are logged
    /// per-channel; this returns once all channels have either completed
    /// or errored. The caller's mutation is independent of delivery — we
    /// never make the engine's path fail because Slack is down.
    pub async fn dispatch(&self, conv: &AlertConversation, event: AlertEvent) {
        if self.channels.is_empty() {
            return;
        }
        let futures = self.channels.iter().map(|ch| {
            let name = ch.name();
            async move {
                if let Err(e) = ch.deliver(conv, event).await {
                    warn!(channel = name, alert = %conv.service, event = ?event, error = %e, "alert delivery failed");
                }
            }
        });
        join_all(futures).await;
    }
}

impl Default for Dispatcher {
    fn default() -> Self {
        Self::empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orca_core::types::{AlertSeverity, AlertState};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Mutex;

    struct CountingChannel {
        name: &'static str,
        count: Arc<AtomicUsize>,
        events: Arc<Mutex<Vec<AlertEvent>>>,
    }

    #[async_trait]
    impl Channel for CountingChannel {
        fn name(&self) -> &'static str {
            self.name
        }
        async fn deliver(&self, _: &AlertConversation, event: AlertEvent) -> anyhow::Result<()> {
            self.count.fetch_add(1, Ordering::SeqCst);
            self.events.lock().await.push(event);
            Ok(())
        }
    }

    struct FailingChannel;
    #[async_trait]
    impl Channel for FailingChannel {
        fn name(&self) -> &'static str {
            "failing"
        }
        async fn deliver(&self, _: &AlertConversation, _: AlertEvent) -> anyhow::Result<()> {
            anyhow::bail!("simulated channel failure")
        }
    }

    fn fake_conv() -> AlertConversation {
        AlertConversation {
            id: uuid::Uuid::now_v7(),
            service: "svc".into(),
            severity: AlertSeverity::Warning,
            state: AlertState::Investigating,
            started_at: chrono::Utc::now(),
            resolved_at: None,
            messages: Vec::new(),
        }
    }

    #[tokio::test]
    async fn dispatcher_fans_to_all_channels() {
        let count = Arc::new(AtomicUsize::new(0));
        let events = Arc::new(Mutex::new(Vec::new()));
        let d = Dispatcher::new(vec![
            Box::new(CountingChannel {
                name: "a",
                count: count.clone(),
                events: events.clone(),
            }),
            Box::new(CountingChannel {
                name: "b",
                count: count.clone(),
                events: events.clone(),
            }),
        ]);

        d.dispatch(&fake_conv(), AlertEvent::Opened).await;

        assert_eq!(count.load(Ordering::SeqCst), 2);
        assert_eq!(events.lock().await.len(), 2);
    }

    #[tokio::test]
    async fn dispatcher_failure_in_one_does_not_block_others() {
        let count = Arc::new(AtomicUsize::new(0));
        let events = Arc::new(Mutex::new(Vec::new()));
        let d = Dispatcher::new(vec![
            Box::new(FailingChannel),
            Box::new(CountingChannel {
                name: "good",
                count: count.clone(),
                events: events.clone(),
            }),
        ]);

        d.dispatch(&fake_conv(), AlertEvent::Opened).await;

        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "good channel must still receive the event when sibling fails"
        );
    }

    #[tokio::test]
    async fn empty_dispatcher_is_a_noop() {
        let d = Dispatcher::empty();
        assert!(d.is_empty());
        d.dispatch(&fake_conv(), AlertEvent::Opened).await;
    }
}
