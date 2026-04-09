# Monitoring

## Prometheus Metrics

Orca exposes a `/metrics` endpoint on the API port (default 6880):

```yaml
# prometheus.yml
scrape_configs:
  - job_name: 'orca'
    static_configs:
      - targets: ['master:6880']
    metrics_path: '/metrics'
```

### Key Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `orca_services_total` | Gauge | Total number of deployed services |
| `orca_instances_total` | Gauge | Running instances by service, project, status |
| `orca_nodes_total` | Gauge | Cluster node count |

## Container Stats

View resource usage per service:

```bash
orca status              # Overview with replica counts
orca logs <service>      # Stream logs
```

## Resource Limits

Set per-service resource constraints:

```toml
[service.resources]
memory = "512Mi"
cpu = 1.0

[service.resources.gpu]
count = 1
vendor = "nvidia"
vram_min = 24000
```

Services exceeding memory limits are OOM-killed and automatically restarted by the watchdog.

## TUI Dashboard

The terminal dashboard is a k9s-style full-screen view stack over the
control-plane API. Launch it with:

```bash
orca tui
```

Remote clusters work too — point `--api` at the master and set
`ORCA_TOKEN`:

```bash
ORCA_TOKEN=$(cat ~/.orca/cluster.token) orca tui --api http://master.example.com:6880
```

### Views

| Key | View | Purpose |
| --- | --- | --- |
| `1` | Services | Grouped by project, rolling CPU / memory sparkline on detail |
| `2` | Nodes | Node addresses, labels, CPU / Mem / Disk / Net sparklines per node |
| `3` | Secrets | List, set, and remove cluster secrets |
| `?` | Help | Full key reference |
| `Esc` | Back | Pop the current view off the stack |

### Services view

Services are grouped by project (collapsible). Each row shows name,
project, image, runtime, replicas, status, node, and domain.

| Key | Action |
| --- | --- |
| `j` / `k` or `↓` / `↑` | Next / previous service |
| `g` / `G` | Jump to top / bottom |
| `Enter` | Detail view (info panel + CPU/Mem sparklines + recent logs) |
| `l` | Full-screen logs |
| `c` | Collapse / expand the project of the selected service |
| `p` | Filter to the project of the selected service |
| `s` | Scale prompt |
| `x` | Stop service |
| `/` | Filter by text |
| `:` | Command mode (`:scale`, `:stop`, `:logs`, `:set KEY VAL`, `:rm KEY`) |

The detail view's **memory sparkline is scaled against the service's
`resources.memory` limit** when configured. If no limit is set it falls
back to the node's total memory, so the sparkline always shows a real
percentage instead of auto-scaling to the sample peak.

### Nodes view

Each node shows its address, labels, heartbeat age, and a strip of
**four sparklines**:

- **CPU %** scaled 0–100
- **Memory** scaled to the node's total RAM (`Mem 6.4/24 GiB`)
- **Disk** scaled to total disk across all mounts
- **Network** as a per-interval delta in KiB/s

A master heartbeat task samples `sysinfo` on the master itself every
2 s; joined nodes push their sample via the heartbeat body. Nodes with
no heartbeat for 60 s are automatically pruned from the cluster view.

### Secrets view

The TUI calls `GET /api/v1/secrets` (admin role only). Values are never
sent over the wire — only the key list. Use command mode to modify:

```
:set KEYCLOAK_DB_PASSWORD sup3rs3cret
:rm STALE_API_KEY
```

### Header footer

The header shows cluster name, running / total services, node count,
uptime, and the **orca version + git commit** of both the TUI and the
master. When the two differ the header prints both versions so you
know one side is lagging.

```
 orca ● | my-cluster | 28/29 running | 3 nodes | 02:14:33 | v0.2.0-rc.1-95210a0
```

Footer hints on the services view:

```
[Services]  28/29 svc  |  1-3:views ↵:detail /filter s:scale x:stop p:project c:collapse ?:help
```

## OpenTelemetry Integration

Push traces and metrics to an external observability platform:

```toml
[observability]
otlp_endpoint = "https://signoz.example.com"

[observability.alerts]
webhook = "https://hooks.slack.com/services/..."
email = "ops@example.com"
```

## Health Check Endpoints

Orca exposes a health endpoint for external monitoring:

```
GET /api/v1/health    # No auth required
```

For service-level health, see [Self-Healing](/reference/self-healing).
