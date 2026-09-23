use std::collections::HashMap;

use bollard::models::{ContainerSummary, MountPoint, MountPointTypeEnum};

use super::*;

fn container(name: &str, volumes: &[&str]) -> ContainerSummary {
    ContainerSummary {
        names: Some(vec![format!("/{name}")]),
        mounts: Some(
            volumes
                .iter()
                .map(|v| MountPoint {
                    typ: Some(MountPointTypeEnum::VOLUME),
                    name: Some(v.to_string()),
                    ..Default::default()
                })
                .collect(),
        ),
        ..Default::default()
    }
}

#[test]
fn the_live_naming_maps_volumes_to_their_service() {
    // Real names on the breakpilot cluster: the hook is registered for
    // "gitea-db", the volume is "orca-gitea-db-data". The old lookup used
    // "gitea-db-data" and never found a hook.
    let owners = owners_from(&[
        container("orca-gitea-db", &["orca-gitea-db-data"]),
        container(
            "orca-breakpilot-nextcloud-db",
            &["orca-breakpilot-nextcloud-db-data"],
        ),
    ]);
    assert_eq!(service_for("orca-gitea-db-data", &owners), "gitea-db");
    assert_eq!(
        service_for("orca-breakpilot-nextcloud-db-data", &owners),
        "breakpilot-nextcloud-db"
    );
}

#[test]
fn a_volume_shared_by_two_containers_belongs_to_the_first_seen() {
    // Nextcloud's app and cron share one volume; the hook lookup needs one
    // owner and either is acceptable, but it must be stable.
    let owners = owners_from(&[
        container(
            "orca-breakpilot-nextcloud",
            &["orca-breakpilot-nextcloud-data"],
        ),
        container(
            "orca-breakpilot-nextcloud-cron",
            &["orca-breakpilot-nextcloud-data"],
        ),
    ]);
    assert_eq!(
        service_for("orca-breakpilot-nextcloud-data", &owners),
        "breakpilot-nextcloud"
    );
}

#[test]
fn non_orca_and_backup_helper_containers_are_ignored() {
    let owners = owners_from(&[
        container("postgres-manual", &["orca-x-data"]),
        container("orca-backup-123", &["orca-x-data"]),
    ]);
    assert!(owners.is_empty());
}

#[test]
fn unmounted_volumes_fall_back_to_the_naming_convention() {
    let none = HashMap::new();
    assert_eq!(service_for("orca-litellm-db-data", &none), "litellm-db");
    assert_eq!(service_for("orca-legacy", &none), "legacy");
}
