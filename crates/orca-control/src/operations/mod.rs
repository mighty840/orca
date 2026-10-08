//! Service lifecycle operations: stop, scale, redeploy, rollback.

mod lifecycle;

use std::time::Duration;

use tracing::info;

use orca_core::ws_types::MasterMessage;

use crate::reconciler::{get_runtime, reconcile_service};
use crate::state::AppState;

pub(crate) use lifecycle::{canary_deploy, rolling_update};
pub use lifecycle::{promote, rollback, scale, start, stop, stop_all};

/// Returned when a redeploy targets a remote node that is not currently connected.
/// Callers can downcast to distinguish this from internal errors (e.g. map to 503).
#[derive(Debug, thiserror::Error)]
#[error("agent {node_id} not connected — try again once the node reconnects")]
pub struct AgentOfflineError {
    pub node_id: u64,
}

/// Graceful shutdown timeout for container stop operations.
pub(crate) const GRACEFUL_TIMEOUT: Duration = Duration::from_secs(5);

/// Try to load a fresh `ServiceConfig` from the on-disk `services/` tree so
/// that `redeploy` picks up any edits to `service.toml` since the last deploy.
///
/// Loads the tree the way `orca deploy` and the infra webhook do
/// (`load_dir`), so the service gets the project and default network of its
/// directory (#287). Reading `services/<name>/service.toml` on its own lost
/// both: `${secrets.KEY}` then resolved against the global scope only (the
/// redeploy failed, or silently used a global secret of the same name) and
/// the container fell back to the name-prefix network. Any load error falls
/// back to the cached config.
fn load_fresh_config(service_name: &str) -> Option<orca_core::config::ServiceConfig> {
    let dir = std::path::Path::new("services");
    let loaded = if dir.is_dir() {
        orca_core::config::ServicesConfig::load_dir(dir)
    } else {
        let mono = std::path::Path::new("services.toml");
        if !mono.exists() {
            return None;
        }
        orca_core::config::ServicesConfig::load(mono)
    };
    match loaded {
        Ok(cfg) => cfg.service.into_iter().find(|s| s.name == service_name),
        Err(e) => {
            tracing::warn!(
                "redeploy {service_name}: using the cached config, reloading failed: {e}"
            );
            None
        }
    }
}

