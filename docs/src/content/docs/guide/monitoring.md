# Monitoring

## Prometheus Metrics

Orca exposes a `/metrics` endpoint on the API port (default 6880):

`/metrics` needs a token with the `status` permission; a `viewer` token is
enough:

```yaml
# prometheus.yml
scrape_configs:
  - job_name: 'orca'
    static_configs:
      - targets: ['master:6880']
    metrics_path: '/metrics'
    authorization:
      type: Bearer
      credentials_file: /etc/prometheus/orca.token   # a viewer token
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
ORCA_TOKEN=<token> orca tui --api http://master.example.com:6880
```

Without `ORCA_TOKEN` the TUI reads `~/.orca/cluster.token`, and re-reads it
after it starts or finishes a token rotation, so it isn't locked out when the
old token is retired. `:sh` and `:exec` run the TUI's own binary with the same
`--api`, so a shell always opens on the cluster on screen.

Refreshes run in the background every 2 s, so a slow master doesn't freeze
keys or redraws. Errors stay in the footer for 10 s; a connection error clears
as soon as the master answers again.

### Views

| Key | View | Purpose |
| --- | --- | --- |
| `0` | Chat | Ask the cluster AI (the landing view) |
| `1` | Services | Grouped by project, CPU / memory sparklines on detail |
| `2` / `n` | Nodes | Addresses, labels, CPU / Mem / Disk / Net sparklines; drain and undrain |
| `3` | Secrets | Keys grouped by scope, with reference counts |
| `4` | Backups | Per-node backup status and snapshots |
| `5` | Webhooks | Registered push triggers and invocation history |
| `6` | Networks | Per-node Docker bridges and public-edge routes |
| `7` | Alerts | AI alert conversations |
| `8` | Token | Cluster-token rotation and each agent's progress |
| `?` | Help | Full key reference (`j` / `k` scroll) |
| `Esc` | Back | Pop the current view (on Services, first clear the filter) |

In Chat, `1`–`7` switch views while the input line is empty.

### Confirmations

Every action that stops, deletes, drains or replaces something asks `y/N`
first, and the prompt names its target: `x` on a service, secret or webhook,
`d` (redeploy), `x` on a node (drain), `s` / `f` / `F` in the Token view,
and the commands `:stop`, `:stop-project`, `:redeploy`, `:rollback`,
`:promote`, `:drain`, `:rm`, `:token-rotate` and `:token-finish`. `y`
confirms, any other key cancels. The prompt takes over the footer until it is
answered. Selections follow their row by name, not by position, so a service
appearing or disappearing above the cursor can't redirect an action.

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
| `x` | Stop service (y/N) |
| `u` | Start (resume) a stopped service |
| `d` | Redeploy: pull the image and recreate (y/N) |
| `r` | Refresh now |
| `/` | Filter by text |
| `:` | Command mode |

The same `s`, `x`, `u`, `d`, `l` keys work in the Detail view, whose info box
grows to show a service's whole failure message and whose log tail refreshes
like the Logs view.

On a narrow terminal the table drops columns in this order: runtime, project,
image, domain, node. Name, replicas and status always stay, and spare width
goes to the name.

The detail view's **memory sparkline is scaled against the service's
`resources.memory` limit** when configured. If no limit is set it falls
back to the node's total memory, so the sparkline always shows a real
percentage instead of auto-scaling to the sample peak.

### Filters

`/` filters the Services, Secrets, Webhooks and Alerts lists. Each list keeps
its own filter, and the title shows "n of m matching". Selection and every
action on the list (`Enter`, `e`, `x`, dismiss, resolve) use the filtered
rows. `Enter` keeps the filter and returns to the list; `Esc` while typing
clears it. On the Services view, `Esc` also clears a kept filter.

### Logs view

The Logs view follows the log live and keeps the last 5000 lines. Scrolling
up holds your place while new lines arrive. The master streams logs for its
own services; for a service on an agent it polls 200 lines every 2 s. The
title shows `[live]` or `[polling]`.

| Key | Action |
| --- | --- |
| `/` | Search; shows matching lines with their original line numbers |
| `Esc` | While typing: clear the search. Otherwise: back |
| `w` | Toggle word wrap |
| `PgUp` / `PgDn` | Scroll |

