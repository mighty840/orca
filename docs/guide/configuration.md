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

Secrets are encrypted and stored in the cluster state (redb). Reference them in config with `${secrets.KEY}`.

```bash
orca secrets set DB_PASS "s3cret"
orca secrets list
orca secrets import -f .env           # Bulk import
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
