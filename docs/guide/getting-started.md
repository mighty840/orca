# Getting Started

## Prerequisites

- Linux (x86_64 or aarch64)
- Docker installed and running
- Rust toolchain (for building from source)
- `protoc` (Protocol Buffers compiler)

::: code-group

```bash [Ubuntu/Debian]
sudo apt install protobuf-compiler build-essential pkg-config libssl-dev
```

```bash [Fedora]
sudo dnf install protobuf-compiler gcc pkg-config openssl-devel
```

:::

## Installation

### From crates.io

```bash
cargo install mallorca
```

This installs the `orca` binary.

### From source

```bash
git clone https://github.com/mighty840/orca.git
cd orca
cargo build --release
# Binary at target/release/orca
```

### Port binding (one-time)

Orca's proxy needs ports 80 and 443. On Linux, grant the capability after each install:

```bash
sudo setcap 'cap_net_bind_service=+ep' $(which orca)
```

## Your First Cluster

Create a minimal configuration:

```bash
mkdir -p services/web

cat > cluster.toml << 'EOF'
[cluster]
name = "my-cluster"
domain = "example.com"
acme_email = "ops@example.com"
EOF

cat > services/web/service.toml << 'EOF'
[[service]]
name = "web"
image = "nginx:alpine"
replicas = 2
port = 80
domain = "example.com"
health = "/"
EOF
```

## Deploy

```bash
orca server &        # Start the control plane
orca deploy          # Auto-discovers services/*/service.toml
```

## Verify

```bash
orca status          # Service health overview
orca logs web        # Stream container logs
orca tui             # Terminal dashboard
```

::: tip
For single-node setups, just omit the `[[node]]` sections in `cluster.toml`. Orca runs everything locally by default.
:::

## One-Click Database

```bash
orca db create postgres mydb
# Deploys postgres:16 with auto-generated password, volume, and health check
# Stores credentials as secrets, prints the connection string
```

## Next Steps

- [Configuration reference](/guide/configuration) -- cluster.toml and service.toml in detail
- [Services](/guide/services) -- projects, networks, and cross-service communication
- [Deployment strategies](/guide/deployment) -- rolling updates, canary, and rollback
