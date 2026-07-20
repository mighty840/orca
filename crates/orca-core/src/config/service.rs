use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::backup::ServiceBackupConfig;
use crate::types::{
    DeployStrategy, PlacementConstraint, PullPolicy, Replicas, ResourceLimits, RuntimeKind,
    VolumeSpec,
};

/// Probe configuration for readiness/liveness checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeConfig {
    /// HTTP path to probe (e.g., "/healthz").
    pub path: String,
    /// Port to probe (defaults to service port).
    pub port: Option<u16>,
    /// Seconds between probes (default: 10).
    #[serde(default = "default_probe_interval")]
    pub interval_secs: u64,
    /// Seconds to wait for response (default: 3).
    #[serde(default = "default_probe_timeout")]
    pub timeout_secs: u64,
    /// Failures before action (default: 3).
    #[serde(default = "default_probe_failures")]
    pub failure_threshold: u32,
    /// Seconds to wait before first probe (default: 5).
    #[serde(default = "default_initial_delay")]
    pub initial_delay_secs: u64,
}

fn default_probe_interval() -> u64 {
    10
}
fn default_probe_timeout() -> u64 {
    3
}
fn default_probe_failures() -> u32 {
    3
}
fn default_initial_delay() -> u64 {
    5
}

/// Build-from-source configuration for a service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildConfig {
    /// Git repository URL (SSH or HTTPS).
    pub repo: String,
    /// Branch to build from (default: "main").
    pub branch: Option<String>,
    /// Dockerfile path relative to repo root (default: "Dockerfile").
    pub dockerfile: Option<String>,
    /// Build context relative to repo root (default: ".").
    pub context: Option<String>,
}

impl BuildConfig {
    /// Branch to use, defaulting to "main".
    pub fn branch_or_default(&self) -> &str {
        self.branch.as_deref().unwrap_or("main")
    }

    /// Dockerfile path, defaulting to "Dockerfile".
    pub fn dockerfile_or_default(&self) -> &str {
        self.dockerfile.as_deref().unwrap_or("Dockerfile")
    }

    /// Build context, defaulting to ".".
    pub fn context_or_default(&self) -> &str {
        self.context.as_deref().unwrap_or(".")
    }
}

/// Services configuration (`services.toml`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServicesConfig {
    pub service: Vec<ServiceConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfig {
    pub name: String,
    /// Project name (set automatically from directory name by load_dir).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default)]
    pub runtime: RuntimeKind,
    /// Container image (for container runtime).
    pub image: Option<String>,
    /// Wasm module path or OCI reference (for wasm runtime).
    pub module: Option<String>,
    #[serde(default)]
    pub replicas: Replicas,
    /// Container port (internal).
    pub port: Option<u16>,
    /// Host port to bind (e.g., 443 for edge proxies). If omitted, ephemeral.
    pub host_port: Option<u16>,
    /// Domain for reverse proxy routing (orca proxy handles TLS).
    /// Mutually exclusive with `domains`.
    pub domain: Option<String>,
    /// Multiple domains for the same service/container (e.g. apex + www,
    /// regional aliases, or a dual-TLD migration). Mutually exclusive with
    /// `domain`. Each entry gets its own cert + SNI route to the same backend,
    /// so you no longer have to duplicate the `[[service]]` block (which would
    /// spawn a redundant container) just to serve a second hostname.
    #[serde(default)]
    pub domains: Vec<String>,
    /// Path routes under the domain (e.g., ["/api/*", "/admin/*"]).
    /// Default: ["/*"] (catch-all).
    #[serde(default)]
    pub routes: Vec<String>,
    /// Health check path (e.g., "/healthz"). Legacy shorthand for liveness probe.
    pub health: Option<String>,
    /// Readiness probe: container must pass before receiving traffic.
    pub readiness: Option<ProbeConfig>,
    /// Liveness probe: container is restarted if this fails.
    pub liveness: Option<ProbeConfig>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    pub resources: Option<ResourceLimits>,
    pub volume: Option<VolumeSpec>,
    pub deploy: Option<DeployStrategy>,
    pub placement: Option<PlacementConstraint>,
    /// Docker network name. Services with the same network can reach each other.
    /// Auto-prefixed with "orca-". If omitted, derived from service name prefix.
    pub network: Option<String>,
    /// Network aliases (resolvable names within the Docker network).
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Host bind mounts (e.g., ["/host/path:/container/path:ro"]).
    #[serde(default)]
    pub mounts: Vec<String>,
    /// Wasm triggers: "http:/path", "cron:expr", "queue:topic", "event:pattern"
    #[serde(default)]
    pub triggers: Vec<String>,
    /// Static assets directory (for builtin:static-server Wasm module).
    pub assets: Option<String>,
    /// Build configuration: clone a repo and build a Docker image from a Dockerfile.
    /// When set, `image` is not required — the built image is used instead.
    pub build: Option<BuildConfig>,
    /// Path to PEM certificate file for BYO TLS (skips ACME provisioning).
    pub tls_cert: Option<String>,
    /// Path to PEM private key file for BYO TLS.
    pub tls_key: Option<String>,
    /// Join the shared `orca-internal` network for cross-service communication.
    #[serde(default)]
    pub internal: bool,
    /// Services that must be running before this service starts.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Command to run in the container (overrides image CMD).
    #[serde(default)]
    pub cmd: Vec<String>,
    /// Extra fixed host:container port bindings (e.g. ["22222:22"]).
    #[serde(default)]
    pub extra_ports: Vec<String>,
    /// Prefix to strip from incoming request paths before forwarding to
    /// this service. Used with `routes` to mount a backend under a
    /// subpath without exposing that subpath to the backend itself —
    /// e.g. `routes = ["/admin/*"]`, `strip_prefix = "/admin"` sends
    /// `/admin/users` upstream as `/users`.
    #[serde(default)]
    pub strip_prefix: Option<String>,
    /// Image pull policy: auto (default), always, never, if-not-present.
    #[serde(default)]
    pub pull_policy: PullPolicy,
    /// Per-service backup settings (pre-hook command, enable/disable).
    #[serde(default)]
    pub backup: Option<ServiceBackupConfig>,
    /// Docker restart policy: "no" (default), "always", "unless-stopped",
    /// or "on-failure[:max-retries]" (e.g. "on-failure:5"). Lets Docker
    /// itself revive a container that crashed on a transient (DNS race at
    /// boot, brief dependency outage) instead of it staying dead until the
    /// next manual redeploy (#121, motivated by the #120 incident).
    #[serde(default)]
    pub restart_policy: Option<String>,
}

