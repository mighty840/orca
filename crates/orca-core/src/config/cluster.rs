use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::ai::AiConfig;
use crate::backup::BackupConfig;

/// Top-level cluster configuration (`cluster.toml`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClusterConfig {
    pub cluster: ClusterMeta,
    #[serde(default)]
    pub node: Vec<NodeConfig>,
    #[serde(default)]
    pub observability: Option<ObservabilityConfig>,
    #[serde(default)]
    pub ai: Option<AiConfig>,
    #[serde(default)]
    pub backup: Option<BackupConfig>,
    #[serde(default)]
    pub cleanup: Option<CleanupConfig>,
    /// API bearer tokens for authentication. Empty = allow all requests.
    /// Deprecated: use `[[token]]` entries with roles instead.
    #[serde(default)]
    pub api_tokens: Vec<String>,
    /// Named API tokens with role-based access control.
    #[serde(default)]
    pub token: Vec<ApiToken>,
    /// Mesh networking configuration (NetBird).
    #[serde(default)]
    pub network: Option<NetworkConfig>,
    /// Encrypted-secrets store (#109). When present, secrets come from a
    /// SOPS/age-encrypted file committed to the config repo instead of the
    /// machine-local AES store.
    #[serde(default)]
    pub secrets: Option<SecretsConfig>,
    /// Fallback proxy for unmatched requests (e.g., point to coolify-proxy).
    #[serde(default)]
    pub fallback: Option<FallbackConfig>,
    /// Remote-deploy timeouts (agent ACK over WebSocket).
    #[serde(default)]
    pub deploy: DeployConfig,
    /// Declarative reconciliation: the master watches a config dir and
    /// continuously applies declared services (K8s-style), no manual deploy.
    #[serde(default)]
    pub reconcile: Option<ReconcileConfig>,
    /// Baseline security response headers the proxy injects. Add-if-absent, so
    /// a backend's own headers always win. Unset = on with a safe default set
    /// (HSTS on HTTPS + nosniff + Referrer-Policy); see [`SecurityHeadersConfig`].
    #[serde(default)]
    pub security_headers: Option<SecurityHeadersConfig>,
}

/// Security response headers the proxy adds to every response it returns —
/// **add-if-absent**, so a backend that sets its own `Content-Security-Policy`,
/// `X-Frame-Options`, `Strict-Transport-Security`, etc. is never overridden
/// (pass-through-plus-defaults, as Traefik/Caddy do). HSTS is applied only to
/// HTTPS responses. The defaults are deliberately non-breaking; `X-Frame-Options`
/// and CSP are off by default (they break iframe-embedded apps and most apps
/// respectively) and are opt-in here or per app.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityHeadersConfig {
    /// Master switch. Default true.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// `Strict-Transport-Security` value (HTTPS responses only). Empty disables
    /// HSTS. Default `max-age=31536000` (1y, no `includeSubDomains`/`preload`).
    #[serde(default = "default_hsts")]
    pub hsts: String,
    /// `X-Content-Type-Options` value. Empty disables. Default `nosniff`.
    #[serde(default = "default_nosniff")]
    pub content_type_options: String,
    /// `Referrer-Policy` value. Empty disables. Default
    /// `strict-origin-when-cross-origin`.
    #[serde(default = "default_referrer_policy")]
    pub referrer_policy: String,
    /// `X-Frame-Options` value. Empty (default) = off — leave it to apps, since
    /// `SAMEORIGIN`/`DENY` break services meant to be embedded in an iframe.
    #[serde(default)]
    pub frame_options: String,
    /// `Content-Security-Policy` value. Empty (default) = off — a blanket CSP
    /// breaks most apps; set per app instead.
    #[serde(default)]
    pub csp: String,
    /// Arbitrary extra response headers to add-if-absent (name → value).
    #[serde(default)]
    pub extra: std::collections::HashMap<String, String>,
}

impl Default for SecurityHeadersConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            hsts: default_hsts(),
            content_type_options: default_nosniff(),
            referrer_policy: default_referrer_policy(),
            frame_options: String::new(),
            csp: String::new(),
            extra: std::collections::HashMap::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_hsts() -> String {
    "max-age=31536000".to_string()
}

