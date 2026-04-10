---
title: "Orca: The Container Orchestrator Between Coolify and Kubernetes"
published: false
tags: rust, devops, containers, opensource
---

There's a gap in the container orchestration world.

If you're running 1-3 services, Docker Compose or Coolify works great. If you're at 50+ nodes, Kubernetes is the answer. But what about the middle ground — 2 to 20 nodes, 20 to 100 services, a small team that doesn't want to maintain a K8s cluster?

That's where **Orca** lives.

```
Docker Compose ──> Coolify ──> Orca ──> Kubernetes
   (1 node)        (1 node)   (2-20)     (20-10k)
```

## What is Orca?

Orca is a **single-binary container orchestrator** written in Rust. One 47MB executable is the control plane, agent, CLI, reverse proxy, and TUI dashboard. `scp` it to a server and you have a production-ready orchestrator with:

- **Auto-TLS** via Let's Encrypt (ACME HTTP-01)
- **Multi-node clustering** with Raft consensus (no etcd)
- **Reverse proxy** with Host/path routing, WebSocket support
- **Secrets management** (encrypted at rest, `${secrets.X}` in configs)
- **Health checks** with configurable liveness/readiness probes
- **Rolling updates** and canary deploys
- **Git push deploy** via webhooks
- **AI operations** — `orca ask "why is the API returning 503s?"`
- **WebAssembly support** — run Wasm modules alongside containers

## Why Not Just Use [X]?

| | Coolify | Orca | Kubernetes |
|---|---------|------|------------|
| Nodes | 1 | 2-20 | 20-10,000 |
| Config | GUI | TOML | YAML |
| Binary | Docker image | Single binary | Many components |
| TLS | Built-in | Built-in (ACME) | cert-manager addon |
| Secrets | Built-in | Built-in (AES-256) | etcd + RBAC |
| Learning curve | Low | Low | High |
| Multi-node | No | Yes (Raft) | Yes (etcd) |
| Wasm support | No | Yes (wasmtime) | No (needs Krustlet) |

**Coolify** is excellent for single-server setups but doesn't scale to multiple nodes. **Kubernetes** scales infinitely but brings enormous complexity for small teams. Orca fills the gap with multi-node support, zero external dependencies, and TOML configs that fit on one screen.

## Getting Started in 60 Seconds

```bash
cargo install mallorca   # installs the `orca` binary

# Set up systemd (handles port binding automatically):
orca install-service
sudo systemctl start orca

# Create a service:
mkdir -p services/web
cat > services/web/service.toml << 'EOF'
[[service]]
name = "web"
image = "nginx:alpine"
port = 80
domain = "example.com"
health = "/"
EOF

# Deploy:
orca deploy
```

That's it. Orca provisions a TLS certificate, sets up the reverse proxy, and starts health checking — all from 7 lines of TOML.

## Multi-Node: No etcd, No YAML

Adding a second node is one command:

```bash
# On the worker node:
orca install-service --leader 10.0.0.1:6880
sudo systemctl start orca-agent
```

The agent connects to the master via WebSocket, receives deploy commands in real-time, and runs a local reverse proxy for domains assigned to it. No shared filesystem, no etcd cluster, no certificate rotation headaches.

Pin services to specific nodes:

```toml
[[service]]
name = "gpu-worker"
image = "my-ml-model:latest"
port = 8080

[service.placement]
node = "gpu-box-1"

[service.resources]
memory = "8Gi"
cpu = 4.0
```

## GitOps: Push to Deploy

Orca supports two deployment patterns:

**Pattern 1: Image webhook** — CI builds and pushes `:latest`, webhook triggers redeploy:

```bash
# Register the webhook:
curl -X POST http://localhost:6880/api/v1/webhooks \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"repo":"myorg/api","service_name":"api","branch":"main","secret":"..."}'
```

**Pattern 2: Infra webhook** — push config changes, orca auto-pulls and redeploys:

```bash
# Register as an infra webhook:
curl -X POST http://localhost:6880/api/v1/webhooks \
  -d '{"repo":"myorg/infra","service_name":"infra","branch":"main","secret":"...","infra":true}'
```

With infra webhooks, your entire cluster state lives in git. Push a config change → orca pulls → reconciles all services. No manual `ssh` + `deploy` needed.

## The TUI Dashboard

Orca comes with a terminal dashboard inspired by k9s:

```
orca tui
```

It shows all services across all nodes, their status, resource usage, and health. Navigate with vim-style keys, view logs, trigger redeploys.

## AI Operations

This is where it gets fun. Orca has a built-in AI assistant that understands your cluster:

```bash
orca ask "why is the API slow?"
```

It reads your service configs, checks container stats, inspects logs, and gives you a diagnosis. Works with any OpenAI-compatible API (Ollama, LiteLLM, vLLM).

## Architecture

```
CLI / TUI / Web API
        |
   Control Plane
   (Raft + redb)
        |  WebSocket
   +----|----+
   v    v    v
 Node  Node  Node
 (Docker + Wasm + Proxy)
```

8 Rust crates, ~15k lines, 120+ tests. Every file under 250 lines. The dependency flow is clean: `core` <- `agent` <- `control` <- `cli`.

## What's New in v0.2.2

The latest release adds:

- **WebSocket streaming** between master and agents (real-time deploy push, no more polling)
- **Auto-reconciliation** on agent reconnect (master sends expected services, agent deploys missing ones)
- **Hot route provisioning** — proxy routes + TLS certs are added immediately when a container deploys
- **`orca deploy <service>`** — deploy a single service by name
- **`orca redeploy <service>`** — force pull + restart
- **Infra webhooks** — git push config changes, auto-pull + redeploy
- **Smart reconciler** — compares config templates, not resolved values, so services aren't restarted unnecessarily

## Contributing

Orca is open source (AGPL-3.0) and actively seeking contributors. Areas where help is wanted:

- **Log streaming** — pipe container logs from remote nodes to the TUI
- **TUI polish** — webhook management, backup dashboard, secrets organizer
- **ARM64 builds** — CI currently only builds x86_64
- **Preview environments** — PR-based ephemeral deploys
- **Nixpacks integration** — auto-detect builds without Dockerfiles

The codebase is designed for contribution: small files, clear module boundaries, comprehensive tests, and a [CLAUDE.md](https://github.com/mighty840/orca/blob/main/CLAUDE.md) that serves as an architecture guide.

```bash
git clone https://github.com/mighty840/orca.git
cd orca
cargo test        # 120+ tests, all passing
cargo build       # single binary output
```

**GitHub:** [github.com/mighty840/orca](https://github.com/mighty840/orca)
**Docs:** [mighty840.github.io/orca](https://mighty840.github.io/orca)
**crates.io:** [crates.io/crates/mallorca](https://crates.io/crates/mallorca)

---

*Orca is built by a small team running real production workloads on it. Every feature exists because we needed it, every bug fix comes from a real incident. If you're in the Coolify-to-K8s gap, give it a try.*
