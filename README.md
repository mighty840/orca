<p align="center">
  <img src="assets/logo.svg" alt="Orca" width="180" height="180">
</p>

<h1 align="center">Orca</h1>

<p align="center">
  <strong>Container + Wasm orchestrator with AI ops</strong><br>
  <em>Fills the gap between Coolify and Kubernetes.</em>
</p>

<p align="center">
  <a href="https://github.com/mighty840/orca/actions/workflows/ci.yml"><img src="https://github.com/mighty840/orca/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://crates.io/crates/mallorca"><img src="https://img.shields.io/crates/v/mallorca.svg" alt="crates.io"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-AGPL--3.0-blue.svg" alt="License"></a>
  <img src="https://img.shields.io/badge/rust-2024_edition-orange.svg" alt="Rust">
  <img src="https://img.shields.io/badge/tests-63_passing-brightgreen.svg" alt="Tests">
</p>

<p align="center">
  <a href="#install">Install</a> &bull;
  <a href="#quick-start">Quick Start</a> &bull;
  <a href="#features">Features</a> &bull;
  <a href="#cli-reference">CLI</a> &bull;
  <a href="#configuration">Config</a> &bull;
  <a href="#architecture">Architecture</a> &bull;
  <a href="CONTRIBUTING.md">Contributing</a>
</p>

---

Orca is a **single-binary orchestrator** for teams that have outgrown one server but don't need Kubernetes. It runs containers and WebAssembly modules as first-class workloads, with built-in reverse proxy, auto-TLS, secrets management, health checks, and an AI operations assistant.

```
Docker Compose ──> Coolify ──> Orca ──> Kubernetes
   (1 node)        (1 node)   (2-20)     (20-10k)
```

## Install

```bash
cargo install mallorca
```

This installs the `orca` binary. Requires `protoc` (for gRPC codegen):
```bash
# Ubuntu/Debian
sudo apt install protobuf-compiler build-essential pkg-config libssl-dev

# Fedora
sudo dnf install protobuf-compiler gcc pkg-config openssl-devel
```

## Quick Start

```bash
# 1. Create configs
cat > cluster.toml << 'EOF'
[cluster]
name = "my-cluster"
domain = "example.com"
EOF

cat > services.toml << 'EOF'
[[service]]
name = "web"
image = "nginx:alpine"
replicas = 2
port = 80
domain = "example.com"
health = "/"
EOF

# 2. Start the server
orca server --proxy-port 8080 &

# 3. Deploy and manage
orca deploy
orca status
orca logs web
orca tui            # terminal dashboard
```

### One-Click Database

```bash
orca db create postgres mydb
# --> Deploys postgres:16 with auto-generated password, volume, health check
# --> Stores password as secret, prints connection string
```

## Features

### Dual Runtime
| | Containers | WebAssembly |
|---|---|---|
| **Backend** | Docker (bollard) | wasmtime (WASI P2) |
| **Cold start** | ~3s | ~5ms |
| **Memory** | 30MB+ | 1-5MB |
| **GPU** | nvidia passthrough | N/A |
| **Use case** | Existing images, databases | Edge functions, API handlers |

### Multi-Node Clustering
- **Raft consensus** for HA control plane (no etcd dependency)
- **Bin-packing scheduler** with GPU awareness and Wasm preference
- **Node join/leave** with heartbeat protocol
- **Cross-provider networking** via NetBird WireGuard mesh

### Production Operations
- **Health checks** — HTTP probing, auto-restart after 3 failures
- **Auto-TLS** — ACME/Let's Encrypt via certbot, self-signed, or custom certs
- **Secrets** — encrypted storage, `${secrets.KEY}` resolution, `.env` import
- **Webhooks** — GitHub/Gitea push triggers auto-redeploy with HMAC-SHA256
- **Backups** — volume tar.gz with pre-hooks, local disk + S3 targets
- **Rollback** — deploy history, one-command rollback to previous config
- **API auth** — bearer token middleware on all endpoints
- **Docker cleanup** — prune unused images, containers, volumes

### AI Ops
- `orca ask "why is the API slow?"` — diagnoses issues using cluster context
- **Conversational alerts** — AI investigates, suggests fixes, tracks resolution
- **GPU monitoring** — thermal and VRAM utilization tracking

