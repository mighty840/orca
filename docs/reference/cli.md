# CLI Reference

All commands are subcommands of the `orca` binary.

## Cluster

### `orca server`
Start the control plane, agent, and proxy on this node.

```bash
orca server              # Foreground
orca server &            # Background
```

### `orca join`
Join this node to an existing cluster.

```bash
orca join 10.0.0.1       # Join by leader address
```

### `orca nodes`
List cluster nodes with status and resource usage.

```bash
orca nodes
```

### `orca tui`
Launch the terminal dashboard.

```bash
orca tui
```

### `orca update`
Self-update the orca binary.

```bash
orca update
```

## Services

### `orca deploy`
Deploy services from `services/*/service.toml`.

```bash
orca deploy              # Deploy all discovered services
```

### `orca status`
Show service status, replicas, and health.

```bash
orca status
orca status --project frontend
```

### `orca logs`
Stream logs from a service.

```bash
orca logs api
orca logs api --tail 100
```

### `orca scale`
Scale a service to N replicas.

```bash
orca scale api 5
```

### `orca stop`
Stop a service (config is retained).

```bash
orca stop api
```

### `orca promote`
Promote a canary deployment to stable.

```bash
orca promote api
```

### `orca rollback`
Rollback to the previous deployment.

```bash
orca rollback api
```

### `orca exec`
Execute a command inside a running container.

```bash
orca exec api -- sh
orca exec api -- cat /etc/hostname
```

## Databases

### `orca db create`
Create a managed database with auto-generated credentials.

```bash
orca db create postgres mydb
orca db create redis cache
orca db create mysql appdb
orca db create mongodb docs
```

### `orca db list`
List database services.

```bash
orca db list
```

## Secrets

### `orca secrets set`
Store an encrypted secret.

```bash
orca secrets set DB_PASS "s3cret"
```

### `orca secrets list`
List secret keys (values are never displayed).

```bash
orca secrets list
```

### `orca secrets import`
Bulk import secrets from an `.env` file.

```bash
orca secrets import -f .env
```

## Operations

### `orca backup`
Backup volumes and configs.

```bash
orca backup create
orca backup all          # Backup everything
orca backup list         # List backups
```

### `orca cleanup`
Prune unused Docker resources (images, containers, volumes).

```bash
orca cleanup
```

### `orca token`
Manage API tokens.

```bash
orca token create --name ci --role deployer
orca token list
```

### `orca webhooks`
Manage git push deploy webhooks.

```bash
orca webhooks                                      # List
orca webhooks add --repo org/app --service app --branch main
```

## AI

### `orca ask`
Ask the AI assistant a question with full cluster context.

```bash
orca ask "why is the API returning 500s?"
orca ask "which service is using the most memory?"
```

### `orca generate`
Generate service configuration from natural language.

```bash
orca generate "deploy redis with 2GB storage"
```
