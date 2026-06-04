//! Detection and reporting of host bind mounts that the volume backup does
//! not capture (issue #83).
//!
//! `orca backup all` tars named Docker volumes, but host bind mounts declared
//! via `mounts = ["/host/path:/container/path"]` point at paths on the host
//! filesystem that live outside any backup. If the node is lost, that data is
//! gone — and previously there was no signal at backup time (not even when a
//! service had bind mounts and *no* named volume, so the volume backup said
//! "nothing to do" and exited clean). We inspect the running orca service
//! containers — Docker is the source of truth for what is actually mounted —
//! and surface every bind mount as a warning.

use std::collections::BTreeMap;

use bollard::Docker;
use bollard::container::ListContainersOptions;
use bollard::models::MountPointTypeEnum;

/// A host bind mount on an orca service container that the backup does not
/// cover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnbackedMount {
    pub service: String,
    pub host_path: String,
    pub container_path: String,
}

/// Inspect all orca service containers for host bind mounts. Named volumes
/// (`Type: volume`) are captured by the volume backup; only bind mounts
/// (`Type: bind`) are returned here. Best-effort: a Docker error logs and
/// yields an empty list rather than failing the backup.
pub(crate) async fn list_unbacked_bind_mounts(docker: &Docker) -> Vec<UnbackedMount> {
    let opts = ListContainersOptions::<String> {
        all: true,
        ..Default::default()
    };
    let containers = match docker.list_containers(Some(opts)).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Failed to list containers for bind-mount check: {e}");
            return Vec::new();
        }
    };

    let mut out = Vec::new();
    for c in containers {
        let Some(service) = service_from_names(c.names.as_deref()) else {
            continue;
        };
        for m in c.mounts.unwrap_or_default() {
            if m.typ == Some(MountPointTypeEnum::BIND) {
                out.push(UnbackedMount {
                    service: service.clone(),
                    host_path: m.source.unwrap_or_default(),
                    container_path: m.destination.unwrap_or_default(),
                });
            }
        }
    }
    out
}

/// Derive the orca service name from a container's Docker names. Containers
/// are named `orca-{service}` (with a leading `/` in the API). Returns `None`
/// for non-orca containers and the transient `orca-backup-*` helpers we spawn
/// during the backup itself.
fn service_from_names(names: Option<&[String]>) -> Option<String> {
    let raw = names?.first()?;
    let name = raw.trim_start_matches('/');
    if !name.starts_with("orca-") || name.starts_with("orca-backup-") {
        return None;
    }
    Some(name.strip_prefix("orca-").unwrap_or(name).to_string())
}

/// Emit a warning for each service with unbacked bind mounts. Prints to stdout
/// (seen by a manual `orca backup all`) and via `tracing::warn!` (captured by
/// scheduled and agent-dispatched runs). No-op when there are no bind mounts.
pub(crate) async fn warn_unbacked_bind_mounts(docker: &Docker) {
    report_unbacked_bind_mounts(list_unbacked_bind_mounts(docker).await);
}

/// Pure formatting/reporting half of [`warn_unbacked_bind_mounts`], split out
/// so it is unit-testable without a live Docker daemon. The summary line is
/// printed last so it becomes the dashboard's last-backup message for
/// scheduled/agent runs (which relay the subprocess's final stdout line).
fn report_unbacked_bind_mounts(mounts: Vec<UnbackedMount>) {
    if mounts.is_empty() {
        return;
    }
    let grouped = group_by_service(&mounts);
    let svc_count = grouped.len();
    let mount_count = mounts.len();

    println!(
        "WARNING: {mount_count} host bind mount(s) on {svc_count} service(s) are NOT backed up \
         (only named volumes are captured):"
    );
    for (svc, paths) in &grouped {
        for p in paths {
            println!("  {svc}: {p}");
        }
        tracing::warn!(
            service = %svc,
            mounts = %paths.join(", "),
            "service has host bind mounts excluded from backup"
        );
    }
    println!(
        "Backup summary: {mount_count} unbacked bind mount(s) on {svc_count} service(s) — \
         data behind these host paths is NOT in the backup."
    );
}

/// Group bind mounts by service into `host -> container` display strings,
/// ordered (BTreeMap) for stable output.
fn group_by_service(mounts: &[UnbackedMount]) -> BTreeMap<String, Vec<String>> {
    let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for m in mounts {
        grouped
            .entry(m.service.clone())
            .or_default()
            .push(format!("{} -> {}", m.host_path, m.container_path));
    }
    grouped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mount(service: &str, host: &str, ctr: &str) -> UnbackedMount {
        UnbackedMount {
            service: service.into(),
            host_path: host.into(),
            container_path: ctr.into(),
        }
    }

    #[test]
    fn service_from_names_strips_prefix_and_slash() {
        assert_eq!(
            service_from_names(Some(&["/orca-freqtrade".to_string()])),
            Some("freqtrade".to_string())
        );
    }

    #[test]
    fn service_from_names_ignores_non_orca() {
        assert_eq!(service_from_names(Some(&["/postgres".to_string()])), None);
    }

    #[test]
    fn service_from_names_ignores_transient_backup_helpers() {
        // The busybox tar containers we spawn are named orca-backup-* and must
        // not be reported as services with unbacked mounts.
        assert_eq!(
            service_from_names(Some(&["/orca-backup-12345".to_string()])),
            None
        );
    }

    #[test]
    fn service_from_names_handles_missing() {
        assert_eq!(service_from_names(None), None);
        assert_eq!(service_from_names(Some(&[])), None);
    }

    #[test]
    fn group_by_service_groups_and_orders() {
        let mounts = vec![
            mount("freqtrade", "/home/u/data", "/freqtrade/user_data"),
            mount("freqtrade", "/home/u/logs", "/freqtrade/logs"),
            mount("api", "/etc/api", "/etc/api"),
        ];
        let grouped = group_by_service(&mounts);
        let keys: Vec<_> = grouped.keys().cloned().collect();
        assert_eq!(keys, vec!["api".to_string(), "freqtrade".to_string()]);
        assert_eq!(grouped["freqtrade"].len(), 2);
        assert_eq!(grouped["api"], vec!["/etc/api -> /etc/api".to_string()]);
    }

    #[test]
    fn report_empty_is_noop() {
        // The no-bind-mounts path must not panic or print.
        report_unbacked_bind_mounts(Vec::new());
    }
}
