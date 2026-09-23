//! Which orca service owns a named volume (#198).
//!
//! Pre-hooks are keyed by service name (`gitea-db`), but volumes are named
//! `orca-<service>-data`. The backup used to strip only the `orca-` prefix
//! and looked for a hook named `gitea-db-data`, so no pre-hook ever ran and
//! every database was tarred live. The owner is now taken from the container
//! that actually mounts the volume, with a name-based fallback.

use std::collections::HashMap;

use bollard::Docker;
use bollard::container::ListContainersOptions;
use bollard::models::{ContainerSummary, MountPointTypeEnum};

/// volume name → service name, from the containers that mount each volume.
pub(crate) fn owners_from(containers: &[ContainerSummary]) -> HashMap<String, String> {
    let mut owners = HashMap::new();
    for c in containers {
        let Some(service) = c
            .names
            .as_deref()
            .and_then(|n| n.first())
            .map(|n| n.trim_start_matches('/'))
            .filter(|n| n.starts_with("orca-") && !n.starts_with("orca-backup-"))
            .map(|n| n.trim_start_matches("orca-").to_string())
        else {
            continue;
        };
        for m in c.mounts.as_deref().unwrap_or_default() {
            if m.typ == Some(MountPointTypeEnum::VOLUME)
                && let Some(vol) = &m.name
            {
                owners.entry(vol.clone()).or_insert_with(|| service.clone());
            }
        }
    }
    owners
}

/// The service a volume belongs to: its mounting container if there is one,
/// else the `orca-<service>-data` naming convention.
pub(crate) fn service_for(volume: &str, owners: &HashMap<String, String>) -> String {
    owners.get(volume).cloned().unwrap_or_else(|| {
        let s = volume.strip_prefix("orca-").unwrap_or(volume);
        s.strip_suffix("-data").unwrap_or(s).to_string()
    })
}

/// Query Docker for every container (running or not) and map volume owners.
/// A Docker error yields an empty map, so `service_for` falls back to names.
pub(crate) async fn volume_owners(docker: &Docker) -> HashMap<String, String> {
    let opts = ListContainersOptions::<String> {
        all: true,
        ..Default::default()
    };
    match docker.list_containers(Some(opts)).await {
        Ok(containers) => owners_from(&containers),
        Err(e) => {
            tracing::warn!("Failed to list containers for volume owners: {e}");
            HashMap::new()
        }
    }
}

#[cfg(test)]
#[path = "volume_owner_tests.rs"]
mod tests;
