//! The Webhooks and Alerts lists as the `/` filter narrows them (#265).
//! Selection indexes and actions go through these, never the raw lists.

use crate::state::AppState;

impl AppState {
    /// Webhooks matching the `/` filter (repo, branch or service).
    pub fn visible_webhooks(&self) -> Vec<&crate::api::WebhookEntry> {
        let f = self.webhook_filter.to_lowercase();
        self.webhooks
            .iter()
            .filter(|w| {
                f.is_empty()
                    || [&w.repo, &w.branch, &w.service_name]
                        .iter()
                        .any(|s| s.to_lowercase().contains(&f))
            })
            .collect()
    }

    /// Alerts matching the `/` filter (service or latest message).
    pub fn visible_alerts(&self) -> Vec<&crate::api::AlertConversation> {
        let f = self.alert_filter.to_lowercase();
        self.alerts
            .iter()
            .filter(|a| {
                f.is_empty()
                    || a.service.to_lowercase().contains(&f)
                    || a.messages
                        .last()
                        .is_some_and(|m| m.content.to_lowercase().contains(&f))
            })
            .collect()
    }
}
