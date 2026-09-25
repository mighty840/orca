//! Services with a deploy, redeploy or reconcile under way (#173).
//!
//! A redeploy empties the service's instance list, then creates the
//! replacement, which (since #172) first gives the old container up to 30 s
//! to stop. The watchdog saw 0/N instances in that window and started its own
//! `create()` for the same name, racing the redeploy. Everything that
//! reconciles a service marks it here, and the watchdog skips marked ones.

use std::collections::HashMap;

use crate::state::AppState;

/// Held for the duration of work on one service. Nested marks (a redeploy
/// calling `reconcile_service`) are counted, so the service stays marked
/// until the outermost one is dropped.
pub(crate) struct InFlight<'a> {
    state: &'a AppState,
    name: String,
}

impl<'a> InFlight<'a> {
    pub(crate) fn mark(state: &'a AppState, name: &str) -> Self {
        *lock(state).entry(name.to_string()).or_insert(0) += 1;
        Self {
            state,
            name: name.to_string(),
        }
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        let mut map = lock(self.state);
        if let Some(n) = map.get_mut(&self.name) {
            *n -= 1;
            if *n == 0 {
                map.remove(&self.name);
            }
        }
    }
}

/// Whether some deploy, redeploy or reconcile is working on `name` now.
pub(crate) fn is_in_flight(state: &AppState, name: &str) -> bool {
    lock(state).contains_key(name)
}

fn lock(state: &AppState) -> std::sync::MutexGuard<'_, HashMap<String, u32>> {
    state
        .deploys_in_flight
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
#[path = "in_flight_tests.rs"]
mod tests;
