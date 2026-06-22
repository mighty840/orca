# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **`service.toml` accepts multiple domains per service.** A `[[service]]` block can now set `domains = ["a.com", "b.com", ...]` to serve several hostnames from the *same* container — each domain gets its own ACME (or BYO) cert and its own SNI/proxy route pointing at the same backend. Previously the only way to expose a second hostname was to duplicate the whole block, which spawned a redundant container; this unblocks apex+www, regional aliases, and dual-TLD cutovers. The single `domain = ".."` form keeps working unchanged; setting both `domain` and `domains` is rejected at deploy time with a clear error. All domains are stamped on the container (`orca.domains` label) so node-local proxies and the orphan-adoption scanner pick them up, surfaced through `DomainDiscovered`, and reflected in `orca status` / the API (`domains` field) and the TUI. (#93)
- **The control plane now self-heals "running but unregistered" services.** A new periodic adoption reconciler on the master fans an `AdoptionScanRequest` out to every connected agent, and for any `orca.managed=true` container whose `orca.service` is missing from the master's registry, it adopts the container: registers an in-memory service entry with a `remote-<node_id>` placeholder (so the service appears in `orca status` and `orca logs` / `orca redeploy` resolve to the right agent) and persists a reconstructed `ServiceConfig` (rebuilt from the `orca.*` labels + the container's image) so the adoption survives a master restart. Only *running* containers are adopted, never a service the master already knows, and the watchdog's existing remote-placement guard means adopted services aren't spuriously redeployed. Previously a container left behind by a missed deploy ACK (or a master restart mid-deploy) stayed invisible to orca until a human ran `docker rm -f` and retried. Configurable via the `[deploy]` block: `adopt_orphans` (default true) and `adopt_interval_secs` (default 30). (#95)

### Fixed

- **Deploys of large images no longer fail with an opaque 30s timeout, and real pull errors now reach the operator.** The master previously waited a hard-coded 30s for a remote agent's deploy ACK — but that single ACK was only sent *after* the agent finished the image pull, awaited inline in the agent's receive loop. So any first-time pull of a multi-GB image (or any unpullable/missing image) blew the 30s budget: the operator saw `deploy timed out after 30 s` (with a misleading "Is `orca server` running?") while the agent's real result — success or the actual `image not found`/pull error — arrived for a request the master had already abandoned, leaving the container running-but-unregistered. The wait is now two-phase: a short **receipt ACK** (new `AgentMessage::DeployReceived`, sent immediately on receipt) confirms the agent is alive — a miss here fails fast with a distinct "agent may be unreachable" message — followed by a long **completion** wait that carries the agent's real terminal result. Both timeouts are configurable via a new `[deploy]` block (`ack_timeout_secs` default 10, `completion_timeout_secs` default 600). The agent now also runs the deploy on a spawned task instead of inline, so a long pull no longer head-of-line-blocks other master→agent commands (Stop, further Deploys). (#88, #94)

## [0.2.8] - 2026-05-14

### Fixed

- **Proxy could get stuck on hung upstreams, requiring a restart to recover.** The reverse proxy's reqwest client was built without timeouts, so a slow/dead backend (or a hung HTTP fallback target) parked the per-request task forever — observed as the proxy going unresponsive while CPU idled and the listener kept accepting connections. The v0.2.7 fallback wire amplified the impact by routing every unmatched-host request through the same un-timed code path. Bounded with `connect_timeout = 10s`, `timeout = 300s`, `pool_idle_timeout = 90s`. (Affects all v0.2.x; surfaced in 0.2.7.)
- **TLS listener could hang on slowloris.** `peek_sni` did an unbounded `TcpStream::peek` before accepting the TLS handshake, so a client that opened TCP and sent no bytes pinned the per-connection task. Bounded to 5s — real TLS clients send ClientHello in well under one second.
- **WebSocket proxy could hang on dead backends.** Both `TcpStream::connect` and the 101-response header read had no timeout, so a backend that accepted the TCP connection but never replied parked the upgrade task. Bounded to 5s and 10s respectively; on timeout the client sees 504 Gateway Timeout instead of hanging.

## [0.2.7] - 2026-05-14

### Added

- **TUI remembers the last-opened project filter.** Selecting a project with `p` or `:project <name>` persists `last_project` to `~/.orca/tui-state.json`. On next launch the filter is reapplied optimistically; if the project no longer exists in the cluster the filter is dropped silently with a status-bar notice. Clearing the filter (Esc / `:project` with no args) is also persisted. (#34)
- **TUI backup dashboard** (`4` or `:backups`) — per-node table of backup status: hostname, role, last-run age, snapshot count, total size, last-result. Aggregated via a new `GET /api/v1/cluster/backups` endpoint that dispatches `BackupStatusRequest` over WS to every connected agent and merges with the master's local snapshot listing. Press `b` on a row to trigger an immediate backup on that node (master row runs `orca backup all` as a subprocess; agent rows dispatch `BackupRequest` over WS). `Enter` drills into a per-snapshot view with file inventory. View is local-only for now — S3 listing tracked separately. (#35)
- **TUI webhook management** (`5` or `:webhooks`) — list every registered webhook with last-trigger time, status, and short commit SHA. `Enter` drills into the last-10 invocation history for that webhook (kept in an in-memory ring on the master). `a` opens command mode pre-filled with `webhook-add `, `e` opens it pre-filled with the row's identity for editing, and `x` deletes after confirmation flash. New endpoints: `GET /api/v1/webhooks/{id}/invocations`; the existing `GET /api/v1/webhooks` now returns a `WebhookEntry` per row that includes `last_invocation`. (#36)
- **TUI secrets organizer** (`3` view, now grouped). Keys are now grouped by inferred scope — `global` for cluster-wide or cross-project secrets, project name for keys referenced by services in exactly one project, and `broken refs` for keys templated in env values but missing from the store. Each row shows a reference-count badge. `Enter` drills into the list of referencing services. Add/edit/delete continues to work through the existing `:set` / `:rm` commands. New endpoint: `GET /api/v1/secrets/usage` returns each key with its referencing services (parsed from `${secrets.KEY}` patterns in `ServiceConfig.env`). (#37)
- **TUI networks view** (`6` / `:networks`) — per-node tree of `orca-*` Docker bridge networks plus the master's public-edge route table. For each node: hostname, role, public-edge `domain → service` mappings, then each bridge network with its attached services and per-network aliases (color-coded: green for services with aliases, yellow for the "no aliases" case the rc2 migration was bitten by). Aggregated via a new `GET /api/v1/cluster/networks` endpoint that dispatches `NetworkStatusRequest` over WS to every connected agent and merges with the master's own enumeration. Read-only; agent edge-route surfacing + ASCII graph rendering are deferred to follow-ups. (#17)
- **Nightly E2E test job in CI.** `.github/workflows/ci.yml` gained a `schedule: cron "0 6 * * *"` and `workflow_dispatch` trigger. The previously-dormant `e2e` job (gated on `github.event_name == 'schedule'`) now actually fires once a day and can be kicked off manually for release validation. (#46)
- **24 new E2E regression tests.** 14 in #46 — auth enforcement, secret env interpolation, the cluster networks dashboard, master backup volume inclusion, GitHub webhook HMAC validation, RBAC role matrix (viewer/deployer/admin), HTTP fallback proxy, and multi-replica route filtering on partial failure. 10 more in #48 — a `fake_agent` fixture (tokio-tungstenite) that joins the master over a real WebSocket so the cluster fan-out RPCs (`cluster/networks`, `cluster/backups`) are exercised end-to-end with an actual joined agent, plus CLI E2E tests for `status`, `logs`, `stop`, `redeploy`, `rollback`, `secrets list`, and the full `webhooks add/list/remove` round-trip. Total ignored suite: 51 tests, all green.

### Fixed

- **TUI secrets view didn't scroll.** Moving the cursor past the bottom of the visible area left the highlight off-screen because the Table widget renders top-of-list only. Mirrored the services-view's `compute_scroll`, slice the flat row list, and surface a `[N/total]` indicator in the title. (#47)
- **TUI networks tab was slow and unresponsive on load.** `enumerate_orca_networks` in the agent did `inspect_container` serially per orca-* container — 20 containers ≈ 20 sequential round-trips to the Docker socket. Replaced the loop with `futures::join_all` so inspects run concurrently. (#47)
- **TUI networks tab didn't scroll.** `draw_networks` rendered the whole tree as a Paragraph with no viewport. Added `state.network_scroll`, window the rendered Lines, and wired `j`/`k`/`g`/`G`/`PgUp`/`PgDn` for scrolling. (#47)
- **HTTP fallback proxy was dead config.** `FallbackConfig.http` was accepted in `cluster.toml` and plumbed through `run_proxy_with_fallback`, but `handler::handle_request` never consulted it — requests to unmatched hosts always returned 404 regardless of fallback. Now the handler forwards to `fallback.http` via the existing `forward_with_retry` path. TLS SNI passthrough was already wired and is unchanged. (#46)
- **Three latent failures in the ignored E2E suite.** The CLI test harness (`OrcaServer`) now pre-declares `api_tokens` so the spawned `orca server` doesn't auto-generate a random token the test client can't see (deploy/scale tests were 401'ing). `e2e_backup_and_restore_volume` pre-pulls `busybox:latest` since `bollard::create_container` doesn't pull on miss. `e2e_health_checker_marks_unhealthy` switched from the legacy `health: ...` shorthand to an explicit `liveness` block with `initial_delay_secs: 0` so it doesn't race the default 5s probe-delay window. (#46)

## [0.2.5-rc.6] - 2026-05-10

### Added

- **rclone S3 backend** — S3 backup targets now use `rclone copyto` / `rclone lsf` instead of the `aws` CLI. Credentials are passed as `--s3-*` flags on each invocation; no rclone config file is required. Install `rclone` on every node (`apt install rclone` or from rclone.org) to enable S3 targets.

### Fixed

- **Double backup dir per day on co-located master+agent nodes** — the master's scheduled backup no longer runs `orca backup all` locally. Volume backups are exclusively dispatched to agents via the `BackupRequest` WebSocket message, so a node that runs both master and agent produces exactly one backup directory per run.
- **Volume tarballs never reaching S3** — `orca backup all` now uploads each volume tarball to all configured S3 targets immediately after the local snapshot completes.
- **S3 credentials silently dropped** — `access_key` and `secret_key` were shadowed by `..` in the `BackupTarget::S3` pattern match and never passed to the upload command. Fixed by explicitly extracting both fields.
- **Backup pruning skipped when Docker connection fails** — restructured `backup_all_volumes` so pruning always runs after the backup attempt regardless of whether Docker was reachable or volumes were found.

## [0.2.4] - 2026-04-28

### Added

- **`orca exec <service> [cmd]`** -- open an interactive shell or run a one-shot command in a running container. Works for both local and remote (agent-placed) services over the existing WebSocket back-channel. The TUI gains a `:sh` keybind that suspends the dashboard and drops into the container's shell.
- **TUI log viewer** -- press `l` on any service to open a streaming log pane without leaving the dashboard. Uses the same `/api/v1/services/{name}/logs` endpoint and streams updates in real time.
- **Backup pre-hook** -- `backup.pre_hook` in a service's config (e.g. `pg_dump`) is now executed inside the container before the volume snapshot is taken. Previously the field was parsed but never invoked.
- **S3 backup restore** -- `orca backup restore --target s3 <key>` downloads the snapshot from S3 and restores it into the volume. Previously `orca backup restore` only worked with local targets.
- **`orca redeploy` routes to correct node** -- redeploying a service pinned to a remote agent now dispatches the Stop+Deploy commands over the WS channel directly to that agent, rather than attempting a local container operation on the master.

### Fixed

- **Infinite 30-second reconcile loop for remote services** -- three-part fix:
  1. Watchdog placement guard: services with `placement.node` set are never reconciled locally by the watchdog, even when `instances.len() == 0` at startup.
  2. Remote placeholder upsert: master now creates a `remote-{node_id}` placeholder `InstanceState` on agent WS connect and removes it on disconnect, so heartbeat and `DeployResult` handlers always have a slot to update.
  3. `reconcile_services` on agent now sends `AgentMessage::DeployResult` for each service deployed during a `MasterMessage::Reconcile`, so master marks the placeholder Running correctly.
- **`orca update` finds prerelease/RC releases** -- the updater now always scans all GitHub releases (not just `/releases/latest`) so RC builds tagged as GitHub prereleases are discovered correctly. Previously `orca update` always returned "no newer release found" when run against an RC binary.
- **Webhook returns 503 when agent is offline** -- a redeploy webhook targeting a service on a disconnected agent now returns `503 Service Unavailable` instead of `500 Internal Server Error`.

## [0.2.3] - 2026-04-14

### Added

- **`${secrets.X}` resolution in `cluster.toml`** -- secret references are now expanded in `ai.api_key`, `ai.endpoint`, and `network.setup_key`, so cluster-level config can be checked into git without leaking credentials (#22).
- **Per-service CPU and memory stats for remote nodes** -- agents stream container resource usage over the WS heartbeat, so `orca status` and the TUI now show live per-container metrics for every node, not just the master (#13).
- **`orca logs <service> --summarize`** -- pipes the recent log buffer through the configured AI backend and returns a concise summary with likely issues and next steps (#23).
- **Multi-arg CLI commands** -- `orca deploy svc1 svc2 svc3`, `orca redeploy svc1 svc2`, and `orca stop svc1 svc2` now accept any number of service names in a single invocation.
- **Shell completions** -- `orca completions <bash|zsh|fish|powershell>` prints a completion script ready to source or drop into your shell's completion directory.
- **Config path resolution** -- the CLI walks up from the current working directory to find `cluster.toml` and `services/`, the same way `git` finds `.git`. Run `orca` commands from any subdirectory of your infra repo.
- **AMD ROCm GPU passthrough** -- services declaring `vendor = "amd"` get `/dev/kfd` and `/dev/dri` mounted, with the `video` and `render` group IDs auto-detected from the host.
- **`orca webhooks add --secret <value> --infra`** -- the `--secret` and `--infra` flags are now wired through the CLI (previously only the API accepted them).

### Fixed

- **WS agent node registration** -- `placement.node = "<agent-name>"` now correctly resolves to remote agents over the WS transport. Services pinned to an agent node previously stayed pending until the master was restarted.
- **Proxy forwards original Host header** -- upstream services see the public hostname instead of the internal container IP. Fixes redirect loops in apps like LiteLLM whose `/ui` endpoint generates absolute URLs from the request host.

## [0.2.2] - 2026-04-09

### Added

- **Bidirectional WebSocket streaming** between agent and master, replacing HTTP heartbeat polling. Agents now maintain a persistent WS connection for real-time state sync.
- **Agent proxy hot-adds routes and TLS certs** on container deploy -- no proxy restart needed (#19).
- **Reconcile remote services on agent reconnect** -- when an agent reconnects after a network partition, the master replays the desired state so the agent converges automatically (#21).
- **Infra webhook** -- git push to your orca-infra repo triggers an automatic `git pull` + redeploy on the cluster. Full GitOps without a CI runner.
- **`orca deploy <service-name>`** -- deploy a single service by name instead of the entire stack.
- **`orca redeploy <service>`** -- force pull the image and restart a service, even if the spec hasn't changed.
- **CLI auto-connects to master on agent nodes** -- all commands work without `--api` when running on an agent that has joined a cluster.
- **Unresolved env template comparison in reconciler** -- prevents unnecessary container restarts when only the resolved value changes (e.g., OAuth token refresh) but the template (`${secrets.X}`) is unchanged.
- **Webhook persistence** -- webhooks are now saved to `~/.orca/webhooks.json` and survive restarts (#20, closed as already-done).

## [0.2.1] - 2026-03-28

### Added

- **iptables NAT rule cleanup** on shutdown, plus stale rule detection on startup (#18).
- **Full spec-change detection in reconciler** -- detects changes to `extra_ports`, `mounts`, `volume`, `domain`, `aliases`, and all other spec fields, not just `image` and `env` (#14).
- **Systemd unit with `AmbientCapabilities`** and automatic `setcap` restore on `orca update` (#8, #16).
- **`orca redeploy <service>`** CLI and API endpoint for force-pull + restart (#15).
- **Container image pull policy** -- configurable per service: `auto`, `always`, `never`, `if-not-present` (#9).
- **`orca install-service`** for both master and agent nodes (use `--leader` flag for agents).
- **`orca update` prerelease/RC discovery** -- finds prerelease and release-candidate versions.
- **Backup auto-pull of busybox** -- the backup subsystem automatically pulls the `busybox` image if it is missing.

## [0.2.0] - 2026-03-14

### Added

- Multi-node clustering with Raft consensus via `openraft` and `redb` storage.
- Bin-packing scheduler with GPU awareness and Wasm preference.
- Cross-provider networking via NetBird WireGuard mesh.
- Built-in reverse proxy with auto-TLS (ACME / Let's Encrypt).
- AI operations assistant (`orca ask`) with conversational diagnostics.
- TUI dashboard with k9s-style navigation.
- Webhook-based CI/CD (GitHub/Gitea push events).
- Backup scheduler with S3 and local targets.
- Secrets management with AES-256 encryption at rest.
- Health checks with configurable liveness probes.
- `orca db create` for one-click database provisioning.
- RBAC with admin, deployer, and viewer roles.

## [0.1.0] - 2026-02-01

### Added

- Initial release: single-node container orchestrator.
- Docker runtime via bollard.
- WebAssembly runtime via wasmtime.
- Basic CLI: `orca server`, `orca deploy`, `orca status`, `orca logs`.
- TOML-based service configuration.

[Unreleased]: https://github.com/mighty840/orca/compare/v0.2.3...HEAD
[0.2.3]: https://github.com/mighty840/orca/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/mighty840/orca/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/mighty840/orca/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/mighty840/orca/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/mighty840/orca/releases/tag/v0.1.0
