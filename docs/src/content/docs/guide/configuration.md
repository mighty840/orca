# Configuration

Orca uses TOML for all configuration. Two files define your infrastructure:

- **`cluster.toml`** -- cluster-level settings (nodes, domain, TLS, AI)
- **`services/<project>/service.toml`** -- service definitions per project

## cluster.toml

```toml
[cluster]
name = "production"
domain = "myapp.com"
acme_email = "ops@myapp.com"
log_level = "info"                    # trace | debug | info | warn | error
api_port = 6880
api_bind = ["127.0.0.1", "100.80.5.14"]  # API listen addresses (default ["0.0.0.0"])

# Node list (omit for single-node)
[[node]]
address = "10.0.0.1"
labels = { zone = "eu-1", role = "general" }

[[node]]
address = "10.0.0.2"
labels = { zone = "eu-1", role = "gpu" }

# GPU declaration on a node
[[node.gpus]]
vendor = "nvidia"
count = 2
model = "A100"

# API authentication
[[token]]
name = "admin"
value = "${secrets.admin_token}"
role = "admin"                        # admin | deployer | viewer
```

`api_bind` limits the API, and the agent channel on it, to the listed
addresses. Keep `127.0.0.1` in the list: the `orca` CLI and TUI on the host
connect there by default.

### RBAC Roles

| Role | Deploy | Stop/Scale/Rollback | Logs/Status/Metrics | Secrets, exec, drain, webhooks, token rotation |
|------|--------|---------------------|---------------------|-----------------------------------------------|
| `admin` | Yes | Yes | Yes | Yes |
| `deployer` | Yes | Yes | Yes | No |
| `viewer` | No | No | Yes | No |

Create tokens via CLI: `orca token create --name ci --role deployer`

A `[[token]]` or `api_tokens` entry whose `${secrets.X}` doesn't resolve, or
that is empty, is dropped with an error; it is never loaded as the literal
string. Agents must join with the cluster token or another admin token.

### Deploy & Reconciliation

Optional `cluster.toml` blocks controlling how deploys are acknowledged, how
orphaned containers are recovered, and (optionally) continuous declarative
reconciliation. Defaults shown:

```toml
[deploy]
ack_timeout_secs = 10          # agent must acknowledge a deploy within this, else "unreachable"
completion_timeout_secs = 600  # time for the agent to pull the image + start (covers multi-GB pulls)
adopt_orphans = true           # adopt orca.managed containers running on an agent but missing from the registry
adopt_interval_secs = 30       # how often to scan agents for orphans
ws_idle_timeout_secs = 30      # silence window before an agent control session is declared dead (#131)

# Declarative reconciliation (K8s-style) — opt-in. When set, the master
# continuously applies a directory of service configs: drop in a service.toml
# and it deploys, no manual `orca deploy`.
[reconcile]
config_dir = "services"        # dir of <project>/service.toml files (or a single services.toml)
interval_secs = 30             # how often to re-apply

# Security response headers the proxy injects. On by default — omit this block
# to get the safe set below. Headers are add-if-absent, so an app that sets its
# own value always wins.
[security_headers]
enabled = true
hsts = "max-age=31536000"                       # HTTPS only; "" to disable; add "; includeSubDomains; preload" if you want it
content_type_options = "nosniff"
referrer_policy = "strict-origin-when-cross-origin"
frame_options = ""                              # off by default (SAMEORIGIN/DENY breaks iframe-embedded apps)
csp = ""                                        # off by default — set Content-Security-Policy per app
# extra = { "Permissions-Policy" = "geolocation=()" }   # arbitrary add-if-absent headers
```

The `[deploy]` wait is two-phase — a short *receipt* ack distinguishes an
unreachable agent from a slow image pull, and the long *completion* wait
surfaces the agent's real pull error instead of a bare timeout.

