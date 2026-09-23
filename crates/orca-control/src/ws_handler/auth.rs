//! Authorization for the agent control-plane WebSocket.
//!
//! The agent channel is not an ordinary API call. Once open, the master sends
//! the node every service spec pinned to it with `${secrets.X}` already
//! resolved, and trusts the node's reports about what is running. So opening
//! it is an admin-level act, whatever the token's role grants on HTTP.

use orca_core::config::Role;

use crate::auth::resolve_token;
use crate::state::AppState;

/// The RBAC action required to open the agent channel.
///
/// [`Role::can`] grants every action to `Admin` and only named actions to
/// `Deployer` and `Viewer`, so this is admin-only without special-casing.
pub(super) const AGENT_ACTION: &str = "agent";

/// Why an agent-channel upgrade was refused.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    /// The token is unknown or empty.
    Unauthorized,
    /// The token is valid but its role may not open the agent channel.
    Forbidden(Role),
}

/// Decide whether `token` may open the agent channel.
pub(super) fn authorize_agent(state: &AppState, token: &str) -> Result<Role, Refusal> {
    let role = resolve_token(state, token).ok_or(Refusal::Unauthorized)?;
    if role.can(AGENT_ACTION) {
        Ok(role)
    } else {
        Err(Refusal::Forbidden(role))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use orca_core::config::{ApiToken, ClusterConfig};
    use orca_core::testing::MockRuntime;
    use tokio::sync::RwLock;

    use super::*;

    fn state_with(legacy: &[&str], named: &[(&str, Role)]) -> AppState {
        let config = ClusterConfig {
            api_tokens: legacy.iter().map(|s| s.to_string()).collect(),
            token: named
                .iter()
                .map(|(v, role)| ApiToken {
                    name: format!("{v}-name"),
                    value: v.to_string(),
                    role: *role,
                })
                .collect(),
            ..Default::default()
        };
        AppState::new(
            config,
            Arc::new(MockRuntime::new()),
            None,
            Arc::new(RwLock::new(HashMap::new())),
            Arc::new(RwLock::new(Vec::new())),
        )
    }

    #[test]
    fn legacy_cluster_token_may_open_the_agent_channel() {
        // The token agents join with today is a legacy api_token, i.e. admin.
        // Requiring admin must not lock out an agent that has not upgraded.
        let state = state_with(&["cluster-tok"], &[]);
        assert_eq!(authorize_agent(&state, "cluster-tok"), Ok(Role::Admin));
    }

    #[test]
    fn named_admin_token_may_open_the_agent_channel() {
        let state = state_with(&[], &[("adm", Role::Admin)]);
        assert_eq!(authorize_agent(&state, "adm"), Ok(Role::Admin));
    }

    #[test]
    fn viewer_and_deployer_tokens_are_forbidden() {
        // A dashboard token must not be able to pose as a node and receive
        // that node's resolved secrets.
        let state = state_with(&[], &[("view", Role::Viewer), ("ci", Role::Deployer)]);
        assert_eq!(
            authorize_agent(&state, "view"),
            Err(Refusal::Forbidden(Role::Viewer))
        );
        assert_eq!(
            authorize_agent(&state, "ci"),
            Err(Refusal::Forbidden(Role::Deployer))
        );
    }

    #[test]
    fn unknown_or_empty_tokens_are_unauthorized() {
        let state = state_with(&["cluster-tok"], &[]);
        assert_eq!(authorize_agent(&state, "nope"), Err(Refusal::Unauthorized));
        assert_eq!(authorize_agent(&state, ""), Err(Refusal::Unauthorized));
    }

    #[test]
    fn an_empty_configured_token_does_not_authenticate_an_empty_presented_one() {
        let state = state_with(&[""], &[("", Role::Admin)]);
        assert_eq!(authorize_agent(&state, ""), Err(Refusal::Unauthorized));
    }

    #[test]
    fn a_prefix_of_a_real_token_is_not_accepted() {
        let state = state_with(&["cluster-token-full"], &[]);
        assert_eq!(
            authorize_agent(&state, "cluster-token"),
            Err(Refusal::Unauthorized)
        );
    }
}
