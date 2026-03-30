use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::ConversationId;

/// An alert is not a dead report — it's a living conversation between the cluster and the operator.
/// Orca's AI observes the issue, opens a conversation, investigates, suggests fixes,
/// and keeps the thread going until the issue is resolved or acknowledged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertConversation {
    pub id: ConversationId,
    pub service: String,
    pub severity: AlertSeverity,
    pub state: AlertState,
    pub started_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub messages: Vec<AlertMessage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertState {
    /// AI is actively investigating.
    Investigating,
    /// AI has a diagnosis and suggested fix.
    AwaitingAction,
    /// Operator acknowledged, fix in progress.
    Acknowledged,
    /// Auto-remediation was applied.
    Remediated,
    /// Issue resolved (manually or automatically).
    Resolved,
    /// Operator dismissed this alert.
    Dismissed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertMessage {
    pub timestamp: DateTime<Utc>,
    pub sender: AlertSender,
    pub content: String,
    /// If this message proposes a fix, the command to run.
    pub suggested_command: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertSender {
    /// The AI assistant.
    Orca,
    /// The human operator.
    Operator,
    /// The system (automated events).
    System,
}
