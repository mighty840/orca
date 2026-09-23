use std::sync::Arc;

use chrono::Utc;
use uuid::Uuid;

use crate::backend::{ChatMessage, LlmBackend, Role};
use crate::channels::{AlertEvent, Dispatcher};
use crate::context::ClusterContext;
use orca_core::types::{
    AlertConversation, AlertMessage, AlertSender, AlertSeverity, AlertState, ConversationId,
};

/// Manages ongoing alert conversations. Each alert is a living thread where
/// the AI investigates, reports findings, suggests fixes, and tracks resolution.
pub struct ConversationEngine<B: LlmBackend> {
    // Shared, so the monitor can call the model and deliver alerts without
    // holding the engine lock across that network I/O (#181).
    backend: Arc<B>,
    conversations: Vec<AlertConversation>,
    dispatcher: Arc<Dispatcher>,
}

impl<B: LlmBackend> ConversationEngine<B> {
    pub fn new(backend: B) -> Self {
        Self::with_dispatcher(backend, Dispatcher::empty())
    }

    pub fn with_dispatcher(backend: B, dispatcher: Dispatcher) -> Self {
        Self {
            backend: Arc::new(backend),
            conversations: Vec::new(),
            dispatcher: Arc::new(dispatcher),
        }
    }

    /// The model backend, for calls made without holding the engine lock.
    pub fn backend(&self) -> Arc<B> {
        Arc::clone(&self.backend)
    }

    /// The delivery channels, for dispatching without holding the engine lock.
    pub fn dispatcher(&self) -> Arc<Dispatcher> {
        Arc::clone(&self.dispatcher)
    }

    /// The prompt that asks the model to diagnose a new alert.
    pub fn open_prompt(
        service: &str,
        trigger_event: &str,
        context: &ClusterContext,
    ) -> Vec<ChatMessage> {
        vec![
            ChatMessage {
                role: Role::System,
                content: context.to_system_prompt(),
            },
            ChatMessage {
                role: Role::User,
                content: format!(
                    "Alert triggered for service '{service}': {trigger_event}\n\n\
                     Investigate this issue. Explain what's happening, the likely root cause, \
                     and suggest a fix as an `orca` command. If the issue might resolve itself, say so."
                ),
            },
        ]
    }

    /// Record a new alert, with the model's diagnosis or the reason there is
    /// none. No I/O: callers dispatch the returned snapshot themselves, after
    /// releasing the engine lock. Returns `None` if the service already has
    /// an active alert (so a slow diagnosis can't open a duplicate).
    pub fn record_open(
        &mut self,
        service: &str,
        severity: AlertSeverity,
        trigger_event: &str,
        diagnosis: Result<String, String>,
    ) -> Option<AlertConversation> {
        if self
            .active_conversations()
            .iter()
            .any(|c| c.service == service)
        {
            return None;
        }
        let id = self.push_opened(service, severity, trigger_event, diagnosis);
        self.conversations.iter().find(|c| c.id == id).cloned()
    }

    /// Mark an alert remediated and return its snapshot. No I/O.
    pub fn record_remediated(
        &mut self,
        conversation_id: ConversationId,
        action_taken: &str,
    ) -> Option<AlertConversation> {
        let conv = self
            .conversations
            .iter_mut()
            .find(|c| c.id == conversation_id)?;
        conv.state = AlertState::Remediated;
        conv.resolved_at = Some(Utc::now());
        conv.messages.push(AlertMessage {
            timestamp: Utc::now(),
            sender: AlertSender::System,
            content: format!("Auto-remediation applied: {action_taken}"),
            suggested_command: None,
        });
        Some(conv.clone())
    }

    /// Append an opened conversation. With no diagnosis, the alert still
    /// carries its trigger plus the reason the model gave none: a degraded
    /// alert beats no alert (#181).
    fn push_opened(
        &mut self,
        service: &str,
        severity: AlertSeverity,
        trigger_event: &str,
        diagnosis: Result<String, String>,
    ) -> ConversationId {
        let trigger = AlertMessage {
            timestamp: Utc::now(),
            sender: AlertSender::System,
            content: trigger_event.to_string(),
            suggested_command: None,
        };
        let (state, second) = match diagnosis {
            Ok(text) => {
                let (suggested_command, content) = extract_command(&text);
                let state = if suggested_command.is_some() {
                    AlertState::AwaitingAction
                } else {
                    AlertState::Investigating
                };
                (
                    state,
                    AlertMessage {
                        timestamp: Utc::now(),
                        sender: AlertSender::Orca,
                        content,
                        suggested_command,
                    },
                )
            }
            Err(reason) => (
                AlertState::Investigating,
                AlertMessage {
                    timestamp: Utc::now(),
                    sender: AlertSender::System,
                    content: format!(
                        "AI diagnosis unavailable ({reason}). This alert carries the raw \
                         signal above only."
                    ),
                    suggested_command: None,
                },
            ),
        };
        let conversation = AlertConversation {
            id: Uuid::now_v7(),
            service: service.to_string(),
            severity,
            state,
            started_at: Utc::now(),
            resolved_at: None,
            messages: vec![trigger, second],
        };
        let id = conversation.id;
        self.conversations.push(conversation);
        id
    }

