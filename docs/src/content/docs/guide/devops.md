# DevOps Guide

A practical, opinionated playbook for running orca in production. Every recommendation here comes from real operational experience migrating a ~20-service cluster (keycloak, gitea, litellm, searxng, and friends) off other orchestrators. Every pitfall listed was learned the hard way.

If you're just getting started, read the [Getting Started](./getting-started) and [Configuration](./configuration) guides first. This page assumes you already have an `orca` binary and a host to run it on.

[[toc]]

## 1. GitOps with orca-infra

Orca services are defined declaratively as `service.toml` files. You should treat those files the way you treat any other infrastructure code: keep them in git, review changes, and roll forward from the repo rather than from a shell session.

### The orca-infra repo

Create a dedicated repo — we use `git.example.com/myorg/infra`. Layout:

```
orca-infra/
├── cluster.toml
├── services/
│   ├── keycloak/
│   │   ├── service.toml
│   │   └── config/
│   │       └── custom-theme/
│   │           └── ...theme files...
│   ├── gitea/
│   │   └── service.toml
│   ├── litellm/
│   │   ├── service.toml
│   │   └── config/
│   │       └── config.yaml
│   ├── searxng/
│   │   └── service.toml
│   ├── compliance-agent/
│   │   └── service.toml
│   └── my-app/
│       └── service.toml
└── README.md
```

Each service gets its own directory. The `config/` subdir, when present, holds files that the `service.toml` mounts into the container (themes, YAML configs, seed SQL, etc.).

### Syncing to the host

The orca server reads services from the `services/` directory **relative to its current working directory**. The simplest workflow:

```bash
# On the host, once:
git clone ssh://git@git.example.com:22222/myorg/infra.git ~/orca

# Install as a systemd service (recommended):
orca install-service                                    # master node
orca install-service --leader <master-ip>:6880          # agent nodes
sudo systemctl start orca       # or orca-agent

# Or start manually:
cd ~/orca && orca server -d
```

To roll out a change: commit in `orca-infra`, `git pull` on the host, `orca deploy`.

To update the binary on all nodes:

```bash
orca update                                 # downloads latest from GitHub
sudo systemctl restart orca                 # master
sudo systemctl restart orca-agent           # agent nodes
```

### A real example: Keycloak

`services/keycloak/service.toml` defines both the database and the app as separate `[[service]]` entries in a single file. `depends_on` controls boot order, secrets are externalized, and the theme directory is mounted read-only.

```toml
[[service]]
name = "keycloak-db"
image = "postgres:16-alpine"
runtime = "docker"

[service.env]
POSTGRES_DB = "keycloak"
POSTGRES_USER = "${secrets.KEYCLOAK_DB_USER}"
POSTGRES_PASSWORD = "${secrets.KEYCLOAK_DB_PASSWORD}"

[[service.volumes]]
source = "keycloak-db-data"
target = "/var/lib/postgresql/data"

[[service]]
name = "keycloak"
image = "quay.io/keycloak/keycloak:25.0"
runtime = "docker"
command = ["start", "--optimized", "--http-enabled=true", "--hostname-strict=false"]
domain = "auth.example.com"
port = 8080
depends_on = ["keycloak-db"]

[service.env]
KC_DB = "postgres"
KC_DB_URL = "jdbc:postgresql://keycloak-db:5432/keycloak"
KC_DB_USERNAME = "${secrets.KEYCLOAK_DB_USER}"
KC_DB_PASSWORD = "${secrets.KEYCLOAK_DB_PASSWORD}"
KC_HOSTNAME = "auth.example.com"
KC_PROXY_HEADERS = "xforwarded"
KEYCLOAK_ADMIN = "${secrets.KEYCLOAK_ADMIN_USERNAME}"
KEYCLOAK_ADMIN_PASSWORD = "${secrets.KEYCLOAK_BOOTSTRAP_PASSWORD}"

[[service.mounts]]
source = "./services/keycloak/config/custom-theme"
target = "/opt/keycloak/themes/custom-theme"
read_only = true

[service.liveness]
path = "/realms/master"
interval_secs = 30
timeout_secs = 10
failure_threshold = 3
initial_delay_secs = 120
```

A few things to note:

