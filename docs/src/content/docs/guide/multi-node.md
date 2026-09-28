# Multi-Node Clustering

Orca scales from a single server to a 20-node cluster with no config rewrites.

## Architecture

```
┌─────────────────────────────┐
│       Control Plane         │
│  Raft consensus (openraft)  │
│  Scheduler (bin-packing)    │
│  API server (axum)          │
└──────────┬──────────────────┘
           │ WebSocket (bidirectional)
    ┌──────┼──────┐
    ▼      ▼      ▼
 Node 1  Node 2  Node 3
```

- **Raft consensus** via `openraft` with `redb` storage -- no etcd dependency
- **Bin-packing scheduler** with GPU awareness and Wasm preference
- **Bidirectional WebSocket streaming** between agents and master -- replaces HTTP heartbeat polling. Agents maintain a persistent WS connection for real-time state sync, command dispatch, and log streaming.
- **Per-container CPU and memory stats for remote services** -- agents stream live resource usage over the WS heartbeat, so `orca status` and the TUI show the same metrics for containers on agent nodes as for those on the master.
- Reads served by any node, writes go through the Raft leader

## Adding Nodes

Declare nodes in `cluster.toml`:

```toml
[[node]]
address = "10.0.0.1"
labels = { zone = "eu-1", role = "general" }

[[node]]
address = "10.0.0.2"
labels = { zone = "eu-1", role = "gpu" }
```

On each worker node, join the cluster:

```bash
# One-time join (foreground). The token comes from ORCA_TOKEN,
# or from ~/.orca/cluster.token when that is unset:
ORCA_TOKEN=<cluster-token> orca join <leader-ip>:6880

# Or as a daemon:
ORCA_TOKEN=<cluster-token> orca join <leader-ip>:6880 --daemon
```

`--token <value>` still works, but the value is visible in the process list;
prefer the environment variable or the token file. Agents must use the cluster
token or another admin token.

### Systemd setup (recommended)

Install orca as a systemd service on each node for auto-start on boot:

```bash
# Master node:
orca install-service
sudo systemctl start orca

# Agent/worker nodes:
orca install-service --leader <leader-ip>:6880
sudo systemctl start orca-agent
```

The `--token` is auto-read from `~/.orca/cluster.token`. Pass
`--token <value>` explicitly if the file doesn't exist. The agent unit loads
the token from `~/.orca/agent.env` (mode 0600) via `EnvironmentFile=`, so it
never appears in `ExecStart`.

Each agent node runs a local reverse proxy (HTTP :80 + HTTPS :443)
for domains assigned to services placed on that node. The systemd unit
includes `AmbientCapabilities=CAP_NET_BIND_SERVICE` so no `setcap` is
needed.

The agent's proxy starts before it registers with the master, since its
routes come from local container labels. A wrong token, an unreachable master
or a late mesh interface no longer takes the node's sites down: registration
and the control session retry in the background for as long as needed, and
the agent logs that it is still serving its local routes.

### Updating nodes

```bash
orca update                          # Downloads latest binary
sudo systemctl restart orca          # Master
sudo systemctl restart orca-agent    # Agent nodes
```

Upgrade the master before any agent. Restarting orca leaves containers
running; the restarted process re-attaches to them. See
[Upgrading to 0.3.0](/guide/upgrading).

The first node to run `orca server` becomes the leader.

### Reconnect and reconciliation

When an agent loses its WebSocket connection (network blip, master restart,
etc.), it reconnects automatically with exponential backoff. On reconnect,
the master:

1. Creates a `remote-{node_id}` placeholder instance for every service placed on that node.
2. Sends a `Reconcile` message with all expected services, except paused ones. A spec fingerprint lets the agent apply changes it missed while disconnected.
3. The agent starts any missing containers and sends `DeployResult` for each, which the master uses to update the service status from `Stopped` to `Running`.

The watchdog never triggers local reconciliation for services with a `placement.node` constraint — those are exclusively managed through the agent WS channel.

This means you can restart the master, upgrade it, or recover from a network
partition -- agents will self-heal without manual intervention.

### Control-session liveness

