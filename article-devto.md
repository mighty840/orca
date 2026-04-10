---
title: "I Built a Container Orchestrator in Rust Because Kubernetes Was Too Much and Coolify Wasn't Enough"
published: false
description: "Orca is a single-binary container orchestrator for 2-20 node clusters. Auto-TLS, multi-node Raft consensus, WebSocket streaming, GitOps webhooks, AI ops — all in one 47MB binary. No etcd, no YAML, no PhD required."
tags: rust, devops, containers, opensource
canonical_url: https://github.com/mighty840/orca
cover_image: https://raw.githubusercontent.com/mighty840/orca/main/assets/logo.svg
---

There's a gap in the container orchestration world that nobody talks about.

**Docker Compose** works for 1 server. **Coolify** and **Dokploy** give you a nice GUI but still cap at one node. **Kubernetes** handles 10,000 nodes but requires a team of platform engineers just to keep the lights on.

What if you have **2 to 20 servers**, **20 to 100 services**, and a team of 1 to 5 engineers who'd rather ship features than debug etcd quorum failures?

That's exactly where I was. So I built **Orca**.

```
Docker Compose ──> Coolify/Dokploy ──> Orca ──> Kubernetes
   (1 node)         (1 node, GUI)      (2-20)     (20-10k)
```

## TL;DR

Orca is a **single-binary container + WebAssembly orchestrator** written in Rust. One 47MB executable replaces your control plane, container agent, CLI, reverse proxy with auto-TLS, and terminal dashboard. Deploy with TOML configs that fit on one screen — no YAML empires, no Helm charts, no CRDs.

