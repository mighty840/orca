use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// -- Identifiers --

pub type NodeId = Uuid;
pub type WorkloadId = Uuid;
pub type DeploymentId = Uuid;
pub type ConversationId = Uuid;

// -- Runtime --

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeKind {
    #[default]
    Container,
    Wasm,
}

// -- GPU --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuSpec {
    /// Number of GPUs required.
    pub count: u32,
    /// GPU vendor filter (e.g., "nvidia", "amd").
    pub vendor: Option<String>,
    /// Minimum VRAM in MiB.
    pub vram_min: Option<u64>,
    /// Specific GPU model (e.g., "A100", "RTX4090", "H100").
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuInfo {
    /// Device index on this node.
    pub index: u32,
    /// Vendor (nvidia, amd).
    pub vendor: String,
    /// Model name.
    pub model: String,
    /// Total VRAM in bytes.
    pub vram_total: u64,
    /// Used VRAM in bytes.
    pub vram_used: u64,
    /// GPU utilization percentage.
    pub utilization: f64,
    /// Temperature in celsius.
    pub temperature: Option<f64>,
    /// Currently allocated to this workload (if any).
    pub allocated_to: Option<WorkloadId>,
}

// -- Workload --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadSpec {
    pub name: String,
    pub runtime: RuntimeKind,
    /// Container image (runtime = container) or Wasm module path/OCI ref (runtime = wasm)
    pub image: String,
    pub replicas: Replicas,
    pub port: Option<u16>,
    pub domain: Option<String>,
    pub health: Option<String>,
    pub env: std::collections::HashMap<String, String>,
    pub resources: Option<ResourceLimits>,
    pub volume: Option<VolumeSpec>,
    pub deploy: Option<DeployStrategy>,
    pub placement: Option<PlacementConstraint>,
    pub triggers: Vec<Trigger>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Replicas {
    Fixed(u32),
    Auto,
}

impl Default for Replicas {
    fn default() -> Self {
        Self::Fixed(1)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub memory: Option<String>,
    pub cpu: Option<f64>,
    /// GPU requirements. If set, scheduler places workload on GPU-equipped nodes.
    pub gpu: Option<GpuSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeSpec {
    pub path: String,
    pub size: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployStrategy {
    #[serde(default = "default_strategy")]
    pub strategy: DeployKind,
    pub max_unavailable: Option<u32>,
}

fn default_strategy() -> DeployKind {
    DeployKind::Rolling
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeployKind {
    Rolling,
    BlueGreen,
    Canary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlacementConstraint {
    pub labels: Option<std::collections::HashMap<String, String>>,
    pub node: Option<String>,
    /// Require GPU-equipped node.
    pub requires_gpu: Option<bool>,
}

// -- Triggers (Wasm) --

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Trigger {
    Http(String),
    Cron(String),
    Queue(String),
    Event(String),
}

impl TryFrom<String> for Trigger {
    type Error = String;
    fn try_from(s: String) -> std::result::Result<Self, Self::Error> {
        if let Some(route) = s.strip_prefix("http:") {
            Ok(Trigger::Http(route.to_string()))
        } else if let Some(cron) = s.strip_prefix("cron:") {
            Ok(Trigger::Cron(cron.to_string()))
        } else if let Some(topic) = s.strip_prefix("queue:") {
            Ok(Trigger::Queue(topic.to_string()))
        } else if let Some(pattern) = s.strip_prefix("event:") {
            Ok(Trigger::Event(pattern.to_string()))
        } else {
            Err(format!("invalid trigger format: {s}"))
        }
    }
}

impl From<Trigger> for String {
    fn from(t: Trigger) -> Self {
        match t {
            Trigger::Http(r) => format!("http:{r}"),
            Trigger::Cron(c) => format!("cron:{c}"),
            Trigger::Queue(q) => format!("queue:{q}"),
            Trigger::Event(e) => format!("event:{e}"),
        }
    }
}

// -- Node --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub id: NodeId,
    pub address: String,
    pub labels: std::collections::HashMap<String, String>,
    pub status: NodeStatus,
    pub resources: NodeResources,
    pub joined_at: DateTime<Utc>,
    pub last_heartbeat: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeStatus {
    Ready,
    NotReady,
    Draining,
    Left,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeResources {
    pub cpu_cores: f64,
    pub memory_bytes: u64,
    pub cpu_used: f64,
    pub memory_used: u64,
    /// GPUs available on this node.
    pub gpus: Vec<GpuInfo>,
}

// -- Workload Instance --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadInstance {
    pub id: WorkloadId,
    pub spec_name: String,
    pub node_id: NodeId,
    pub runtime: RuntimeKind,
    pub status: WorkloadStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub health: HealthState,
    /// GPU device indices assigned to this workload.
    pub gpu_devices: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkloadStatus {
    Pending,
    Creating,
    Running,
    Stopping,
    Stopped,
    Failed,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthState {
    Unknown,
    Healthy,
    Unhealthy,
    NoCheck,
}

// -- Resource Stats --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceStats {
    pub cpu_percent: f64,
    pub memory_bytes: u64,
    pub network_rx_bytes: u64,
    pub network_tx_bytes: u64,
    pub gpu_stats: Vec<GpuStats>,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuStats {
    pub index: u32,
    pub utilization: f64,
    pub vram_used: u64,
    pub vram_total: u64,
    pub temperature: Option<f64>,
    pub power_watts: Option<f64>,
}

// -- Conversational Alerts --

/// An alert is not a dead report — it's a living conversation between the cluster and the operator.
/// Orca's AI observes the issue, opens a conversation, investigates, suggests fixes,
/// and keeps the thread going until the issue is resolved or acknowledged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertConversation {
    pub id: ConversationId,
    pub service: String,
    pub severity: AlertSeverity,
    pub state: AlertState,
    pub started_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub messages: Vec<AlertMessage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertState {
    /// AI is actively investigating.
    Investigating,
    /// AI has a diagnosis and suggested fix.
    AwaitingAction,
    /// Operator acknowledged, fix in progress.
    Acknowledged,
    /// Auto-remediation was applied.
    Remediated,
    /// Issue resolved (manually or automatically).
    Resolved,
    /// Operator dismissed this alert.
    Dismissed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertMessage {
    pub timestamp: DateTime<Utc>,
    pub sender: AlertSender,
    pub content: String,
    /// If this message proposes a fix, the command to run.
    pub suggested_command: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertSender {
    /// The AI assistant.
    Orca,
    /// The human operator.
    Operator,
    /// The system (automated events).
    System,
}
