//! Live log following for the Logs view (#265).
//!
//! The view polled the last 50 lines every 2 s and replaced its buffer, so
//! anything older than 50 lines was gone and fast output was missed. It now
//! streams `?follow=true`, appends what arrives, and keeps up to
//! [`MAX_LOG_LINES`]. The master only streams its own services; for one on
//! an agent it sends a single batch and closes. When a stream ends, the view
//! falls back to polling that service.

use std::collections::HashSet;

use tokio::task::AbortHandle;

use crate::api::ApiClient;
use crate::background::{Fetched, LOG_TAIL};
use crate::state::{AppState, View};

/// Lines kept in a followed log.
pub const MAX_LOG_LINES: usize = 5000;

#[derive(Default)]
pub struct FollowState {
    /// The running stream: its service and a handle to stop it.
    running: Option<(String, AbortHandle)>,
    /// True until the running stream's first chunk, which replaces the
    /// buffer instead of appending to it.
    fresh: bool,
    /// Services whose stream ended; they are polled instead.
    polled: HashSet<String>,
}

/// Whether `service`'s log is being streamed (or is about to be), so the
/// caller shouldn't poll it.
pub(crate) fn is_followed(state: &AppState, service: &str) -> bool {
    !state.bg.follow.polled.contains(service)
}

/// Keep the stream matching the view: start it in a Logs view, stop it
/// anywhere else. Called every tick.
pub(crate) fn sync(client: &ApiClient, state: &mut AppState) {
    if !matches!(state.view, View::Logs { .. }) {
        // Next time, try streaming again: the container may be back.
        state.bg.follow.polled.clear();
    }
    let wanted = match &state.view {
        View::Logs { service } if is_followed(state, service) => Some(service.clone()),
        _ => None,
    };
    let running = state.bg.follow.running.as_ref().map(|(s, _)| s.clone());
    if running == wanted {
        return;
    }
    if let Some((_, handle)) = state.bg.follow.running.take() {
        handle.abort();
    }
    let Some(service) = wanted else {
        return;
    };
    let tx = state.bg.tx.clone();
    let client = client.clone();
    let name = service.clone();
    let task = tokio::spawn(async move {
        let mut resp = match client.logs_follow(&name, LOG_TAIL).await {
            Ok(resp) => resp,
            Err(e) => {
                let error = Some(format!("{e}"));
                let _ = tx.send(Fetched::FollowEnded {
                    service: name,
                    error,
                });
                return;
            }
        };
        while let Ok(Some(bytes)) = resp.chunk().await {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            if tx
                .send(Fetched::LogChunk {
                    service: name.clone(),
                    text,
                })
                .is_err()
            {
                return;
            }
        }
        let _ = tx.send(Fetched::FollowEnded {
            service: name,
            error: None,
        });
    });
    state.bg.follow.running = Some((service, task.abort_handle()));
    state.bg.follow.fresh = true;
}

pub(crate) fn apply_chunk(state: &mut AppState, service: &str, text: &str) {
    let showing = matches!(&state.view, View::Logs { service: s } if s == service);
    if !showing {
        return;
    }
    if std::mem::take(&mut state.bg.follow.fresh) {
        state.logs.clear();
    }
    let added = append_capped(&mut state.logs, text, MAX_LOG_LINES);
    // Scrolled up: hold the position instead of letting new lines push the
    // view along.
    if state.service_scroll > 0 {
        state.service_scroll += added;
    }
}

pub(crate) fn apply_ended(state: &mut AppState, service: &str, error: Option<String>) {
    if state
        .bg
        .follow
        .running
        .as_ref()
        .is_some_and(|(s, _)| s == service)
    {
        state.bg.follow.running = None;
    }
    // Poll from now on: an agent's service (one batch, then closed), a
    // container that exited, or an error.
    state.bg.follow.polled.insert(service.to_string());
    if let Some(e) = error {
        state.logs = format!("Failed to follow logs: {e}");
    }
}

/// Append `text` to `buf`, then drop whole lines from the front beyond
/// `max_lines`. Returns the number of complete lines added.
pub(crate) fn append_capped(buf: &mut String, text: &str, max_lines: usize) -> usize {
    buf.push_str(text);
    let lines = buf.matches('\n').count();
    if lines > max_lines {
        let drop = lines - max_lines;
        let cut = buf
            .match_indices('\n')
            .nth(drop - 1)
            .map_or(0, |(i, _)| i + 1);
        buf.drain(..cut);
    }
    text.matches('\n').count()
}

#[cfg(test)]
#[path = "log_follow_tests.rs"]
mod tests;
