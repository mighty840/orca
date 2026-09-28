# CLI Reference

All commands are subcommands of the `orca` binary.

Every command that talks to the control plane accepts the global `--api <url>`
flag (default `http://127.0.0.1:6880`). The token comes from `ORCA_TOKEN`, or
from `~/.orca/cluster.token` when that is unset.

## Cluster

### `orca server`
Start the control plane, agent, and proxy on this node.

```bash
orca server                          # Foreground, reads ./cluster.toml
orca server -d                       # Run as a background daemon
orca server --config /etc/orca/cluster.toml
orca server --teardown-on-exit       # Development and tests only
```

| Flag | Purpose |
|------|---------|
| `-c, --config <path>` | Path to `cluster.toml` (default: `cluster.toml`) |
| `--proxy-port <port>` | Proxy port for HTTP traffic (default: `80`) |
| `-d, --daemon` | Run in the background |
| `--teardown-on-exit` | On exit, stop and remove every orca-managed container on this host |

Stopping or restarting `orca server` (Ctrl-C, `systemctl stop|restart`,
`orca shutdown`) leaves workloads running; the next start re-attaches to them.
`SIGTERM` shuts the API down gracefully, with the drain capped at 10 seconds.
`--teardown-on-exit` restores the old cleanup and is meant for development and
tests only.

The server refuses to start when `~/.orca/cluster.db` can't be opened or its
services can't be read, instead of starting with empty state. Fix or restore
the store first. It also exits with an error when the proxy can't bind port 80
or 443.

### `orca shutdown`
Stop the orca daemon started with `-d` (sends `SIGTERM`). Workloads keep running.

### `orca reload`
Restart the daemon in its original working directory and redeploy all services.

### `orca join`
Join this node to an existing cluster as an agent.

```bash
ORCA_TOKEN=<cluster-token> orca join 10.0.0.1:6880
orca join 10.0.0.1:6880              # Token from ~/.orca/cluster.token
orca join 10.0.0.1:6880 --daemon
```

| Flag | Purpose |
|------|---------|
| `--token <value>` | Cluster token. Also read from `ORCA_TOKEN`; falls back to `~/.orca/cluster.token` |
| `-d, --daemon` | Run in the background |
| `--setup-key <key>` | NetBird setup key for mesh networking |

Prefer `ORCA_TOKEN` or the token file: a `--token` value is visible in the
process list and the unit file. When `--token` and the file disagree, the flag
wins and a warning names the file (the value is never logged).

### `orca install-service`
Install a systemd unit that starts orca on boot.

```bash
orca install-service                          # Master: orca.service
orca install-service --leader 10.0.0.1:6880   # Agent: orca-agent.service
```

The agent unit reads its token from `~/.orca/agent.env` (mode 0600) via
`EnvironmentFile=`, not from `ExecStart`. `--token` defaults to
`~/.orca/cluster.token`.

### `orca nodes`
List cluster nodes with status and resource usage. `--gpus` shows each
node's declared GPU inventory (from `[[node.gpus]]` in `cluster.toml`).

```bash
orca nodes
orca nodes --gpus     # GPU inventory per node (alias: `orca gpus`)
```

