# Orca AI Operations Guide

> For AI agents (Claude, GPT, etc.) managing infrastructure through Orca.
> Complete API reference, operational patterns, and decision trees for
> autonomous deployment, monitoring, and incident response.

## Quick Reference

```bash
orca deploy                    # Deploy all from services/*/service.toml
orca status                    # Cluster and service status
orca scale <service> <count>   # Scale replicas
orca logs <service> --tail 100 # View logs
orca stop <service>            # Stop one service
orca promote <service>         # Promote canary to stable
orca tui                       # Terminal dashboard
orca update                    # Self-update binary
orca backup all                # Backup all volumes
orca token create --name ci --role deployer  # Create service account
orca token list                # List API tokens
```

## Architecture

```
                    ┌─────────────────┐
                    │   orca server   │
                    │   (master)      │
                    │  API :6880      │
                    │  Proxy :80/443  │
                    │  Watchdog 30s   │
                    │  Health  10s    │
                    │  Stats   30s    │
                    └────┬───────┬────┘
                         │       │
                    ┌────▼──┐ ┌──▼────┐
                    │Agent 1│ │Agent 2│
                    │(node) │ │(node) │
                    └───────┘ └───────┘
```

## Directory Structure

```
~/orca/
  cluster.toml              # Cluster config
  services/
    <project>/              # Each directory = project (namespace)
      service.toml          # Service definitions
      secrets.json          # Per-project secrets (gitignored)
```

## REST API

Base: `http://<master>:6880` | Auth: `Authorization: Bearer <token>`

Auth supports RBAC roles: `admin` (full), `deployer` (CI/CD), `viewer` (read-only).

| Method | Path | Auth | Purpose |
|--------|------|------|---------|
| POST | /api/v1/deploy | Yes | Deploy services |
| GET | /api/v1/status?project= | Yes | Service status |
| POST | /api/v1/services/{name}/scale | Yes | Scale replicas |
| DELETE | /api/v1/services/{name} | Yes | Stop service |
| DELETE | /api/v1/projects/{name} | Yes | Stop project |
| POST | /api/v1/stop | Yes | Stop ALL |
| GET | /api/v1/services/{name}/logs | Yes | Container logs |
| GET | /api/v1/cluster/info | Yes | Node list |
| POST | /api/v1/services/{name}/promote | Yes | Promote canary |
| POST | /api/v1/services/{name}/rollback | Yes | Rollback service |
| POST | /api/v1/cluster/nodes/{id}/drain | Yes | Drain node |
| POST | /api/v1/cluster/nodes/{id}/undrain | Yes | Undrain node |
| POST | /api/v1/webhooks/github | Sig | Git push webhook |
| GET | /api/v1/webhooks | Yes | List webhooks |
| GET | /api/v1/health | No | Health check |
| GET | /metrics | No | Prometheus metrics |

### Deploy payload

```json
{
  "services": [{
    "name": "my-app",
    "project": "frontend",
    "image": "nginx:alpine",
    "replicas": 2,
    "port": 80,
    "domain": "app.example.com",
    "health": "/healthz",
    "env": {"KEY": "${secrets.KEY}"},
    "resources": {"memory": "512Mi", "cpu": 1.0},
    "internal": true,
    "placement": {"node": "worker-1"}
  }]
}
```

## Service Configuration (service.toml)

```toml
[[service]]
name = "my-app"
image = "myorg/myapp:latest"
replicas = 2
port = 8080
domain = "app.example.com"
health = "/healthz"
internal = true

[service.env]
DATABASE_URL = "${secrets.DB_URL}"

[service.resources]
memory = "512Mi"
cpu = 1.0

[service.placement]
node = "worker-1"

[service.readiness]
path = "/ready"
interval_secs = 5
timeout_secs = 3
failure_threshold = 3
initial_delay_secs = 10

[service.liveness]
path = "/healthz"
interval_secs = 10
timeout_secs = 3
failure_threshold = 3

[service.build]
repo = "git@github.com:org/repo.git"
branch = "main"
dockerfile = "Dockerfile"
```

## Secrets

```bash
cd ~/orca/services/myapp
orca secrets set DB_PASS "s3cret"    # Set
orca secrets list                     # List keys
# Use in toml: "${secrets.DB_PASS}"
```

## RBAC (Role-Based Access Control)

```toml
# cluster.toml
[[token]]
name = "sharang"
value = "abc123..."
role = "admin"

[[token]]
name = "gitea-ci"
value = "def456..."
role = "deployer"

[[token]]
name = "grafana"
value = "ghi789..."
role = "viewer"
```

