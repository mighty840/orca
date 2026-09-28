# REST API Reference

Base URL: `http://<master>:6880`

All endpoints except `/api/v1/health` and `/api/v1/webhooks/github` require a bearer token:

```
Authorization: Bearer <token>
```

Each route needs a permission. `viewer` holds `status`, `logs` and
`cluster_info`; `deployer` adds `deploy`, `stop`, `scale` and `rollback`;
`admin` holds everything. `secrets` and the routes marked **admin** are
admin-only, and so is any route not listed here.

## Endpoints

### Health & Metrics

| Method | Path | Permission | Description |
|--------|------|------------|-------------|
| `GET` | `/api/v1/health` | none | Cluster health check |
| `GET` | `/metrics` | `status` | Prometheus metrics |

### Services

| Method | Path | Permission | Description |
|--------|------|------------|-------------|
| `POST` | `/api/v1/deploy` | `deploy` | Deploy services |
| `GET` | `/api/v1/status?project=` | `status` | Service status |
| `GET` | `/api/v1/services/{name}/logs` | `logs` | Container logs (`tail`, `follow`) |
| `GET` | `/api/v1/services/{name}/exec` | admin | Interactive exec (WebSocket) |
| `POST` | `/api/v1/services/{name}/scale` | `scale` | Scale replicas |
| `POST` | `/api/v1/services/{name}/start` | `deploy` | Start (resume) a paused service |
| `POST` | `/api/v1/services/{name}/redeploy` | `deploy` | Pull the image and recreate |
| `POST` | `/api/v1/services/{name}/promote` | `deploy` | Promote canary |
| `POST` | `/api/v1/services/{name}/rollback` | `rollback` | Rollback deploy |
| `DELETE` | `/api/v1/services/{name}` | `stop` | Stop service |
| `DELETE` | `/api/v1/projects/{project}` | `stop` | Stop all in project |
| `POST` | `/api/v1/stop` | `stop` | Stop ALL services |

### Cluster

| Method | Path | Permission | Description |
|--------|------|------------|-------------|
| `GET` | `/api/v1/cluster/info` | `cluster_info` | Node list and cluster info |
| `POST` | `/api/v1/cluster/nodes/{id}/drain` | admin | Drain a node |
| `POST` | `/api/v1/cluster/nodes/{id}/undrain` | admin | Undrain a node |
| `GET` | `/api/v1/cluster/networks` | `status` | Per-node Docker networks and public routes |
| `GET` | `/api/v1/cluster/backups` | `status` | Backup status per node |
| `POST` | `/api/v1/cluster/backups/trigger` | admin | Trigger a backup |
| `GET` | `/api/v1/ws/agent` | admin token | Agent channel (WebSocket, token in `Authorization`) |

### Cluster token rotation

| Method | Path | Permission | Description |
|--------|------|------------|-------------|
| `POST` | `/api/v1/cluster/token/rotate` | admin | Start a rotation |
| `GET` | `/api/v1/cluster/token/rotation` | admin | Rotation progress |
| `POST` | `/api/v1/cluster/token/rotation/finish` | admin | Retire the old token |

### Secrets

| Method | Path | Permission | Description |
|--------|------|------------|-------------|
| `GET` | `/api/v1/secrets` | `secrets` | List keys (never values) |
| `GET` | `/api/v1/secrets/usage` | `secrets` | Keys with the services that reference them |
| `POST` | `/api/v1/secrets/{key}` | `secrets` | Create or update a secret |
| `DELETE` | `/api/v1/secrets/{key}` | `secrets` | Remove a secret |

### Webhooks

| Method | Path | Permission | Description |
|--------|------|------------|-------------|
| `POST` | `/api/v1/webhooks/github` | signature | Git push webhook |
| `GET` | `/api/v1/webhooks` | `status` | List registered webhooks |
| `POST` | `/api/v1/webhooks` | admin | Register a webhook |
| `DELETE` | `/api/v1/webhooks/{id}` | admin | Remove a webhook |
| `GET` | `/api/v1/webhooks/{id}/invocations` | `status` | Recent invocations |

### AI and alerts

| Method | Path | Permission | Description |
|--------|------|------------|-------------|
| `POST` | `/api/v1/ask` | `status` | Ask the AI assistant |
| `GET` | `/api/v1/alerts` | `status` | List alert conversations |
| `GET` | `/api/v1/alerts/{id}` | `status` | One conversation |
| `POST` | `/api/v1/alerts/{id}/reply` | admin | Reply to the AI |
| `POST` | `/api/v1/alerts/{id}/dismiss` | admin | Dismiss |
| `POST` | `/api/v1/alerts/{id}/resolve` | admin | Resolve |

## Deploy Payload

```json
POST /api/v1/deploy
Content-Type: application/json

{
  "services": [{
    "name": "my-app",
    "project": "frontend",
    "image": "nginx:alpine",
    "replicas": 2,
    "port": 80,
    "domain": "app.example.com",
    "health": "/healthz",
    "env": {
      "KEY": "${secrets.KEY}"
    },
    "resources": {
      "memory": "512Mi",
      "cpu": 1.0
    },
    "internal": true,
    "placement": {
      "node": "worker-1"
    }
  }]
}
```

### Response

```json
{
  "deployed": ["my-app"],
  "errors": []
}
```

| Status | Meaning |
|--------|---------|
| `200` | Every service deployed |
| `206` | Some services deployed, the rest are listed in `errors` |
| `400` | Validation failed; nothing deployed |
| `500` | No service deployed |

## Scale Payload

```json
POST /api/v1/services/my-app/scale
Content-Type: application/json

{
  "replicas": 5
}
```

## Log Query

```
GET /api/v1/services/my-app/logs?tail=100
```

Returns plaintext log output. `tail` defaults to 100; `follow=true` streams new
lines for services on the master.

## Status Response

```json
GET /api/v1/status

{
  "services": [
    {
      "name": "my-app",
      "project": "frontend",
      "status": "running",
      "replicas": 2,
      "running_replicas": 2,
      "image": "nginx:alpine",
      "domain": "app.example.com"
    }
  ]
}
```

## Token Rotation

`POST /api/v1/cluster/token/rotate` takes no body. `POST
/api/v1/cluster/token/rotation/finish` takes an optional body:

```json
{ "force": false }
```

All three routes return the rotation status. No response contains a token:

```json
{
  "in_progress": true,
  "token_file": "/root/.orca/cluster.token",
  "nodes": [
    { "node_id": 7, "address": "10.0.0.7:6880", "rotated": true, "persisted": true, "detail": null }
  ]
}
```

`rotated` means the agent uses the new token now; `persisted` means its next
start will too. `detail` says where it saved the token, or what must be fixed
by hand. Starting a rotation while one runs, finishing when none runs, or
finishing while an agent is not `rotated` and `persisted` (without `force`)
returns `409` with the reason as plain text.

## Webhook Verification

GitHub/Gitea webhooks are verified using HMAC-SHA256. The webhook secret is
configured when registering the webhook via `orca webhooks add`. Registering a
webhook without a secret returns `400`, and pushes without a valid signature
are rejected.

## Error Responses

All errors return JSON:

```json
{
  "error": "service not found: my-app"
}
```

| Status | Meaning |
|--------|---------|
| `401` | Missing or invalid token |
| `403` | Insufficient role permissions |
| `404` | Service or resource not found |
| `409` | Token rotation refused (see above) |
| `500` | Internal server error |
| `503` | Target agent node is offline (webhook redeploy to disconnected node) |
