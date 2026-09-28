//! Start, redeploy, roll back and promote a service from the TUI (#265).
//!
//! Start only resumes a paused service, so it runs at once. The other three
//! replace running containers and ask y/N first (`crate::confirm`).

use crate::api::{ApiClient, ServiceAction};
use crate::confirm::Confirm;
use crate::state::AppState;

/// The service a command or key acts on: the named one, else the one in
/// view or selected.
pub(crate) fn target(state: &AppState, name: Option<&str>) -> Option<String> {
    name.map(str::to_string)
        .or_else(|| crate::current_service_name(state))
}

/// `u`, `:start`, `d`, `:redeploy`, `:rollback`, `:promote`.
pub(crate) async fn request(
    client: &ApiClient,
    state: &mut AppState,
    action: ServiceAction,
    name: Option<&str>,
) {
    let Some(service) = target(state, name) else {
        state.flash(format!("Usage: :{} <service>", action.verb()));
        return;
    };
    match action {
        ServiceAction::Start => run(client, state, action, &service).await,
        _ => crate::confirm::arm(state, Confirm::Service(action, service)),
    }
}

pub(crate) async fn run(
    client: &ApiClient,
    state: &mut AppState,
    action: ServiceAction,
    service: &str,
) {
    match client.service_action(service, action).await {
        Ok(()) => state.flash(match action {
            ServiceAction::Start => format!("Started {service}"),
            ServiceAction::Redeploy => format!("Redeploying {service}"),
            ServiceAction::Rollback => format!("Rolling back {service}"),
            ServiceAction::Promote => format!("Promoted {service}"),
        }),
        Err(e) => state.error = Some(format!("{} failed: {e}", action.verb())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::View;

    fn client() -> ApiClient {
        ApiClient::new("http://127.0.0.1:9")
    }

    #[tokio::test]
    async fn redeploy_rollback_and_promote_ask_first() {
        for action in [
            ServiceAction::Redeploy,
            ServiceAction::Rollback,
            ServiceAction::Promote,
        ] {
            let mut state = AppState::new();
            request(&client(), &mut state, action, Some("api")).await;
            assert_eq!(
                state.pending_confirm,
                Some(Confirm::Service(action, "api".into()))
            );
            assert!(state.error.is_none(), "nothing was called yet");
        }
    }

    #[tokio::test]
    async fn start_runs_at_once() {
        let mut state = AppState::new();
        request(&client(), &mut state, ServiceAction::Start, Some("api")).await;
        assert!(state.pending_confirm.is_none());
        let err = state.error.as_deref().unwrap_or("");
        assert!(err.starts_with("start failed"), "the call was made: {err}");
    }

    #[test]
    fn the_service_in_view_is_the_default_target() {
        let mut state = AppState::new();
        state.view = View::Detail {
            service: "db".into(),
        };
        assert_eq!(target(&state, None).as_deref(), Some("db"));
        assert_eq!(target(&state, Some("api")).as_deref(), Some("api"));
    }
}
