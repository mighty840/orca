# Orca Architecture

> The container + Wasm orchestrator that fills the gap between Coolify and Kubernetes.

## Positioning

```
Docker Compose → Coolify → **Orca** → K8s
  (1 node)      (1 node,   (2-20 nodes,  (20-10k nodes,
                  GUI)      simple config)  YAML empire)
```

Orca targets teams that have outgrown a single server but don't need (or want) Kubernetes.
It runs **containers and WebAssembly modules** as first-class workloads, with a single
binary, typed config, built-in proxy, auto-TLS, and a web UI that feels like Coolify.

## Design Principles

1. **Single binary** — one static executable is the agent, control plane, CLI, and web UI
2. **Config fits on one screen** — if it's longer than 30 lines, the tool failed
3. **Dual runtime** — OCI containers and Wasm modules are both first-class citizens
4. **Batteries included** — proxy, TLS, secrets, logs, metrics, git-push deploy
5. **Migrate, don't rewrite** — import Coolify/docker-compose configs directly
6. **Production from day one** — no "it works in demo" footguns

## Crate Map

```
orca/
├── crates/
│   ├── orca-core/       # Shared types, config parsing, state machine, runtime trait
│   ├── orca-agent/      # Node agent: runs workloads, reports health, manages proxy
│   ├── orca-control/    # Control plane: Raft consensus, scheduler, API server
│   ├── orca-cli/        # CLI binary (thin wrapper over orca-control API)
│   ├── orca-tui/        # Terminal UI (ratatui, sits on top of CLI/API)
│   ├── orca-web/        # Web dashboard (Dioxus fullstack)
│   └── orca-proxy/      # Reverse proxy + TLS + routing (pingora-based)
├── proto/
│   └── orca.proto       # gRPC service definitions (control ↔ agent)
└── docs/
    └── architecture.md  # This file
```

### Binary Modes

The single `orca` binary operates in multiple modes:

```bash
orca server              # Control plane + agent (default for single-node)
orca agent               # Agent only (joins existing cluster)
orca cli / orca deploy   # CLI commands (talks to control plane API)
orca tui                 # Terminal UI
orca web                 # Web dashboard (can also be auto-started by server)
```

## Runtime Abstraction

The core abstraction is the `Runtime` trait. Every workload — container or Wasm — implements it:

```rust
#[async_trait]
pub trait Runtime: Send + Sync + 'static {
    async fn create(&self, spec: &WorkloadSpec) -> Result<WorkloadHandle>;
    async fn start(&self, handle: &WorkloadHandle) -> Result<()>;
    async fn stop(&self, handle: &WorkloadHandle, timeout: Duration) -> Result<()>;
    async fn remove(&self, handle: &WorkloadHandle) -> Result<()>;
    async fn status(&self, handle: &WorkloadHandle) -> Result<WorkloadStatus>;
    async fn logs(&self, handle: &WorkloadHandle, opts: &LogOpts) -> Result<LogStream>;
    async fn exec(&self, handle: &WorkloadHandle, cmd: &[String]) -> Result<ExecResult>;
    async fn stats(&self, handle: &WorkloadHandle) -> Result<ResourceStats>;
}
```

Two implementations:

| | `ContainerRuntime` | `WasmRuntime` |
|---|---|---|
| Backend | bollard (Docker) / youki (OCI) | wasmtime + WASI preview 2 |
| Startup | ~500ms | ~5ms |
| Memory | 30MB+ | 1-5MB |
| Isolation | Linux namespaces + cgroups | Wasm sandbox |
| Networking | Bridge/overlay + iptables | WASI sockets / virtual net |
| Use case | Existing Docker images, databases | Edge functions, API handlers, workers |

### Wasm Component Model

Orca uses the **Wasm Component Model** (not raw Wasm modules) with WASI Preview 2:

