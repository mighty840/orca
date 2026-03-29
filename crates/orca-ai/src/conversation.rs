use chrono::Utc;
use uuid::Uuid;

use crate::backend::{ChatMessage, LlmBackend, Role};
use crate::context::ClusterContext;
use orca_core::types::{
    AlertConversation, AlertMessage, AlertSender, AlertSeverity, AlertState, ConversationId,
};

/// Manages ongoing alert conversations. Each alert is a living thread where
/// the AI investigates, reports findings, suggests fixes, and tracks resolution.
pub struct ConversationEngine<B: LlmBackend> {
    backend: B,
    conversations: Vec<AlertConversation>,
}

impl<B: LlmBackend> ConversationEngine<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            conversations: Vec::new(),
        }
    }

    /// Start a new alert conversation. The AI opens with its initial diagnosis.
    pub async fn open_alert(
        &mut self,
        service: &str,
        severity: AlertSeverity,
        trigger_event: &str,
        context: &ClusterContext,
    ) -> anyhow::Result<&AlertConversation> {
        let system_prompt = context.to_system_prompt();

        let messages = vec![
            ChatMessage {
                role: Role::System,
                content: system_prompt,
            },
            ChatMessage {
                role: Role::User,
                content: format!(
                    "Alert triggered for service '{service}': {trigger_event}\n\n\
                     Investigate this issue. Explain what's happening, the likely root cause, \
                     and suggest a fix as an `orca` command. If the issue might resolve itself, say so."
                ),
            },
        ];

        let response = self.backend.chat(&messages).await?;

        let (suggested_command, content) = extract_command(&response.content);

        let conversation = AlertConversation {
            id: Uuid::now_v7(),
            service: service.to_string(),
            severity,
            state: if suggested_command.is_some() {
                AlertState::AwaitingAction
            } else {
                AlertState::Investigating
            },
            started_at: Utc::now(),
            resolved_at: None,
            messages: vec![
                AlertMessage {
                    timestamp: Utc::now(),
                    sender: AlertSender::System,
                    content: trigger_event.to_string(),
                    suggested_command: None,
                },
                AlertMessage {
                    timestamp: Utc::now(),
                    sender: AlertSender::Orca,
                    content,
                    suggested_command,
                },
            ],
        };

        self.conversations.push(conversation);
        Ok(self.conversations.last().unwrap())
    }

    /// Operator responds to an alert conversation (ask follow-up, approve fix, dismiss).
    pub async fn operator_reply(
        &mut self,
        conversation_id: ConversationId,
        message: &str,
        context: &ClusterContext,
    ) -> anyhow::Result<&AlertConversation> {
        let conv = self
            .conversations
            .iter_mut()
            .find(|c| c.id == conversation_id)
            .ok_or_else(|| anyhow::anyhow!("conversation not found"))?;

        conv.messages.push(AlertMessage {
            timestamp: Utc::now(),
            sender: AlertSender::Operator,
            content: message.to_string(),
            suggested_command: None,
        });

        // Check for special operator commands
        let lower = message.trim().to_lowercase();
        if lower == "dismiss" || lower == "ignore" {
            conv.state = AlertState::Dismissed;
            conv.resolved_at = Some(Utc::now());
            conv.messages.push(AlertMessage {
                timestamp: Utc::now(),
                sender: AlertSender::System,
                content: "Alert dismissed by operator.".to_string(),
                suggested_command: None,
            });
            return Ok(conv);
        }

        if lower == "resolve" || lower == "resolved" {
            conv.state = AlertState::Resolved;
            conv.resolved_at = Some(Utc::now());
            conv.messages.push(AlertMessage {
                timestamp: Utc::now(),
                sender: AlertSender::System,
                content: "Alert marked as resolved by operator.".to_string(),
                suggested_command: None,
            });
            return Ok(conv);
        }

        // Build chat history for continued conversation
        let system_prompt = context.to_system_prompt();
        let mut messages = vec![ChatMessage {
            role: Role::System,
            content: system_prompt,
        }];

        for msg in &conv.messages {
            let role = match msg.sender {
                AlertSender::Orca => Role::Assistant,
                AlertSender::Operator | AlertSender::System => Role::User,
            };
            messages.push(ChatMessage {
                role,
                content: msg.content.clone(),
            });
        }

        let response = self.backend.chat(&messages).await?;
        let (suggested_command, content) = extract_command(&response.content);

        if suggested_command.is_some() {
            conv.state = AlertState::AwaitingAction;
        }

        conv.messages.push(AlertMessage {
            timestamp: Utc::now(),
            sender: AlertSender::Orca,
            content,
            suggested_command,
        });

        Ok(conv)
    }

    /// Feed new data into an existing conversation (e.g., the issue got worse, or metrics changed).
    pub async fn update_alert(
        &mut self,
        conversation_id: ConversationId,
        update: &str,
        context: &ClusterContext,
    ) -> anyhow::Result<&AlertConversation> {
        // Inject as a system message, then get AI's updated analysis
        self.operator_reply(conversation_id, &format!("[System update] {update}"), context)
            .await
    }

    /// Mark an alert as remediated (auto-fix was applied).
    pub fn mark_remediated(&mut self, conversation_id: ConversationId, action_taken: &str) {
        if let Some(conv) = self
            .conversations
            .iter_mut()
            .find(|c| c.id == conversation_id)
        {
            conv.state = AlertState::Remediated;
            conv.resolved_at = Some(Utc::now());
            conv.messages.push(AlertMessage {
                timestamp: Utc::now(),
                sender: AlertSender::System,
                content: format!("Auto-remediation applied: {action_taken}"),
                suggested_command: None,
            });
        }
    }

    pub fn active_conversations(&self) -> Vec<&AlertConversation> {
        self.conversations
            .iter()
            .filter(|c| {
                !matches!(
                    c.state,
                    AlertState::Resolved | AlertState::Dismissed | AlertState::Remediated
                )
            })
            .collect()
    }

    pub fn get_conversation(&self, id: ConversationId) -> Option<&AlertConversation> {
        self.conversations.iter().find(|c| c.id == id)
    }

    pub fn all_conversations(&self) -> &[AlertConversation] {
        &self.conversations
    }
}

/// Extract an `orca ...` command from the AI's response, if present.
fn extract_command(content: &str) -> (Option<String>, String) {
    // Look for lines like: `orca scale api 5` or ```orca config set ...```
    for line in content.lines() {
        let trimmed = line.trim().trim_start_matches('`').trim_end_matches('`');
        if trimmed.starts_with("orca ") {
            return (Some(trimmed.to_string()), content.to_string());
        }
    }
    (None, content.to_string())
}
