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

The terminal dashboard provides a real-time cluster overview:

```bash
orca tui
```

```
┌─orca ──────────────────────────────────────────────┐
│ Cluster: prod    Nodes: 3/3    Services: 6/6       │
│ CPU: ████░░░░ 48%     Mem: █████░░░ 62%            │
├────────────────────────────────────────────────────┤
│ Services                │ Logs: api                 │
│  ● api        3/3       │ 12:04:01 GET /health 200  │
│  ● postgres   1/1       │ 12:04:02 POST /v1/… 201   │
│  ● redis      1/1       │ 12:04:03 GET /health 200  │
├─────────────────────────┴─────────────────────────┤
│ [d]eploy  [s]cale  [l]ogs  [r]ollback  [q]uit     │
└────────────────────────────────────────────────────┘
```

Keybindings: `j/k` navigate, `Enter` detail view, `l` logs, `d` deploy, `/` search, `?` help.

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
