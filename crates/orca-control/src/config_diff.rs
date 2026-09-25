//! Which declared services must be applied (#177). Shared by the declarative
//! loop and the infra webhook so both paths agree on what counts as a change.

use orca_core::config::ServiceConfig;

use crate::state::AppState;

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
        .collect()
}
