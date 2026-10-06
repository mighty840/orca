use std::collections::HashSet;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use orca_core::error::{OrcaError, Result};
use orca_core::runtime::{AsAny, ExecResult, LogOpts, LogStream, Runtime, WorkloadHandle};
use orca_core::testing::{MockOpKind, MockRuntime, spec};
use orca_core::types::{ResourceStats, WorkloadStatus};
use tokio::sync::mpsc;

use super::*;

/// A runtime that behaves like Docker where it matters here: a create takes
/// a while, and a second create of a name already being created fails with
/// "name already in use" (the 409 seen on the agents).
struct SlowRuntime {
    inner: MockRuntime,
    creating: StdMutex<HashSet<String>>,
}

impl SlowRuntime {
    fn new() -> Self {
        Self {
            inner: MockRuntime::new(),
            creating: StdMutex::new(HashSet::new()),
        }
    }
}

impl AsAny for SlowRuntime {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[async_trait]
impl Runtime for SlowRuntime {
    fn name(&self) -> &str {
        "slow"
    }
    async fn create(&self, spec: &WorkloadSpec) -> Result<WorkloadHandle> {
        if !self.creating.lock().unwrap().insert(spec.name.clone()) {
            return Err(OrcaError::Runtime(format!(
                "409 Conflict: the container name orca-{} is already in use",
                spec.name
            )));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        let handle = self.inner.create(spec).await;
        self.creating.lock().unwrap().remove(&spec.name);
        handle
    }
    async fn start(&self, h: &WorkloadHandle) -> Result<()> {
        self.inner.start(h).await
    }
    async fn stop(&self, h: &WorkloadHandle, t: Duration) -> Result<()> {
        self.inner.stop(h, t).await
    }
    async fn remove(&self, h: &WorkloadHandle) -> Result<()> {
        self.inner.remove(h).await
    }
    async fn status(&self, h: &WorkloadHandle) -> Result<WorkloadStatus> {
        self.inner.status(h).await
    }
    async fn logs(&self, h: &WorkloadHandle, o: &LogOpts) -> Result<LogStream> {
        self.inner.logs(h, o).await
    }
    async fn exec(&self, h: &WorkloadHandle, c: &[String]) -> Result<ExecResult> {
        self.inner.exec(h, c).await
    }
    async fn stats(&self, h: &WorkloadHandle) -> Result<ResourceStats> {
        self.inner.stats(h).await
    }
}

struct Harness {
    runtime: Arc<SlowRuntime>,
    queues: ServiceQueues,
    results: mpsc::Receiver<AgentMessage>,
    _domains: mpsc::Receiver<(String, String, u16)>,
}

fn harness() -> Harness {
    let runtime = Arc::new(SlowRuntime::new());
    let (out_tx, results) = mpsc::channel(64);
    let (domain_tx, domains) = mpsc::channel(64);
    let queues = ServiceQueues::new(OpContext {
        runtime: runtime.clone(),
        agent: Arc::new(AgentClient::new("http://127.0.0.1:9".into(), 1)),
        domain_tx,
        out_tx,
    });
    Harness {
        runtime,
        queues,
        results,
        _domains: domains,
    }
}

/// The next `n` deploy results as (service, success).
async fn deploy_results(rx: &mut mpsc::Receiver<AgentMessage>, n: usize) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    while out.len() < n {
        let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("a deploy result")
            .expect("channel open");
        if let AgentMessage::DeployResult {
            service_name,
            success,
            ..
        } = msg
        {
            out.push((service_name, success));
        }
    }
    out
}

#[tokio::test]
async fn two_deploys_of_one_service_run_one_after_the_other() {
    // The 2026-10-06 promote: a second deploy of the dashboard arrived while
    // the first was still running, and its create failed with a 409.
    let mut h = harness();
    h.queues
        .submit("web", ServiceOp::Deploy(Box::new(spec("web"))));
    h.queues
        .submit("web", ServiceOp::Deploy(Box::new(spec("web"))));
    let results = deploy_results(&mut h.results, 2).await;
    assert_eq!(results, [("web".into(), true), ("web".into(), true)]);
    assert_eq!(h.runtime.inner.count(MockOpKind::Create).await, 2);
}

#[tokio::test]
async fn a_stop_never_overtakes_the_deploy_sent_before_it() {
    let mut h = harness();
    h.queues
        .submit("web", ServiceOp::Deploy(Box::new(spec("web"))));
    h.queues.submit("web", ServiceOp::Stop);
    h.queues
        .submit("web", ServiceOp::Deploy(Box::new(spec("web"))));
    deploy_results(&mut h.results, 2).await;
    let kinds: Vec<MockOpKind> = h
        .runtime
        .inner
        .recorded_ops()
        .await
        .iter()
        .map(|op| op.kind())
        .collect();
    assert_eq!(
        kinds,
        [
            MockOpKind::Create,
            MockOpKind::Start,
            MockOpKind::Stop,
            MockOpKind::Remove,
            MockOpKind::Create,
            MockOpKind::Start,
        ],
        "deploy, then the stop, then the redeploy"
    );
}

#[tokio::test]
async fn different_services_still_deploy_in_parallel() {
    let mut h = harness();
    let started = Instant::now();
    for name in ["a", "b", "c", "d"] {
        h.queues
            .submit(name, ServiceOp::Deploy(Box::new(spec(name))));
    }
    let results = deploy_results(&mut h.results, 4).await;
    assert!(results.iter().all(|(_, ok)| *ok));
    assert!(
        started.elapsed() < Duration::from_millis(180),
        "four 50 ms deploys took {:?}: they ran one after another",
        started.elapsed()
    );
}
