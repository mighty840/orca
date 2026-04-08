# Backlog

Tracked work items, grouped by area. Each entry should be specific enough
to start without re-research.

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
