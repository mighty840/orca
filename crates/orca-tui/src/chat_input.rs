//! Chat-view input handling — `handle_chat_key` + slash-command dispatcher.
//!
//! Kept in its own module so `keys.rs` stays under the 500-line file ceiling
//! and the chat-specific keymap is one focused thing to read.

use crossterm::event::KeyCode;

use crate::api::ApiClient;
use crate::state::{AppState, InputMode, View};

/// Handle a single key event while the user is on `View::Chat`.
///
/// Typing fills `chat_input`; Enter sends; Esc clears. Digit / `:` / `?` /
/// `q` shortcuts only fire when the buffer is empty so they don't fight
/// a mid-word draft.
pub(crate) async fn handle_chat_key(state: &mut AppState, client: &ApiClient, code: KeyCode) {
    if state.chat_pending {
        if matches!(code, KeyCode::Esc) {
            state.chat_input.clear();
        }
        return;
    }
    match code {
        // Transcript scrolling. `chat_scroll` is "lines back from latest"
        // — 0 means pinned to the bottom, increases as you scroll up.
        KeyCode::PageUp => {
            state.chat_scroll = state.chat_scroll.saturating_add(10);
        }
        KeyCode::PageDown => {
            state.chat_scroll = state.chat_scroll.saturating_sub(10);
        }
        KeyCode::Up => {
            state.chat_scroll = state.chat_scroll.saturating_add(1);
        }
        KeyCode::Down => {
            state.chat_scroll = state.chat_scroll.saturating_sub(1);
        }
        KeyCode::Esc => state.chat_input.clear(),
        KeyCode::Backspace => {
            state.chat_input.pop();
        }
        KeyCode::Enter => send_chat(state, client).await,
        KeyCode::Char('q') if state.chat_input.is_empty() => state.should_quit = true,
        KeyCode::Char(':') if state.chat_input.is_empty() => {
            state.input_mode = InputMode::Command;
            state.command_input.clear();
        }
        KeyCode::Char('?') if state.chat_input.is_empty() => state.push_view(View::Help),
        KeyCode::Char('1') if state.chat_input.is_empty() => {
            state.view_stack.clear();
            state.view = View::Services;
        }
        KeyCode::Char('2') if state.chat_input.is_empty() => state.push_view(View::Nodes),
        KeyCode::Char('3') if state.chat_input.is_empty() => {
            crate::refresh_secrets_usage(client, state).await;
            state.selected_secret = 0;
            state.push_view(View::Secrets);
        }
        KeyCode::Char('4') if state.chat_input.is_empty() => {
            crate::refresh_backups(client, state).await;
            state.selected_backup_node = 0;
            state.push_view(View::Backups);
        }
        KeyCode::Char('5') if state.chat_input.is_empty() => {
            crate::refresh_webhooks(client, state).await;
            state.selected_webhook = 0;
            state.push_view(View::Webhooks);
        }
        KeyCode::Char('6') if state.chat_input.is_empty() => {
            crate::refresh_networks(client, state).await;
            state.network_scroll = 0;
            state.push_view(View::Networks);
        }
        KeyCode::Char(c) => state.chat_input.push(c),
        _ => {}
    }
}

/// Parse a `/cmd` line into a `SlashAction`. Pure function so we can pin
/// the routing in unit tests without standing up the whole TUI state.
pub(crate) fn parse_slash(raw: &str) -> Option<SlashAction> {
    let rest = raw.strip_prefix('/')?;
    let mut parts = rest.split_whitespace();
    let verb = parts.next()?;
    Some(match verb {
        "services" | "svc" => SlashAction::Goto(View::Services),
        "nodes" => SlashAction::Goto(View::Nodes),
        "secrets" => SlashAction::Goto(View::Secrets),
        "backups" => SlashAction::Goto(View::Backups),
        "webhooks" => SlashAction::Goto(View::Webhooks),
        "networks" => SlashAction::Goto(View::Networks),
        "logs" => match parts.next() {
            Some(svc) => SlashAction::Logs(svc.to_string()),
            None => SlashAction::Usage("Usage: /logs <service>".to_string()),
        },
        "clear" | "reset" => SlashAction::Clear,
        "help" | "?" => SlashAction::Goto(View::Help),
        other => SlashAction::Unknown(other.to_string()),
    })
}

