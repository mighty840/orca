//! Rejoin reconcile (#213): a running container whose spec changed while the
//! agent was unreachable is recreated; everything else is left alone.

use std::sync::Arc;

use tokio::sync::mpsc;

use orca_core::runtime::Runtime;
use orca_core::testing::{MockOpKind, MockRuntime, spec};
use orca_core::types::WorkloadSpec;
use orca_core::ws_types::AgentMessage;

use super::reconcile_services;
use crate::grpc::AgentClient;

struct Harness {
    mock: Arc<MockRuntime>,
    runtime: Arc<dyn Runtime>,
    agent: Arc<AgentClient>,
}

impl Harness {
    fn new() -> Self {
        let mock = Arc::new(MockRuntime::new());
        Self {
            runtime: mock.clone(),
            mock,
            agent: Arc::new(AgentClient::new("http://localhost:0".into(), 1)),
        }
    }

    /// Run one reconcile pass; returns the DeployResults the agent reported.
    async fn reconcile(&self, expected: Vec<WorkloadSpec>) -> Vec<(String, bool)> {
        let (domain_tx, _domain_rx) = mpsc::channel(16);
        let (out_tx, mut out_rx) = mpsc::channel(16);
        reconcile_services(
            expected.into_iter().map(Box::new).collect(),
            &self.runtime,
            &self.agent,
            &domain_tx,
            &out_tx,
        )
        .await;
        drop(out_tx);
        let mut results = Vec::new();
        while let Some(msg) = out_rx.recv().await {
            if let AgentMessage::DeployResult {
                service_name,
                success,
                ..
            } = msg
            {
                results.push((service_name, success));
            }
        }
        results
    }

    async fn creates(&self) -> usize {
        self.mock.count(MockOpKind::Create).await
    }
}

/// A spec as the master sends it: fingerprinted.
fn stamped(name: &str, env: &[(&str, &str)]) -> WorkloadSpec {
    let mut s = spec(name);
    for (k, v) in env {
        s.env.insert(k.to_string(), v.to_string());
    }
    s.stamp_fingerprint();
    s
}

#[tokio::test]
async fn missing_service_is_deployed_once_and_then_left_alone() {
    let h = Harness::new();
    let s = stamped("signoz", &[]);
    assert_eq!(
        h.reconcile(vec![s.clone()]).await,
        [("signoz".into(), true)]
    );
    assert_eq!(h.creates().await, 1);

    // Same spec again: running and up to date, so nothing happens.
    assert!(h.reconcile(vec![s]).await.is_empty());
    assert_eq!(h.creates().await, 1);
}

#[tokio::test]
async fn running_service_with_a_changed_spec_is_recreated() {
    // The 2026-09-23 incident: signoz's env gained SMTP settings while the
    // agent was disconnected. On rejoin the master sent the new spec, and
    // the agent skipped it because the old container was running.
    let h = Harness::new();
    h.reconcile(vec![stamped("signoz", &[])]).await;
    assert_eq!(h.creates().await, 1);

    let changed = stamped("signoz", &[("SMTP_SMARTHOST", "smtp.example.com:587")]);
    assert_eq!(
        h.reconcile(vec![changed.clone()]).await,
        [("signoz".into(), true)],
        "a spec change that missed the agent must be applied on rejoin"
    );
    assert_eq!(h.creates().await, 2);

    // And it converges: the next pass is a no-op.
    assert!(h.reconcile(vec![changed]).await.is_empty());
    assert_eq!(h.creates().await, 2);
}

#[tokio::test]
async fn container_created_before_fingerprinting_is_not_recreated() {
    // Upgrading the agent must not recreate every container on the node:
    // containers without the label are left running until their next deploy.
    let h = Harness::new();
    h.reconcile(vec![stamped("nextcloud", &[])]).await;
    h.mock.set_fingerprint("nextcloud", None).await;

    let changed = stamped("nextcloud", &[("PHP_MEMORY_LIMIT", "2G")]);
    assert!(h.reconcile(vec![changed]).await.is_empty());
    assert_eq!(h.creates().await, 1);
}

#[tokio::test]
async fn spec_from_an_older_master_is_not_treated_as_drift() {
    let h = Harness::new();
    h.reconcile(vec![stamped("jitsi-web", &[])]).await;

    let mut unstamped = stamped("jitsi-web", &[("X", "1")]);
    unstamped.fingerprint = None;
    assert!(h.reconcile(vec![unstamped]).await.is_empty());
    assert_eq!(h.creates().await, 1);
}
