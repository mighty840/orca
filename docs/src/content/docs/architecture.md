# Architecture

## Crate Map

Orca is a Cargo workspace with 8 crates:

| Crate | Type | Purpose |
|-------|------|---------|
| `orca-core` | lib | Types, config parsing, `Runtime` trait, secrets, errors |
| `orca-agent` | lib | Docker + Wasm runtime implementations |
| `orca-control` | lib | API server (axum), reconciler, scheduler, Raft consensus |
| `orca-proxy` | lib | Reverse proxy + TLS + Wasm routing (pingora) |
| `orca-ai` | lib | LLM backend, conversational alerts, GPU monitor |
| `orca-cli` | bin | Single `orca` binary (all commands) |
| `orca-tui` | bin | Terminal UI dashboard (ratatui) |
| `orca-web` | bin | Web dashboard (Dioxus, stub) |

### Dependency Flow

```
core <-- agent <-- control <-- cli
  ^                   ^
  |                   |
  +--- proxy          +--- tui / web
  +--- ai
```

## System Diagram

```
┌──────────────────────────────────────┐
│         CLI / TUI / Web UI           │
└───────────────┬──────────────────────┘
                │ REST / WebSocket
┌───────────────▼──────────────────────┐
│          Control Plane               │
│  ┌────────────────────────────────┐  │
│  │  API Server (axum)             │  │
│  │  Scheduler (bin-packing + GPU) │  │
│  │  Raft Consensus (openraft)     │  │
│  │  State Store (redb)            │  │
│  │  Health Checker + AI Monitor   │  │
│  └────────────────────────────────┘  │
└───────────────┬──────────────────────┘
                │ gRPC (mTLS)
     ┌──────────┼──────────┐
     ▼          ▼          ▼
┌─────────┐ ┌─────────┐ ┌─────────┐
│ Node 1  │ │ Node 2  │ │ Node 3  │
│ Docker  │ │ Docker  │ │ Docker  │
│ Wasm    │ │ Wasm    │ │ Wasm    │
│ Proxy   │ │ Proxy   │ │ Proxy   │
└─────────┘ └─────────┘ └─────────┘
```

## Runtime Trait

The core abstraction. Every workload -- container or Wasm -- implements it:

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

Two implementations: `ContainerRuntime` (Docker via bollard) and `WasmRuntime` (wasmtime + WASI P2).

## Key Dependencies

| Crate | Purpose |
|-------|---------|
| `openraft` | Raft consensus -- pure Rust, async |
| `redb` | Embedded KV store -- zero-config, ACID |
| `axum` | HTTP API -- Tokio ecosystem |
| `tonic` | gRPC -- control-plane to agent |
| `bollard` | Docker API client |
| `wasmtime` | Wasm runtime -- WASI P2 |
| `pingora` | Reverse proxy -- Cloudflare-proven |
| `ratatui` | TUI framework |
| `clap` | CLI parsing |
| `tracing` | Structured logging + OpenTelemetry |

## Design Principles

1. **Single binary** -- one executable is agent, control plane, CLI, and proxy
2. **Config fits on one screen** -- TOML, not YAML
3. **Dual runtime** -- containers and Wasm are both first-class
4. **Batteries included** -- proxy, TLS, secrets, metrics, git-push deploy
5. **No `.unwrap()` in library code** -- proper error handling everywhere
6. **Max 250 lines per file** -- enforced by convention