Create tokens: `orca token create --name gitea-ci --role deployer`

| Role | Deploy | Stop/Scale | Logs/Status | Drain/Tokens |
|------|--------|------------|-------------|--------------|
| admin | Yes | Yes | Yes | Yes |
| deployer | Yes | Yes | Yes | No |
| viewer | No | No | Yes | No |

## Canary Deployments

```toml
[[service]]
name = "api"
image = "myapp:v2"

[service.deploy]
strategy = "canary"
canary_weight = 20    # 20% traffic to new version
```

Flow:
1. `orca deploy` → starts canary instances alongside stable
2. Proxy splits traffic: 80% stable, 20% canary
3. Monitor metrics/logs to verify canary is healthy
4. `orca promote api` → shifts 100% to canary, removes old

## Git Push Deploy (Webhooks)

```bash
# Register webhook
orca webhooks add --repo org/myapp --service myapp --branch main

# Configure in GitHub/Gitea:
# URL: https://master:6880/api/v1/webhooks/github
# Secret: <webhook secret>
# Events: Push
```

On push to matching branch → auto-redeploy the service.

## Persistent State

Services survive server restarts. State persisted to `~/.orca/cluster.db` (redb).
- Deploy → config saved to store
- Stop → containers stopped, config kept
- Restart → configs loaded, containers recreated

## Operational Patterns

### Deploy new service
1. `mkdir -p ~/orca/services/myapp`
2. Write `service.toml`
3. Set secrets if needed
4. `orca deploy`
5. Verify: `orca status` or `curl https://domain`

### Rolling update (zero downtime)
Change image in `service.toml` → `orca deploy`. Orca starts new → waits healthy → stops old.

### Cross-service communication
Same project = shared network. Use container aliases:
```toml
[[service]]
name = "myapp-db"
aliases = ["db"]
# Other services reach it at "db:5432"
```
Cross-project: set `internal = true` on both services.

### Deploy to specific node
```toml
[service.placement]
node = "vmd169252"   # Hostname or address substring
```

## Self-Healing

| Scenario | Detection | Action | Time |
|----------|-----------|--------|------|
| Container crash | Watchdog | Restart | ~30s |
| Health check fail | Health checker | Restart after threshold | ~30s |
| Dead route | Watchdog | Remove from table | ~30s |
| Agent disconnect | Heartbeat | Exponential backoff retry | 5-60s |
| Duplicate deploy | Reconciler | Skip (idempotent) | Instant |

## Troubleshooting

```
Service unreachable?
├─ orca status → "stopped"? → check orca logs <service>
│  ├─ OOM → increase resources.memory
│  ├─ "connection refused" → dependency not ready
│  └─ App error → fix code, redeploy
├─ orca status → "running" but 404?
│  ├─ DNS not pointing to master → fix A record
│  └─ Route not registered → redeploy
├─ TLS error → ACME failed
│  ├─ DNS doesn't resolve → fix DNS first
│  └─ Port 80 blocked → check firewall
└─ Node missing from cluster?
   ├─ Agent not running → restart agent
   └─ Firewall blocking 6880 → open port
```

## AI Agent Automation

### Deploy + verify pattern
```python
# Deploy
resp = POST("/api/v1/deploy", json={"services": [config]})
assert "my-app" in resp["deployed"]

# Wait for healthy (max 60s)
for _ in range(12):
    status = GET("/api/v1/status")
    svc = next(s for s in status["services"] if s["name"] == "my-app")
    if svc["status"] == "running":
        break
    sleep(5)

# Verify HTTPS
assert GET(f"https://{domain}").status_code == 200
```

### Incident response pattern
```python
status = GET("/api/v1/status")
for svc in status["services"]:
    if svc["status"] != "running":
        logs = GET(f"/api/v1/services/{svc['name']}/logs?tail=50")
        if "OOMKilled" in logs:
            # Increase memory and redeploy
            pass
        elif svc["running_replicas"] == 0:
            # Full outage — redeploy
            POST("/api/v1/deploy", json={"services": [updated_config]})
        # else: partial — watchdog handles it within 30s
```

### Monitoring
```yaml
# Prometheus scrape
scrape_configs:
  - job_name: 'orca'
    static_configs:
      - targets: ['master:6880']
    metrics_path: '/metrics'
```

Key metrics: `orca_services_total`, `orca_instances_total{service,project,status}`, `orca_nodes_total`
