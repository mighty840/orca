# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/mighty840/orca/compare/v0.2.2...HEAD
[0.2.2]: https://github.com/mighty840/orca/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/mighty840/orca/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/mighty840/orca/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/mighty840/orca/releases/tag/v0.1.0
