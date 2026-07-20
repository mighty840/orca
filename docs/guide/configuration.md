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
value = "your-token-here"
role = "admin"                        # admin | deployer | viewer
```

### RBAC Roles

| Role | Deploy | Stop/Scale | Logs/Status | Drain/Tokens |
|------|--------|------------|-------------|--------------|
| `admin` | Yes | Yes | Yes | Yes |
| `deployer` | Yes | Yes | Yes | No |
| `viewer` | No | No | Yes | No |

Create tokens via CLI: `orca token create --name ci --role deployer`

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
surfaces the agent's real pull error instead of a bare timeout. `[reconcile]`
applies only new or changed services each pass; unchanged services are left
untouched and removed ones are not auto-pruned. See
[Self-healing](/reference/self-healing).

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

[service.env]
DATABASE_URL = "${secrets.db_url}"    # Secret reference

[service.resources]
memory = "512Mi"
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

[cluster.network]
provider  = "netbird"
setup_key = "${secrets.netbird_key}"
```

::: warning
Secret files (`secrets.json`) should be added to `.gitignore`. Never commit plaintext secrets.
:::

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
