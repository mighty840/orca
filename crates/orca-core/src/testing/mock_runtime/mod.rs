//! Mock implementation of the [`Runtime`](crate::runtime::Runtime) trait.
//!
//! [`MockRuntime`] records every successful operation in call order and can be
//! told to fail a chosen operation, which is what lets tests assert on deploy
//! *ordering* and on *failure* paths without Docker:
//!
//! - ordering — that a graceful [`stop`](MockOp::Stop) with a real grace period
//!   precedes a `Remove`, rather than a container being torn down without one;
//! - failure — that a deploy which cannot create its replacement leaves the
//!   previous workload alone instead of removing it and reporting success.

mod ops;
mod runtime_impl;

use ops::FailMode;
pub use ops::{MockOp, MockOpKind};

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::types::WorkloadStatus;

/// A mock [`Runtime`](crate::runtime::Runtime) that tracks operations without
/// running real workloads.
///
/// Use this in integration tests to verify reconciler behavior, API endpoints,
/// and other components that depend on a runtime.
pub struct MockRuntime {
    /// Recorded operations, in order. Prefer [`MockRuntime::recorded_ops`].
    pub ops: Arc<Mutex<Vec<MockOp>>>,
    /// Current status per runtime_id.
    statuses: Arc<Mutex<HashMap<String, WorkloadStatus>>>,
    /// Counter for generating unique IDs.
    counter: Arc<Mutex<u64>>,
    /// Pending injected failures, keyed by the operation they apply to.
    failures: Arc<Mutex<HashMap<MockOpKind, FailMode>>>,
    /// If set, the mock host port returned by `resolve_host_port`.
    pub mock_host_port: Option<u16>,
}

impl MockRuntime {
    /// Create a new mock runtime.
    pub fn new() -> Self {
        Self {
            ops: Arc::new(Mutex::new(Vec::new())),
            statuses: Arc::new(Mutex::new(HashMap::new())),
            counter: Arc::new(Mutex::new(0)),
            failures: Arc::new(Mutex::new(HashMap::new())),
            mock_host_port: None,
        }
    }

    /// Create a mock runtime that returns a fixed host port.
    pub fn with_host_port(port: u16) -> Self {
        Self {
            mock_host_port: Some(port),
            ..Self::new()
        }
    }

    // --- inspection -----------------------------------------------------

    /// Get a copy of all recorded operations, in call order.
    pub async fn recorded_ops(&self) -> Vec<MockOp> {
        self.ops.lock().await.clone()
    }

    /// Recorded operations for one workload, in call order.
    ///
    /// Matches on [`MockOp::workload`], so records made from a spec and from a
    /// handle both match the same bare name.
    pub async fn ops_for(&self, workload: &str) -> Vec<MockOp> {
        self.ops
            .lock()
            .await
            .iter()
            .filter(|op| op.workload() == workload)
            .cloned()
            .collect()
    }

    /// How many operations of `kind` were recorded.
    pub async fn count(&self, kind: MockOpKind) -> usize {
        self.ops
            .lock()
            .await
            .iter()
            .filter(|o| o.kind() == kind)
            .count()
    }

    /// Drop all recorded operations, keeping statuses and injected failures.
    ///
    /// Useful to ignore setup noise before exercising the behavior under test.
    pub async fn clear_ops(&self) {
        self.ops.lock().await.clear();
    }

    // --- failure injection ----------------------------------------------

    /// Fail the next call of `kind`, then resume normal behavior.
    pub async fn fail_next(&self, kind: MockOpKind) {
        self.fail_next_n(kind, 1).await;
    }

    /// Fail the next `times` calls of `kind`, then resume normal behavior.
    pub async fn fail_next_n(&self, kind: MockOpKind, times: usize) {
        self.failures
            .lock()
            .await
            .insert(kind, FailMode::Times(times));
    }

    /// Fail every call of `kind` until [`MockRuntime::clear_failures`].
    pub async fn fail_always(&self, kind: MockOpKind) {
        self.failures.lock().await.insert(kind, FailMode::Always);
    }

    /// Remove all injected failures.
    pub async fn clear_failures(&self) {
        self.failures.lock().await.clear();
    }

    /// Consume one injected failure for `kind`, if one is pending.
    ///
    /// Returns `true` when the caller should fail this operation.
    async fn take_failure(&self, kind: MockOpKind) -> bool {
        let mut failures = self.failures.lock().await;
        match failures.get_mut(&kind) {
            Some(FailMode::Always) => true,
            Some(FailMode::Times(remaining)) => {
                *remaining -= 1;
                if *remaining == 0 {
                    failures.remove(&kind);
                }
                true
            }
            None => false,
        }
    }

    // --- status control -------------------------------------------------

    /// Force the reported status of a workload.
    ///
    /// Lets a test stage a state the mock would not reach on its own, such as a
    /// container that exited cleanly ([`WorkloadStatus::Completed`]) or crashed
    /// ([`WorkloadStatus::Failed`]) while orca believed it was running.
    pub async fn set_status(&self, runtime_id: &str, status: WorkloadStatus) {
        self.statuses
            .lock()
            .await
            .insert(runtime_id.to_string(), status);
    }

    /// Append an operation record.
    async fn record(&self, op: MockOp) {
        self.ops.lock().await.push(op);
    }
}

impl Default for MockRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
