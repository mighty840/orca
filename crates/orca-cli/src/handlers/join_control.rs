//! The agent's control-plane startup, which runs next to an already serving
//! proxy (#209).
//!
//! The proxy used to start only after registration succeeded, and
//! registration retried until it did. A wrong token, an unreachable master or
//! a mesh interface that came up late meant no public traffic on the node at
//! all, although its containers and routes were local: 11.5 minutes of every
//! site on breakpilot's agent on 2026-09-23.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use orca_agent::grpc::AgentClient;
use tracing::{error, info, warn};

/// Used when neither a cached nor a configured email exists. Let's Encrypt
/// rejects it when creating an account, which signals that `acme_email`
/// must be set on the master.
const PLACEHOLDER_EMAIL: &str = "admin@localhost";

fn email_cache() -> PathBuf {
    dirs_next::home_dir()
        .unwrap_or_else(|| ".".into())
        .join(".orca/acme_email")
}

/// The ACME contact email for this start, without asking the master.
pub(crate) fn cached_acme_email() -> String {
    email_from(
        std::fs::read_to_string(email_cache()).ok(),
        std::env::var("ORCA_ACME_EMAIL").ok(),
    )
}

/// The cached email (from the last fetch from the master), else the
/// `ORCA_ACME_EMAIL` environment variable, else a placeholder.
fn email_from(cached: Option<String>, env: Option<String>) -> String {
    [cached, env]
        .into_iter()
        .flatten()
        .map(|e| e.trim().to_string())
        .find(|e| !e.is_empty())
        .unwrap_or_else(|| PLACEHOLDER_EMAIL.to_string())
}

/// Register with the master, retrying for as long as it takes. The proxy
/// is already serving meanwhile, so this never exits the agent.
pub(crate) async fn register_until_accepted(
    agent: &AgentClient,
    address: &str,
    labels: &HashMap<String, String>,
    local_routes: usize,
) {
    let mut delay = Duration::from_secs(2);
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match agent.register(address, labels).await {
            Ok(()) => {
                info!("Registered with cluster after {attempt} attempt(s)");
                return;
            }
            Err(e) if attempt == 1 || attempt.is_multiple_of(10) => error!(
                "control plane unreachable ({e}); still serving {local_routes} local \
                 route(s), retrying in {delay:?}"
            ),
            Err(e) => warn!("registration attempt {attempt} failed: {e}, retrying in {delay:?}"),
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(30));
    }
}

/// Fetch the master's `acme_email` and cache it for the next start.
pub(crate) async fn refresh_acme_email(agent: &AgentClient) {
    let Some(info) = agent.fetch_cluster_info().await else {
        return;
    };
    let Some(email) = info.get("acme_email").and_then(|v| v.as_str()) else {
        return;
    };
    let path = email_cache();
    if std::fs::read_to_string(&path).is_ok_and(|c| c.trim() == email) {
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match std::fs::write(&path, format!("{email}\n")) {
        Ok(()) => info!("Cached ACME email {email} for the next start"),
        Err(e) => warn!("cannot cache the ACME email in {}: {e}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::email_from;

    #[test]
    fn the_cached_email_wins_then_the_env_then_a_placeholder() {
        assert_eq!(
            email_from(Some("ops@x.com\n".into()), Some("env@x.com".into())),
            "ops@x.com"
        );
        assert_eq!(email_from(None, Some("env@x.com".into())), "env@x.com");
        assert_eq!(email_from(Some("  \n".into()), None), "admin@localhost");
    }
}
