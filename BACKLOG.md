# Backlog

Tracked work items, grouped by area. Each entry should be specific enough
to start without re-research.

## File size cleanup

- **Split files over 275 lines back under the original limit.** The
  multi-node rollout pushed 12 files over 275 lines. CI limit was
  temporarily relaxed to 420 to unblock v0.2.0-rc.2; needs to come
  back down. Offenders:
  - `crates/orca-proxy/src/lib.rs` (406) — move `RouteTarget` to its
    own module
  - `crates/orca-agent/src/docker/runtime.rs` (384) — split out
    `LocalRoute` + `registry_credentials` helpers
  - `crates/orca-tui/src/state.rs` (370) — extract `MetricHistory` +
    `parse_human_bytes` into a `metrics` submodule
  - `crates/orca-agent/src/grpc/client.rs` (347) — split heartbeat
    loop and re-register logic
  - `crates/orca-control/src/reconciler.rs` (338) — move remote
    placement + placeholder instance into a separate file
  - `crates/orca-control/src/lib.rs` (314), `webhook.rs` (311),
    `health.rs` (305), `api/handlers/ops.rs`, `ui/nodes.rs`,
    `ui.rs`, `handlers/server.rs` — each has one module-sized chunk
    that can be moved cleanly.

## Remote log + exec streaming

- **Secure websocket log stream from joined nodes to master.** Today
  the master can only serve logs for its locally-managed containers.
  For remote-scheduled services the TUI shows a placeholder message.
  Implementation:
  - Each joined node runs a small agent HTTP/WS listener on 6881.
  - Authentication via the cluster token (same as heartbeat).
  - Endpoints: `GET /api/v1/logs/<container>?tail=&follow=` returns
    a chunked text body; `WS /api/v1/exec/<container>` for
    interactive shell with line-discipline forwarding.
  - The master's `logs` handler proxies to the target node's
    listener using the stored agent address (`RegisteredNode.address`).
  - The TUI's `client.logs(...)` continues to hit `/api/v1/services/{name}/logs`
    on the master — all remote detail is hidden.
- **`orca exec` and TUI `:sh`** should ride the same WS exec channel.
  Ratatui suspends to run an interactive pty on the socket.
- **Hard rule:** the master must NOT ssh into joined nodes — agent-to-
  master communication is strictly HTTP/WS with the cluster token so
  the trust boundary is a single shared secret.

## TUI: networks tab

- **Networks view** showing the full routing graph for the cluster, in
  order of external-to-internal depth:
  1. **Public edge** — the domains served by each node's proxy, grouped
     by node. Each row includes the A record target IP so a mismatch
     (e.g. DNS pointing to the wrong box) jumps out visually.
  2. **Docker networks** — one block per `orca-<network>` bridge,
     listing the services attached and their aliases. Cross-network
     container links (a service with `internal = true` plus aliases
     referenced from another network) should be drawn as connecting
     edges.
  3. **Inter-node links** — if a service on node A calls a service on
     node B by public domain, draw that as a dashed edge so it's
     obvious traffic is hair-pinning through the edge proxy.
- Backend: `GET /api/v1/cluster/networks` that returns, per node, the
  docker networks, their attached services+aliases, and the set of
  route-table entries (domain → service). The TUI renders this as an
  ASCII graph using ratatui's canvas widget.
- Useful for debugging the kind of issue we hit today where
  `compliance-dashboard` couldn't resolve `compliance-agent` because the
  alias was missing — a networks tab would have shown the orca-certifai
  bridge with only one name in the alias list.

## Environments per project

- **First-class `dev` / `stage` / `prod` environments per project.** Today
  every service is single-environment. We need:
  - **Default `dev`.** Existing service.toml definitions stay as-is and
    are implicitly the dev environment of their project.
  - **Per-environment image tags.** A service can pin different tags per
    environment (e.g. `:latest` in dev, `:sha-...` in stage, `:v1.2.0`
    in prod). The `image` field becomes a map keyed by environment, or
    a sibling `[image.<env>]` block.
  - **Per-environment secrets and domains.** `${secrets.X}` resolves to
    the env-scoped secret first (e.g. `prod.LITELLM_API_KEY`) then falls
    back to the unscoped one. Domains can be templated
    (`auth-{env}.meghsakha.com`) or fully overridden per env.
  - **`orca env promote <project> <from> <to>` CLI.** Copies the entire
    service definition from the source env to the destination env, then
    runs an interactive checklist that the operator must walk through
    before the new env is activated:
    1. Required secrets exist in the destination env (lists missing).
    2. Domains resolve and TLS certs can be issued for them.
    3. External dependencies (databases, registries) are reachable.
    4. Image tags are present in the registry.
    5. Resource quota for the destination env is sufficient.
  - **`orca env list <project>` and `orca env diff <project> <a> <b>`** —
    show what's deployed where and what would change on promotion.
  - **TUI environment switcher.** A top-level pill/tab in the services
    view that filters by environment, with a "Promote..." action that
    walks the same checklist interactively.
  - **State storage.** Environment lives in cluster.db / service.toml as
    a first-class field on `ServiceConfig`.