fn default_nosniff() -> String {
    "nosniff".to_string()
}

fn default_referrer_policy() -> String {
    "strict-origin-when-cross-origin".to_string()
}

/// Declarative-reconciliation configuration. When `config_dir` is set, the
/// master periodically loads service definitions from it and applies any that
/// are new or changed — so adding a service is just dropping its `service.toml`
/// in the dir, no `orca deploy` needed. Unchanged services are left alone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconcileConfig {
    /// Directory (of `<project>/service.toml` files) or a single
    /// `services.toml` the master continuously applies. Unset = disabled.
    pub config_dir: Option<String>,
    /// Seconds between reconcile passes over `config_dir`. Default 30.
    #[serde(default = "default_reconcile_interval")]
    pub interval_secs: u64,
}

fn default_reconcile_interval() -> u64 {
    30
}

/// Remote-deploy timeout configuration. Controls how long the master waits
/// for an agent to (1) acknowledge receipt of a deploy command and (2) report
/// the deploy complete. The two phases are split so a slow first-time image
/// pull (long, expected) is never conflated with an unreachable agent (short,
/// a genuine failure) — see #88 / #94.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployConfig {
    /// Seconds to wait for the agent to acknowledge it received the deploy
    /// command and started work. A miss means the agent is unreachable or the
    /// WS session is dead — fail fast. Default 10s.
    #[serde(default = "default_deploy_ack_timeout")]
    pub ack_timeout_secs: u64,
    /// Seconds to wait for the agent to report the deploy finished — i.e. image
    /// pull, container create, and start all done. Must cover multi-GB first
    /// pulls; the old hard-coded 30s ceiling made such services undeployable.
    /// Default 600s.
    #[serde(default = "default_deploy_completion_timeout")]
    pub completion_timeout_secs: u64,
    /// Whether the master periodically scans connected agents for
    /// `orca.managed=true` containers missing from its registry and adopts
    /// them (#95) — the self-healing complement to the ACK split. Default
    /// true; set false to disable auto-adoption.
    #[serde(default = "default_adopt_orphans")]
    pub adopt_orphans: bool,
    /// Interval in seconds between orphan-adoption scans. Default 30.
    #[serde(default = "default_adopt_interval")]
    pub adopt_interval_secs: u64,
}

impl Default for DeployConfig {
    fn default() -> Self {
        Self {
            ack_timeout_secs: default_deploy_ack_timeout(),
            completion_timeout_secs: default_deploy_completion_timeout(),
            adopt_orphans: default_adopt_orphans(),
            adopt_interval_secs: default_adopt_interval(),
        }
    }
}

fn default_deploy_ack_timeout() -> u64 {
    10
}

fn default_deploy_completion_timeout() -> u64 {
    600
}

fn default_adopt_orphans() -> bool {
    true
}

fn default_adopt_interval() -> u64 {
    30
}

/// Fallback proxy configuration. When orca's route table has no match,
/// requests are forwarded here. Lets orca coexist with another reverse proxy.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FallbackConfig {
    /// HTTP fallback target (e.g., "127.0.0.1:8081").
    pub http: Option<String>,
    /// HTTPS/TLS fallback target for SNI passthrough (e.g., "127.0.0.1:8443").
    pub tls: Option<String>,
}

/// SOPS/age-encrypted secrets store configuration (#109).
///
/// The master decrypts the file in-process at load with a local age
/// identity and re-encrypts on every mutation. Standard `sops`/`age`
/// tooling can always decrypt the file without orca — recovery is
/// "have the repo + the key."
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretsConfig {
    /// SOPS-encrypted JSON secrets file. Relative paths resolve against
    /// the directory containing cluster.toml, so the file lives in the
    /// same git repo the declarative reconciler converges on.
    pub encrypted_file: String,
    /// age identity file (one `AGE-SECRET-KEY-…` per line) used for
    /// in-process decryption. Optional when the `ROPS_AGE` /
    /// `ROPS_AGE_KEY_FILE` environment variables are set instead.
    #[serde(default)]
    pub age_key_file: Option<String>,
    /// age public keys (`age1…`) the file is encrypted to — the master's
    /// key plus the operator's offline key. Required when orca creates
    /// the file; an existing file's recipient set is reused as-is.
    #[serde(default)]
    pub age_recipients: Vec<String>,
    /// Commit and push the encrypted file after each mutation so the
    /// config repo stays the source of truth (best-effort: failures are
    /// logged loudly, never lose the local write). Default: true.
    #[serde(default = "default_secrets_git_autocommit")]
    pub git_autocommit: bool,
}