`[reconcile]` applies new or changed services each pass and leaves unchanged
ones alone. Every declared field counts as a change, but only a container spec
change recreates the container; placement, probes, `pull_policy`, the deploy
strategy, certificates and `replicas` are updated in place and persisted. A
placement change stops the service on its old node and starts it on the new
one. `replicas` is compared with the last declared value, so a manual
`orca scale` holds until `service.toml` changes it. A service removed from the
directory is pruned, paused or not. A spec that failed to deploy is left alone
for 15 minutes unless it changes (see
[Deployment](/guide/deployment#failed-deploys)). Invalid configs are rejected
before anything is applied. See [Self-healing](/reference/self-healing).

`[security_headers]` makes the proxy add a baseline set of security headers
(`Strict-Transport-Security` on HTTPS, `X-Content-Type-Options`,
`Referrer-Policy`) to every response — once, centrally, instead of per app. It's
**add-if-absent**: a backend that already sets a header keeps its own value
(apps override the defaults just by sending the header). `X-Frame-Options` and
`Content-Security-Policy` are off by default — `X-Frame-Options` breaks apps
meant to be embedded in an iframe (Jitsi, Element, Collabora), and a blanket CSP
breaks most apps, so set CSP per app. Set `enabled = false` to turn it off
entirely. The agent's node-local proxy uses the default-on set.

## service.toml

Each project directory contains a `service.toml` with one or more `[[service]]` blocks:

```toml
[[service]]
name = "api"
image = "myorg/api:latest"
replicas = 3
port = 8080
domain = "api.myapp.com"
health = "/healthz"
internal = true                       # Internal-only (no public route)
restart_policy = "on-failure:5"       # no (default) | always | unless-stopped | on-failure[:N]
                                      # Docker revives crashed containers on its own — masks
                                      # transient boot races instead of dead-until-redeploy
# host_port = 3478                    # Fixed host port on all interfaces (default: random, 127.0.0.1)
# extra_ports = ["22222:22", "127.0.0.1:5433:5432", "10000:10000/udp"]

[service.env]
DATABASE_URL = "${secrets.db_url}"    # Secret reference

[service.resources]
memory = "512Mi"                      # bytes, Ki/Mi/Gi/Ti or K/M/G/T[B]; fractions like "1.5Gi"
cpu = 1.0

[service.resources.gpu]
count = 1
vendor = "nvidia"
vram_min = 24000

[service.deploy]
strategy = "rolling"                  # rolling | canary
max_unavailable = 1
canary_weight = 20                    # % traffic to canary (canary only)

[service.placement]
node = "worker-1"                     # Pin to node
labels = { zone = "eu-1" }           # Or match by labels

[service.volume]
path = "/data"
size = "10Gi"

[service.build]
repo = "git@github.com:org/repo.git"
branch = "main"
dockerfile = "Dockerfile"
```

#### Multiple domains

Serve one container on several hostnames — apex + www, regional aliases, or a
dual-TLD migration — with `domains` instead of `domain`. Each entry gets its own
TLS certificate and proxy route to the same backend, so you no longer duplicate
the `[[service]]` block (which would spawn a redundant container):

```toml
[[service]]
name = "marketing"
image = "myorg/marketing:latest"
port = 8080
domains = ["example.com", "www.example.com", "example.org"]
```

`domain` and `domains` are mutually exclusive — setting both is rejected at deploy.

#### Host ports

| Key | Binds to | Use |
|-----|----------|-----|
| `port` alone | A random host port on `127.0.0.1` | Normal case: only the node's proxy and health checks use it |
| `port` + `host_port` | `host_port` on all interfaces | Deliberate public ports, e.g. TURN on 3478 |
| `extra_ports` | The address you give; `host:container` means `0.0.0.0` | Raw TCP/UDP such as git SSH or Jitsi media |

`extra_ports` accepts `host:container`, `host:container/proto` and
`ip:host:container[/proto]`. An entry that publishes a well-known database
port (5432, 3306, 27017, 6379, 9000, 6333) on all interfaces logs a warning
suggesting a `127.0.0.1:` prefix. Docker's port publishing bypasses ufw, so
anything on `0.0.0.0` is reachable from every network the host is on.

A running container picks up a changed binding the next time it is recreated;
upgrading orca restarts nothing.

#### Memory limits

`resources.memory` takes a byte count, binary units (`Ki`, `Mi`, `Gi`, `Ti`)
or decimal units (`K`, `M`, `G`, `T`, optionally with `B`), including
fractions such as `1.5Gi`. Any other value fails validation and the service is
not deployed.

#### Restart policy

`restart_policy` is passed to Docker: `no` (default), `always`,
`unless-stopped`, or `on-failure[:N]`. It lets Docker revive a container that
crashed on a transient error. The watchdog heals services independently of it;
see [Self-healing](/reference/self-healing).

### Wasm Services

```toml
[[service]]
name = "edge-fn"
runtime = "wasm"
module = "./modules/api.wasm"         # Local path or OCI reference
triggers = ["http:/api/edge/*"]       # HTTP, cron, queue, event
replicas = "auto"                     # Auto-scale (Wasm is cheap)

[service.env]
API_KEY = "${secrets.edge_key}"
```

## Secrets

Secrets are stored encrypted and referenced in config with `${secrets.KEY}`.
Two backends exist:

- **Default:** a machine-local AES-256-GCM store (`~/.orca/secrets.json`,
  key at `~/.orca/master.key`). Simple, but tied to one machine.
- **Recommended — `[secrets]` in cluster.toml:** an age/SOPS-encrypted file
  committed to your config repo. Keys stay plaintext (readable `git diff`),
  values are encrypted to your age recipients. The master decrypts
  in-process; recovery never needs orca — `sops -d` / `age -d` with the key
  is enough.

```toml
[secrets]
encrypted_file = "secrets.enc.json"            # relative to cluster.toml
age_key_file   = "/root/.config/orca/age.key"  # master's age identity (keep OUT of git)
age_recipients = [                             # encrypt to master + operator
  "age1masterkey…",
  "age1operatorkey…",
]
# git_autocommit = true   # commit+push after each mutation (default)
```

Generate keys with `age-keygen`. Because the file is encrypted to *both*
recipients, the operator can always decrypt locally, independent of the
master. With `git_autocommit` (default), every `orca secrets set/remove`
commits and pushes `secrets.enc.json` so the repo stays the source of truth;
push failures are logged loudly but never lose the local write.

```bash
orca secrets set DB_PASS "s3cret"
orca secrets list
orca secrets import -f .env           # Bulk import
orca secrets migrate                  # move legacy local store → encrypted file
```

The CLI uses the encrypted backend when run from the directory containing
cluster.toml (offline/recovery access included — no running master needed).

### Project-scoped secrets

Secrets can be scoped to a project with a `<project>.<key>` prefix. For a
service in project `p`, `${secrets.KEY}` resolves `p.KEY` first and falls
back to the bare global `KEY` — so a project can override a shared default
without leaking its value to other projects. Explicit cross-project
references (`${secrets.other.KEY}`) resolve as written.

```bash
orca secrets set --project inka db_password "s3cret"   # stored as inka.db_password
orca secrets get --project inka db_password            # scoped first, global fallback
orca secrets remove --project inka db_password
orca secrets list                                      # grouped by scope
```

`${secrets.X}` is resolved both in `service.toml` (any string field, including
`env` values) and in selected `cluster.toml` fields:

| File | Resolved fields |
|------|-----------------|
| `service.toml` | All string fields, including `env`, `image`, `domain`, `aliases` |
| `cluster.toml` | `ai.api_key`, `ai.endpoint`, SMTP password, `network.setup_key`, named token values, S3 backup credentials |

This means cluster-level config can be safely committed to git -- credentials
live in the encrypted secret store, not in the TOML.

```toml
[ai]
provider = "litellm"
endpoint = "${secrets.ai_endpoint}"
api_key  = "${secrets.ai_api_key}"

[network]
provider  = "netbird"
setup_key = "${secrets.netbird_key}"
```

A `${secrets.X}` in `service.toml` that doesn't resolve, or a secrets store
that can't be opened, fails the deploy and names the service and the missing
keys. The container is never started with the literal string.

::: warning
Secret files (`secrets.json`) should be added to `.gitignore`. Never commit plaintext secrets.
:::

## Backups

```toml
[backup]
schedule = "0 0 3 * * *"          # six-field cron (with seconds): daily 03:00
retention_days = 30               # default 30
keep_min = 7                      # always keep the newest N copies of each artifact
prune_s3 = false                  # apply retention to S3 targets too (opt-in)
bind_mount_max_mb = 512           # larger bind-mount sources are reported, not archived
age_recipients = ["age1..."]      # encrypt backups to these age public keys

[[backup.targets]]
type = "local"
path = "/var/lib/orca/backups"

[[backup.targets]]
type = "s3"
bucket = "my-backups"
region = "eu-central-1"
prefix = "orca/"
endpoint = "https://s3.example.com"          # optional, for S3-compatible stores
access_key = "${secrets.S3_ACCESS_KEY}"
secret_key = "${secrets.S3_SECRET_KEY}"
```

Per service:

```toml
[service.backup]
enabled = true
pre_hook = "pg_dump -U postgres -d mydb -F c -f /var/lib/postgresql/data/orca-dump.pgc"
```

### Encryption with age

Set `age_recipients` before the first scheduled run. Generate a key pair with
`age-keygen`, keep the private key out of band (not on the cluster), and add
the `age1...` public key to `cluster.toml`.

With recipients set:

- Config artifacts (`cluster.toml`, `secrets.json`, `master.key`,
  `webhooks.json`, TLS certificates, the ACME account, `backup_config.json`)
  are encrypted before they are stored or uploaded.
- Each Docker volume tarball is encrypted to `<volume>.tar.gz.age` right after
  it is written, and the plaintext is removed. If encryption fails, the volume
  counts as failed and nothing is kept in clear. The run summary says
  `volumes N/M encrypted`.
- The bind-mount archive (`bind-mounts.tar.gz`) is encrypted too.

Without recipients, artifacts that hold key material are skipped with a
warning, and volume tarballs and the bind-mount archive are stored
unencrypted.

Restore with the private key:

```bash
orca backup restore-basic --identity ~/age.key
orca backup restore-volume <volume> --identity ~/age.key
```

### Failures and retention

`orca backup` exits non-zero when a run fails, every target must store each
artifact, and a failed scheduled run raises a critical alert (without needing
the LLM). A failing `pre_hook` is reported. No pruning happens after a failed
run, local pruning only touches orca's own artifacts, and `keep_min` keeps the
newest copies whatever their age. S3 objects are only pruned with
`prune_s3 = true`; a bucket lifecycle rule is the alternative. S3 targets need
`rclone` on every node.

## Observability

```toml
[observability]
otlp_endpoint = "https://signoz.example.com"

[observability.alerts]
webhook = "https://hooks.slack.com/services/..."
email = "ops@myapp.com"
```

## AI Configuration

See the [AI Ops guide](/guide/ai-ops) for full details.

```toml
[ai]
provider = "litellm"                  # litellm | ollama | openai
endpoint = "https://llm.example.com"
model = "qwen3-30b"
api_key = "${secrets.ai_api_key}"
```

## Shell completion

Static completion (subcommands and flags) is generated by `orca completions <shell>`.
For **dynamic** completion of live values — service names, secret keys, webhook
and alert ids — enable clap's dynamic engine by adding one line to your shell rc:

```bash
# bash
source <(COMPLETE=bash orca)
# zsh
source <(COMPLETE=zsh orca)
# fish
COMPLETE=fish orca | source
```

Dynamic candidates are fetched from the master on demand; if it's unreachable
the completion simply offers nothing (never errors).
