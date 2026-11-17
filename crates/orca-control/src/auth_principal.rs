//! Who a request comes from (#80): the token's name and role, attached to
//! the request by `auth_middleware` so handlers (`whoami`) and the audit log
//! can name the caller.

use orca_core::config::Role;

use crate::state::AppState;

/// Every action `Role::can` understands, as `ROUTE_POLICY` names them.
pub(crate) const ACTIONS: &[&str] = &[
    "status",
    "logs",
    "cluster_info",
    "deploy",
    "stop",
    "scale",
    "rollback",
    "secrets",
    crate::auth::ADMIN_ONLY,
];

/// The caller of an authenticated request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    /// `[[token]]` name, `legacy-<n>` for the n-th `api_tokens` entry, or
    /// `anonymous` on a cluster with no tokens.
    pub name: String,
    pub role: Role,
    /// An `api_tokens` entry (admin; possibly the agents' join token).
    pub legacy: bool,
    /// No tokens are configured, so every caller is admin.
    pub open: bool,
}

impl Principal {
    /// The caller on a cluster with no tokens configured.
    pub(crate) fn anonymous() -> Self {
        Self {
            name: "anonymous".into(),
            role: Role::Admin,
            legacy: false,
            open: true,
        }
    }

    /// The actions this caller's role may perform.
    pub fn actions(&self) -> Vec<String> {
        ACTIONS
            .iter()
            .filter(|a| self.role.can(a))
            .map(|a| a.to_string())
            .collect()
    }
}

/// The principal a presented token belongs to, or `None`. Same matching as
/// [`crate::auth::resolve_token`], which the agent WebSocket keeps using.
pub(crate) fn resolve_principal(state: &AppState, token: &str) -> Option<Principal> {
    if token.is_empty() {
        return None;
    }
    let legacy = state
        .api_tokens
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .position(|t| crate::auth::ct_eq(token, t));
    if let Some(i) = legacy {
        return Some(Principal {
            name: format!("legacy-{}", i + 1),
            role: Role::Admin,
            legacy: true,
            open: false,
        });
    }
    state
        .cluster_config
        .token
        .iter()
        .find(|t| crate::auth::ct_eq(token, &t.value))
        .map(|t| Principal {
            name: t.name.clone(),
            role: t.role,
            legacy: false,
            open: false,
        })
}
