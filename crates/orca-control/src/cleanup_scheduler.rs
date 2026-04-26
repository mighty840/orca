//! Scheduled cleanup: docker system prune on all nodes + registry GC on master.

use std::str::FromStr;
use std::sync::Arc;

use cron::Schedule;
use orca_core::config::CleanupConfig;
use orca_core::ws_types::MasterMessage;
use tracing::{error, info, warn};

use crate::backup_scheduler::duration_until_next;
use crate::state::AppState;

/// Spawn a background task that runs cleanup on the configured cron schedule.
pub fn spawn_cleanup_scheduler(
    config: CleanupConfig,
    state: Arc<AppState>,
) -> Option<tokio::task::JoinHandle<()>> {
    let schedule_str = config.schedule.as_ref()?;
    let schedule = match Schedule::from_str(schedule_str) {
        Ok(s) => s,
        Err(e) => {
            error!("Invalid cleanup cron schedule '{}': {e}", schedule_str);
            return None;
        }
    };

    info!("Cleanup scheduler started with schedule: {schedule_str}");
    let handle = tokio::spawn(async move {
        loop {
            let sleep_dur = match duration_until_next(&schedule) {
                Some(d) => d,
                None => {
                    error!("No upcoming cron occurrence, stopping cleanup scheduler");
                    break;
                }
            };
            info!("Next cleanup in {}s", sleep_dur.as_secs());
            tokio::time::sleep(sleep_dur).await;
            run_cleanup(&config, &state).await;
        }
    });
    Some(handle)
}

async fn run_cleanup(config: &CleanupConfig, state: &AppState) {
    info!("Starting scheduled cleanup run");
    prune_local_docker().await;
    dispatch_prune_to_agents(state).await;
    prune_registry(config).await;
    info!("Cleanup run complete");
}

async fn prune_local_docker() {
    match tokio::process::Command::new("docker")
        .args(["system", "prune", "-f"])
        .output()
        .await
    {
        Ok(out) if out.status.success() => {
            info!(
                "docker system prune: {}",
                String::from_utf8_lossy(&out.stdout).trim()
            );
        }
        Ok(out) => error!(
            "docker system prune failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(e) => error!("failed to spawn docker system prune: {e}"),
    }
}

async fn dispatch_prune_to_agents(state: &AppState) {
    let agents = state.ws_agents.read().await;
    if agents.is_empty() {
        return;
    }
    info!("Dispatching PruneSystem to {} agent(s)", agents.len());
    for (node_id, tx) in agents.iter() {
        if let Err(e) = tx.send(MasterMessage::PruneSystem).await {
            error!("Failed to send PruneSystem to agent {node_id}: {e}");
        }
    }
}

async fn prune_registry(config: &CleanupConfig) {
    let container = &config.registry_container;
    let keep = config.registry_keep_tags;

    // Prune old tags: keep only the `keep` most-recent per repository.
    let tag_prune = format!(
        r#"for img in $(ls /var/lib/registry/docker/registry/v2/repositories/ 2>/dev/null); do \
  tag_dir=/var/lib/registry/docker/registry/v2/repositories/$img/_manifests/tags; \
  total=$(ls $tag_dir 2>/dev/null | wc -l); \
  del=$((total > {keep} ? total - {keep} : 0)); \
  for tag in $(ls $tag_dir 2>/dev/null | head -n $del); do rm -rf $tag_dir/$tag; done; \
  [ $del -gt 0 ] && echo "$img: pruned $del old tags"; \
done"#
    );

    match tokio::process::Command::new("docker")
        .args(["exec", container, "sh", "-c", &tag_prune])
        .output()
        .await
    {
        Ok(out) if out.status.success() => {
            let msg = String::from_utf8_lossy(&out.stdout);
            if !msg.trim().is_empty() {
                info!("Registry tag prune:\n{}", msg.trim());
            }
        }
        Ok(out) => warn!(
            "Registry tag prune failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(e) => warn!("Failed to spawn registry tag prune: {e}"),
    }

    // Run garbage-collect to free unreferenced blobs.
    match tokio::process::Command::new("docker")
        .args([
            "exec",
            container,
            "registry",
            "garbage-collect",
            "/etc/distribution/config.yml",
            "--delete-untagged",
        ])
        .output()
        .await
    {
        Ok(out) if out.status.success() => info!("Registry GC complete"),
        Ok(out) => warn!(
            "Registry GC failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(e) => warn!("Failed to spawn registry GC: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orca_core::config::CleanupConfig;

    #[test]
    fn no_schedule_returns_none() {
        use orca_core::config::{ClusterConfig, ClusterMeta};
        use orca_core::testing::MockRuntime;
        use std::collections::HashMap;
        use std::sync::Arc;
        use tokio::sync::RwLock;

        let runtime = Arc::new(MockRuntime::new());
        let state = Arc::new(crate::state::AppState::new(
            ClusterConfig {
                cluster: ClusterMeta {
                    name: "test".into(),
                    api_port: 0,
                    grpc_port: 0,
                    ..Default::default()
                },
                ..Default::default()
            },
            runtime,
            None,
            Arc::new(RwLock::new(HashMap::new())),
            Arc::new(RwLock::new(Vec::new())),
        ));

        let cfg = CleanupConfig {
            schedule: None,
            registry_keep_tags: 5,
            registry_container: "orca-registry".into(),
        };
        assert!(spawn_cleanup_scheduler(cfg, state).is_none());
    }

    #[test]
    fn invalid_schedule_returns_none() {
        use orca_core::config::{ClusterConfig, ClusterMeta};
        use orca_core::testing::MockRuntime;
        use std::collections::HashMap;
        use std::sync::Arc;
        use tokio::sync::RwLock;

        let runtime = Arc::new(MockRuntime::new());
        let state = Arc::new(crate::state::AppState::new(
            ClusterConfig {
                cluster: ClusterMeta {
                    name: "test".into(),
                    api_port: 0,
                    grpc_port: 0,
                    ..Default::default()
                },
                ..Default::default()
            },
            runtime,
            None,
            Arc::new(RwLock::new(HashMap::new())),
            Arc::new(RwLock::new(Vec::new())),
        ));

        let cfg = CleanupConfig {
            schedule: Some("not a cron".into()),
            registry_keep_tags: 5,
            registry_container: "orca-registry".into(),
        };
        assert!(spawn_cleanup_scheduler(cfg, state).is_none());
    }
}