```toml
[[service]]
name = "edge-api"
runtime = "wasm"
module = "./modules/api.wasm"      # local path
# OR
module = "oci://ghcr.io/myorg/api-wasm:latest"  # OCI artifact
triggers = ["http:/api/v1/*"]       # HTTP trigger (like Spin)
env = { DATABASE_URL = "${secrets.db_url}" }

[[service]]
name = "cron-job"
runtime = "wasm"
module = "./modules/cron.wasm"
triggers = ["cron:0 */5 * * *"]     # Cron trigger
```

Trigger types:
- `http:<route>` — HTTP request triggers the Wasm component
- `cron:<schedule>` — Time-based invocation
- `queue:<topic>` — Message queue trigger (built-in NATS-like)
- `event:<pattern>` — React to cluster events

This is inspired by Spin/Fermyon but integrated into the orchestrator rather than being separate.

## Cluster Architecture

```
                    ┌─────────────────────────┐
                    │     Web UI (Dioxus)      │
                    │     TUI (ratatui)        │
                    │     CLI                  │
                    └───────────┬─────────────┘
                                │ REST/WebSocket
                    ┌───────────▼─────────────┐
                    │     Control Plane        │
                    │  ┌──────────────────┐   │
                    │  │   API (axum)      │   │
                    │  └──────────────────┘   │
                    │  ┌──────────────────┐   │
                    │  │   Scheduler       │   │
                    │  │   - bin packing   │   │
                    │  │   - affinity      │   │
                    │  │   - wasm-prefer   │   │
                    │  └──────────────────┘   │
                    │  ┌──────────────────┐   │
                    │  │   Raft (openraft) │   │
                    │  │   + redb store    │   │
                    │  └──────────────────┘   │
                    │  ┌──────────────────┐   │
                    │  │   Git receiver    │   │
                    │  │   (webhook +      │   │
                    │  │    built-in repo) │   │
                    │  └──────────────────┘   │
                    └───────────┬─────────────┘
                                │ gRPC (mTLS)
               ┌────────────────┼────────────────┐
               ▼                ▼                ▼
        ┌─────────────┐  ┌─────────────┐  ┌─────────────┐
        │   Node 1    │  │   Node 2    │  │   Node 3    │
        │   Agent     │  │   Agent     │  │   Agent     │
        │ ┌─────────┐ │  │ ┌─────────┐ │  │ ┌─────────┐ │
        │ │Container│ │  │ │Container│ │  │ │Container│ │
        │ │Runtime  │ │  │ │Runtime  │ │  │ │Runtime  │ │
        │ ├─────────┤ │  │ ├─────────┤ │  │ ├─────────┤ │
        │ │Wasm     │ │  │ │Wasm     │ │  │ │Wasm     │ │
        │ │Runtime  │ │  │ │Runtime  │ │  │ │Runtime  │ │
        │ ├─────────┤ │  │ ├─────────┤ │  │ ├─────────┤ │
        │ │Proxy    │ │  │ │Proxy    │ │  │ │Proxy    │ │
        │ │(pingora)│ │  │ │(pingora)│ │  │ │(pingora)│ │
        │ └─────────┘ │  │ └─────────┘ │  │ └─────────┘ │
        └─────────────┘  └─────────────┘  └─────────────┘
```

### Consensus & State

- **Raft** via `openraft` — leader election, log replication
- **redb** as the state store — embedded, zero-config, crash-safe, no etcd dependency
- State is small (cluster config + workload specs + health) — fits in memory, persisted to disk
- Reads served by any node, writes go through Raft leader

### Networking