### Nodes view

Each node shows its address, labels, heartbeat age, and a strip of
**four sparklines**:

- **CPU %** scaled 0–100
- **Memory** scaled to the node's total RAM (`Mem 6.4/24 GiB`)
- **Disk** scaled to total disk across all mounts
- **Network** as a per-interval delta in KiB/s

`j` / `k` select a node; the selection stays on its node across refreshes.
`x` drains the selected node (y/N) and `u` undrains it. When not every node's
strip fits the terminal height, the view shows as many as fit around the
selected node.

A master heartbeat task samples `sysinfo` on the master itself every
2 s; joined nodes push their sample via the heartbeat body. Nodes with
no heartbeat for 60 s are automatically pruned from the cluster view.

### Secrets view

The TUI calls `GET /api/v1/secrets` (admin role only). Values are never
sent over the wire — only the key list.

| Key | Action |
| --- | --- |
| `Enter` | Services that reference the key |
| `a` | Add (opens `:set`) |
| `e` | Edit the selected key (opens `:set KEY`) |
| `x` | Delete (y/N) |
| `p` | Cycle scope: all, global, per-project, broken refs |

Or use command mode. The value is kept exactly as typed and shown as bullets
in the command bar:

```
:set KEYCLOAK_DB_PASSWORD <value>
:rm STALE_API_KEY
```

### Alerts view

`7` or `:alerts` lists AI alert conversations; `Enter` opens one. `a` toggles
resolved alerts, `d` dismisses, `R` resolves. In an open alert, `:reply <msg>`
answers the AI. Alerts are polled with the status while an alert view is open,
so replies appear without pressing `r`.

### Token view

`8` or `:token` shows whether a cluster-token rotation is running and each
agent's progress, refreshed every 2 s. `s` starts a rotation, `f` finishes it
and `F` forces the finish, each after y/N. A refused finish shows the
master's reason. See
[Rotating the cluster token](/guide/multi-node#rotating-the-cluster-token).

### Commands

| Command | Action |
| --- | --- |
| `:scale <svc> <n>` | Scale a service |
| `:stop <svc>` / `:start <svc>` | Stop (y/N) / resume a service |
| `:redeploy <svc>` | Pull the image and recreate (y/N) |
| `:rollback <svc>` | Roll back to the previous deploy (y/N) |
| `:promote <svc>` | Promote canary instances to stable (y/N) |
| `:stop-project <p>` | Stop a whole project (y/N) |
| `:logs <svc>` | Open a service's logs |
| `:filter <text>` / `:project <name>` | Filter services |
| `:chat`, `:services`, `:nodes`, `:secrets`, `:backups`, `:webhooks`, `:networks`, `:alerts`, `:token`, `:help` | Open a view |
| `:token-rotate` / `:token-finish [--force]` | Start or finish a rotation (y/N) |
| `:reply <msg>`, `:dismiss`, `:resolve` | Act on the open alert |
| `:webhook-add <repo> <branch> <svc> [--secret X] [--infra]` | Register a webhook |
| `:webhook-edit <repo> <branch> <svc> [flags]` | Update a webhook |
| `:webhook-rm <service>` | Remove a webhook |
| `:set <KEY> <value>` / `:rm <KEY>` | Set / remove (y/N) a secret |
| `:drain <id>` / `:undrain <id>` | Drain (y/N) / undrain a node |
| `:sh [svc]` | Interactive shell in the selected or named service |
| `:exec <svc> <cmd>` | Run a command in a service's container |
| `:q` | Quit |

`:start`, `:redeploy`, `:rollback` and `:promote` act on the selected service
when the name is left out.

### Header and footer

The header shows the cluster name, service counts, node count, uptime, and
the **orca version + git commit** of both the TUI and the master. When the
two differ the header prints both versions so you know one side is lagging.

The footer shows the current view's keys, for example on the Services view:

```
[Services] 28/29 svc | 0-8:views ↵:detail l:logs /filter s:scale x:stop u:start d:redeploy p:project c:collapse ?:help
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