/// Redeploy a service using a rolling update: start new instances before
/// stopping old ones, with a 30-second graceful shutdown timeout.
///
/// Re-reads `service.toml` from disk if available so config edits take effect
/// without needing a full `orca deploy`.
pub async fn redeploy(state: &AppState, service_name: &str) -> anyhow::Result<()> {
    // A second redeploy of this service waits for this one (#291).
    let _exclusive = crate::in_flight::exclusive(state, service_name).await;
    // Held across the whole redeploy, including the window after the instance
    // list is cleared below, so the watchdog doesn't race it (#173).
    let _in_flight = crate::in_flight::InFlight::mark(state, service_name);
    let cached_config = {
        let services = state.services.read().await;
        let svc = services
            .get(service_name)
            .ok_or_else(|| anyhow::anyhow!("service '{}' not found", service_name))?;
        svc.config.clone()
    };

    // Reload from disk when possible so mount/env changes take effect.
    let config = if let Some(fresh) = load_fresh_config(service_name) {
        tracing::debug!("redeploy: reloaded config from disk for {service_name}");
        fresh
    } else {
        cached_config.clone()
    };

    // Persist updated config in state immediately.
    {
        let mut services = state.services.write().await;
        if let Some(svc) = services.get_mut(service_name) {
            svc.config = config.clone();
        }
    }

    // Collect old instance handles.
    let old_handles: Vec<_> = {
        let services = state.services.read().await;
        services
            .get(service_name)
            .map(|svc| svc.instances.iter().map(|i| i.handle.clone()).collect())
            .unwrap_or_default()
    };

    // Detect remote placement. Config takes precedence over instance runtime_ids
    // so redeploy works even when the service has no running instances on master
    // (e.g. stopped or newly registered placeholder).
    let remote_node: Option<u64> = 'remote: {
        if let Some(node_name) = config.placement.as_ref().and_then(|p| p.node.as_deref()) {
            let nodes = state.registered_nodes.read().await;
            let found = nodes.iter().find_map(|(id, n)| {
                n.address
                    .split(':')
                    .next()
                    .filter(|h| *h == node_name)
                    .map(|_| *id)
            });
            // The master self-registers in registered_nodes (#134), so a
            // service pinned to the master's own node resolves here too —
            // but it must take the LOCAL path below. Routing it through
            // ws_agents fails with AgentOfflineError: the master holds no
            // WS connection to itself. Surfaced as webhook/CLI redeploys
            // 503ing for every master-hosted service the moment placement
            // pins were added.
            let found = found.filter(|id| *id != crate::master_node::master_node_id());
            if found.is_some() {
                break 'remote found;
            }
        }
        old_handles.iter().find_map(|h| {
            h.runtime_id
                .strip_prefix("remote-")
                .and_then(|s| s.parse::<u64>().ok())
        })
    };

    if let Some(node_id) = remote_node {
        let spec = crate::routes::service_config_to_spec(&config)?;
        let agents = state.ws_agents.read().await;
        if let Some(tx) = agents.get(&node_id) {
            let _ = tx
                .send(MasterMessage::Stop {
                    service_name: service_name.to_string(),
                })
                .await;
            tx.send(MasterMessage::Deploy {
                spec: Box::new(spec),
            })
            .await
            .map_err(|_| anyhow::anyhow!("agent {node_id} channel closed"))?;
        } else {
            return Err(anyhow::Error::new(AgentOfflineError { node_id }));
        }
        info!("Redeployed service {service_name} via agent {node_id}");
        return Ok(());
    }

    let runtime = get_runtime(state, config.runtime)?;

    // Clear instance list so reconcile creates fresh replicas.
    let old_instances = {
        let mut services = state.services.write().await;
        services
            .get_mut(service_name)
            .map(|svc| std::mem::take(&mut svc.instances))
            .unwrap_or_default()
    };

    // Start new instances (reconcile will create the desired count).
    if let Err(e) = reconcile_service(state, &config).await {
        // The runtime put the old containers back (#174). Track them again
        // and go back to the config they run, or the watchdog would see 0/N
        // and try the failing spec again right away.
        restore_after_failed_redeploy(state, runtime, &cached_config, old_instances).await;
        return Err(e);
    }

    // Gracefully stop old instances with a 30-second timeout.
    for handle in &old_handles {
        let _ = runtime.stop(handle, GRACEFUL_TIMEOUT).await;
        let _ = runtime.remove(handle).await;
    }

    info!("Redeployed service: {service_name}");
    Ok(())
}

