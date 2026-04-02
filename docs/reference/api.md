# REST API Reference

Base URL: `http://<master>:6880`

All endpoints except `/api/v1/health` and `/metrics` require authentication via bearer token:

```
Authorization: Bearer <token>
```

## Endpoints

### Health & Metrics

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `GET` | `/api/v1/health` | No | Cluster health check |
| `GET` | `/metrics` | No | Prometheus metrics |

### Services

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `POST` | `/api/v1/deploy` | Yes | Deploy services |
| `GET` | `/api/v1/status?project=` | Yes | Service status |
| `POST` | `/api/v1/services/{name}/scale` | Yes | Scale replicas |
| `DELETE` | `/api/v1/services/{name}` | Yes | Stop service |
| `DELETE` | `/api/v1/projects/{name}` | Yes | Stop all in project |
| `POST` | `/api/v1/stop` | Yes | Stop ALL services |
| `GET` | `/api/v1/services/{name}/logs` | Yes | Container logs |
| `POST` | `/api/v1/services/{name}/promote` | Yes | Promote canary |
| `POST` | `/api/v1/services/{name}/rollback` | Yes | Rollback deploy |

### Cluster

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `GET` | `/api/v1/cluster/info` | Yes | Node list and cluster info |
| `POST` | `/api/v1/cluster/nodes/{id}/drain` | Yes | Drain a node |
| `POST` | `/api/v1/cluster/nodes/{id}/undrain` | Yes | Undrain a node |

### Webhooks

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `POST` | `/api/v1/webhooks/github` | Signature | Git push webhook |
| `GET` | `/api/v1/webhooks` | Yes | List registered webhooks |

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

Returns plaintext log output.

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

## Webhook Verification

GitHub/Gitea webhooks are verified using HMAC-SHA256. The webhook secret is configured when registering the webhook via `orca webhooks add`.

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
| `500` | Internal server error |