A node is *reachable* exactly when its control session is alive, and a
session is alive exactly when it carries traffic (#131). Agents heartbeat
every 5s and the master pings every 30s, and both sides enforce
**read-idle deadlines** on top of that:

- The master closes a session that has been silent for
  `[deploy].ws_idle_timeout_secs` (default 30s) — the node immediately
  stops being reachable and its remote service state is dropped rather
  than served stale.
- The agent tears down its side after 90s of silence and reconnects with
  backoff.

This catches *half-open* connections — the peer vanished without closing
the TCP stream (NAT timeout, VM freeze, cable pull) — which previously
left a zombie session that looked healthy while every deploy timed out.
A missed deploy acknowledgment also kills the session outright: the next
deploy fails fast with `node is unreachable until it rejoins` instead of
re-timing-out, and the node returns as soon as the agent's reconnect
lands. No manual agent restart is ever required.

### Webhook behaviour when an agent is offline

If a git push webhook fires while the target agent is disconnected, the API
returns `503 Service Unavailable` (not 500). Retry once the agent reconnects,
or use `orca redeploy <service>` manually.

## Rotating the Cluster Token

`orca token rotate` replaces the cluster token without locking agents out.
Run it on the master. Agents need 0.3.0-rc.5 or later to take part.

1. **Start** the rotation:

   ```bash
   orca token rotate
   ```

   The master writes a new token to `~/.orca/cluster.token`, keeps the old one
   in `cluster.token.previous`, and accepts both, across a restart too. It
   pushes the new token to every connected agent. Each agent switches in
   memory, without a restart, and saves the token where its next start reads
   it: `~/.orca/cluster.token`, or an `agent.env` that holds the old token.
   Offline agents get it when they reconnect.

2. **Update** everything else that uses the cluster token: CI, laptops,
   scripts. The TUI on the master re-reads the token file by itself.

3. **Check** progress:

   ```bash
   orca token rotate --status
   ```

   ```
   NODE             ADDRESS                      STATE
   7                10.0.0.7:6880                on the new token
   9                10.0.0.9:6880                new token in memory only: unit passes --token
   12               10.0.0.12:6880               waiting (offline, or an agent too old to rotate)
   ```

   - **in memory only**: the agent's unit passes `--token`, so its next start
     would use the old one. Remove `--token` from the unit and restart the
     agent; the new token is already in `~/.orca/cluster.token`.
   - **waiting**: the agent is offline or older than rc.5. Bring it online or
     upgrade it.

4. **Finish** once every agent is on the new token:

   ```bash
   orca token rotate --finish
   ```

   The old token stops working. `--finish` refuses, naming the agents, while
   any agent known to the rotation or registered now isn't on the new token
   for good. `--finish --force` retires it anyway; those agents are locked out
   at their next restart.

No step prints or logs a token. The same flow is in the TUI (view `8`) and
the API (`/api/v1/cluster/token/...`).

Rotation covers the cluster token in `~/.orca/cluster.token`. When
`cluster.toml` sets `api_tokens`, the master doesn't use that file and
`orca token rotate` refuses; rotate `api_tokens` and named `[[token]]` entries
by editing `cluster.toml`.

## Placement Constraints

Control where services run:

```toml
[service.placement]
node = "gpu-worker-1"             # Pin to specific node
labels = { zone = "eu-1" }        # Match by labels
```

## GPU Nodes

Declare GPU hardware so the scheduler can place GPU workloads:

```toml
[[node]]
address = "10.0.0.3"
labels = { role = "gpu" }

[[node.gpus]]
vendor = "nvidia"
count = 2
model = "A100"
```

## Drain Mode

Remove a node from scheduling without stopping the cluster:

```bash
# List nodes and their ids
orca nodes

# TUI: view 2 (Nodes), select a node, x drains (asks y/N), u undrains.
# Or :drain <id> / :undrain <id> in command mode.

# Via API (admin token)
POST /api/v1/cluster/nodes/{id}/drain
POST /api/v1/cluster/nodes/{id}/undrain
```

Draining a node migrates its workloads to other nodes before taking it offline.

## Cross-Provider Networking

Orca nodes can span multiple cloud providers using [NetBird](https://netbird.io) for WireGuard mesh networking:

```toml
[network]
provider = "netbird"
setup_key = "${secrets.netbird_key}"
```

```
┌─ Hetzner ────┐    ┌─ AWS ────────┐    ┌─ Home Lab ───┐
│  Node 1      │◄──►│  Node 2      │◄──►│  Node 3      │
│  orca agent  │    │  orca agent  │    │  orca agent  │
└──────────────┘    └──────────────┘    └──────────────┘
        └────── WireGuard encrypted tunnel ──────┘
```

No manual VPN setup, firewall rules, or port forwarding required.

## Scheduler Algorithm

```
1. Filter nodes by constraints (memory, CPU, labels, affinity)
2. Score by: available resources, image cache, locality
3. Prefer Wasm runtime when workload supports it
4. Spread replicas across failure domains
```

Wasm workloads can be colocated -- hundreds of instances on one node at ~1-5MB each.
