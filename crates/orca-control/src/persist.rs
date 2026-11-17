//! Storing the configs a deploy recorded, so a master restart restores them.

use orca_core::config::ServiceConfig;
use tracing::warn;

use crate::state::AppState;

/// Store each config that was deployed, and each agent-placed config whose
/// deploy the agent received but did not finish in time.
///
/// A remote deploy records the config in memory before it is dispatched,
/// so that a lost ack cannot leave it stale. When the completion then timed
/// out (a slow pull), the container usually came up anyway, but the config
/// was never stored: after a master restart the service was unknown, and
/// orphan adoption registered its container with a hollow config (#313).
/// A deploy the agent rejected, or never got, keeps the stored config.
pub(crate) async fn persist_recorded(
    state: &AppState,
    configs: &[ServiceConfig],
    deployed: &[String],
    errors: &[String],
    context: &str,
) {
    let timed_out = |name: &str| {
        let prefix = format!("{name}: ");
        errors
            .iter()
            .any(|e| e.starts_with(&prefix) && e.contains(crate::remote_deploy::COMPLETION_TIMEOUT))
    };
    let Some(store) = &state.store else {
        return;
    };
    let recorded: Vec<&ServiceConfig> = {
        let services = state.services.read().await;
        configs
            .iter()
            .filter(|cfg| {
                deployed.contains(&cfg.name)
                    || (timed_out(&cfg.name)
                        && crate::placement::placed_on_agent(cfg)
                        && services
                            .get(&cfg.name)
                            .is_some_and(|s| s.config.spec_matches(cfg)))
            })
            .collect()
    };
    for cfg in recorded {
        if let Err(e) = store.set_service(&cfg.name, cfg) {
            warn!("{context}: persist {} failed: {e}", cfg.name);
        }
    }
}
