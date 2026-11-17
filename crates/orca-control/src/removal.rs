//! Removing a service for good: `orca rm` (#315) and the declarative prune.
//!
//! `orca stop` only pauses. Before `orca rm`, only the declarative loop
//! could remove a service, so on clusters without `[reconcile]` every
//! renamed or retired service stayed paused forever, and a re-sync could
//! start it again next to its successor.

use orca_core::config::{ServiceConfig, ServicesConfig};
use orca_core::types::Replicas;
use orca_core::ws_types::MasterMessage;
use tracing::{info, warn};

use crate::state::AppState;

/// `orca rm`: stop a service and remove it. Returns what was kept on disk.
/// Without `force` it refuses a service the declarative loop would deploy
/// again, and one whose workloads it could not confirm stopped.
pub async fn remove(state: &AppState, name: &str, force: bool) -> anyhow::Result<Vec<String>> {
    if !state.services.read().await.contains_key(name) {
        anyhow::bail!("service '{name}' not found");
    }
    if !force && let Some(dir) = declared_in(state, name) {
        anyhow::bail!(
            "'{name}' is still declared in {dir}, so the declarative reconciler would deploy \
             it again; remove it there, or pass --force"
        );
    }
    remove_service(state, name, !force).await
}

/// Stop a service's workloads wherever they run, then drop it from the
/// registry, the store and the proxy. With `strict`, a stop that fails
/// stops the removal; the declarative prune removes the service anyway.
pub(crate) async fn remove_service(
    state: &AppState,
    name: &str,
    strict: bool,
) -> anyhow::Result<Vec<String>> {
    let Some(config) = state
        .services
        .read()
        .await
        .get(name)
        .map(|s| s.config.clone())
    else {
        return Ok(Vec::new());
    };
    if let Err(e) = stop_workloads(state, &config).await {
        if strict {
            return Err(e.context(format!("{name} was not removed")));
        }
        warn!("removing {name}: stopping its workloads failed: {e:#}");
    }

    state.services.write().await.remove(name);
    state.last_failures.write().await.remove(name);
    state.failed_deploys.write().await.remove(name);
    state.deploy_history.write().await.forget(name);
    if let Some(store) = &state.store {
        if let Err(e) = store.remove_service(name) {
            warn!("removing {name} from the store failed: {e}");
        }
        let _ = store.unmark_stopped(name);
    }
    {
        let mut routes = state.route_table.write().await;
        routes.retain(|_, targets| {
            targets.retain(|t| t.service_name != name);
            !targets.is_empty()
        });
    }
    state
        .wasm_triggers
        .write()
        .await
        .retain(|t| t.service_name != name);
    info!("Removed service: {name}");
    Ok(kept_on_disk(&config))
}

async fn stop_workloads(state: &AppState, config: &ServiceConfig) -> anyhow::Result<()> {
    let mut stopped = config.clone();
    stopped.replicas = Replicas::Fixed(0);
    use crate::remote_stop::StopOutcome;
    match crate::remote_stop::stop_target(state, config).await {
        Ok(Some(node)) => match crate::remote_stop::stop_on_agent(state, &stopped, node).await? {
            StopOutcome::Confirmed => Ok(()),
            // Removed now, the record is gone while the container may run
            // on; the adoption scan would find it again.
            StopOutcome::Queued => anyhow::bail!(
                "node {node} is offline, so the stop of {} could not be confirmed",
                config.name
            ),
            StopOutcome::Unconfirmed => {
                anyhow::bail!("node {node} did not confirm the stop of {}", config.name)
            }
        },
        Ok(None) => crate::reconciler::scale(state, &config.name, 0).await,
        // The pin no longer resolves (node gone, renamed, ambiguous): tell
        // every agent; only the one that hosts the container acts (#176).
        // Not confirmed, so still an error: a container that survives is
        // found by the adoption scan once its node is back.
        Err(e) if crate::placement::placed_on_agent(config) => {
            for tx in state.ws_agents.read().await.values() {
                let _ = tx
                    .send(MasterMessage::Stop {
                        service_name: config.name.clone(),
                    })
                    .await;
            }
            Err(e.context("asked every connected agent to stop it, unconfirmed"))
        }
        Err(e) => Err(e),
    }
}

/// The `[reconcile].config_dir` that still declares `name`, if any.
fn declared_in(state: &AppState, name: &str) -> Option<String> {
    let dir = state
        .cluster_config
        .reconcile
        .as_ref()?
        .config_dir
        .clone()?;
    let path = std::path::Path::new(&dir);
    let loaded = if path.is_dir() {
        ServicesConfig::load_dir(path)
    } else {
        ServicesConfig::load(path)
    };
    let declared = loaded.ok()?.service.iter().any(|c| c.name == name);
    declared.then_some(dir)
}

/// What `orca rm` leaves on disk: volumes and bind mounts are data.
fn kept_on_disk(config: &ServiceConfig) -> Vec<String> {
    let mut kept = Vec::new();
    if config.volume.is_some() {
        kept.push(format!("volume orca-{}-data", config.name));
    }
    kept.extend(
        config
            .mounts
            .iter()
            .filter_map(|m| m.split(':').next())
            .filter(|host| host.starts_with('/'))
            .map(|host| format!("bind mount {host}")),
    );
    kept
}

#[cfg(test)]
#[path = "removal_tests.rs"]
mod tests;
