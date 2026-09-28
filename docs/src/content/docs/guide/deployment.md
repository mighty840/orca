# Deployment

## Rolling Updates

The default strategy. Orca starts new containers, waits for them to be ready, then stops old ones:

```toml
[service.deploy]
strategy = "rolling"
max_unavailable = 1
```

Update an image in `service.toml` and redeploy:

```bash
orca deploy
```

Orca handles the rest: pull image, start new replicas, verify readiness, drain old replicas.

### Graceful replacement

When a container is replaced (a deploy, a spec change, a webhook redeploy, a
health restart):

1. The old container gets `SIGTERM` and 30 seconds to shut down, then a
   normal remove (forced only if that fails). Databases are no longer killed
   mid-write.
2. It is kept, stopped, as `orca-<svc>.replaced` until the new container runs.
3. If the new container fails to create or start (a host port already bound,
   a bad mount), it is removed and the old one comes back under its name,
   with the same container id, running.

The old container is still stopped before the new one starts, so a
single-replica service has a short gap.

### Readiness

Traffic is routed to a new container once it answers. With an explicit
`health` path or `[service.readiness]` probe, it must answer 2xx or 3xx.
Without one, any HTTP response counts as ready, so an app that answers 404 or
401 on `/` doesn't wait out the whole readiness budget. A service without a
host port is probed on its container address.

### Failed deploys

- `orca deploy` exits `1` when any service failed, and the API reports the
  error. Scripts and CI see the failure.
- A rolling update stops at the first failed replica and leaves the old
  instances running. A failed `orca redeploy` re-registers the running old
  instances and the previous config.
- A failed spec is not recorded as applied.
- The declarative loop (`[reconcile]`) and the infra webhook leave a failed
  spec alone for 15 minutes instead of retrying every pass. A changed spec
  (your fix) is applied immediately, and `orca deploy` / `orca redeploy`
  always try.
- The reason shows in `orca status` and the TUI detail view.

::: warning
Image `VOLUME`s that no volume or mount covers get a fresh anonymous volume on
every container create, so data there is lost at the next redeploy. Deploys
log each uncovered path: an error when a declared volume lies inside it
(partial coverage), a warning when nothing covers it. Check the deploy log
after bumping a database image tag.
:::

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

A changed `build` section (repo, branch, Dockerfile) rebuilds the image on the
next deploy.

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
orca webhooks list         # List registered webhooks
```

::: tip
Webhook payloads are verified with HMAC-SHA256 signatures to prevent unauthorized deploys. A webhook without a secret is rejected.
:::

## TLS Certificates

### Auto-TLS (ACME)

Set `acme_email` in `cluster.toml` and Orca handles Let's Encrypt certificates automatically:

```toml
[cluster]
acme_email = "ops@example.com"
```

Failing orders back off per domain: 5 minutes, 15 minutes, 1 hour, then every
6 hours. A `rateLimited` answer from Let's Encrypt pauses all orders for an
hour. This keeps domains whose DNS no longer points at the cluster from using
up the account-wide order limit and blocking renewals of healthy domains.

### Fallback certificate

A TLS handshake without SNI, for an IP address, or for a name that has no
certificate yet gets a self-signed fallback certificate. The client sees a
certificate warning and then orca's error page, instead of a failed handshake.
An unknown hostname is logged once as a warning, which is early notice that a
certificate never provisioned.

### Custom Certificates

For BYO certs, set `tls_cert` and `tls_key` (PEM paths) on the service. ACME is skipped for it.

::: warning
Port 80 must be accessible from the internet for ACME HTTP-01 challenges to succeed. `orca server` exits with an error when it can't bind port 80 or 443; an agent keeps running but logs that it serves no public traffic and issues no certificates.
:::

## Proxy Behaviour

- **HTTP to HTTPS redirects** answer `308 Permanent Redirect` and keep the
  query string, so POST and PUT requests (uploads, `docker push`, webhooks
  sent to an `http://` URL) keep their method and body.
- **Timeouts follow progress**, not a wall clock:

  | Phase | Limit |
  |-------|-------|
  | Request body upload | 120 s without data |
  | Waiting for response headers after the upload | 10 minutes |
  | Response body | 120 s idle per chunk |

  TCP keepalive catches backends that disappear. The slow-backend warning
  fires at 120 s.
- **Upstream errors**: a timeout returns `504`, other failures (connection
  refused while a container is recreated, TLS errors) return `502`. The
  client gets "upstream timed out" or "upstream unavailable"; the log line
  gives the kind of failure and the full cause.
- **Request bodies stream** to the backend. With several backends, only
  bodies of known size up to 1 MB are buffered so a `502` can be retried on a
  second backend. Wasm triggers reject bodies over 10 MB with `413`.
- `Host` headers are matched case-insensitively.

## Persistent State

Services survive server restarts:
- **Deploy** -- config saved to redb store
- **Stop** -- containers stopped, config retained, service stays paused
- **Server stop or restart** -- containers keep running; on start orca loads
  the configs and re-attaches to them

The master refuses to start when `~/.orca/cluster.db` can't be opened or its
services can't be read, instead of starting empty and recreating everything.
A single undecodable row is skipped with a warning. Fix or restore the store
(`orca backup restore-basic`) before starting.
