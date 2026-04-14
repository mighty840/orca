# Services

## Directory Structure

Orca uses a directory-based project model. Each subdirectory under `services/` is a project (namespace):

```
~/orca/
  cluster.toml
  services/
    frontend/
      service.toml          # Web app, CDN, etc.
    backend/
      service.toml          # API, workers
    data/
      service.toml          # Postgres, Redis
```

Each project gets its own Docker network, secrets scope, and logical grouping.

## Defining Services

A `service.toml` can contain multiple `[[service]]` blocks:

```toml
[[service]]
name = "api"
image = "myorg/api:latest"
replicas = 3
port = 8080
domain = "api.example.com"
health = "/healthz"

[[service]]
name = "worker"
image = "myorg/worker:latest"
replicas = 2
```

Deploy all services at once:

```bash
orca deploy              # Discovers services/*/service.toml
```

## Networking

### Same-Project Communication

Services in the same project share a Docker network. Use container names or aliases:

```toml
[[service]]
name = "myapp-db"
image = "postgres:16"
port = 5432
aliases = ["db"]
# Other services in this project reach it at "db:5432"
```

### Cross-Project Communication

Set `internal = true` on both services to enable cross-project routing through the internal network:

```toml
[[service]]
name = "shared-cache"
image = "redis:7-alpine"
port = 6379
internal = true
```

### Public vs Internal

| Setting | Behavior |
|---------|----------|
| `domain = "app.example.com"` | Publicly routed through proxy |
| `internal = true` | Reachable only within the cluster |
| Both | Public route + internal alias |

## Dual Runtime

Orca runs both containers and WebAssembly modules as first-class workloads:

| | Containers | WebAssembly |
|---|---|---|
| **Backend** | Docker (bollard) | wasmtime (WASI P2) |
| **Cold start** | ~3s | ~5ms |
| **Memory** | 30MB+ | 1-5MB |
| **GPU** | nvidia + amd (ROCm) passthrough | N/A |
| **Use case** | Existing images, databases | Edge functions, API handlers |

### Wasm Triggers

```toml
[[service]]
name = "edge"
runtime = "wasm"
module = "./edge.wasm"
triggers = ["http:/api/edge/*"]       # HTTP path match

[[service]]
name = "reporter"
runtime = "wasm"
module = "./report.wasm"
triggers = ["cron:0 9 * * *"]         # Daily at 9am
```

Trigger types: `http:<route>`, `cron:<schedule>`, `queue:<topic>`, `event:<pattern>`

## Resource Limits

```toml
[service.resources]
memory = "512Mi"
cpu = 1.0

[service.resources.gpu]
count = 1
vendor = "nvidia"           # "nvidia" | "amd"
vram_min = 24000
```

### AMD ROCm

Set `vendor = "amd"` to passthrough an AMD GPU via ROCm. Orca mounts
`/dev/kfd` and `/dev/dri` into the container and auto-detects the host's
`video` and `render` group IDs so the container process can access the
devices without manual `--group-add` flags:

```toml
[[service]]
name = "inference"
image = "myorg/inference-rocm:latest"

[service.resources.gpu]
count  = 1
vendor = "amd"
```

The scheduler matches the service against any node that declared a GPU
with the same `vendor` in `cluster.toml`.

## Placement Constraints

Pin services to specific nodes or label-matched groups:

```toml
[service.placement]
node = "gpu-worker-1"           # Exact node match
labels = { zone = "eu-1" }      # Label selector
```
