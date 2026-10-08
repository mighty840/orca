//! Services with a deploy, redeploy or reconcile under way (#173).
//!
//! A redeploy empties the service's instance list, then creates the
//! replacement, which (since #172) first gives the old container up to 30 s
//! to stop. The watchdog saw 0/N instances in that window and started its own
//! `create()` for the same name, racing the redeploy. Everything that
//! reconciles a service marks it here, and the watchdog skips marked ones.
//!
//! Marking doesn't wait for anyone. A whole redeploy also takes the
//! service's [`exclusive`] lock (#291): two redeploys of one service (two
//! webhooks, a webhook and a dependents restart) used to interleave their
//! create/stop/remove steps, so one failed with a 409 or removed the
//! container the other had just created.

use std::collections::HashMap;
use std::sync::Arc;

use crate::state::AppState;

/// Who is working on which service.
#[derive(Default)]
pub struct Registry {
    /// Nesting count of `InFlight` marks per service.
    counts: HashMap<String, u32>,
    /// One lock per service, held for a whole redeploy (#291).
    ops: HashMap<String, Arc<tokio::sync::Mutex<()>>>,
}

/// Held for the duration of work on one service. Nested marks (a redeploy
/// calling `reconcile_service`) are counted, so the service stays marked
/// until the outermost one is dropped.
pub(crate) struct InFlight<'a> {
    state: &'a AppState,
    name: String,
}

impl<'a> InFlight<'a> {
    pub(crate) fn mark(state: &'a AppState, name: &str) -> Self {
        *lock(state).counts.entry(name.to_string()).or_insert(0) += 1;
        Self {
            state,
            name: name.to_string(),
        }
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        let mut map = lock(self.state);
        if let Some(n) = map.counts.get_mut(&self.name) {
            *n -= 1;
            if *n == 0 {
                map.counts.remove(&self.name);
            }
        }
    }
}

/// Whether some deploy, redeploy or reconcile is working on `name` now.
pub(crate) fn is_in_flight(state: &AppState, name: &str) -> bool {
    lock(state).counts.contains_key(name)
}

/// Wait until no other whole operation is running on `name`, then hold it
/// off until the guard is dropped (#291). Not reentrant: take it once, at
/// the outermost operation.
pub(crate) async fn exclusive(state: &AppState, name: &str) -> tokio::sync::OwnedMutexGuard<()> {
    let op = lock(state).ops.entry(name.to_string()).or_default().clone();
    match op.clone().try_lock_owned() {
        Ok(guard) => guard,
        Err(_) => {
            tracing::info!(service = %name, "waiting for another operation on this service to finish");
            op.lock_owned().await
        }
    }
}

fn lock(state: &AppState) -> std::sync::MutexGuard<'_, Registry> {
    state
        .deploys_in_flight
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
#[path = "in_flight_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "service_ops_tests.rs"]
mod service_ops_tests;
