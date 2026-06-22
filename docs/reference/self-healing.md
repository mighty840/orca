# Self-Healing

Orca automatically detects and recovers from common failure scenarios without manual intervention.

## Recovery Matrix

| Scenario | Detection | Action | Recovery Time |
|----------|-----------|--------|---------------|
| Container crash | Watchdog (30s cycle) | Restart container | ~30s |
| Health check failure | Health checker | Restart after threshold | ~30s |
| Stale proxy route | Watchdog | Remove dead route | ~30s |
| Agent disconnect | Heartbeat | Exponential backoff retry | 5-60s |
| Duplicate deploy | Reconciler | Skip (idempotent) | Instant |
| Remote service at startup | Agent WS connect | Placeholder upsert + Reconcile | On reconnect |
| Orphan container (running, unregistered) | Adoption reconciler (30s) | Re-register into the registry | ~30s |
| Failed / crashing container | Agent heartbeat | Record reason + exit code + log tail | ~5s |

## Watchdog

The watchdog runs on a 30-second cycle and checks:

1. **Container state** -- are all expected containers running?
2. **Route validity** -- do proxy routes point to live containers?
3. **Resource cleanup** -- are there orphaned resources?

If a container is missing or stopped, the watchdog restarts it using the persisted config from `~/.orca/cluster.db`.

## Health Checker

Per-service health checks probe your application's readiness:

```toml
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
```

- **Readiness** -- determines if a container should receive traffic
- **Liveness** -- determines if a container should be restarted

After `failure_threshold` consecutive failures, the container is restarted.

## Stale Route Cleanup

When a container dies, its proxy route becomes stale. The watchdog detects routes pointing to non-existent containers and removes them from the routing table within one cycle (~30s).

## Agent Resilience

If a worker node loses connection to the control plane:

1. The agent retries with **exponential backoff** (5s, 10s, 20s, 40s, 60s max)
2. Workloads on the disconnected node **continue running** -- they don't stop
3. On reconnection, the agent reconciles state with the control plane
4. If the node is unreachable beyond the heartbeat timeout, the scheduler marks it unhealthy and migrates workloads

## Persistent State

All service configurations are persisted to `~/.orca/cluster.db` (redb). This means:

- Server restarts automatically recreate all containers
- Deploys are idempotent -- redeploying the same config is a no-op
- Rollback is always available from deploy history

## Orphan Adoption

If a container ends up running on an agent but missing from the master's
registry — e.g. a master restart mid-deploy, or a deploy whose completion ack
was missed — the adoption reconciler re-registers it automatically. Every
`adopt_interval_secs` (default 30) the master scans each agent for
`orca.managed` containers, and for any **running** one whose service it doesn't
know, it adopts the container: reconstructs the `ServiceConfig` from the
container's labels + image, registers a remote placeholder (so `orca status`,
`orca logs`, and `orca redeploy` work against it), and persists it so the
adoption survives a restart. Only running containers are adopted, and a service
the master already knows is never overwritten. Disable with
`adopt_orphans = false` in `[deploy]`.

## Failure Reasons

When a service is degraded or stopped, `orca status` shows **why** — no log
scraping required:

- **Deploy-time failures** are classified (`ImagePullError`, `AgentUnreachable`,
  `DeployTimeout`, `DeployError`) and carry the real underlying error.
- **Runtime crashes** are detected from agent heartbeats: the agent reports the
  container's exit code, restart count, and a tail of `docker logs`, and the
  master classifies them (`CrashLoopBackOff`, `Error`).

The reason appears inline in `orca status` (a `↳ reason: …` line), in the status
API (`last_failure`), and in the TUI detail pane — only while the service is
actually degraded, and it clears once the service deploys or reports healthy
again.

## Troubleshooting

```
Service unreachable?
├─ orca status --> "stopped"?
│  ├─ Check orca logs <service>
│  ├─ OOM? --> increase resources.memory
│  └─ App error? --> fix code, redeploy
├─ orca status --> "running" but 404?
│  ├─ DNS not pointing to master --> fix A record
│  └─ Route not registered --> redeploy
├─ TLS error?
│  ├─ DNS doesn't resolve --> fix DNS first
│  └─ Port 80 blocked --> check firewall
└─ Node missing from cluster?
   ├─ Agent not running --> restart agent
   └─ Firewall blocking 6880 --> open port
```
