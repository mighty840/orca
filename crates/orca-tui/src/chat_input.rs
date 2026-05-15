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
    let pending = state.chat_pending;
    match code {
        // ---- always-available: navigation, scrolling, clearing input ----
        KeyCode::Esc => state.chat_input.clear(),
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
        KeyCode::Char('7') if state.chat_input.is_empty() => {
            crate::refresh_alerts(client, state).await;
            state.selected_alert = 0;
            state.push_view(View::Alerts);
        }
        // ---- composition: only when not waiting on the AI ----
        KeyCode::Backspace if !pending => {
            state.chat_input.pop();
        }
        KeyCode::Enter if !pending => send_chat(state, client).await,
        KeyCode::Char(c) if !pending => state.chat_input.push(c),
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

    // ---------- chat-view key handler contract ----------
    //
    // These tests pin the navigation contract from `View::Chat` that was
    // missed in two regressions:
    //   1. `7` not handled — pressing it appended '7' to the input buffer
    //      instead of switching to Alerts.
    //   2. `chat_pending = true` short-circuited the handler, locking out
    //      every navigation key (including the digit shortcuts) until the
    //      LLM call returned.
    // The tests are coarse-grained on purpose — they assert behaviour the
    // user can directly observe (view change, buffer content), not internal
    // structure, so a future refactor doesn't break them without cause.

    use crate::api::ApiClient;
    use crossterm::event::KeyCode;

    /// Build a client pointed at a port nothing listens on so the
    /// `refresh_*` HTTP calls inside the handler fail fast (ECONNREFUSED
    /// is immediate on localhost — no timeout in the way). The handler is
    /// supposed to push the view regardless of HTTP success, which is what
    /// we're verifying.
    fn dead_client() -> ApiClient {
        ApiClient::new("http://127.0.0.1:1")
    }

    fn chat_state() -> AppState {
        let mut s = AppState::new();
        s.view = View::Chat;
        s
    }

    #[tokio::test]
    async fn digit_one_through_seven_switch_views_from_empty_chat() {
        // The whole digit shortcut surface — `7` was the regression that
        // motivated this test. The earlier code missed it entirely so
        // pressing it just typed '7'. This loop catches that AND any
        // future drift (e.g. a new digit silently disappearing).
        let cases = [
            ('1', View::Services),
            ('2', View::Nodes),
            ('3', View::Secrets),
            ('4', View::Backups),
            ('5', View::Webhooks),
            ('6', View::Networks),
            ('7', View::Alerts),
        ];
        for (key, expected) in cases {
            let mut state = chat_state();
            let client = dead_client();
            handle_chat_key(&mut state, &client, KeyCode::Char(key)).await;
            assert_eq!(
                state.view, expected,
                "key '{key}' should switch to {expected:?}, got {:?}",
                state.view
            );
            assert!(
                state.chat_input.is_empty(),
                "key '{key}' must NOT be buffered as a character"
            );
        }
    }

    #[tokio::test]
    async fn digit_does_not_navigate_when_input_buffer_has_text() {
        // Mid-word digits are characters, not shortcuts — otherwise typing
        // "what's the limit at 5" jumps you to Webhooks halfway through.
        let mut state = chat_state();
        state.chat_input = "hello ".into();
        let client = dead_client();
        handle_chat_key(&mut state, &client, KeyCode::Char('1')).await;
        assert_eq!(
            state.view,
            View::Chat,
            "input has content; digit must not navigate"
        );
        assert_eq!(state.chat_input, "hello 1");
    }

    #[tokio::test]
    async fn navigation_works_while_chat_pending() {
        // The whole point of async dispatch is that the user can leave
        // the chat view while the AI is thinking. An earlier build
        // returned early when `chat_pending` was true and froze the TUI.
        let cases = [
            ('1', View::Services),
            ('2', View::Nodes),
            ('6', View::Networks),
            ('7', View::Alerts),
        ];
        for (key, expected) in cases {
            let mut state = chat_state();
            state.chat_pending = true;
            let client = dead_client();
            handle_chat_key(&mut state, &client, KeyCode::Char(key)).await;
            assert_eq!(
                state.view, expected,
                "navigation key '{key}' MUST work while chat is pending (regression test)"
            );
        }
    }

    #[tokio::test]
    async fn typing_is_suppressed_while_chat_pending() {
        // Composition keys (Backspace, Char, Enter) are the only ones
        // gated on `!pending` — a queued second message while one is in
        // flight would race the engine.
        let mut state = chat_state();
        state.chat_pending = true;
        let client = dead_client();
        handle_chat_key(&mut state, &client, KeyCode::Char('x')).await;
        assert_eq!(state.chat_input, "", "character typing must be suppressed");
        handle_chat_key(&mut state, &client, KeyCode::Backspace).await;
        // backspace ignored — buffer was empty anyway, but importantly no panic
        assert_eq!(state.chat_input, "");
    }

    #[tokio::test]
    async fn esc_clears_input_even_while_pending() {
        // Esc is the user's "I changed my mind about this draft" escape
        // hatch and must remain available regardless of in-flight state.
        let mut state = chat_state();
        state.chat_input = "in-progress draft".into();
        state.chat_pending = true;
        let client = dead_client();
        handle_chat_key(&mut state, &client, KeyCode::Esc).await;
        assert_eq!(state.chat_input, "");
    }

    #[tokio::test]
    async fn scroll_keys_work_independent_of_pending() {
        // PgUp/PgDn/Up/Down scroll the transcript — they should never be
        // gated on input or pending state.
        for pending in [false, true] {
            let mut state = chat_state();
            state.chat_pending = pending;
            state.chat_scroll = 5;
            let client = dead_client();
            handle_chat_key(&mut state, &client, KeyCode::PageUp).await;
            assert_eq!(state.chat_scroll, 15);
            handle_chat_key(&mut state, &client, KeyCode::PageDown).await;
            assert_eq!(state.chat_scroll, 5);
            handle_chat_key(&mut state, &client, KeyCode::Up).await;
            assert_eq!(state.chat_scroll, 6);
            handle_chat_key(&mut state, &client, KeyCode::Down).await;
            assert_eq!(state.chat_scroll, 5);
        }
    }

    #[tokio::test]
    async fn q_quits_only_when_input_is_empty() {
        // 'q' is a common letter — must not quit mid-word. Empty buffer +
        // 'q' is the conventional quit.
        let mut state = chat_state();
        state.chat_input = "qu".into();
        let client = dead_client();
        handle_chat_key(&mut state, &client, KeyCode::Char('q')).await;
        assert!(!state.should_quit, "q must NOT quit while typing");
        assert_eq!(state.chat_input, "quq");

        let mut empty = chat_state();
        handle_chat_key(&mut empty, &client, KeyCode::Char('q')).await;
        assert!(empty.should_quit, "q on empty buffer should quit");
    }

    #[tokio::test]
    async fn enter_on_slash_dispatches_to_view() {
        // The integration between input buffer → slash parse → view push.
        let mut state = chat_state();
        state.chat_input = "/nodes".into();
        let client = dead_client();
        handle_chat_key(&mut state, &client, KeyCode::Enter).await;
        assert_eq!(state.view, View::Nodes);
        assert_eq!(state.chat_input, "", "input must clear after Enter");
    }

    #[tokio::test]
    async fn enter_on_empty_input_is_noop() {
        // Avoid burning an LLM call on stray Enters.
        let mut state = chat_state();
        let client = dead_client();
        handle_chat_key(&mut state, &client, KeyCode::Enter).await;
        assert!(state.chat.is_empty(), "no turn should have been pushed");
        assert!(!state.chat_pending, "no request should have been spawned");
    }
}
