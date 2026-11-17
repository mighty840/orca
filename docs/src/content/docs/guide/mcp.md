# MCP Server

`orca mcp` serves your cluster to an AI client (Claude Desktop, an IDE, an
agent) over the [Model Context Protocol](https://modelcontextprotocol.io).
The client can then read services, logs, nodes, alerts and backups, and use
prompts that gather the data for a diagnosis.

It is **read-only**: no tool deploys, stops, scales or changes anything. A
model can propose a command; you run it.

## Setup

Create a viewer token on the master, and give it to the client:

```bash
orca token create --name mcp --role viewer
```

Then configure the client to start `orca mcp`. For Claude Desktop
(`claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "orca": {
      "command": "orca",
      "args": ["mcp", "--api", "http://master:6880"],
      "env": { "ORCA_TOKEN": "<the viewer token>" }
    }
  }
}
```

Pass the token in `ORCA_TOKEN` rather than `--token`: command-line arguments
are visible to every user on the machine. Without either, `orca mcp` reads
`~/.orca/cluster.token`. The
cluster token (and any legacy `api_tokens` entry) is **refused**: it is
admin, and the cluster token is also what agents join with. Pass
`--allow-cluster-token` to use it anyway.

Every call is an ordinary API request made with that token, so the master's
role checks apply: a viewer is offered only what a viewer may read, and the
secrets inventory needs an admin token. These are reads, so they are not in
the [audit log](/reference/api) and not counted by `[api_rate_limit]`, which
cover changes only.

`orca mcp` limits its own reads instead: 60 API calls per minute by default
(a burst of 60, then one per second), for tools, resources and prompts
together; a prompt makes two or three. Over the limit a tool returns an
error saying when to retry, and the call never reaches the master. Change
it with `--reads-per-minute`.

## Remote clients: `/mcp` on the master

The master can serve the same tools over Streamable HTTP at `/mcp`, for
clients that connect by URL instead of starting a command:

```toml
[mcp]
enabled = true
allowed_hosts = ["100.80.5.14"]   # Host headers to accept, besides loopback
                                  # and cluster.domain; a port is optional
reads_per_minute = 60             # per token, across its sessions
```

Point the client at `http://<master>:6880/mcp` with an
`Authorization: Bearer <token>` header. Each request is checked as its own
token, and so is every API call made for it, exactly as if the client had
called the API.

- `/mcp` is never served on a cluster with no API tokens, even when enabled.
- A request whose `Host` is not allowed is rejected (DNS-rebinding
  protection), and so is one from a browser page: a request carrying an
  `Origin` header is refused unless `[mcp] allowed_origins` lists it.
- Reads are limited per token (`reads_per_minute`), but, as reads, they are
  not in the audit log.
- Port 6880 is plain HTTP. Reach it over your mesh network, or publish it
  through orca's proxy with TLS.

## What the client sees

| Tool | Resource | Reads |
|------|----------|-------|
| `get_status` | `orca://services` | Every service: replicas, status, node, CPU, memory, last failure |
| `get_service` | `orca://services/{name}` | One service's status and configuration, env values masked |
| `get_history` | `orca://services/{name}/history` | Recorded deploys |
| `get_logs` | `orca://services/{name}/logs?tail=` | The last log lines (default 200, at most 2000) |
| `get_cluster` | `orca://cluster` | Nodes: CPU, memory, disk, network, drain, GPUs |
| `list_networks` | `orca://networks` | Networks per node and domain routes |
| `list_alerts`, `get_alert` | `orca://alerts`, `orca://alerts/{id}` | AI monitor alerts |
| `list_backups` | `orca://backups` | Backup status per node |
| `list_webhooks` | `orca://webhooks` | Git webhooks, never their secrets |
| `secrets_inventory` | `orca://secrets` | Secret names and where they are used, never values (admin) |
| `whoami` | `orca://whoami` | The token in use |

Prompts: `diagnose(service)`, `failing_services(project?)`,
`capacity_review()`, `explain_alert(id)`. They only gather data for the
client's model; orca makes no LLM call for them.

## What is protected

- Credentials in logs and failure messages (`password=`, bearer tokens,
  connection strings, long opaque strings) are masked before they reach the
  client, as in [AI Ops](/guide/ai-ops) prompts.
- Prompts mark logs and failure text as data from the cluster, not
  instructions, inside fences the data cannot close.
- Responses are capped at 256 KiB; long logs keep the newest lines.
- Logs go to stderr; stdout carries only the protocol.