/// After a failed redeploy: re-register the old instances that are running
/// again and restore the previous config and routes.
async fn restore_after_failed_redeploy(
    state: &AppState,
    runtime: &dyn orca_core::runtime::Runtime,
    previous: &orca_core::config::ServiceConfig,
    old_instances: Vec<crate::state::InstanceState>,
) {
    let mut running = Vec::new();
    for inst in old_instances {
        if matches!(
            runtime.status(&inst.handle).await,
            Ok(orca_core::types::WorkloadStatus::Running)
        ) {
            running.push(inst);
        }
    }
    {
        let mut services = state.services.write().await;
        if let Some(svc) = services.get_mut(&previous.name) {
            svc.config = previous.clone();
            for inst in running {
                if !svc
                    .instances
                    .iter()
                    .any(|i| i.handle.runtime_id == inst.handle.runtime_id)
                {
                    svc.instances.push(inst);
                }
            }
        }
    }
    crate::routes::update_container_routes(state, previous).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialize all tests that call `set_current_dir` — that syscall is
    /// process-wide, so concurrent tests would corrupt each other's CWD.
    fn with_cwd<F: FnOnce()>(dir: &std::path::Path, f: F) {
        use std::sync::Mutex;
        static CWD_LOCK: Mutex<()> = Mutex::new(());
        let _guard = CWD_LOCK.lock().unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir).unwrap();
        f();
        std::env::set_current_dir(&prev).unwrap();
    }

    #[test]
    fn load_fresh_config_returns_none_when_no_services_dir() {
        let tmp = tempfile::tempdir().unwrap();
        with_cwd(tmp.path(), || {
            let result = load_fresh_config("does-not-exist");
            assert!(result.is_none());
        });
    }

    #[test]
    fn load_fresh_config_loads_from_per_service_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let svc_dir = tmp.path().join("services").join("myapp");
        std::fs::create_dir_all(&svc_dir).unwrap();
        std::fs::write(
            svc_dir.join("service.toml"),
            "[[service]]\nname = \"myapp\"\nimage = \"myapp:latest\"\nport = 8080\n",
        )
        .unwrap();
        with_cwd(tmp.path(), || {
            let result = load_fresh_config("myapp");
            assert!(result.is_some());
            assert_eq!(result.unwrap().name, "myapp");
        });
    }

    #[test]
    fn load_fresh_config_keeps_the_project_and_network_of_its_directory() {
        // #287: `services/git-t0001-stage/service.toml` was read on its own,
        // so a redeploy lost the project and resolved project-scoped secrets
        // against the global scope.
        let tmp = tempfile::tempdir().unwrap();
        let svc_dir = tmp.path().join("services").join("git-t0001-stage");
        std::fs::create_dir_all(&svc_dir).unwrap();
        std::fs::write(
            svc_dir.join("service.toml"),
            "[[service]]\nname = \"git-t0001-stage\"\nimage = \"gitea:1\"\nport = 3000\n\
             [service.env]\nDB_PASS = \"${secrets.GIT_DB_PASSWORD}\"\n",
        )
        .unwrap();
        with_cwd(tmp.path(), || {
            let cfg = load_fresh_config("git-t0001-stage").expect("found");
            assert_eq!(cfg.project.as_deref(), Some("git-t0001-stage"));
            assert_eq!(cfg.network.as_deref(), Some("git-t0001-stage"));
        });
    }

    #[test]
    fn load_fresh_config_finds_a_service_in_a_project_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let svc_dir = tmp.path().join("services").join("platform");
        std::fs::create_dir_all(&svc_dir).unwrap();
        std::fs::write(
            svc_dir.join("service.toml"),
            "[[service]]\nname = \"portal\"\nimage = \"portal:2\"\nport = 80\n",
        )
        .unwrap();
        with_cwd(tmp.path(), || {
            let cfg = load_fresh_config("portal").expect("found");
            assert_eq!(cfg.image.as_deref(), Some("portal:2"));
            assert_eq!(cfg.project.as_deref(), Some("platform"));
        });
    }

    #[test]
    fn graceful_timeout_is_5_seconds() {
        assert_eq!(GRACEFUL_TIMEOUT, Duration::from_secs(5));
    }

    #[test]
    fn load_fresh_config_loads_from_monolithic_services_toml() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("services.toml"),
            "[[service]]\nname = \"myapp\"\nimage = \"myapp:latest\"\nport = 8080\n",
        )
        .unwrap();
        with_cwd(tmp.path(), || {
            let result = load_fresh_config("myapp");
            assert!(result.is_some());
            assert_eq!(result.unwrap().name, "myapp");
        });
    }

    #[test]
    fn load_fresh_config_per_service_takes_precedence_over_monolithic() {
        let tmp = tempfile::tempdir().unwrap();
        // Monolithic has image v1, per-service has image v2.
        std::fs::write(
            tmp.path().join("services.toml"),
            "[[service]]\nname = \"myapp\"\nimage = \"myapp:v1\"\nport = 8080\n",
        )
        .unwrap();
        let svc_dir = tmp.path().join("services").join("myapp");
        std::fs::create_dir_all(&svc_dir).unwrap();
        std::fs::write(
            svc_dir.join("service.toml"),
            "[[service]]\nname = \"myapp\"\nimage = \"myapp:v2\"\nport = 8080\n",
        )
        .unwrap();
        with_cwd(tmp.path(), || {
            let result = load_fresh_config("myapp");
            let cfg = result.expect("should find config");
            assert_eq!(
                cfg.image.as_deref(),
                Some("myapp:v2"),
                "per-service file must win over monolithic"
            );
        });
    }

    #[test]
    fn load_fresh_config_service_not_in_file_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("services.toml"),
            "[[service]]\nname = \"other\"\nimage = \"other:latest\"\nport = 9090\n",
        )
        .unwrap();
        with_cwd(tmp.path(), || {
            let result = load_fresh_config("myapp");
            assert!(
                result.is_none(),
                "should return None when service not in file"
            );
        });
    }

    #[test]
    fn load_fresh_config_invalid_toml_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let svc_dir = tmp.path().join("services").join("broken");
        std::fs::create_dir_all(&svc_dir).unwrap();
        std::fs::write(svc_dir.join("service.toml"), "this is not valid toml!!!").unwrap();
        with_cwd(tmp.path(), || {
            // Invalid TOML should fall through without panicking and return None.
            let result = load_fresh_config("broken");
            assert!(result.is_none());
        });
    }
}
