# Contributing to Orca

Thank you for your interest in contributing to Orca! This guide will help you get started.

## Development Setup

```bash
# Clone the repo
git clone https://github.com/mighty840/orca.git
cd orca

# Install dependencies
# Ubuntu/Debian: sudo apt install protobuf-compiler build-essential pkg-config libssl-dev
# Fedora: sudo dnf install protobuf-compiler gcc pkg-config openssl-devel

# Build
cargo build

# Run tests
cargo test

# Run lints
cargo fmt --all -- --check
cargo clippy -- -D warnings
```

## Project Structure

```
crates/
  orca-core/       Types, config, Runtime trait, secrets, backups
  orca-agent/      Docker + Wasm runtime implementations
  orca-control/    API server, reconciler, scheduler, Raft
  orca-proxy/      Reverse proxy with TLS and Wasm routing
  orca-ai/         AI operations assistant
  orca-cli/        CLI binary (the `orca` command)
  orca-tui/        Terminal UI dashboard (library)
  orca-web/        Web dashboard (stub)
```

## Code Conventions

- **Max 250 lines per file.** Split into submodules when a file grows beyond this.
- **`thiserror`** for error types, **`anyhow`** for application errors.
- **`tracing`** for logging — never `println!` for errors.
- Run `cargo fmt --all` and `cargo clippy -- -D warnings` before every commit.
- Write unit tests in `#[cfg(test)] mod tests` at the bottom of each file.
- See [AGENTS.md](AGENTS.md) for detailed Rust coding guidelines.

## Pull Request Process

1. Fork the repo and create a branch from `main`.
2. Make your changes with tests.
3. Ensure `cargo fmt`, `cargo clippy`, and `cargo test` all pass.
4. Ensure no file exceeds 250 lines.
5. Open a PR with a clear description of the change.

## What to Work On

Check [GitHub Issues](https://github.com/mighty840/orca/issues) for open tasks. Good first issues are labeled `good-first-issue`.

Key areas where contributions are welcome:
- **ACME/Let's Encrypt** automation (HTTP-01 challenge in the proxy)
- **Nixpacks** integration for auto-detect builds
- **Service templates** (Postgres, Redis, WordPress, etc.)
- **Preview environments** (PR-based temporary deploys)
- **S3 backup** implementation (finish the stub)
- **Notification channels** (Telegram, PagerDuty)
- **TUI improvements** (k9s-style resource views, exec, filtering)

## Reporting Issues

File issues at [github.com/mighty840/orca/issues](https://github.com/mighty840/orca/issues) with:
- What you expected to happen
- What actually happened
- Steps to reproduce
- `orca --version` output

## License

By contributing, you agree that your contributions will be licensed under AGPL-3.0.
