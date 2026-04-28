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
# One-time join (foreground):
orca join <leader-ip>:6880 --token <cluster-token>

# Or as a daemon:
orca join <leader-ip>:6880 --token <cluster-token> --daemon
```

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
`--token <value>` explicitly if the file doesn't exist.

Each agent node runs a local reverse proxy (HTTP :80 + HTTPS :443)
for domains assigned to services placed on that node. The systemd unit
includes `AmbientCapabilities=CAP_NET_BIND_SERVICE` so no `setcap` is
needed.

### Updating nodes

```bash
orca update                          # Downloads latest binary
sudo systemctl restart orca          # Master
sudo systemctl restart orca-agent    # Agent nodes
```

The first node to run `orca server` becomes the leader.

### Reconnect and reconciliation

When an agent loses its WebSocket connection (network blip, master restart,
etc.), it reconnects automatically with exponential backoff. On reconnect,
the master:

1. Creates a `remote-{node_id}` placeholder instance for every service placed on that node.
2. Sends a `Reconcile` message with all expected services.
3. The agent starts any missing containers and sends `DeployResult` for each, which the master uses to update the service status from `Stopped` to `Running`.

The watchdog never triggers local reconciliation for services with a `placement.node` constraint — those are exclusively managed through the agent WS channel.

This means you can restart the master, upgrade it, or recover from a network
partition -- agents will self-heal without manual intervention.

### Webhook behaviour when an agent is offline

If a git push webhook fires while the target agent is disconnected, the API
returns `503 Service Unavailable` (not 500). Retry once the agent reconnects,
or use `orca redeploy <service>` manually.

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
# Via CLI
orca nodes                           # List nodes

# Via API
POST /api/v1/cluster/nodes/{id}/drain
POST /api/v1/cluster/nodes/{id}/undrain
```

Draining a node migrates its workloads to other nodes before taking it offline.

## Cross-Provider Networking

Orca nodes can span multiple cloud providers using [NetBird](https://netbird.io) for WireGuard mesh networking:

```toml
[cluster.network]
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