**GitHub:** [github.com/mighty840/orca](https://github.com/mighty840/orca)
**Install:** `cargo install mallorca`

## The Problem

I was running ~60 services across 3 servers for multiple projects — compliance platforms, trading bots, YouTube automation pipelines, chat servers, AI gateways. Coolify worked great at first, but then I needed:

- Services on **multiple nodes** with DNS-based routing per node
- **Auto-TLS** without manually configuring Caddy/Traefik per domain
- **Git push deploys** that actually work across nodes
- **Rolling updates** that don't take down the whole stack
- Config as code, not clicking through a GUI

Kubernetes was the obvious answer, but for 3 nodes and a solo developer? That's like buying a Boeing 747 to commute to work.

## What Orca Actually Does

### Single Binary, Everything Included

```bash
cargo install mallorca
orca install-service          # systemd unit with auto port binding
sudo systemctl start orca
```

That one binary runs:
- **Control plane** with Raft consensus (openraft + redb — no etcd)
- **Container runtime** via Docker/bollard
- **WebAssembly runtime** via wasmtime (5ms cold start, ~2MB per instance)
- **Reverse proxy** with Host/path routing, WebSocket proxying, rate limiting
- **ACME client** for automatic Let's Encrypt certificates
- **Secrets store** with AES-256 encryption at rest
- **Health checker** with liveness/readiness probes and auto-restart
- **AI assistant** that diagnoses cluster issues in natural language

### TOML Config That Humans Can Read

```toml
[[service]]
name = "api"
image = "myorg/api:latest"
port = 8080
domain = "api.example.com"
health = "/healthz"

[service.env]
DATABASE_URL = "${secrets.DB_URL}"
REDIS_URL = "redis://cache:6379"

[service.resources]
memory = "512Mi"
cpu = 1.0

[service.liveness]
path = "/healthz"
interval_secs = 30
failure_threshold = 3
```

Compare that to the equivalent Kubernetes YAML. I'll wait.

### Multi-Node in One Command

```bash
# On worker nodes:
orca install-service --leader 10.0.0.1:6880
sudo systemctl start orca-agent
```

The agent connects to the master via **bidirectional WebSocket** — no HTTP polling, no gRPC complexity. Deploy commands arrive instantly. When an agent reconnects after a network blip, the master sends the full desired state and the agent self-heals.

```toml
[service.placement]
node = "gpu-box"         # Pin to a specific node
```

### GitOps Without a CI Runner

Orca has a built-in **infra webhook**. Point your git host at the orca API, and every push triggers `git pull` + full reconciliation:

```bash
# One-time setup:
curl -X POST http://localhost:6880/api/v1/webhooks \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"repo":"myorg/infra","service_name":"infra","branch":"main",
       "secret":"...","infra":true}'
```

Push a config change → orca pulls → deploys only what changed. No Jenkins, no GitHub Actions runner, no ArgoCD.

For image-only updates (CI pushes new `:latest`), register a per-service webhook and orca force-pulls + restarts.

## How It Compares

| Feature | Coolify | Dokploy | Orca | K8s |
|---------|---------|---------|------|-----|
| Multi-node | No | No | Yes (Raft) | Yes (etcd) |
| Config format | GUI | GUI | TOML | YAML |
| Auto-TLS | Yes | Yes | Yes (ACME) | cert-manager |
| Secrets | GUI | GUI | AES-256, `${secrets.X}` | etcd + RBAC |
| Rolling updates | Basic | Basic | Yes + canary | Yes |
| Health checks | Basic | Basic | Liveness + readiness | Yes |
| WebSocket proxy | Partial | Partial | Full | Ingress-dependent |
| Wasm support | No | No | Yes (wasmtime) | Krustlet (dead) |
| AI ops | No | No | Yes | No |
| GitOps webhook | Yes | Yes | Yes + infra webhook | ArgoCD/Flux |
| Self-update | No | Docker pull | `orca update` | Cluster upgrade |
| Lines of config per service | ~0 (GUI) | ~0 (GUI) | ~10 TOML | ~50-100 YAML |
| External dependencies | Docker, DB | Docker | Docker only | etcd, CoreDNS, ... |
| Binary size | Docker image | Docker image | 47MB | N/A |

## The Smart Reconciler

One thing that drove me crazy with other orchestrators: redeploy a stack and *everything* restarts, even services that haven't changed.

Orca's reconciler compares the **unresolved config templates** (with `${secrets.X}` intact), not the resolved values. If your OAuth token refreshed but your config didn't change, the container stays running. Only actual config changes trigger a rolling update.

```bash
orca deploy              # Reconcile all — skips unchanged services
orca deploy api          # Reconcile just one service
orca redeploy api        # Force pull image + restart (for :latest updates)
```

## What's Coming in v0.3

The roadmap is driven by what we actually need in production:

- **Remote log streaming** — `orca logs <service>` for containers on any node, piped via WebSocket
- **Preview environments** — `orca env create pr-123` spins up an ephemeral copy of a project
- **Per-project secrets** — `${secrets.X}` resolves project scope first, then global
- **TUI webhook manager** — add/edit/delete webhooks from the terminal dashboard
- **TUI backup dashboard** — per-node backup status, manual trigger, restore
- **ARM64 builds** — native binaries for Raspberry Pi / Graviton
- **Log forwarding** — ship container logs to Loki, SigNoz, or any OpenTelemetry collector
- **Nixpacks integration** — auto-detect and build without Dockerfiles

## Architecture for the Curious

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
               │ WebSocket
    ┌──────────┼──────────┐
    ▼          ▼          ▼
┌────────┐ ┌────────┐ ┌────────┐
│ Node 1 │ │ Node 2 │ │ Node 3 │
│ Docker │ │ Docker │ │ Docker │
│ Wasm   │ │ Wasm   │ │ Wasm   │
│ Proxy  │ │ Proxy  │ │ Proxy  │
└────────┘ └────────┘ └────────┘
```

8 Rust crates, ~15k lines, 120+ tests. Every source file under 250 lines. The dependency flow is strict: `core` <- `agent` <- `control` <- `cli`. No circular deps, no god modules.

## Want to Contribute?

Orca is open source (AGPL-3.0) and actively looking for contributors. The codebase is designed to be approachable:

- **Small files** — 250 line max, split into clear submodules
- **Comprehensive tests** — 120+ unit and integration tests
- **Architecture guide** — [CLAUDE.md](https://github.com/mighty840/orca/blob/main/CLAUDE.md) documents every crate, convention, and design decision
- **Real issues** — every open issue comes from production usage, not hypotheticals

Good first issues:

1. **ARM64 CI build** — add a GitHub Actions matrix for aarch64
2. **TUI log viewer** — stream container logs in a ratatui pane
3. **Backup `--exclude`** — skip specific volumes from nightly backup
4. **Service templates** — WordPress, Supabase, n8n one-click configs

```bash
git clone https://github.com/mighty840/orca.git
cd orca
cargo test        # 120+ tests
cargo build       # single binary
```

## Links

- **GitHub:** [github.com/mighty840/orca](https://github.com/mighty840/orca)
- **Docs:** [mighty840.github.io/orca](https://mighty840.github.io/orca)
- **crates.io:** [crates.io/crates/mallorca](https://crates.io/crates/mallorca)
- **Changelog:** [CHANGELOG.md](https://github.com/mighty840/orca/blob/main/CHANGELOG.md)

---

*Orca is built by developers running real production workloads on it — trading bots, compliance platforms, YouTube automation, AI gateways. Every feature exists because we needed it, every bug fix comes from a real 3 AM incident. If you're stuck between Coolify and Kubernetes, give it a shot.*

*Star the repo if this resonates. Open an issue if something's broken. PRs welcome.*
