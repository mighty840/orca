//! Moving a service when its placement changes (#177).
//!
//! Placement edits used to be ignored. Now that they are applied, the
//! workload must also leave its old node, or the reconciler starts a second
//! copy on the new node while the old one keeps running unmanaged.

use std::fmt;
use std::time::Duration;

use orca_core::ws_types::MasterMessage;
use tracing::{info, warn};

use crate::state::{AppState, InstanceState};

const STOP_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Location {
    Master,
    Agent(u64),
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Master => f.write_str("the master"),
            Self::Agent(id) => write!(f, "node {id}"),
        }
    }
}

/// Where a service's instances run now, or `None` when it has none (or only
/// placeholders the master cannot attribute to a node).
fn current_location(instances: &[InstanceState]) -> Option<Location> {
    let id = &instances.first()?.handle.runtime_id;
    match id.strip_prefix("remote-") {
        Some(node) => node.parse().ok().map(Location::Agent),
        None => Some(Location::Master),
    }
}

/// Stop `name` where it runs now if that is not `target` (the node
/// `find_target_node` picked; `None` means the master). Afterwards the
/// service has no instances, so the caller deploys it fresh on the target.
///
/// A move is stop-then-start: the service is briefly down, as with any
/// cross-node move without shared storage.
pub(crate) async fn stop_if_moved(state: &AppState, name: &str, target: Option<u64>) {
    let to = target.map_or(Location::Master, Location::Agent);
    let (from, handles, config) = {
        let services = state.services.read().await;
        let Some(svc) = services.get(name) else {
            return;
        };
        let Some(from) = current_location(&svc.instances) else {
            return;
        };
        if from == to {
            return;
        }
        let handles: Vec<_> = svc.instances.iter().map(|i| i.handle.clone()).collect();
        (from, handles, svc.config.clone())
    };
    info!("Moving {name} from {from} to {to} (placement changed)");

    match from {
        Location::Agent(node_id) => {
            let agents = state.ws_agents.read().await;
            match agents.get(&node_id) {
                Some(tx) => {
                    let _ = tx
                        .send(MasterMessage::Stop {
                            service_name: name.to_string(),
                        })
                        .await;
                }
                None => warn!(
                    "{name}: node {node_id} is offline, so its old container keeps running \
                     there; stop it by hand once the node is back"
                ),
            }
        }
        Location::Master => match crate::reconciler::get_runtime(state, config.runtime) {
            Ok(runtime) => {
                for handle in &handles {
                    let _ = runtime.stop(handle, STOP_TIMEOUT).await;
                    let _ = runtime.remove(handle).await;
                }
            }
            Err(e) => warn!("{name}: cannot stop the old local instances: {e}"),
        },
    }

    if let Some(svc) = state.services.write().await.get_mut(name) {
        svc.instances.clear();
    }
    // Drop routes to the old instances until the new ones report in.
    match config.runtime {
        orca_core::types::RuntimeKind::Container => {
            crate::routes::update_container_routes(state, &config).await
        }
        orca_core::types::RuntimeKind::Wasm => {
            crate::routes::update_wasm_triggers(state, &config).await
        }
    }
}

#[cfg(test)]
#[path = "relocate_tests.rs"]
mod tests;
