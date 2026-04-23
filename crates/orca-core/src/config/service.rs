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
    pub domain: Option<String>,
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
}

impl ServiceConfig {
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
            && self.domain == other.domain
            && self.routes == other.routes
            && self.volume == other.volume
            && self.mounts == other.mounts
            && self.aliases == other.aliases
            && self.extra_ports == other.extra_ports
            && self.strip_prefix == other.strip_prefix
            && self.network == other.network
            && self.internal == other.internal
            && self.health == other.health
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config() -> ServiceConfig {
        ServiceConfig {
            name: "test-svc".into(),
            project: None,
            runtime: RuntimeKind::Container,
            image: Some("nginx:latest".into()),
            module: None,
            replicas: Replicas::Fixed(1),
            port: Some(80),
            host_port: None,
            domain: Some("test.example.com".into()),
            routes: vec!["/*".into()],
            health: Some("/healthz".into()),
            readiness: None,
            liveness: None,
            env: HashMap::from([("KEY".into(), "val".into())]),
            resources: None,
            volume: None,
            deploy: None,
            placement: None,
            network: Some("web".into()),
            aliases: vec!["test".into()],
            mounts: vec!["/host:/container".into()],
            triggers: vec![],
            assets: None,
            build: None,
            tls_cert: None,
            tls_key: None,
            internal: false,
            depends_on: vec![],
            cmd: vec![],
            extra_ports: vec!["8080:80".into()],
            strip_prefix: Some("/api".into()),
            pull_policy: Default::default(),
            backup: None,
        }
    }

    #[test]
    fn identical_configs_match() {
        let a = base_config();
        let b = base_config();
        assert!(a.spec_matches(&b));
    }

    #[test]
    fn image_change_detected() {
        let a = base_config();
        let mut b = base_config();
        b.image = Some("nginx:1.27".into());
        assert!(!a.spec_matches(&b));
    }

    #[test]
    fn env_change_detected() {
        let a = base_config();
        let mut b = base_config();
        b.env.insert("NEW_KEY".into(), "new_val".into());
        assert!(!a.spec_matches(&b));
    }

    #[test]
    fn extra_ports_change_detected() {
        let a = base_config();
        let mut b = base_config();
        b.extra_ports = vec!["9090:90".into()];
        assert!(!a.spec_matches(&b));
    }

    #[test]
    fn mounts_change_detected() {
        let a = base_config();
        let mut b = base_config();
        b.mounts.push("/extra:/path".into());
        assert!(!a.spec_matches(&b));
    }

    #[test]
    fn volume_change_detected() {
        let a = base_config();
        let mut b = base_config();
        b.volume = Some(crate::types::VolumeSpec {
            path: "/data".into(),
            size: None,
        });
        assert!(!a.spec_matches(&b));
    }

    #[test]
    fn domain_change_detected() {
        let a = base_config();
        let mut b = base_config();
        b.domain = Some("new.example.com".into());
        assert!(!a.spec_matches(&b));
    }

    #[test]
    fn aliases_change_detected() {
        let a = base_config();
        let mut b = base_config();
        b.aliases.push("new-alias".into());
        assert!(!a.spec_matches(&b));
    }

    #[test]
    fn strip_prefix_change_detected() {
        let a = base_config();
        let mut b = base_config();
        b.strip_prefix = None;
        assert!(!a.spec_matches(&b));
    }

    #[test]
    fn network_change_detected() {
        let a = base_config();
        let mut b = base_config();
        b.network = Some("internal".into());
        assert!(!a.spec_matches(&b));
    }

    #[test]
    fn internal_flag_change_detected() {
        let a = base_config();
        let mut b = base_config();
        b.internal = true;
        assert!(!a.spec_matches(&b));
    }

    #[test]
    fn port_change_detected() {
        let a = base_config();
        let mut b = base_config();
        b.port = Some(8080);
        assert!(!a.spec_matches(&b));
    }

    #[test]
    fn cmd_change_detected() {
        let a = base_config();
        let mut b = base_config();
        b.cmd = vec!["--debug".into()];
        assert!(!a.spec_matches(&b));
    }

    #[test]
    fn non_spec_fields_ignored() {
        let a = base_config();
        let mut b = base_config();
        // These changes should NOT trigger a recreate
        b.name = "different-name".into();
        b.project = Some("other-project".into());
        b.replicas = Replicas::Fixed(5);
        assert!(a.spec_matches(&b));
    }

    #[test]
    fn unresolved_secret_templates_match() {
        let mut a = base_config();
        let mut b = base_config();
        // Both have the same unresolved template — should match
        a.env.insert("TOKEN".into(), "${secrets.MY_TOKEN}".into());
        b.env.insert("TOKEN".into(), "${secrets.MY_TOKEN}".into());
        assert!(a.spec_matches(&b));
    }

    #[test]
    fn resolved_vs_unresolved_differs() {
        let mut a = base_config();
        let mut b = base_config();
        // a has the template, b has a resolved value — should NOT match
        a.env.insert("TOKEN".into(), "${secrets.MY_TOKEN}".into());
        b.env
            .insert("TOKEN".into(), "actual-secret-value-123".into());
        assert!(!a.spec_matches(&b));
    }
}
