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

### Systemd service (recommended)

Install orca as a systemd service for auto-start on boot and automatic
port 80/443 binding (no manual `setcap` needed):

```bash
# Master node (control plane + proxy):
orca install-service
sudo systemctl start orca

# Agent node (joined to a master):
orca install-service --leader <master-ip>:6880
sudo systemctl start orca-agent
```

The systemd unit uses `AmbientCapabilities=CAP_NET_BIND_SERVICE`, so
the binary can bind to privileged ports without root or setcap.

View logs with `journalctl -u orca -f` (or `orca-agent` on agent nodes).

### Port binding (manual, without systemd)

If not using systemd, grant the capability after each install or update:

```bash
sudo setcap 'cap_net_bind_service=+ep' $(which orca)
```

> **Note:** `orca update` attempts to restore setcap automatically via
> `sudo -n setcap`. If passwordless sudo isn't configured, you'll need
> to run the setcap command manually after each update. Using systemd
> avoids this entirely.

## Updating

```bash
orca update          # Downloads latest release, restores setcap
orca reload          # Restarts the daemon and redeploys all services
```

If running via systemd:

```bash
orca update
sudo systemctl restart orca   # or orca-agent on agent nodes
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

Deploy or redeploy individual services:

```bash
orca deploy web              # Deploy only the "web" service
orca redeploy web            # Force pull the image and restart
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

## GitOps with the Infra Webhook

If you keep your service definitions in a git repo (recommended -- see the
[DevOps guide](/guide/devops)), you can set up an infra webhook so that every
`git push` to the repo automatically runs `git pull` + `orca deploy` on the
cluster:

```bash
curl -X POST http://127.0.0.1:6880/api/v1/webhooks \
  -H "Authorization: Bearer $(cat ~/.orca/cluster.token)" \
  -H "Content-Type: application/json" \
  -d '{
    "repo": "myorg/orca-infra",
    "service_name": "__infra__",
    "branch": "main",
    "secret": "your-webhook-secret"
  }'
```

Now pushing to `main` in your infra repo triggers a full cluster reconcile --
no SSH, no `git pull && orca deploy` by hand.

## Next Steps

- [Configuration reference](/guide/configuration) -- cluster.toml and service.toml in detail
- [Services](/guide/services) -- projects, networks, and cross-service communication
- [Deployment strategies](/guide/deployment) -- rolling updates, canary, and rollback
