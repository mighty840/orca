# Upgrading to 0.3.0

0.3.0 hardens authentication, backups, deploys and the proxy. Most changes need
no action, but a few change behaviour that scripts or firewall rules may rely
on. Read this page before upgrading a 0.2.x cluster.

## Order

1. **Back up** the master: `orca backup basic`, and keep a copy of
   `~/.orca/master.key` outside the cluster.
2. **Set `[backup] age_recipients`** (see below) if you haven't.
3. **Upgrade the master first**, then each agent:

   ```bash
   orca update
   sudo systemctl restart orca          # master
   sudo systemctl restart orca-agent    # each agent, after the master
   ```

A 0.3.0 agent sends its token in an `Authorization` header, which older
masters don't read, so it can't register with an older master. A 0.3.0 master
still accepts the old query-string token from older agents, with a deprecation
warning. Restarting orca no longer removes containers, so the upgrade itself
restarts no workloads.

## Behaviour changes

### Authentication

- **Only `/api/v1/health` and `/api/v1/webhooks/github` are unauthenticated.**
  `/metrics` needs a token with the `status` permission (a `viewer` token).
  Add a bearer token to your Prometheus scrape config. Any route missing from
  the RBAC table is admin-only.
- **The agent channel is admin-only.** Agents must join with the cluster token
  or another admin token; a `[[token]]` with a narrower role is refused.
- **Webhooks without a secret are rejected.** Registering one returns `400`,
  and pushes to a stored secretless webhook fail. Re-register them with
  `--secret`.
- **A token whose `${secrets.X}` doesn't resolve is dropped**, never loaded as
  the literal string.
- `orca server` no longer prints the cluster token; it prints the token file's
  path.
- The agent's installed systemd unit loads the token from `~/.orca/agent.env`
  instead of `ExecStart`. Re-run `orca install-service --leader <master>:6880`
  on agents to switch.

### Stopping orca leaves workloads running

Stopping or restarting `orca server` no longer stops and removes containers;
the next start re-attaches. If you relied on the old cleanup (development,
tests), use `orca server --teardown-on-exit`.

### Backend ports bind to 127.0.0.1

A service's `port` used to be published on a random host port on `0.0.0.0`.
It now binds to `127.0.0.1`. Anything that reached a backend on that random
port from another machine must go through the proxy or use an explicit
`host_port`, which stays public. `extra_ports` keep the address you give
(`host:container` still means `0.0.0.0`); a database port published that way
logs a warning. Containers switch on their next recreate. See
[Configuration](/guide/configuration#host-ports).

### The master refuses an unreadable store

If `~/.orca/cluster.db` can't be opened, or its services or paused-service
list can't be read, the master refuses to start instead of starting empty.
Fix or restore the store (`orca backup restore-basic`) before starting.

### Deploys report failures

- `orca deploy` exits `1` when a service fails to deploy. Scripts that ignored
  failures will now see them.
- A failed replacement keeps the old container running.
- A `${secrets.X}` that doesn't resolve fails the deploy, naming the service
  and the missing keys, instead of starting the container with the literal
  string. Check your references before upgrading.
- Invalid memory limits (for example `"4g"`) fail validation instead of
  silently removing the limit. See
  [Configuration](/guide/configuration#memory-limits).
- CPU and memory changes that were ignored before are applied on the first
  reconcile after the upgrade.

### Backups

- **Set `[backup] age_recipients` before the first scheduled run.** Key
  material (`master.key`, `webhooks.json`, TLS certificates, the ACME account,
  `backup_config.json`) is only backed up encrypted and is skipped with a
  warning otherwise. Volume tarballs and the bind-mount archive are stored
  unencrypted without it, and can contain keys.

  ```bash
  age-keygen -o orca-backup.key    # keep this file off the cluster
  ```

  ```toml
  [backup]
  age_recipients = ["age1..."]     # the public key printed by age-keygen
  ```

- **Database pre-hooks run for the first time.** They were never matched
  before, so the first runs create dump files in the database volumes and
  volume tarballs grow. A failing hook is reported.
- **Backup failures are reported.** `orca backup` exits non-zero and a failed
  run raises a critical alert. Problems that were hidden may show up in the
  first few runs.
- **S3 pruning is opt-in** (`[backup] prune_s3 = true`). Nothing is deleted
  from a bucket unless you enable it.

### Token rotation needs current agents

`orca token rotate --finish` refuses until every agent is on the new token.
Agents older than 0.3.0-rc.5 ignore the rotation, so upgrade every agent
before finishing. See
[Rotating the cluster token](/guide/multi-node#rotating-the-cluster-token).

### Proxy

- HTTP-to-HTTPS redirects are `308` instead of `301`, and keep the query
  string.
- Upstream timeouts return `504` instead of `502`, and timeouts follow
  progress instead of a 120 s wall clock. See
  [Deployment](/guide/deployment#proxy-behaviour).
- `orca server` exits with an error when it can't bind port 80 or 443.

## After upgrading

```bash
orca status                  # every service running, no failure reasons
orca nodes                   # every agent registered
orca backup all              # one run with the new settings; check its summary
```