Each node runs an instance of `orca-proxy` (based on Cloudflare's `pingora`):

- **Ingress routing** — route external traffic to the right workload on the right node
- **Auto-TLS** — ACME (Let's Encrypt) built-in, certs stored in cluster state
- **Service mesh** — inter-service traffic routed via proxy, mTLS between nodes
- **Wasm-aware** — HTTP requests to Wasm workloads go directly to the in-process Wasm runtime (no container networking overhead)

### Cross-Provider Networking (NetBird)

Orca nodes can span multiple cloud providers (Hetzner, AWS, home lab, etc.) by
using [NetBird](https://netbird.io) for WireGuard-based mesh networking. This is
the same pattern used with Coolify today.

```
┌─ Hetzner ──────┐    ┌─ AWS ───────────┐    ┌─ Home Lab ──────┐
│  Node 1        │    │  Node 2         │    │  Node 3          │
│  orca agent    │◄──►│  orca agent     │◄──►│  orca agent      │
│  netbird       │    │  netbird        │    │  netbird          │
└────────────────┘    └─────────────────┘    └──────────────────┘
        ▲                     ▲                       ▲
        └─────── WireGuard encrypted tunnel ──────────┘
                    (via NetBird coordination)
```

Orca bootstraps networking automatically:
1. `orca init` generates or imports a NetBird setup key
2. `orca join` on each node installs and configures NetBird
3. Nodes discover each other via NetBird's management plane
4. All inter-node gRPC and proxy traffic flows over the WireGuard mesh
5. No manual VPN setup, firewall rules, or port forwarding needed

Configuration:
```toml
[cluster]
name = "signalops"

[cluster.network]
provider = "netbird"                    # or "manual" for pre-existing VPN
setup_key = "${secrets.netbird_key}"    # NetBird setup key
management_url = "https://api.netbird.io"  # or self-hosted
```

### Scheduler

The scheduler decides where to place workloads:

```
1. Filter nodes by constraints (memory, CPU, labels, affinity)
2. Score by: available resources, existing image cache, locality
3. Prefer Wasm runtime when workload supports it (faster, lighter)
4. Spread replicas across failure domains (nodes, zones)
```

Wasm workloads can be **colocated** — hundreds of Wasm instances on one node since each
uses ~1-5MB. Container workloads use traditional bin-packing.

## Configuration

### Cluster Config (`cluster.toml`)

```toml
[cluster]
name = "signalops"
domain = "signalops.com"           # base domain for auto-routing
acme_email = "ops@signalops.com"   # Let's Encrypt
log_level = "info"

# Nodes can also auto-discover via mDNS on LAN
[[node]]
address = "10.0.0.1"
labels = { zone = "eu-1", role = "general" }

[[node]]
address = "10.0.0.2"
labels = { zone = "eu-1", role = "general" }

[[node]]
address = "10.0.0.3"
labels = { zone = "eu-2", role = "gpu" }
```

### Service Config (`services.toml`)

```toml
# Container workload — your existing Docker images just work
[[service]]
name = "api"
image = "ghcr.io/signalops/api:latest"
replicas = 3
port = 8080
health = "/healthz"
domain = "api.signalops.com"
env = { DATABASE_URL = "${secrets.db_url}" }
resources = { memory = "256Mi", cpu = 0.5 }

[service.deploy]
strategy = "rolling"       # rolling | blue-green | canary
max_unavailable = 1

# Wasm workload — sub-millisecond cold start
[[service]]
name = "edge-functions"
runtime = "wasm"
module = "oci://ghcr.io/signalops/edge:latest"
triggers = ["http:/api/edge/*"]
replicas = "auto"          # auto-scale Wasm instances (they're cheap)
env = { API_KEY = "${secrets.edge_key}" }

# Database — pinned to a node with volume
[[service]]
name = "postgres"
image = "postgres:16"
replicas = 1
port = 5432
volume = { path = "/var/lib/postgresql/data", size = "10Gi" }
placement = { labels = { zone = "eu-1" } }

# Redis — standard container
[[service]]
name = "redis"
image = "redis:7-alpine"
replicas = 1
port = 6379
volume = { path = "/data", size = "1Gi" }

# Static site — Wasm-served (no nginx needed)
[[service]]
name = "docs"
runtime = "wasm"
module = "builtin:static-server"
assets = "./dist/"
domain = "docs.signalops.com"

# Cron job — Wasm (boots in <5ms, runs, exits)
[[service]]
name = "daily-report"
runtime = "wasm"
module = "./jobs/report.wasm"
triggers = ["cron:0 9 * * *"]
```

### Secrets (`orca secrets set`)

```bash
orca secrets set db_url "postgres://..."
orca secrets set edge_key "sk-..."
# Stored encrypted in Raft state, referenced as ${secrets.name} in config
```

## Coolify Migration Path

Orca can import from Coolify and docker-compose directly:

```bash
# Import from docker-compose
orca import docker-compose ./docker-compose.yml

# Import from Coolify (reads Coolify's SQLite DB)
orca import coolify /data/coolify

# Import generates services.toml — review and deploy
orca deploy
```

The `import coolify` command:
1. Reads Coolify's internal SQLite database
2. Extracts service definitions, env vars, domains, volumes
3. Generates `services.toml` with equivalent Orca config
4. Lists manual migration steps (DNS changes, etc.)

Migration checklist for production:
- [ ] Import config → `orca import coolify`
- [ ] Review generated `services.toml`
- [ ] Set up orca cluster (can start single-node)
- [ ] Migrate secrets → `orca secrets import`
- [ ] Deploy services → `orca deploy`
- [ ] Switch DNS to orca proxy
- [ ] Decommission Coolify

## Web UI (orca-web)

Built with **Dioxus fullstack** — same Rust, compiles to WASM for the browser.

### Pages

```
/                     Dashboard — cluster health, resource usage, alerts
/services             Service list — status, replicas, health, quick actions
/services/:name       Service detail — logs, metrics, config, deploy history
/nodes                Node list — resource usage, workloads, labels
/nodes/:id            Node detail — running workloads, system metrics
/deployments          Deploy history — rollback, diff, audit log
/secrets              Secret management — create, rotate, usage tracking
/proxy                Proxy config — domains, certs, routing rules
/settings             Cluster settings — nodes, ACME, alerts, webhooks
/terminal             Web terminal — run orca CLI commands from the browser
```

### Real-time

- WebSocket connection from UI → API server
- Live updates: logs streaming, health status changes, deploy progress
- No polling — event-driven via the cluster event bus

## TUI (orca-tui)

Built with **ratatui**. Dashboard-style interface:

```
┌─orca ─────────────────────────────────────────────────────────┐
│ Cluster: signalops    Nodes: 3/3 ●    Services: 6/6 ●        │
│ CPU: ████░░░░ 48%     Mem: █████░░░ 62%    Wasm: 12 instances │
├───────────────────────────────────────────────────────────────┤
│ Services                          │ Logs: api                 │
│                                   │                           │
│  ● api          3/3  rolling      │ 12:04:01 GET /health 200  │
│  ● edge-fn      12   auto-scale   │ 12:04:02 POST /v1/… 201  │
│  ● postgres     1/1  stable       │ 12:04:02 GET /v1/… 200   │
│  ● redis        1/1  stable       │ 12:04:03 GET /health 200  │
│  ● docs         1/1  stable       │ 12:04:05 POST /v1/… 201  │
│  ● daily-report cron  next: 09:00 │                           │
│                                   │                           │
├───────────────────────────────────┴───────────────────────────┤
│ [d]eploy  [s]cale  [l]ogs  [r]ollback  [n]odes  [q]uit       │
└───────────────────────────────────────────────────────────────┘
```

Keybindings:
- `j/k` — navigate services
- `Enter` — service detail view
- `l` — toggle log panel
- `d` — trigger deploy
- `s` — scale service
- `/` — search/filter
- `?` — help

## Git-Push Deploy

Two modes:

### 1. Webhook (recommended)
Configure GitHub/Gitea/GitLab webhook → orca API:

```bash
orca webhooks add \
  --repo github.com/signalops/api \
  --service api \
  --branch main \
  --secret ${secrets.webhook_secret}
```

On push: orca receives webhook → builds image (if Dockerfile present or uses buildpacks)
→ updates service image → rolling deploy.

### 2. Built-in Git Receiver
Orca can act as a git remote (like Dokku/Heroku):

```bash
git remote add orca orca@signalops.com:api.git
git push orca main
```

Orca receives the push → detects language → builds via buildpacks or Dockerfile →
deploys. For Wasm workloads, it compiles to `.wasm` if a Rust/Go/etc. source is pushed.

## Observability

### Built-in

- **Logs** — aggregated from all nodes, queryable via CLI/TUI/UI
- **Metrics** — OpenTelemetry-native, exposes Prometheus endpoint
- **Health** — per-service health checks, cluster-wide health dashboard
- **Events** — deploy, scale, crash, restart — all logged with audit trail

### External Integration

```toml
[observability]
# Push metrics to existing Prometheus/SigNoz
otlp_endpoint = "https://signoz.meghsakha.com"

# Send alerts
[observability.alerts]
webhook = "https://hooks.slack.com/..."
email = "ops@signalops.com"
```

## Milestone Plan

### M0: Foundation (weeks 1-3)
- [ ] `orca-core`: Config parsing, types, Runtime trait
- [ ] `orca-agent`: Single-node container runtime (bollard)
- [ ] `orca-cli`: `orca deploy`, `orca status`, `orca logs`, `orca stop`, `orca top`
- [ ] `orca-proxy`: Basic reverse proxy with auto-TLS
- **Goal**: Replace docker-compose for local/single-server use

### M1: Wasm Runtime (weeks 4-5)
- [ ] `WasmRuntime` implementation (wasmtime + WASI P2)
- [ ] HTTP trigger for Wasm workloads
- [ ] `module = "oci://..."` support (OCI artifact pulling)
- **Goal**: Run Wasm workloads alongside containers

### M2: Multi-node (weeks 6-8)
- [ ] `orca-control`: Raft consensus (openraft)
- [ ] Node join/leave protocol
- [ ] Scheduler (bin-packing + Wasm-aware + GPU-aware)
- [ ] gRPC agent ↔ control plane communication
- [ ] NetBird integration for cross-provider WireGuard mesh networking
- **Goal**: Orchestrate across 2-5 nodes, even across different cloud providers

### M3: UI Layer (weeks 9-11)
- [ ] `orca-tui`: Dashboard, service list, log viewer
- [ ] `orca-web`: Dioxus fullstack dashboard
- [ ] WebSocket live updates
- **Goal**: Visual management on par with Coolify

### M4: Production Ready (weeks 12-14)
- [ ] `orca import coolify` migration tool
- [ ] `orca import docker-compose`
- [ ] Secrets management (Infisical-style: encrypted, versioned, env-scoped, auditable)
- [ ] `orca secrets set/get/list/history/rollback` with per-environment scoping
- [ ] `orca secrets import` from .env, Infisical, Coolify
- [ ] Git-push deploy (webhook + built-in receiver)
- [ ] Health checks + auto-restart + rollback
- **Goal**: Migrate real workloads from Coolify

### M5: Polish (weeks 15-16)
- [ ] Auto-scaling for Wasm workloads
- [ ] Cron + queue triggers
- [ ] Buildpack support (auto-detect language, build image)
- [ ] Documentation + examples
- **Goal**: Public release

## Key Dependencies

| Crate | Purpose | Why this one |
|---|---|---|
| `openraft` | Raft consensus | Pure Rust, async, well-maintained |
| `redb` | Embedded KV store | Zero-config, ACID, no C deps |
| `axum` | HTTP API | Tokio ecosystem, tower middleware |
| `tonic` | gRPC | De facto Rust gRPC, works with axum |
| `bollard` | Docker API client | Mature, async, full API coverage |
| `wasmtime` | Wasm runtime | Bytecode Alliance, WASI P2 support |
| `pingora` | Reverse proxy | Cloudflare-proven, TLS + H2 + H3 |
| `ratatui` | TUI framework | Active community, flexible |
| `dioxus` | Web UI | Fullstack Rust, familiar to team |
| `rcgen` + `rustls` | TLS/certs | Pure Rust TLS, ACME client |
| `serde` + `toml` | Config | Standard Rust serialization |
| `tracing` | Logging/telemetry | OpenTelemetry integration |
| `clap` | CLI parsing | Derive API, completions |