### `orca tui`
Launch the terminal dashboard. See [Monitoring](/guide/monitoring#tui-dashboard).

```bash
orca tui
orca tui --api http://master.example.com:6880
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
orca deploy -f services/web/service.toml
```

`orca deploy` exits `1` when any service fails to deploy, after printing each
error. A failed replacement keeps the old container running; see
[Deployment](/guide/deployment#failed-deploys).

### `orca status`
Show service status, replicas, health, and the reason a degraded service is
failing.

```bash
orca status
```

### `orca logs`
Show logs from a service.

```bash
orca logs api                     # Last 100 lines
orca logs api --tail 500
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

A manual scale is kept until `replicas` changes in `service.toml`.

### `orca stop`
Pause one or more services. The config is retained and the service stays
paused across master restarts and agent reconnects.

```bash
orca stop api
orca stop api worker          # Stop multiple services in one call
orca stop                     # Stop all services
```

### `orca start`
Resume one or more paused services.

```bash
orca start api
orca start api worker
```

### `orca redeploy`
Force-pull the image and recreate one or more services, even when the spec hasn't changed.

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

### `orca build`
Build a service's image from its `[service.build]` section.

```bash
orca build           # All services with a build section
orca build api
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

```bash
orca secrets set DB_PASS "<value>"            # Store an encrypted secret
orca secrets set --project inka DB_PASS "<value>"
orca secrets get DB_PASS                      # Print a value to stdout
orca secrets remove DB_PASS
orca secrets list                             # Keys only, never values
orca secrets import -f .env                   # Bulk import from a .env file
orca secrets migrate                          # Legacy local store -> [secrets] encrypted file
```

`--project` (`-p`) scopes `set`, `get` and `remove` to `<project>.<key>`. See
[Configuration](/guide/configuration#secrets).

## Operations

### `orca backup`
Back up and restore volumes and config files.

```bash
orca backup all           # Volumes + config files + bind mounts
orca backup basic         # Config files only
orca backup list          # Backups on every target (S3 recursively)

orca backup restore <id> --identity ~/age.key
orca backup restore-basic --identity ~/age.key
orca backup restore-volume pgdata --identity ~/age.key
orca backup restore-volume pgdata --from-s3 agents/<host>/<date>/pgdata.tar.gz.age --identity ~/age.key
```

| Subcommand | Purpose |
|------------|---------|
| `all` | Volumes, config files and bind-mount sources |
| `basic` | Config files only (`cluster.toml`, secrets, `cluster.db`, ...) |
| `list` | List backups on every target |
| `restore <id>` | Restore one config backup whose name or S3 key contains `<id>` |
| `restore-basic` | Restore the newest copy of every config file, `master.key` first, from local or S3 targets. `--force` restores even when a server answers on port 6880 |
| `restore-volume <volume>` | Restore a Docker volume from the latest local backup, or from `--from-s3 <key>` |

`--identity <file>` is the age private key (from `age-keygen`) that decrypts
`.age` backups. Existing files are moved aside, not overwritten. `orca backup`
exits non-zero when a run fails. See
[Configuration](/guide/configuration#backups).

### `orca cleanup`
Prune unused Docker resources (images, containers, volumes).

```bash
orca cleanup
```

### `orca token`
Manage API tokens and rotate the cluster token.

```bash
orca token show                              # Show the current cluster token
orca token create --name ci --role deployer  # Roles: admin, deployer, viewer
orca token list

orca token rotate                            # Start a cluster-token rotation
orca token rotate --status                   # Each agent's progress
orca token rotate --finish                   # Retire the old token
orca token rotate --finish --force           # Retire it even if agents lag behind
```

`orca token rotate` runs on the master. It writes a new token to
`~/.orca/cluster.token`, keeps accepting the old one (also across a master
restart), and pushes the new one to every connected agent. `--finish` refuses
while any agent isn't on the new token for good; `--force` overrides that and
can lock such agents out at their next restart. No step prints a token. See
[Multi-Node](/guide/multi-node#rotating-the-cluster-token) for the walkthrough.

### `orca webhooks`
Manage git push deploy webhooks.

```bash
orca webhooks list
orca webhooks add --repo myorg/app --service app --branch main
orca webhooks remove app                                     # Remove by service name
orca webhooks remove 'myorg-*' navidrome                     # Multiple + glob (*, ?)

# Provide a shared secret so the webhook handler can verify the signature
# (omit --secret and one is generated and printed once):
orca webhooks add --repo myorg/app --service app --branch main \
    --secret "$(openssl rand -hex 32)"

# Infra webhook -- triggers `git pull` + redeploy on the cluster
# whenever your orca-infra repo receives a push. --service is still
# required; it only names the webhook:
orca webhooks add --repo myorg/orca-infra --service infra --branch main --infra \
    --secret "$(openssl rand -hex 32)"
```

Flags:

| Flag | Purpose |
|------|---------|
| `--repo <owner/name>` | Source repository |
| `--service <name>` | Service to redeploy on push (with `--infra`, a label for the webhook) |
| `--branch <name>` | Branch filter (default: `main`) |
| `--secret <value>` | HMAC shared secret for signature verification (generated if omitted) |
| `--infra` | Treat as an orca-infra webhook: `git pull` + redeploy the cluster on push |

`orca webhooks remove <name>...` accepts multiple service names and glob
patterns (`*`, `?`). Removal keys on the service name, so a name removes
every webhook registered for that service.

### `orca import`
Convert existing deployments into `service.toml`.

```bash
orca import docker-compose docker-compose.yml
orca import coolify /data/coolify
```

`--analyze` adds suggestions for names and config.

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

### `orca alerts`
Work with AI alert conversations.

```bash
orca alerts list              # Open alerts (--all includes resolved)
orca alerts view <id>
orca alerts reply <id> "is the database up?"
orca alerts fix <id>          # Print the suggested command from the Fix section
orca alerts dismiss <id>
orca alerts resolve <id>
```

`orca alerts fix` only prints the command; review it and run it yourself.