    /// Open an alert with a fixed message and no LLM call, for failures
    /// orca detects deterministically (a failed backup, #197). Delivery must
    /// not depend on the model endpoint being up: that's the #181 failure mode.
    pub async fn open_system_alert(
        &mut self,
        service: &str,
        severity: AlertSeverity,
        message: &str,
    ) -> &AlertConversation {
        let conversation = AlertConversation {
            id: Uuid::now_v7(),
            service: service.to_string(),
            severity,
            state: AlertState::Investigating,
            started_at: Utc::now(),
            resolved_at: None,
            messages: vec![AlertMessage {
                timestamp: Utc::now(),
                sender: AlertSender::System,
                content: message.to_string(),
                suggested_command: None,
            }],
        };
        let id = conversation.id;
        self.conversations.push(conversation);
        self.dispatch_for(id, AlertEvent::Opened).await;
        self.conversations
            .iter()
            .find(|c| c.id == id)
            .expect("just pushed")
    }

    /// Start a new alert conversation, with the AI's initial diagnosis when
    /// the model answers. Holds `&mut self` across the model call; the monitor
    /// uses [`Self::record_open`] instead to avoid that.
    pub async fn open_alert(
        &mut self,
        service: &str,
        severity: AlertSeverity,
        trigger_event: &str,
        context: &ClusterContext,
    ) -> anyhow::Result<&AlertConversation> {
        // A failed or slow model no longer means no alert (#181): the
        // conversation opens either way, degraded when there's no diagnosis.
        let messages = Self::open_prompt(service, trigger_event, context);
        let diagnosis = self
            .backend
            .chat(&messages)
            .await
            .map(|r| r.content)
            .map_err(|e| format!("{e:#}"));
        let id = self.push_opened(service, severity, trigger_event, diagnosis);
        self.dispatch_for(id, AlertEvent::Opened).await;
        Ok(self
            .conversations
            .iter()
            .find(|c| c.id == id)
            .expect("just pushed"))
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
            self.dispatch_for(conversation_id, AlertEvent::Dismissed)
                .await;
            return Ok(self
                .conversations
                .iter()
                .find(|c| c.id == conversation_id)
                .expect("just mutated"));
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
            self.dispatch_for(conversation_id, AlertEvent::Resolved)
                .await;
            return Ok(self
                .conversations
                .iter()
                .find(|c| c.id == conversation_id)
                .expect("just mutated"));
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

        self.dispatch_for(conversation_id, AlertEvent::Updated)
            .await;
        Ok(self
            .conversations
            .iter()
            .find(|c| c.id == conversation_id)
            .expect("just mutated"))
    }

    /// Feed new data into an existing conversation (e.g., the issue got worse, or metrics changed).
    pub async fn update_alert(
        &mut self,
        conversation_id: ConversationId,
        update: &str,
        context: &ClusterContext,
    ) -> anyhow::Result<&AlertConversation> {
        // Inject as a system message, then get AI's updated analysis
        self.operator_reply(
            conversation_id,
            &format!("[System update] {update}"),
            context,
        )
        .await
    }

    /// Mark an alert as remediated (auto-fix was applied).
    pub async fn mark_remediated(&mut self, conversation_id: ConversationId, action_taken: &str) {
        if self
            .record_remediated(conversation_id, action_taken)
            .is_some()
        {
            self.dispatch_for(conversation_id, AlertEvent::Remediated)
                .await;
        }
    }

    /// Fan a delivery event for the conversation with `id`. Looks the
    /// conversation up by id and clones a snapshot so we don't hold a borrow
    /// across the dispatcher's await.
    async fn dispatch_for(&self, id: ConversationId, event: AlertEvent) {
        if self.dispatcher.is_empty() {
            return;
        }
        if let Some(snapshot) = self.conversations.iter().find(|c| c.id == id).cloned() {
            self.dispatcher.dispatch(&snapshot, event).await;
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

use crate::command_parser::extract_command;
