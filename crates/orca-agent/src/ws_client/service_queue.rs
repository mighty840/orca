//! One deploy or stop at a time per service, in the order the master sent
//! them (#279).
//!
//! Deploys used to run as independent tasks and stops inline, so two deploys
//! of one service could race (the second `create` failed with Docker's 409
//! "container name already in use"), and nothing kept a stop from overtaking
//! a deploy sent before it. Each service now gets a queue worked off by one
//! task; different services still deploy in parallel, and the message loop
//! never waits on a pull.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use orca_core::runtime::Runtime;
use orca_core::types::WorkloadSpec;
use orca_core::ws_types::AgentMessage;

use crate::grpc::AgentClient;

/// Work for one service.
pub(crate) enum ServiceOp {
    Deploy(Box<WorkloadSpec>),
    Stop,
}

/// What every operation needs: this session's runtime and channels.
#[derive(Clone)]
pub(crate) struct OpContext {
    pub runtime: Arc<dyn Runtime>,
    pub agent: Arc<AgentClient>,
    pub domain_tx: mpsc::Sender<(String, String, u16)>,
    pub out_tx: mpsc::Sender<AgentMessage>,
}

/// The per-service queues of one WS session. Dropping it ends the workers
/// once their current operation is done.
pub(crate) struct ServiceQueues {
    ctx: OpContext,
    queues: Mutex<HashMap<String, mpsc::UnboundedSender<ServiceOp>>>,
}

impl ServiceQueues {
    pub(crate) fn new(ctx: OpContext) -> Self {
        Self {
            ctx,
            queues: Mutex::new(HashMap::new()),
        }
    }

    /// Queue `op` behind any earlier operation on `service`. Returns at once.
    pub(crate) fn submit(&self, service: &str, op: ServiceOp) {
        let mut queues = self.queues.lock().unwrap_or_else(|e| e.into_inner());
        let tx = queues
            .entry(service.to_string())
            .or_insert_with(|| spawn_worker(service.to_string(), self.ctx.clone()));
        if let Err(mpsc::error::SendError(op)) = tx.send(op) {
            // The worker is gone (it panicked): start a fresh one.
            let fresh = spawn_worker(service.to_string(), self.ctx.clone());
            let _ = fresh.send(op);
            queues.insert(service.to_string(), fresh);
        }
    }
}

fn spawn_worker(service: String, ctx: OpContext) -> mpsc::UnboundedSender<ServiceOp> {
    let (tx, mut rx) = mpsc::unbounded_channel::<ServiceOp>();
    tokio::spawn(async move {
        while let Some(op) = rx.recv().await {
            match op {
                ServiceOp::Deploy(spec) => {
                    super::deploy::deploy_and_report(
                        ctx.runtime.clone(),
                        ctx.agent.clone(),
                        ctx.domain_tx.clone(),
                        ctx.out_tx.clone(),
                        spec,
                    )
                    .await
                }
                ServiceOp::Stop => ctx.agent.stop_service(ctx.runtime.as_ref(), &service).await,
            }
        }
    });
    tx
}

#[cfg(test)]
#[path = "service_queue_tests.rs"]
mod tests;