#[derive(Debug, PartialEq)]
pub(crate) enum SlashAction {
    Goto(View),
    Logs(String),
    Clear,
    Usage(String),
    Unknown(String),
}

/// Submit the current chat input. Empty input is a no-op. Lines starting
/// with `/` are slash commands; anything else hits `/api/v1/ask`.
async fn send_chat(state: &mut AppState, client: &ApiClient) {
    let raw = state.chat_input.trim().to_string();
    state.chat_input.clear();
    if raw.is_empty() {
        return;
    }
    if let Some(action) = parse_slash(&raw) {
        match action {
            SlashAction::Goto(view) => match view {
                View::Services => {
                    state.view_stack.clear();
                    state.view = View::Services;
                }
                View::Secrets => {
                    crate::refresh_secrets_usage(client, state).await;
                    state.selected_secret = 0;
                    state.push_view(View::Secrets);
                }
                View::Backups => {
                    crate::refresh_backups(client, state).await;
                    state.push_view(View::Backups);
                }
                View::Webhooks => {
                    crate::refresh_webhooks(client, state).await;
                    state.push_view(View::Webhooks);
                }
                View::Networks => {
                    crate::refresh_networks(client, state).await;
                    state.push_view(View::Networks);
                }
                other => state.push_view(other),
            },
            SlashAction::Logs(svc) => {
                crate::refresh_logs_named(client, state, &svc).await;
                state.push_view(View::Logs { service: svc });
            }
            SlashAction::Clear => state.chat.clear(),
            SlashAction::Usage(msg) => state.flash(msg),
            SlashAction::Unknown(verb) => state.flash(format!("Unknown slash: /{verb}")),
        }
        return;
    }
    crate::send_chat_message(state, client, raw);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_routes_named_views() {
        assert_eq!(
            parse_slash("/services"),
            Some(SlashAction::Goto(View::Services))
        );
        assert_eq!(parse_slash("/svc"), Some(SlashAction::Goto(View::Services)));
        assert_eq!(parse_slash("/nodes"), Some(SlashAction::Goto(View::Nodes)));
        assert_eq!(
            parse_slash("/secrets"),
            Some(SlashAction::Goto(View::Secrets))
        );
        assert_eq!(
            parse_slash("/backups"),
            Some(SlashAction::Goto(View::Backups))
        );
        assert_eq!(
            parse_slash("/webhooks"),
            Some(SlashAction::Goto(View::Webhooks))
        );
        assert_eq!(
            parse_slash("/networks"),
            Some(SlashAction::Goto(View::Networks))
        );
        assert_eq!(parse_slash("/help"), Some(SlashAction::Goto(View::Help)));
        assert_eq!(parse_slash("/?"), Some(SlashAction::Goto(View::Help)));
    }

    #[test]
    fn slash_logs_with_and_without_arg() {
        assert_eq!(
            parse_slash("/logs api"),
            Some(SlashAction::Logs("api".to_string()))
        );
        assert!(matches!(parse_slash("/logs"), Some(SlashAction::Usage(_))));
    }

    #[test]
    fn slash_clear_alias() {
        assert_eq!(parse_slash("/clear"), Some(SlashAction::Clear));
        assert_eq!(parse_slash("/reset"), Some(SlashAction::Clear));
    }

    #[test]
    fn slash_unknown_verb_reports() {
        assert_eq!(
            parse_slash("/foo bar"),
            Some(SlashAction::Unknown("foo".to_string()))
        );
    }

    #[test]
    fn non_slash_input_returns_none() {
        // Anything that doesn't start with `/` is a regular chat message,
        // not a slash. Caller hands it to `send_chat_message`.
        assert!(parse_slash("hello world").is_none());
        assert!(parse_slash("").is_none());
    }
}
