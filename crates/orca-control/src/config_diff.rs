//! Which declared services must be applied (#177). Shared by the declarative
//! loop and the infra webhook so both paths agree on what counts as a change.

use orca_core::config::ServiceConfig;

use crate::state::AppState;

/// How long a declared spec whose deploy failed is left alone (#174).
pub(crate) const FAILED_DEPLOY_COOLDOWN: std::time::Duration =
    std::time::Duration::from_secs(15 * 60);

/// The services in `declared` that are new or whose declaration changed.
///
/// The baseline is the last declared config, which the store persists on
/// every successful apply. The in-memory config is used when the store has
/// none. The in-memory config can differ from what was declared: a manual
/// `orca scale` changes its replica count, and that must not be reverted on
/// every pass.
///
/// A service counts as changed when:
/// - its container spec differs from the running one (`spec_matches`
///   against the in-memory config, which also catches drift); or
/// - any other declared field differs from the baseline
///   (`declared_matches`: placement, probes, pull policy, certificates, …);
///   before #177 these were ignored, so the edit was never applied or
///   persisted, and a master restart restored the stale config; or
/// - `replicas` differs from the baseline.
pub(crate) async fn changed_services(
    state: &AppState,
    declared: impl IntoIterator<Item = ServiceConfig>,
) -> Vec<ServiceConfig> {
    let stored = state
        .store
        .as_ref()
        .and_then(|s| s.get_all_services().ok())
        .unwrap_or_default();
    let failed = state.failed_deploys.read().await;
    let services = state.services.read().await;
    declared
        .into_iter()
        .filter(|cfg| match services.get(&cfg.name) {
            None => true,
            Some(svc) => {
                let baseline = stored.get(&cfg.name).unwrap_or(&svc.config);
                !svc.config.spec_matches(cfg)
                    || !baseline.declared_matches(cfg)
                    || baseline.replicas != cfg.replicas
            }
        })
        .filter(|cfg| !in_cooldown(&failed, cfg))
        .collect()
}

/// This exact spec failed to deploy recently (#174). A failed replace stops
/// the old container and restores it; retrying the same spec every pass
/// would repeat that. A changed spec, e.g. the fix, goes through at once, and
/// `orca deploy` / `orca redeploy` bypass this.
fn in_cooldown(
    failed: &std::collections::HashMap<String, (ServiceConfig, std::time::Instant)>,
    cfg: &ServiceConfig,
) -> bool {
    let Some((spec, at)) = failed.get(&cfg.name) else {
        return false;
    };
    let same = spec.declared_matches(cfg) && spec.replicas == cfg.replicas;
    let waiting = at.elapsed() < FAILED_DEPLOY_COOLDOWN;
    if same && waiting {
        tracing::debug!(
            "skipping {}: this spec failed to deploy {}s ago; retrying after {} min or \
             when the spec changes",
            cfg.name,
            at.elapsed().as_secs(),
            FAILED_DEPLOY_COOLDOWN.as_secs() / 60
        );
    }
    same && waiting
}
