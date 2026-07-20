# CLI Reference

All commands are subcommands of the `orca` binary.

## Cluster

### `orca server`
Start the control plane, agent, and proxy on this node.

```bash
orca server              # Foreground
orca server &            # Background
```

### `orca join`
Join this node to an existing cluster.

```bash
orca join 10.0.0.1       # Join by leader address
```

### `orca nodes`
List cluster nodes with status and resource usage. `--gpus` shows each
node's declared GPU inventory (from `[[node.gpus]]` in `cluster.toml`).

```bash
orca nodes
orca nodes --gpus     # GPU inventory per node (alias: `orca gpus`)
```

### `orca tui`
Launch the terminal dashboard.

```bash
orca tui
```

### `orca update`
Self-update the orca binary.

```bash
orca update
```

## Services

### `orca deploy`
Deploy services from `services/*/service.toml`.

```bash
orca deploy              # Deploy all discovered services
orca deploy api          # Deploy a single service by name
orca deploy api worker   # Deploy multiple services in one call
```

### `orca status`
Show service status, replicas, and health.

```bash
orca status
orca status --project frontend
```

### `orca logs`
Stream logs from a service.

```bash
orca logs api
orca logs api --tail 100
orca logs api --follow            # stream new lines live (Ctrl-C to stop)
orca logs api --summarize         # AI-summarized digest with likely causes
```

`--follow` streams live for master-local services and polls for new lines
for agent-pinned services. `--summarize` requires an `[ai]` section in
`cluster.toml`. See the
[AI Ops guide](/guide/ai-ops) for setup.

### `orca scale`
Scale a service to N replicas.

```bash
orca scale api 5
```

### `orca stop`
Stop a service (config is retained).

```bash
orca stop api
orca stop api worker          # Stop multiple services in one call
```

### `orca redeploy`
Force-pull the image and restart one or more services, even when the spec hasn't changed.

```bash
orca redeploy api
orca redeploy api worker billing   # Redeploy multiple services in one call
```

### `orca promote`
Promote a canary deployment to stable.

```bash
orca promote api
```

### `orca rollback`
Rollback to the previous deployment.

```bash
orca rollback api
```

### `orca exec`
Execute a command inside a running container.

```bash
orca exec api -- sh
orca exec api -- cat /etc/hostname
```

## Databases

### `orca db create`
Create a managed database with auto-generated credentials.

```bash
orca db create postgres mydb
orca db create redis cache
orca db create mysql appdb
orca db create mongodb docs
```

### `orca db list`
List database services.

```bash
orca db list
```

## Secrets

### `orca secrets set`
Store an encrypted secret.

```bash
orca secrets set DB_PASS "s3cret"
```

### `orca secrets list`
List secret keys (values are never displayed).

```bash
orca secrets list
```

### `orca secrets import`
Bulk import secrets from an `.env` file.

```bash
orca secrets import -f .env
```

## Operations

### `orca backup`
Backup volumes and configs.

```bash
orca backup all           # Volumes + config files (secrets.json, cluster.toml)
orca backup basic         # Config files only
orca backup restore-basic # Restore latest secrets.json / cluster.toml from local backup
orca backup list          # List backups
```

### `orca cleanup`
Prune unused Docker resources (images, containers, volumes).

```bash
orca cleanup
```

### `orca token`
Manage API tokens.

```bash
orca token create --name ci --role deployer
orca token list
```

### `orca webhooks`
Manage git push deploy webhooks.

```bash
orca webhooks                                                # List
orca webhooks add --repo myorg/app --service app --branch main
orca webhooks remove app                                     # Remove by service name
orca webhooks remove 'myorg-*' navidrome                     # Multiple + glob (*, ?)

# Provide a shared secret so the webhook handler can verify the signature:
orca webhooks add --repo myorg/app --service app --branch main \
    --secret "$(openssl rand -hex 32)"

# Infra webhook -- triggers `git pull` + redeploy on the cluster
# whenever your orca-infra repo receives a push (no --service needed):
orca webhooks add --repo myorg/orca-infra --branch main --infra \
    --secret "$(openssl rand -hex 32)"
```

Flags:

| Flag | Purpose |
|------|---------|
| `--repo <owner/name>` | Source repository |
| `--service <name>` | Service to redeploy on push (omit with `--infra`) |
| `--branch <name>` | Branch filter (default: `main`) |
| `--secret <value>` | HMAC shared secret for signature verification |
| `--infra` | Treat as an orca-infra webhook: `git pull` + redeploy the cluster on push |

`orca webhooks remove <name>...` accepts multiple service names and glob
patterns (`*`, `?`). Removal keys on the service name, so a name removes
every webhook registered for that service.

### `orca completions`

**Dynamic completion (recommended)** — completes live values (service names,
secret keys, webhook and alert ids), not just subcommands and flags. Add one
line to your shell rc:

```bash
# bash
source <(COMPLETE=bash orca)
# zsh
source <(COMPLETE=zsh orca)
# fish
COMPLETE=fish orca | source
```

**Static script** — subcommands and flags only, for offline install:

```bash
orca completions bash       > /etc/bash_completion.d/orca
orca completions zsh        > "${fpath[1]}/_orca"
orca completions fish       > ~/.config/fish/completions/orca.fish
orca completions powershell > orca.ps1
```

Supported shells: `bash`, `zsh`, `fish`, `powershell`.

## AI

### `orca ask`
Ask the AI assistant a question with full cluster context.

```bash
orca ask "why is the API returning 500s?"
orca ask "which service is using the most memory?"
```

### `orca generate`
Generate service configuration from natural language.

```bash
orca generate "deploy redis with 2GB storage"
```
