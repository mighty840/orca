//! Services registered by orphan adoption are observed, never recreated
//! (#313).
//!
//! Adoption builds a record from a running container's labels: image,
//! placement, port, domains. It has no env, volume or mounts. Replaying it
//! (an agent's re-sync on rejoin, a redeploy, a start) replaced working
//! containers with ones that had no database password and no data volume.
//! Such a record now only describes what runs; a deploy from file replaces
//! it with a real config.

use orca_core::config::ServiceConfig;
use orca_core::types::Replicas;

/// Refuse to create workloads from an adopted record.
pub(crate) fn refuse_recreate(config: &ServiceConfig) -> anyhow::Result<()> {
    if config.adopted {
        anyhow::bail!(
            "{} was adopted from a running container, so orca has no env, volume or mounts \
             on record for it and will not recreate it; deploy it from its service.toml \
             (orca deploy -f <file>) first",
            config.name
        );
    }
    Ok(())
}

/// Mark a stored record as adopted when it has the exact shape adoption
/// gave it before records carried the mark: pinned to a node by numeric id,
/// no project and nothing a `service.toml` would add. Returns whether it
/// marked it.
pub(crate) fn mark_legacy_record(config: &mut ServiceConfig) -> bool {
    if config.adopted || !looks_adopted(config) {
        return false;
    }
    config.adopted = true;
    true
}

fn looks_adopted(c: &ServiceConfig) -> bool {
    let pinned_by_id = c.placement.as_ref().is_some_and(|p| {
        p.labels.is_none()
            && p.requires_gpu.is_none()
            && p.node
                .as_deref()
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    });
    pinned_by_id
        && c.project.is_none()
        && c.env.is_empty()
        && c.volume.is_none()
        && c.mounts.is_empty()
        && c.cmd.is_empty()
        && c.aliases.is_empty()
        && c.depends_on.is_empty()
        && c.health.is_none()
        && c.resources.is_none()
        && c.build.is_none()
        && c.module.is_none()
        && c.restart_policy.is_none()
        && matches!(c.replicas, Replicas::Fixed(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adopted_shape() -> ServiceConfig {
        let c = orca_core::ws_types::ManagedContainer {
            service_name: "macli-postgres".into(),
            image: "postgres:16".into(),
            status: "running".into(),
            container_id: "abc".into(),
            port: Some(5432),
            domains: vec![],
            network: Some("orca-macli".into()),
            routes: vec![],
            strip_prefix: None,
        };
        let mut config = crate::adoption::config_from_managed(7, &c);
        config.adopted = false; // as stored before the mark existed
        config
    }

    #[test]
    fn an_adopted_record_cannot_be_recreated() {
        let mut config = adopted_shape();
        config.adopted = true;
        let err = refuse_recreate(&config).unwrap_err().to_string();
        assert!(err.contains("orca deploy -f"), "{err}");
    }

    #[test]
    fn a_record_stored_by_an_older_adoption_is_marked() {
        let mut config = adopted_shape();
        assert!(mark_legacy_record(&mut config));
        assert!(config.adopted);
        assert!(!mark_legacy_record(&mut config), "marked once");
    }

    #[test]
    fn a_service_deployed_from_file_is_not_marked() {
        let mut with_env = adopted_shape();
        with_env.env.insert("POSTGRES_PASSWORD".into(), "x".into());
        let mut with_project = adopted_shape();
        with_project.project = Some("macli".into());
        let mut pinned_by_name = adopted_shape();
        pinned_by_name.placement.as_mut().unwrap().node = Some("prod1".into());
        for mut c in [with_env, with_project, pinned_by_name] {
            assert!(!mark_legacy_record(&mut c), "{c:?}");
        }
    }
}
