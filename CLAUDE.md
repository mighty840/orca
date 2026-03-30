# Orca — Container + Wasm Orchestrator

## Build & Test

```bash
cargo build                          # Build all crates
cargo test                           # Unit + integration tests
cargo test -- --ignored              # E2E tests (requires Docker)
cargo fmt --all                      # Format
cargo clippy -- -D warnings          # Lint
cargo fmt --all && cargo clippy -- -D warnings && cargo test  # Full check
```

## Architecture

Cargo workspace with 8 crates:

| Crate | Type | Purpose |
|-------|------|---------|
| `orca-core` | lib | Types, config, `Runtime` trait, API types, errors |
| `orca-agent` | lib | Docker + Wasm runtime implementations |
| `orca-control` | lib | API server, reconciler, shared state |
| `orca-proxy` | lib | Reverse proxy + Wasm trigger routing |
| `orca-ai` | lib | LLM backend, conversational alerts, monitor |
| `orca-cli` | bin | Single `orca` binary (all commands) |
| `orca-tui` | bin | TUI dashboard (stub, M3) |
| `orca-web` | bin | Web dashboard (stub, M3) |

**Key abstraction:** The `Runtime` trait in `orca-core` is implemented by `ContainerRuntime` (Docker/bollard) and `WasmRuntime` (wasmtime). The reconciler dispatches to the correct runtime based on `RuntimeKind`.

**Dependency flow:** `core` <- `agent` <- `control` <- `cli`. `proxy` depends only on `core`. `ai` depends only on `core`.

## Conventions

- **Max 250 lines per file.** Split into submodules when a file grows beyond this.
- **`thiserror`** for error types, **`anyhow`** for application-level errors.
- **`tracing`** for all logging — never `println!` for errors or diagnostics.
- **`Arc<RwLock<>>`** for shared state between async tasks.
- Prefer `&str` over `String` in function parameters where possible.
- No `.unwrap()` in library code. Use `.expect()` only for true invariants.

## Testing

- **Unit tests:** `#[cfg(test)] mod tests` at the bottom of each source file.
- **Integration tests:** `crates/*/tests/*.rs` — test crate-level interactions without external services.
- **E2E tests:** `tests/e2e/` — require Docker, guarded by `#[ignore]`, run with `cargo test -- --ignored`.
- Use `MockRuntime` from `orca-core` test utilities for tests that need a `Runtime`.
- Test config parsing with the example `.toml` files in the repo root.

## File Organization

When splitting `foo.rs` into a directory:
```
foo/
  mod.rs       # Re-exports all public items
  types.rs     # Data structures
  logic.rs     # Business logic
```

All public items must remain re-exported from `mod.rs` so external `use` paths don't change.

## Crate Versioning

All crates share the workspace version in `Cargo.toml`. Bump version in `[workspace.package]` before publishing.