impl ServiceConfig {
    /// All hostnames this service should be served on, normalizing the single
    /// `domain` and multi `domains` forms into one list. `domains` wins when
    /// set; otherwise the single `domain` (if any) is returned as a 1-element
    /// list. Empty when neither is set.
    pub fn all_domains(&self) -> Vec<String> {
        if !self.domains.is_empty() {
            self.domains.clone()
        } else {
            self.domain.iter().cloned().collect()
        }
    }

    /// The primary hostname — the first of `all_domains()`. Used where a single
    /// representative domain is wanted (e.g. the `orca.domain` label, a status
    /// summary column).
    pub fn primary_domain(&self) -> Option<String> {
        self.all_domains().into_iter().next()
    }

    /// Validate cross-field invariants. Currently rejects setting both `domain`
    /// and `domains`, which would be ambiguous.
    pub fn validate(&self) -> Result<(), String> {
        if self.domain.is_some() && !self.domains.is_empty() {
            return Err(format!(
                "service '{}': set either `domain` or `domains`, not both",
                self.name
            ));
        }
        if let Some(p) = &self.restart_policy
            && !matches!(p.as_str(), "no" | "always" | "unless-stopped")
            && !(p.strip_prefix("on-failure").is_some_and(|rest| {
                rest.is_empty()
                    || rest
                        .strip_prefix(':')
                        .is_some_and(|n| n.parse::<u32>().is_ok())
            }))
        {
            return Err(format!(
                "service '{}': invalid restart_policy {:?} — use \"no\", \"always\", \
                 \"unless-stopped\", or \"on-failure[:max-retries]\"",
                self.name, p
            ));
        }
        // #89 (network == name): NOT a hard error — prod runs five such
        // services healthily (incl. one agent-pinned), so the collision
        // alone doesn't break registration; the reported failure needs an
        // unidentified co-factor. `load_dir` logs a warning as a debugging
        // breadcrumb instead.
        Ok(())
    }

    /// Returns `true` if the deployment-relevant fields of two configs match.
    ///
    /// Used by the reconciler to decide whether a running service needs to
    /// be recreated. Fields like `name`, `project`, `replicas` are handled
    /// separately by the reconciler and are NOT compared here.
    pub fn spec_matches(&self, other: &Self) -> bool {
        self.image == other.image
            && self.module == other.module
            && self.env == other.env
            && self.cmd == other.cmd
            && self.port == other.port
            && self.host_port == other.host_port
            && self.all_domains() == other.all_domains()
            && self.routes == other.routes
            && self.volume == other.volume
            && self.mounts == other.mounts
            && self.aliases == other.aliases
            && self.extra_ports == other.extra_ports
            && self.strip_prefix == other.strip_prefix
            && self.network == other.network
            && self.restart_policy == other.restart_policy
            && self.internal == other.internal
            && self.health == other.health
    }
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