- `${secrets.KEYCLOAK_DB_PASSWORD}` references encrypted secrets — see [Secrets management](#_3-secrets-management).
- The mount source is a **relative path** starting with `./services/...`. That relative path is resolved against orca's current working directory, which is why running orca from the right directory matters (see pitfall below).
- `initial_delay_secs = 120` gives Keycloak ~90s of uninterrupted boot time before the first probe fires.

::: danger Pitfall: always run orca from the same working directory
orca looks for `services/` **relative to the current working directory**. If you run `orca server -d` from `~`, it will look for `~/services/` and find nothing. Worse, if you run `orca deploy keycloak` from `~` while the server is running in `~/orca`, the deploy command will fail to find the toml.

**Rule:** use `orca install-service` — it sets `WorkingDirectory=~/orca` in the systemd unit automatically. If running manually, always `cd ~/orca` first.
:::

## 2. cluster.toml

`cluster.toml` is the single source of truth for cluster-level configuration: cluster identity, default domain, ACME settings, and the backup schedule. It lives at the root of your orca-infra repo next to `services/`.

Here's what the cluster config looks like:

```toml
[cluster]
name = "my-cluster"
domain = "example.com"

[acme]
email = "ops@example.com"
directory = "https://acme-v02.api.letsencrypt.org/directory"

[proxy]
http_port = 80
https_port = 443

[backup]
enabled = true
schedule = "0 0 3 * * *"   # daily at 03:00
retention_days = 14

[[backup.targets]]
type = "local"
path = "/var/lib/orca/backups"

[[backup.targets]]
type = "s3"
endpoint = "https://nbg1.your-objectstorage.com"
bucket = "my-backups"
region = "nbg1"
access_key = "${secrets.HETZNER_S3_ACCESS_KEY}"
secret_key = "${secrets.HETZNER_S3_SECRET_KEY}"
```

::: warning Pitfall: cluster.toml is only read on startup
Changes to `cluster.toml` — including backup schedules, ACME email, and proxy ports — are loaded exactly once, when the orca server boots. Editing the file does nothing until you restart:

```bash
orca shutdown
cd ~/orca && orca server -d
```

Don't forget to `cd ~/orca` before restarting — otherwise orca will come back up pointing at the wrong (empty) services directory.
:::

## 3. Secrets management

Secrets are stored encrypted at rest with AES-256 and decrypted in-memory when a service starts. You reference them from `service.toml` env blocks with `${secrets.NAME}`.

### Setting secrets

```bash
cd ~/orca
orca secrets set KEYCLOAK_DB_USER keycloak
orca secrets set KEYCLOAK_DB_PASSWORD 'S0me-l0ng-rand0m-string'
orca secrets set KEYCLOAK_ADMIN_USERNAME admin
orca secrets set KEYCLOAK_BOOTSTRAP_PASSWORD 'an0ther-l0ng-string'
```

And in `service.toml`:

```toml
[service.env]
KC_DB_USERNAME = "${secrets.KEYCLOAK_DB_USER}"
KC_DB_PASSWORD = "${secrets.KEYCLOAK_DB_PASSWORD}"
KEYCLOAK_ADMIN = "${secrets.KEYCLOAK_ADMIN_USERNAME}"
KEYCLOAK_ADMIN_PASSWORD = "${secrets.KEYCLOAK_BOOTSTRAP_PASSWORD}"
```

### Listing and rotating

```bash
orca secrets list               # names only, never values
orca secrets set KEY new-value  # overwrite
orca secrets rm KEY
```

After rotating a secret, redeploy the consuming service: `orca deploy keycloak`.

::: danger Pitfall: secrets.json is written relative to cwd
`orca secrets set` writes the encrypted `secrets.json` to the **current working directory**, not to a fixed location under `~/.orca/`. This is a known wart and is being fixed in an upcoming release.

Until then: **always run `orca secrets set` from `~/orca`**. Otherwise, you'll end up with a `secrets.json` in `~` that the server (running from `~/orca`) never reads, and you'll spend an hour wondering why your env vars are blank.
:::

## 4. CI/CD with webhooks

There are two CI/CD patterns for orca. Pick one per service based on how much rollback discipline you need.

### Pattern 1: service-tagged image + webhook redeploy

Your CI builds an image, pushes `:latest` (and optionally `:<sha>` for debugging), then POSTs to an orca webhook. Orca force-pulls the `:latest` tag and restarts the service. Best for stateless apps where "forward fix" is the recovery strategy.

### Pattern 2: pinned SHA tag + GitOps PR

Your CI builds an image tagged `:<sha>` only, then opens a PR against `orca-infra` to bump the toml. A human merges. Orca picks up the change on `orca deploy`. Best for databases, auth services, anything where you want an auditable history and a one-click revert.

The rest of this section walks through Pattern 1 using **my-app**.

### Gitea Actions workflow

`.gitea/workflows/ci.yml` in the my-app repo:

```yaml
name: build-and-deploy

on:
  push:
    branches: [main]

jobs:
  build:
    runs-on: docker
    steps:
      - uses: actions/checkout@v4

      - name: Log in to registry
        run: |
          echo "${{ secrets.REGISTRY_PASSWORD }}" | \
            docker login registry.example.com \
              -u "${{ secrets.REGISTRY_USERNAME }}" --password-stdin

      - name: Build image
        run: |
          docker build \
            -t registry.example.com/my-app:latest \
            -t registry.example.com/my-app:${{ github.sha }} \
            .

      - name: Push image
        run: |
          docker push registry.example.com/my-app:latest
          docker push registry.example.com/my-app:${{ github.sha }}

      - name: Trigger orca webhook
        env:
          SECRET: ${{ secrets.ORCA_WEBHOOK_SECRET }}
        run: |
          BODY='{"repo":"myorg/my-app","ref":"refs/heads/main","sha":"${{ github.sha }}"}'
          SIG=$(printf '%s' "$BODY" | openssl dgst -sha256 -hmac "$SECRET" | awk '{print $2}')
          curl -fsSL -X POST https://orca.example.com/api/v1/webhooks/github \
            -H "Content-Type: application/json" \
            -H "X-Hub-Signature-256: sha256=$SIG" \
            -d "$BODY"
```

Required Gitea repo secrets:

- `REGISTRY_USERNAME` — service account for the private registry
- `REGISTRY_PASSWORD` — its password or token
- `ORCA_WEBHOOK_SECRET` — the HMAC key you registered with orca (next step)

### Registering the webhook with orca

Webhooks are registered via the orca REST API. You'll need your cluster admin token (written to `~/.orca/cluster.token` on first boot).

```bash
curl -X POST http://127.0.0.1:6880/api/v1/webhooks \
  -H "Authorization: Bearer $(cat ~/.orca/cluster.token)" \
  -H "Content-Type: application/json" \
  -d '{
    "repo": "myorg/my-app",
    "service_name": "my-app",
    "branch": "main",
    "secret": "the-same-value-as-ORCA_WEBHOOK_SECRET"
  }'
```

From then on, every push to `main` will:

1. Build and push `:latest` and `:<sha>` images
2. Call the webhook
3. Orca verifies the HMAC, force-pulls `:latest`, and restarts the service

::: tip :latest vs :sha pull behavior
Orca force-pulls `:latest` tags on every reconcile (this landed in main today). For `:<sha>` or any other immutable tag, orca skips the pull if the image already exists locally. This means Pattern 1 ("just bump latest") works as expected, and Pattern 2 ("bump the sha in toml") is efficient — no pointless re-pulls.
:::

::: tip Webhooks are persisted
As of v0.2.2, registered webhooks are saved to `~/.orca/webhooks.json` and survive restarts. You no longer need to re-register them after a reboot or `orca shutdown`.
:::

::: tip Manage webhooks from the TUI
`orca tui` → press `5` (or `:webhooks`) for a dashboard of every registered webhook with last-trigger metadata (status, commit, age). Press `Enter` on a row to see the last 10 invocations; `a` to add, `e` to edit, `x` to delete. The invocation history is an in-memory ring on the master — it resets on `orca shutdown`.
:::

### Infra webhook: GitOps auto-deploy

In addition to per-service webhooks, orca supports an **infra webhook** that
triggers a full `git pull` + `orca deploy` whenever you push to your
orca-infra repo. This is the simplest GitOps setup -- no CI runner required.

Register the infra webhook:

```bash
curl -X POST http://127.0.0.1:6880/api/v1/webhooks \
  -H "Authorization: Bearer $(cat ~/.orca/cluster.token)" \
  -H "Content-Type: application/json" \
  -d '{
    "repo": "myorg/orca-infra",
    "service_name": "__infra__",
    "branch": "main",
    "secret": "your-infra-webhook-secret"
  }'
```

Then add a webhook in your Git host (Gitea, GitHub, etc.) pointing at
`https://orca.example.com/api/v1/webhooks/github` with the same secret.

Now the workflow is:

1. Edit a `service.toml` in orca-infra, commit, push
2. Git host fires the webhook
3. Orca runs `git pull` in its working directory and redeploys changed services

### `orca deploy` vs `orca redeploy`

- **`orca deploy`** (no args) -- discovers all `services/*/service.toml` files and reconciles. Only containers whose spec has changed are recreated.
- **`orca deploy <service-name>`** -- same as above, but scoped to a single service. Useful for deploying one service without touching the rest.
- **`orca redeploy <service>`** -- force-pulls the container image and restarts the service, even if the spec hasn't changed. Use this when you've pushed a new `:latest` image and want to pick it up immediately.

::: tip When to use which
Use `orca deploy` for config changes (env vars, ports, domains, mounts).
Use `orca redeploy` for image-only updates where the tag hasn't changed (e.g., `:latest`).
:::

## 5. Private Docker registry pulls

Orca uses the host's `~/.docker/config.json` for registry authentication. Log in once on the host:

```bash
docker login registry.example.com
```

From then on, any `service.toml` that references `registry.example.com/foo:tag` will be pulled with those credentials.

### The chicken-and-egg problem

If your private registry is itself a service managed by orca (and routed through orca's proxy on :443), then pulling **any** image from that registry requires the orca proxy to be up, which requires the registry image to already be cached locally.

::: danger Pitfall: pre-pull the registry image
Before your very first `orca server -d` on a fresh host, manually pull the registry image:

```bash
docker pull registry:2   # or whatever image your registry service uses
```

Otherwise you get a classic bootstrap deadlock: orca can't start the registry because it can't pull the registry image, because the proxy that routes to the registry isn't up yet.

On subsequent boots this isn't a problem — the image is cached — but it bites you hard on first-time setup and on any host migration.
:::

## 6. Backups

Orca has a built-in backup scheduler configured via `cluster.toml`. It snapshots volumes and, optionally, runs per-service `pre_hook` commands (e.g. `pg_dump`) before snapshotting.

::: tip Backup dashboard in the TUI
`orca tui` → press `4` (or `:backups`) for a per-node table of backup status: hostname, role (master / agent), last-run age, snapshot count, total size, and the last result message. Press `b` on a row to trigger an immediate backup on that node (master row runs `orca backup all` as a subprocess; agent rows dispatch over WS). `Enter` drills into a per-snapshot listing with file inventory. The view shows local-disk snapshots only — S3 listing is tracked separately (#41).
:::

### cluster.toml backup config

```toml
[backup]
enabled = true
schedule = "0 0 3 * * *"   # six-field cron: sec min hour dom mon dow — daily at 03:00:00
retention_days = 14

[[backup.targets]]
type = "local"
path = "/var/lib/orca/backups"

[[backup.targets]]
type = "s3"
endpoint = "https://nbg1.your-objectstorage.com"
bucket = "my-backups"
region = "nbg1"
access_key = "${secrets.HETZNER_S3_ACCESS_KEY}"
secret_key = "${secrets.HETZNER_S3_SECRET_KEY}"
```

Hetzner Object Storage works as a drop-in S3 target — just point `endpoint` at the region-specific hostname and set `region` to match. Same pattern works for Backblaze B2, MinIO, Wasabi, etc.

::: info Prerequisite: rclone required for S3 targets
S3 uploads and downloads use [`rclone`](https://rclone.org) under the hood. Install it on **every node** (master and all agents) before enabling an S3 target:

```bash
# Debian/Ubuntu
sudo apt install rclone

# Or from rclone.org
curl https://rclone.org/install.sh | sudo bash
```

No rclone configuration file is needed — credentials are passed as CLI flags on each invocation. Local-only targets work without rclone.
:::

### Per-service pre_hook for database dumps

For Postgres-backed services, add a pre-hook that writes a dump into a location that gets snapshotted:

```toml
[service.backup]
pre_hook = "pg_dump -U postgres -d keycloak -F c -f /var/backups/keycloak.dump"
```

Make sure `/var/backups` is a mounted volume so the dump is actually included in the snapshot.

### Verifying the scheduler is running

```bash
grep "Backup scheduler started" ~/.orca/orca.log
```

If you don't see that line, either `[backup]` is missing from `cluster.toml`, `enabled = false`, or the cron expression failed to parse (remember: six fields, including seconds).

## 7. Health checks for slow-starting services

Orca supports liveness probes per service:

```toml
[service.liveness]
path = "/healthz"
interval_secs = 30
timeout_secs = 5
failure_threshold = 3
initial_delay_secs = 10
```

The probe hits `http://<container>:<port><path>` on the service's primary port. If `failure_threshold` consecutive probes fail, orca restarts the container.

### Keycloak: the important one

Keycloak is the canonical slow-starting service: it takes ~90 seconds to fully initialize on a cold boot (JPA schema, Infinispan, theme compilation). Without `initial_delay_secs`, orca will kill it mid-boot every single time and you'll get into a crash loop that looks like a broken image.

```toml
[service.liveness]
path = "/realms/master"
interval_secs = 30
timeout_secs = 10
failure_threshold = 3
initial_delay_secs = 120
```

Two non-obvious things:

1. **`initial_delay_secs = 120`** — gives Keycloak a full two minutes of uninterrupted boot. Overkill on fast hardware, safe on slow hardware, never too short.
2. **`path = "/realms/master"`** — the obvious choice is `/health/ready`, but Keycloak exposes that on the **management port 9000**, not the main HTTP port 8080 that orca probes. `/realms/master` is a cheap GET on port 8080 that returns 200 once the server is fully up.

::: danger Pitfall: no initial_delay_secs = crash loop on slow starters
Any service that takes more than a few seconds to accept connections — Keycloak, Gitea on first migration, anything JVM-based — needs `initial_delay_secs`. Otherwise orca's first probe fails, the `failure_threshold` ticks down quickly, and the container is killed before it ever finishes booting. You'll waste an hour thinking the image is broken.
:::

## 8. Reverse proxy and TLS

Orca's built-in proxy listens on 80/443 and routes HTTP(S) traffic to services by the `domain` field in their `service.toml`. ACME certs are automatically issued from Let's Encrypt for every service with a `domain` set, using the email from `cluster.toml`.

### HTTP routing

```toml
[[service]]
name = "gitea"
image = "gitea/gitea:1.22"
domain = "git.example.com"
port = 3000
```

Any request to `https://git.example.com` gets routed to the `gitea` container on port 3000. No extra config.

### Non-HTTP ports: extra_ports

For services that need a raw TCP port exposed outside the proxy — Gitea SSH, Postgres on a bastion, etc. — use `extra_ports`:

```toml
[[service]]
name = "gitea"
image = "gitea/gitea:1.22"
domain = "git.example.com"
port = 3000
extra_ports = ["22222:22"]
```

This publishes container port 22 on host port 22222. That's how Gitea SSH (`ssh://git@git.example.com:22222`) works on this cluster.

### Strict-cookie clients: Keycloak, etc.

Some upstreams are picky about headers and cookies. If you're debugging weird login loops, verify orca's proxy is doing all of these correctly (it does, as of current main — this is informational):

- **`Set-Cookie` is appended, not inserted.** Upstreams like Keycloak often return multiple `Set-Cookie` headers in one response; a proxy that uses `HeaderMap::insert` will clobber all but the last one and break sessions. Orca uses `append`.
- **`X-Forwarded-Proto`, `X-Forwarded-Host`, `X-Forwarded-For` are injected** on every forwarded request. Keycloak reads `X-Forwarded-Proto` to generate correct `https://` URLs in its OIDC redirect responses; without it, you get redirects to `http://` and browsers refuse the resulting insecure cookies.
- **Upstream redirects are NOT auto-followed.** Orca's HTTP client is configured with `redirect::Policy::none()`. If the proxy follows a 302 from Keycloak, the client browser never sees the redirect, the auth handshake breaks, and you get infinite login loops.

::: tip Debugging login loops
If a service that works direct-to-container stops working through the proxy, 95% of the time it's one of the above three issues. Orca has all three handled in current main — if you're running older builds, upgrade first before going deep on a debugging session.
:::

## 9. Migrating from another orchestrator

This is the playbook we used to move ~20 services off Coolify onto orca. Works for any source orchestrator (Coolify, docker-compose, Portainer, hand-rolled systemd) as long as you can `docker inspect` the running containers.

### Step-by-step

**1. Inspect the source container.**

```bash
docker inspect <container-name> | less
```

Pay attention to: `Env`, `Mounts`, `NetworkSettings.Networks`, `HostConfig.PortBindings`, `Cmd`, `Entrypoint`.

**2. For DB-backed services, stop the app first, then dump the DB.**

```bash
docker stop my-app               # disconnect all clients
docker exec my-app-db \
  pg_dump -U postgres -d myapp -F c -f /tmp/db.dump
docker cp my-app-db:/tmp/db.dump ./myapp.dump
```

Stopping the app first guarantees a consistent dump — no half-written rows, no surprise migrations mid-dump.

**3. Write the new `service.toml`.** Externalize every secret-looking value into `${secrets.X}`. Don't just copy Coolify's inlined env — take the migration as an opportunity to clean up.

**4. Set the secrets.**

```bash
cd ~/orca
orca secrets set MY_APP_DB_USER postgres
orca secrets set MY_APP_DB_PASSWORD 'value-from-docker-inspect'
# ... repeat for every secret
```

**5. Deploy the new orca-managed containers.**

```bash
cd ~/orca && orca deploy my-app
```

This starts the new DB (empty) and the new app. The app will almost certainly fail its liveness check because the DB is empty — that's fine, we're about to fix it.

**6. Restore the DB dump into the new DB container.**

```bash
docker cp ./myapp.dump orca-my-app-db:/tmp/db.dump

docker exec orca-my-app-db \
  psql -U postgres -d myapp \
  -c 'DROP SCHEMA public CASCADE; CREATE SCHEMA public;'

docker exec orca-my-app-db \
  pg_restore -U postgres -d myapp /tmp/db.dump
```

The `DROP SCHEMA` is important — `pg_restore` doesn't like pre-existing tables from the fresh init.

**7. Restart the app.**

```bash
orca restart my-app
```

It should now pick up the restored data and pass its liveness check.

**8. Cut DNS / proxy over.** Point the domain at the orca host. If orca is on the same host as the old orchestrator, take the old service down first to free port 80/443.

**9. Verify, then tear down the old containers.** Give it at least 24 hours in production before `docker rm`ing the old ones — you want a recent, known-good snapshot to roll back to if anything surfaces.

## 10. Common gotchas reference

A skimmable list of every pitfall covered above:

- **cwd matters for services/.** Use `orca install-service` (sets `WorkingDirectory` automatically) or always `cd ~/orca` before commands.
- **cwd matters for secrets.json.** `orca secrets set` writes to `$PWD/secrets.json`. Always run from `~/orca`. (Being fixed.)
- **cluster.toml is only read on startup.** Restart orca after editing: `sudo systemctl restart orca`.
- **setcap is not needed with systemd.** `orca install-service` generates a unit with `AmbientCapabilities=CAP_NET_BIND_SERVICE`. Only use `setcap` if running without systemd.
- **Agent nodes need `--leader` for systemd.** Use `orca install-service --leader <ip>:6880` on joined nodes — this creates `orca-agent.service` with the join command.
- **`orca update` auto-restores setcap** via `sudo -n setcap`. If passwordless sudo isn't configured, run `sudo setcap` manually or use systemd (which doesn't need it).
- **Webhooks are persisted** to `~/.orca/webhooks.json` as of v0.2.2. No need to re-register after restarts.
- **S3 backup targets require `rclone` on every node.** `apt install rclone` (or from rclone.org) on master and all agents. No config file needed.
- **Pre-pull your registry image** before the first boot if your registry is managed by orca. Otherwise bootstrap deadlock.
- **Slow-starting services need `initial_delay_secs`.** Keycloak: 120s. Don't skimp.
- **Keycloak's `/health/ready` is on port 9000, not 8080.** Use `/realms/master` on 8080 for liveness.
- **Cron schedules in `cluster.toml` need six fields** (with seconds), not five. `0 0 3 * * *`, not `0 3 * * *`.
- **Stop the app before dumping its DB** during a migration. Consistent dumps only come from quiesced databases.
- **`DROP SCHEMA public CASCADE`** before `pg_restore`ing into a fresh DB container.

If you hit something that isn't on this list, file an issue — the list grows with every migration.
