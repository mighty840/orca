# Orca

**Container + Wasm orchestrator with AI ops — fills the gap between Coolify and Kubernetes.**

[![CI](https://github.com/mighty840/orca/actions/workflows/ci.yml/badge.svg)](https://github.com/mighty840/orca/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/mallorca.svg)](https://crates.io/crates/mallorca)
[![License: AGPL-3.0](https://img.shields.io/badge/license-AGPL--3.0-blue.svg)](LICENSE)

Orca is a single-binary orchestrator for teams that have outgrown one server but don't need Kubernetes. It runs **containers and WebAssembly modules** as first-class workloads, with built-in reverse proxy, secrets management, health checks, and an AI operations assistant.

```
Docker Compose --> Coolify --> Orca --> Kubernetes
  (1 node)       (1 node)   (2-20)    (20-10k)
```

## Install

```bash
cargo install mallorca
```

This installs the `orca` binary.

## Quick Start

```bash
# Create a cluster config
cat > cluster.toml << 'EOF'
[cluster]
name = "my-cluster"
domain = "example.com"
EOF

# Create a service config
cat > services.toml << 'EOF'
[[service]]
name = "web"
image = "nginx:alpine"
replicas = 2
port = 80
domain = "example.com"
health = "/"
EOF

# Start the server
orca server --proxy-port 8080 &

# Deploy
orca deploy
orca status
orca logs web
orca tui
```

## Features

### Dual Runtime
- **Containers** via Docker with GPU passthrough support
- **WebAssembly** via wasmtime with WASI Preview 2 (sub-5ms cold start)

### Multi-Node Clustering
- Raft consensus for HA (no etcd dependency)
- Bin-packing scheduler with GPU awareness and Wasm preference
- Node join/leave with heartbeat protocol

### Operations
- **Health checks** with automatic restart after 3 failures
- **Secrets management** with encrypted storage and `${secrets.KEY}` resolution
- **Webhook deploy** from GitHub/Gitea with HMAC-SHA256 validation
- **Backups** with pre-hooks (pg_dump), local disk and S3 targets
- **Rollback** to any previous deploy
- **TUI dashboard** for terminal-based cluster management

### AI Ops
- `orca ask "why is the API slow?"` — AI diagnoses issues using cluster context
- Conversational alerts that investigate, suggest fixes, and track resolution
- GPU thermal and VRAM monitoring

### Developer Experience
- Single static binary — `scp` it to a server and run
- TOML config (not YAML) — fits on one screen
- One-click databases — `orca db create postgres mydb`
- Config as code — version control your infrastructure

## CLI Reference

```
orca server               Start control plane + agent + proxy
orca deploy               Deploy services from services.toml
orca status               Show cluster and service status
orca logs <service>       Stream service logs
orca scale <service> N    Scale to N replicas
orca stop <service>       Stop a service
orca rollback <service>   Rollback to previous deploy
orca tui                  Launch terminal dashboard
orca join <leader>        Join this node to a cluster
orca nodes                List cluster nodes
orca secrets set K V      Store a secret
orca secrets list         List secret keys
orca db create TYPE NAME  Create a database service
orca backup create        Create a backup
orca cleanup              Prune unused Docker resources
orca ask "question"       Ask the AI assistant
```

## Configuration

### cluster.toml
```toml
[cluster]
name = "production"
domain = "myapp.com"
acme_email = "ops@myapp.com"

# API authentication
api_tokens = ["${secrets.api_token}"]

# Multi-node
[[node]]
address = "10.0.0.1"
labels = { role = "general" }

[[node]]
address = "10.0.0.2"
labels = { role = "gpu" }

# AI operations assistant
[ai]
provider = "ollama"
model = "qwen3:30b"

# Backups
[backup]
retention_days = 30
[[backup.targets]]
type = "local"
path = "/var/backups/orca"
```

### services.toml
```toml
# Container workload
[[service]]
name = "api"
image = "myapp:latest"
replicas = 3
port = 8080
domain = "api.myapp.com"
health = "/healthz"

[service.env]
DATABASE_URL = "${secrets.db_url}"

# Wasm workload (sub-5ms cold start)
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

8 Rust crates, ~10k lines of code, 100 source files, 59 tests.

## Building from Source

```bash
git clone https://github.com/mighty840/orca.git
cd orca
cargo build --release
```

Requires: Rust 2024 edition, `protoc` (for gRPC codegen).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## License

AGPL-3.0. See [LICENSE](LICENSE).