fn default_secrets_git_autocommit() -> bool {
    true
}

/// Mesh networking configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// Network provider: "netbird" (default).
    #[serde(default = "default_network_provider")]
    pub provider: String,
    /// NetBird setup key for joining the mesh.
    pub setup_key: Option<String>,
    /// NetBird management URL (default: api.netbird.io).
    pub management_url: Option<String>,
}

/// Named API token with role-based access control.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiToken {
    /// Human-readable name (e.g., "sharang", "gitea-ci").
    pub name: String,
    /// Bearer token value.
    pub value: String,
    /// Role: admin, deployer, or viewer.
    #[serde(default = "default_role")]
    pub role: Role,
}

/// Access control role.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Full access: deploy, stop, scale, drain, manage tokens.
    #[default]
    Admin,
    /// Deploy, stop, scale, logs, status. For CI/CD service accounts.
    Deployer,
    /// Read-only: status, logs, metrics. For dashboards.
    Viewer,
}

impl Role {
    /// Check if this role can perform the given action.
    ///
    /// `secrets` is admin-only. Everything else escalates from viewer →
    /// deployer → admin in the obvious way.
    pub fn can(self, action: &str) -> bool {
        match self {
            Role::Admin => true,
            Role::Deployer => matches!(
                action,
                "deploy" | "stop" | "scale" | "rollback" | "status" | "logs" | "cluster_info"
            ),
            Role::Viewer => matches!(action, "status" | "logs" | "cluster_info"),
        }
    }
}

fn default_role() -> Role {
    Role::Admin
}

fn default_network_provider() -> String {
    "netbird".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterMeta {
    #[serde(default = "default_cluster_name")]
    pub name: String,
    pub domain: Option<String>,
    pub acme_email: Option<String>,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default = "default_api_port")]
    pub api_port: u16,
    #[serde(default = "default_grpc_port")]
    pub grpc_port: u16,
}

impl Default for ClusterMeta {
    fn default() -> Self {
        Self {
            name: default_cluster_name(),
            domain: None,
            acme_email: None,
            log_level: default_log_level(),
            api_port: default_api_port(),
            grpc_port: default_grpc_port(),
        }
    }
}

fn default_cluster_name() -> String {
    "orca".into()
}

pub(crate) fn default_log_level() -> String {
    "info".into()
}

pub(crate) fn default_api_port() -> u16 {
    6880
}

pub(crate) fn default_grpc_port() -> u16 {
    6881
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    pub address: String,
    #[serde(default)]
    pub labels: HashMap<String, String>,
    /// GPU devices available on this node.
    #[serde(default)]
    pub gpus: Vec<NodeGpuConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeGpuConfig {
    /// Vendor: "nvidia" or "amd".
    pub vendor: String,
    /// Number of GPUs of this type.
    #[serde(default = "default_gpu_count")]
    pub count: u32,
    /// Model name for scheduling (e.g., "A100", "RTX4090").
    pub model: Option<String>,
}

pub(crate) fn default_gpu_count() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservabilityConfig {
    pub otlp_endpoint: Option<String>,
    pub alerts: Option<AlertChannelConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertChannelConfig {
    pub webhook: Option<String>,
    pub email: Option<String>,
}

/// Scheduled cleanup configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupConfig {
    /// 6-field cron expression for cleanup runs (e.g. "0 0 3 * * *" = 3am daily).
    pub schedule: Option<String>,
    /// Number of most-recent tags to keep per registry repository (default 5).
    #[serde(default = "default_registry_keep_tags")]
    pub registry_keep_tags: u32,
    /// Registry container name (default "orca-registry").
    #[serde(default = "default_registry_container")]
    pub registry_container: String,
}

fn default_registry_keep_tags() -> u32 {
    5
}

fn default_registry_container() -> String {
    "orca-registry".into()
}