## Multi-node

- **Replace bind-mount workaround for joined-node config files.** Today
  config files mounted into containers (`librechat.yaml`, `logo.svg`,
  `settings.yml`, etc.) live on a single host. On a joined node the
  service.toml's mount path won't exist. Either ship config files to
  the agent before deploy or move to ConfigMap-style API objects.
- **`orca volume copy <src> <dst>` CLI command.** Currently we shell out
  to `docker run --rm -v src:/s -v dst:/d alpine tar...`. Wrap that in
  a first-class subcommand so migrations don't need raw docker.
- **Single-binary install.** PATH conflict between `/usr/local/bin/orca`
  (system) and `~/.local/bin/orca` (user) caused state loss this session.
  `orca update` should know which path it's installed at and replace
  in-place; `orca install` should default to `/usr/local/bin/orca`.
- **setcap survives binary updates.** `mv` across filesystems creates a
  new inode and clears `cap_net_bind_service`. Either:
  (a) `orca update` runs `setcap` after replacing the binary, OR
  (b) ship a systemd unit with `AmbientCapabilities=CAP_NET_BIND_SERVICE`.
- **Hot reload of cluster.toml.** Backup config, ACME email, and other
  cluster-level settings only load at startup. Watch the file (or
  SIGHUP) to apply without `orca shutdown && orca server -d`.
- **Reconciler: detect spec changes beyond `same_image`.** Today the
  skip-path only re-deploys when image/module/env/cmd change.
  `extra_ports`, `mounts`, `volume`, `domain`, and `aliases` should
  also trigger a recreate.
- **`orca redeploy <service>` CLI subcommand.** Today the only way to
  force a fresh image pull + recreate is via the webhook endpoint.
- **`orca deploy` should resolve `services/` upward.** Errors with
  "services.toml not found" if invoked from the wrong cwd. Walk up to
  find `cluster.toml` like git finds `.git`.
- **Manifest of mounted files in service.toml gets pushed to remote
  agent on deploy.** Right now bind-mount paths must already exist on
  the target node — fine for the master, broken for joined nodes.

## Backups

- **Per-service `pre_hook` actually runs.** `ServiceBackupConfig` defines
  `pre_hook` (e.g. `pg_dump`) but the scheduler doesn't invoke it yet.
- **`orca backup all` should support an `--exclude` filter.** Not every
  volume needs to roll up to S3 (e.g. cache/temp).
- **Restore from S3.** `s3_backend::restore` is unimplemented; the CLI
  prints "S3 restore not yet supported."

## TUI

- **Single-project view filter.** Let user scope the TUI to one project
  at a time instead of a flat list of all services.
- **Remember last-opened project.** On TUI launch, reopen whichever
  project was active last session. If none (first run or project
  deleted), start with no project selected.
- **Backups per node.** Show backup status grouped by node — last run
  time, volume count, total size, any failures. Needs
  `GET /api/v1/cluster/backups` aggregating results from all nodes.

## Tags & project-level config

- **Tags on nodes, services, and projects.** Free-form key-value labels
  (e.g. `env=prod`, `team=compliance`, `tier=frontend`) on nodes,
  services, and projects. Stored in cluster.db, surfaced in TUI and
  `orca status`. Enables filtering, batch ops, and placement affinity
  beyond the current `placement.node` field.
- **Project-level environment variables (secrets).** Today secrets are
  global. Add per-project scoping so `${secrets.X}` resolves project
  scope first, then falls back to global. CLI: `orca secrets set
  --project <name> KEY VALUE`. Stored alongside global secrets with a
  project prefix in the secrets store.
