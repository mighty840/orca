//! Background dispatch + completion polling for the chat view.
//!
//! Pulled out of `lib.rs` so the chat lifecycle (send → spawn → drain) is
//! one focused thing to read and `lib.rs` stays under the file-size
//! ceiling.

use crate::api::ApiClient;
use crate::state::{AppState, ChatRole, ChatTaskResult, ChatTurn};

/// Send a chat turn to `/api/v1/ask`. Appends the user turn immediately,
/// then dispatches the LLM call as a background tokio task. The result
/// lands on `state.chat_result_rx`; [`drain_chat_result`] picks it up on
/// the next event-loop tick — so the TUI stays responsive while the AI
/// is thinking.
pub(crate) fn send_chat_message(state: &mut AppState, client: &ApiClient, question: String) {
    let history: Vec<(String, String)> = state
        .chat
        .iter()
        .map(|t| {
            let role = match t.role {
                ChatRole::User => "user",
                ChatRole::Assistant => "assistant",
            };
            (role.to_string(), t.content.clone())
        })
        .collect();
    state.chat.push(ChatTurn {
        role: ChatRole::User,
        content: question.clone(),
    });
    // Sending pins the view to the latest exchange — every chat client
    // works this way. Manual scrolling resumes after the reply lands.
    state.chat_scroll = 0;
    state.chat_pending = true;

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    state.chat_result_rx = Some(rx);
    let client = client.clone();
    tokio::spawn(async move {
        let result = match client.ask(&question, &history).await {
            Ok(Some(reply)) => ChatTaskResult::Reply(reply),
            Ok(None) => ChatTaskResult::Unavailable,
            Err(e) => ChatTaskResult::Error(format!("{e}")),
        };
        // Receiver may have been dropped if the user quit; ignore.
        let _ = tx.send(result);
    });
}

/// Poll the background chat task — called every event-loop tick. Cheap
/// when nothing is pending. On completion, appends the assistant turn
/// (or an error turn) and clears the spinner state.
pub(crate) fn drain_chat_result(state: &mut AppState) {
    let Some(rx) = state.chat_result_rx.as_mut() else {
        return;
    };
    match rx.try_recv() {
        Ok(result) => {
            state.chat_result_rx = None;
            state.chat_pending = false;
            let turn = match result {
                ChatTaskResult::Reply(r) => {
                    state.chat_unavailable = false;
                    ChatTurn {
                        role: ChatRole::Assistant,
                        content: r,
                    }
                }
                ChatTaskResult::Unavailable => {
                    state.chat_unavailable = true;
                    ChatTurn {
                        role: ChatRole::Assistant,
                        content: "AI is not configured on this server (HTTP 503). Add an [ai] block to cluster.toml and restart.".into(),
                    }
                }
                ChatTaskResult::Error(e) => ChatTurn {
                    role: ChatRole::Assistant,
                    content: format!("(error: {e})"),
                },
            };
            state.chat.push(turn);
        }
        Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
            state.chat_result_rx = None;
            state.chat_pending = false;
        }
    }
}
