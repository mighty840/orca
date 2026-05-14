# Backlog

Tracked work items, grouped by area. Each entry should be specific enough
to start without re-research.

## File size cleanup

- **Split files over 275 lines back under the original limit.** The
  multi-node rollout pushed 12 files over 275 lines. CI limit was
  temporarily relaxed to 420 to unblock v0.2.0-rc.2, then to 500 in
  v0.2.8 when the proxy-timeout fix pushed `orca-proxy/src/lib.rs`
  past 420; needs to come back down. Offenders:
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

## TUI: networks tab follow-ups

#17 shipped the tree-list view (`6` / `:networks`): per-node public-edge
domains + `orca-*` bridge networks with services and aliases. The
following pieces from the original proposal are deferred:

- **A-record IP per public edge row.** Resolve the domain's A record on
  master and surface alongside the proxy entry so DNS pointing at the
  wrong box jumps out visually.
- **ASCII graph rendering** with ratatui's canvas widget — cross-network
  links (services with `internal = true` referenced from another network)
  drawn as connecting edges, inter-node hair-pinned traffic as dashed
  edges. Today both axes are rendered as nested lists.
- **Missing-alias detection.** When service A's env references service B
  by hostname but B isn't aliased on a shared network, the dashboard
  should highlight the row in red. Needs heuristic env parsing across
  services.
- **Agent edge routes.** The `domains` field is currently only populated
  for the master row; agents have their own proxy route tables, surface
  them via a new WS field.

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
- **Backup dashboard: restore action.** #35 shipped the dashboard
  (table, `b` trigger, `Enter` drill-down). Still to do: `r` on a
  snapshot row to restore it. Needs a new WS RPC
  (`MasterMessage::RestoreRequest` / `AgentMessage::RestoreResult`)
  since today only `orca backup restore` works, run manually on each
  node. S3-snapshot listing tracked separately in #41.
- **Secrets organizer: inline add/edit/delete.** #37 shipped the read-only
  organizer (grouping, ref counts, drill-down). Inline `a` / `e` / `x`
  actions in the secrets view are deferred — they pair cleanly with
  project-scoped secrets (see "Project-level environment variables (secrets)"
  in this file) so it makes sense to land them together.
- **AI chat interface as TUI landing page.** Open the TUI to an
  `orca ask`-style chat pane by default. The user types questions,
  the AI responds with cluster context (services, health, stats).
  Previous conversations persist in the session. Services/logs/etc
  are secondary tabs. This makes the TUI the primary ops interface.
- **Log viewer.** Stream logs from any service (local or remote) in a
  TUI pane. Depends on WS log streaming (#12).
- **Alert delivery: Slack, webhook, email (#24).** Config exists but
  delivery is unimplemented. Conversational alerts only visible via
  `orca alerts list` today.

## CLI

- **`orca ask` should work locally without a running server.** Today it
  sends the question to the API which reads AI config from the running
  server's cluster.toml. Should fall back to reading cluster.toml from
  CWD or `~/.orca/` and calling the LLM directly from the CLI.
- **Resolve `cluster.toml` and `services/` upward.** Today all CLI
  commands only work from the orca working directory. Should walk up
  to find `cluster.toml` like git finds `.git/`, or fall back to
  `~/.orca/cluster.toml` for global config.
- **Wire up `orca logs --summarize`.** Currently prints a stub. Should
  fetch logs and send to the AI backend for analysis. (#23)
- **Secrets resolution in `cluster.toml`.** Today `${secrets.X}` only
  works in service.toml env vars. AI api_key and other cluster config
  values should also resolve from the secrets store.
- **Multi-argument commands.** `orca redeploy`, `orca stop`, `orca deploy`,
  and `orca logs` should accept multiple service names in a single
  invocation, e.g. `orca redeploy api web worker`. Today each command
  takes only one service name, requiring separate calls.
- **Shell auto-completion.** Generate completions for bash/zsh/fish via
  `orca completions bash > /etc/bash_completion.d/orca`. Use clap's
  built-in `clap_complete` crate. Should complete subcommands, flags,
  and dynamically complete service names by querying the API.
- **`orca redeploy` must route to the correct node.** Today `redeploy`
  runs on master and tries to create the container locally even when
  the service is placed on a remote agent. Should check placement and
  dispatch via WS/heartbeat to the target node.

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
