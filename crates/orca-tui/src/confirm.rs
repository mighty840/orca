//! y/N confirmation for destructive actions (#262).
//!
//! An action that stops, drains or deletes something is armed first and
//! shown in the footer; the next keypress answers it. Only `y` confirms, so a
//! stray key can never stop a service. The armed action names its target, so
//! it can't drift to another row if the list refreshes in between.

use crate::api::ApiClient;
use crate::state::AppState;

/// A destructive action waiting for y/N.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    StopService(String),
    StopProject(String),
    DrainNode(u64),
    DeleteWebhook(String),
    DeleteSecret(String),
}

impl Confirm {
    /// The question shown in the footer.
    pub fn prompt(&self) -> String {
        let what = match self {
            Confirm::StopService(s) => format!("Stop service '{s}'"),
            Confirm::StopProject(p) => format!("Stop every service in project '{p}'"),
            Confirm::DrainNode(id) => format!("Drain node {id}"),
            Confirm::DeleteWebhook(s) => format!("Delete the webhook for '{s}'"),
            Confirm::DeleteSecret(k) => format!("Delete secret '{k}'"),
        };
        format!("{what}? y:confirm  any other key:cancel")
    }
}

/// Arm `action`; the footer shows it until the next keypress.
pub(crate) fn arm(state: &mut AppState, action: Confirm) {
    state.pending_confirm = Some(action);
}

/// Resolve the armed action with the key that was pressed. Returns false
/// when nothing was armed, so the key is handled normally.
pub(crate) async fn answer(client: &ApiClient, state: &mut AppState, yes: bool) -> bool {
    let Some(action) = state.pending_confirm.take() else {
        return false;
    };
    if !yes {
        state.flash("Cancelled".into());
        return true;
    }
    run(client, state, action).await;
    true
}

async fn run(client: &ApiClient, state: &mut AppState, action: Confirm) {
    match action {
        Confirm::StopService(name) => match client.stop(&name).await {
            Ok(()) => state.flash(format!("Stopped {name}")),
            Err(e) => state.error = Some(format!("Stop failed: {e}")),
        },
        Confirm::StopProject(project) => match client.stop_project(&project).await {
            Ok(()) => state.flash(format!("Stopped project {project}")),
            Err(e) => state.error = Some(format!("Stop project failed: {e}")),
        },
        Confirm::DrainNode(id) => match client.drain(id).await {
            Ok(()) => state.flash(format!("Draining node {id}")),
            Err(e) => state.error = Some(format!("Drain failed: {e}")),
        },
        Confirm::DeleteWebhook(service) => {
            crate::webhook_actions::delete_webhook(client, state, &service).await
        }
        Confirm::DeleteSecret(key) => {
            crate::secrets_actions::delete_secret(client, state, &key).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> ApiClient {
        // Nothing listens here: a confirmed action would fail, a cancelled
        // one never calls it.
        ApiClient::new("http://127.0.0.1:9")
    }

    #[tokio::test]
    async fn any_key_but_y_cancels_without_calling_the_api() {
        let mut state = AppState::new();
        arm(&mut state, Confirm::StopService("api".into()));
        assert!(answer(&client(), &mut state, false).await);
        assert!(state.pending_confirm.is_none());
        assert_eq!(state.status_msg.as_deref(), Some("Cancelled"));
        assert!(state.error.is_none(), "no stop was attempted");
    }

    #[tokio::test]
    async fn y_runs_the_armed_action() {
        let mut state = AppState::new();
        arm(&mut state, Confirm::StopService("api".into()));
        assert!(answer(&client(), &mut state, true).await);
        let err = state.error.as_deref().unwrap_or("");
        assert!(
            err.starts_with("Stop failed"),
            "the stop was attempted: {err}"
        );
    }

    #[tokio::test]
    async fn nothing_armed_leaves_the_key_to_normal_handling() {
        let mut state = AppState::new();
        assert!(!answer(&client(), &mut state, true).await);
    }

    #[test]
    fn prompts_name_the_target() {
        assert_eq!(
            Confirm::DrainNode(7).prompt(),
            "Drain node 7? y:confirm  any other key:cancel"
        );
        assert!(
            Confirm::StopService("api".into())
                .prompt()
                .contains("'api'")
        );
    }
}
