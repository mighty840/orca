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
  <a href="https://mighty840.github.io/orca">Documentation</a> &bull;
  <a href="#quick-start">Quick Start</a> &bull;
  <a href="#features">Features</a> &bull;
  <a href="CONTRIBUTING.md">Contributing</a>
</p>

---

Orca is a **single-binary orchestrator** for teams that have outgrown one server but don't need Kubernetes. It runs containers and WebAssembly modules as first-class workloads, with built-in reverse proxy, auto-TLS, secrets management, health checks, and an AI operations assistant. Deploy with TOML configs that fit on one screen — no YAML empires.

```
Docker Compose ──> Coolify ──> Orca ──> Kubernetes
   (1 node)        (1 node)   (2-20)     (20-10k)
```

## Quick Start

```bash
cargo install mallorca
sudo setcap 'cap_net_bind_service=+ep' $(which orca)
orca server
```

Create a service in `services/web/service.toml` and deploy:

```toml
[[service]]
name = "web"
image = "nginx:alpine"
replicas = 2
port = 80
domain = "example.com"
health = "/"
```

```bash
orca deploy && orca status
```

## Features

### Single Binary, Batteries Included

One static executable is the agent, control plane, CLI, and reverse proxy. `scp` it to a server and you have a production-ready orchestrator with auto-TLS, secrets, health checks, and Prometheus metrics.

### Dual Runtime

Run Docker containers and WebAssembly modules side by side. Containers for existing images and databases (~3s cold start). Wasm for edge functions and API handlers (~5ms cold start, ~1-5MB memory).

### Multi-Node Clustering

Raft consensus via `openraft` with embedded `redb` storage — no etcd. Bin-packing scheduler with GPU awareness. Nodes can span multiple cloud providers via NetBird WireGuard mesh.

### Self-Healing

Watchdog restarts crashed containers in ~30s. Health checks with configurable thresholds. Stale route cleanup. Agent reconnection with exponential backoff. Services survive server restarts.

### AI Operations

`orca ask "why is the API slow?"` — diagnoses issues using cluster context. Works with any OpenAI-compatible API (Ollama, LiteLLM, vLLM, OpenAI). Conversational alerts, config generation, and optional auto-remediation.

### Developer Experience

TOML config that fits on one screen. TUI dashboard with k9s-style navigation. Git push deploy via webhooks. One-click database creation. RBAC with admin/deployer/viewer roles.

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
               │ gRPC
    ┌──────────┼──────────┐
    ▼          ▼          ▼
┌────────┐ ┌────────┐ ┌────────┐
│ Node 1 │ │ Node 2 │ │ Node 3 │
│ Docker │ │ Docker │ │ Docker │
│ Wasm   │ │ Wasm   │ │ Wasm   │
│ Proxy  │ │ Proxy  │ │ Proxy  │
└────────┘ └────────┘ └────────┘
```

**8 Rust crates** | **~12k lines** | **63 tests** | **all files under 250 lines**

## Documentation

Full documentation at **[mighty840.github.io/orca](https://mighty840.github.io/orca)**:

- [Getting Started](https://mighty840.github.io/orca/guide/getting-started) — install, first cluster, first deploy
- [Configuration](https://mighty840.github.io/orca/guide/configuration) — cluster.toml and service.toml reference
- [CLI Reference](https://mighty840.github.io/orca/reference/cli) — every command with examples
- [REST API](https://mighty840.github.io/orca/reference/api) — full endpoint reference
- [Architecture](https://mighty840.github.io/orca/architecture) — crate map, runtime trait, design principles

## Contributing

We welcome contributions! See [CONTRIBUTING.md](CONTRIBUTING.md) for setup instructions and guidelines.

Key areas where help is wanted:

- ACME/Let's Encrypt automation
- Nixpacks integration for auto-detect builds
- Service templates (WordPress, Supabase, etc.)
- Preview environments (PR-based deploys)

## License

AGPL-3.0. See [LICENSE](LICENSE).