### Developer Experience
- **Single binary** — `scp` to a server and run
- **TOML config** — not YAML, fits on one screen
- **Config as code** — version control your infrastructure
- **TUI dashboard** — k9s-style terminal UI with search, detail view, keybindings
- **63 tests** — unit, integration, E2E framework

## CLI Reference

```
CLUSTER
  orca server               Start control plane + agent + proxy
  orca join <leader>         Join this node to a cluster
  orca nodes                 List cluster nodes
  orca tui                   Launch terminal dashboard

SERVICES
  orca deploy                Deploy services from services.toml
  orca status                Show service status
  orca logs <service>        Stream service logs
  orca scale <service> N     Scale to N replicas
  orca stop <service>        Stop a service
  orca rollback <service>    Rollback to previous deploy

DATABASES
  orca db create TYPE NAME   Create postgres/mysql/redis/mongodb
  orca db list               List database services

SECRETS
  orca secrets set K V       Store a secret
  orca secrets list          List secret keys
  orca secrets import -f .env  Import from .env file

OPS
  orca backup create         Backup configs and volumes
  orca backup list           List existing backups
  orca cleanup               Prune unused Docker resources
  orca ask "question"        Ask the AI assistant
```

## Configuration

<details>
<summary><strong>cluster.toml</strong> (click to expand)</summary>

```toml
[cluster]
name = "production"
domain = "myapp.com"
acme_email = "ops@myapp.com"
api_tokens = ["${secrets.api_token}"]

[[node]]
address = "10.0.0.1"
labels = { role = "general" }

[[node]]
address = "10.0.0.2"
labels = { role = "gpu" }

[ai]
provider = "ollama"
model = "qwen3:30b"

[backup]
retention_days = 30
[[backup.targets]]
type = "local"
path = "/var/backups/orca"
[[backup.targets]]
type = "s3"
bucket = "my-backups"
region = "eu-central-1"
```
</details>

<details>
<summary><strong>services.toml</strong> (click to expand)</summary>

```toml
# Container service
[[service]]
name = "api"
image = "myapp:latest"
replicas = 3
port = 8080
domain = "api.myapp.com"
health = "/healthz"
[service.env]
DATABASE_URL = "${secrets.db_url}"

# Wasm edge function (5ms cold start)
[[service]]
name = "edge-fn"
runtime = "wasm"
module = "./modules/edge.wasm"
triggers = ["http:/api/edge/*"]

# GPU workload
[[service]]
name = "llm"
image = "vllm/vllm-openai:latest"
port = 8000
[service.resources]
memory = "32Gi"
cpu = 8.0
[service.resources.gpu]
count = 1
vendor = "nvidia"
vram_min = 24000
```
</details>

## Architecture

```
┌─────────────────────────────────────┐
│         CLI / TUI / API             │
└──────────────┬──────────────────────┘
               │
┌──────────────▼──────────────────────┐
│         Control Plane               │
│  Raft consensus (openraft + redb)   │
│  Scheduler (bin-packing + GPU)      │
│  API server (axum)                  │
│  Health checker + AI monitor        │
└──────────────┬──────────────────────┘
               │
    ┌──────────┼──────────┐
    ▼          ▼          ▼
┌────────┐ ┌────────┐ ┌────────┐
│ Node 1 │ │ Node 2 │ │ Node 3 │
│ Docker │ │ Docker │ │ Docker │
│ Wasm   │ │ Wasm   │ │ Wasm   │
│ Proxy  │ │ Proxy  │ │ Proxy  │
└────────┘ └────────┘ └────────┘
```

**8 Rust crates** | **~12k lines** | **100+ source files** | **63 tests** | **All files under 250 lines**

## Building from Source

```bash
git clone https://github.com/mighty840/orca.git
cd orca
cargo build --release
```

## Contributing

We welcome contributions! See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

Key areas:
- Nixpacks integration for auto-detect builds
- Service templates (WordPress, Supabase, etc.)
- Preview environments (PR-based deploys)
- ACME cert renewal automation

## License

AGPL-3.0. See [LICENSE](LICENSE).
