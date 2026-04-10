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
the master replays the full desired state for that agent. The agent's
reconciler then converges: it starts missing containers, stops unknown ones,
and updates any whose spec has drifted.

This means you can restart the master, upgrade it, or recover from a network
partition -- agents will self-heal without manual intervention.

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
