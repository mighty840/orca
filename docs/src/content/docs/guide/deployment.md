# Deployment

## Rolling Updates

The default strategy. Orca starts new containers, waits for health checks, then stops old ones -- zero downtime:

```toml
[service.deploy]
strategy = "rolling"
max_unavailable = 1
```

Update an image in `service.toml` and redeploy:

```bash
orca deploy
```

Orca handles the rest: pull image, start new replicas, verify health, drain old replicas.

## Canary Deployments

Split traffic between stable and canary versions:

```toml
[service.deploy]
strategy = "canary"
canary_weight = 20        # 20% traffic to new version
```

### Canary Workflow

1. **Deploy** -- `orca deploy` starts canary instances alongside stable
2. **Observe** -- Proxy splits traffic (80% stable, 20% canary)
3. **Promote** -- `orca promote api` shifts 100% to canary, removes old
4. **Or rollback** -- `orca rollback api` removes canary, keeps stable

```bash
orca deploy                # Start canary
orca status                # Watch canary health
orca logs api              # Check for errors
orca promote api           # Ship it
```

## Rollback

Every deploy is versioned. Roll back to the previous config with:

```bash
orca rollback <service>
```

State is persisted in `~/.orca/cluster.db` (redb), so deploy history survives server restarts.

## Build from Source

Orca can build images from a Git repository:

```toml
[service.build]
repo = "git@github.com:org/repo.git"
branch = "main"
dockerfile = "Dockerfile"
context = "."
```

## Git Push Deploy

### Webhooks

Register a webhook to auto-deploy on push:

```bash
orca webhooks add --repo org/myapp --service myapp --branch main
```

Configure in GitHub/Gitea:
- **URL:** `https://<master>:6880/api/v1/webhooks/github`
- **Secret:** your webhook secret
- **Events:** Push

On push to the matching branch, Orca automatically redeploys the service.

### Managing Webhooks

```bash
orca webhooks              # List registered webhooks
```

::: tip
Webhook payloads are verified with HMAC-SHA256 signatures to prevent unauthorized deploys.
:::

## TLS Certificates

### Auto-TLS (ACME)

Set `acme_email` in `cluster.toml` and Orca handles Let's Encrypt certificates automatically:

```toml
[cluster]
acme_email = "ops@example.com"
```

### Custom Certificates

For BYO certs, place them in the configured cert directory and reference them in the service config.

::: warning
Port 80 must be accessible from the internet for ACME HTTP-01 challenges to succeed.
:::

## Persistent State

Services survive server restarts:
- **Deploy** -- config saved to redb store
- **Stop** -- containers stopped, config retained
- **Server restart** -- configs loaded, containers recreated automatically
