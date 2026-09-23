//! `orca backup all`: volumes, their pre-hooks, S3 upload and bind mounts.
//! Every outcome is recorded on the run's [`BackupReport`] (#197).

use bollard::Docker;

use super::helpers::{
    create_backup_dir, list_orca_volumes, prune_old_backup_dirs, run_backup_container,
};
use super::{bind_archive, bind_mounts, volume_owner};
use crate::handlers::backup_report::BackupReport;

/// Backup all orca-prefixed Docker volumes to `~/.orca/backups/{timestamp}/`.
pub async fn backup_all_volumes(report: &mut BackupReport) {
    let backup_cfg = crate::handlers::backup::load_backup_config();
    attempt_backup(&backup_cfg, report).await;
    // Prune runs unconditionally after the attempt — success or failure — so a
    // transient Docker failure never skips cleanup. Older backups are preserved
    // until they age past retention_days, so a failed run doesn't delete the
    // most-recent good backup before a new one exists.
    prune_old_backup_dirs(backup_cfg.retention_days);
}

async fn attempt_backup(backup_cfg: &orca_core::backup::BackupConfig, report: &mut BackupReport) {
    let docker = match Docker::connect_with_local_defaults() {
        Ok(d) => d,
        Err(e) => {
            report.fail(format!("cannot connect to Docker: {e}"));
            return;
        }
    };

    let Some(backup_dir) = create_backup_dir() else {
        report.fail("cannot create the local backup directory");
        return;
    };

    let Some(volumes) = list_orca_volumes(&docker).await else {
        report.fail("cannot list Docker volumes");
        return;
    };

    if volumes.is_empty() {
        println!("No orca volumes found.");
    } else {
        let hooks = load_service_hooks();
        let owners = volume_owner::volume_owners(&docker).await;

        println!("Backing up {} volume(s) to {}", volumes.len(), backup_dir);
        report.volumes_total = volumes.len() as u32;
        let mut hook_failed: Vec<String> = Vec::new();

        for vol in &volumes {
            print!("  {vol} ... ");
            // The owning service comes from the container that mounts the
            // volume (#198): volumes are `orca-<service>-data`, so stripping
            // only `orca-` never matched a hook key.
            let service_name = volume_owner::service_for(vol, &owners);
            if let Some(hook) = hooks.get(&service_name) {
                let container = format!("orca-{service_name}");
                match run_pre_hook(&docker, &container, hook).await {
                    Ok(()) => report.hooks_run += 1,
                    Err(e) => {
                        // No fresh dump. Still take the raw copy (better than
                        // nothing), but the volume counts as failed.
                        print!("pre-hook FAILED, raw copy only ... ");
                        report.fail(format!("pre-hook for {vol} failed: {e:#}"));
                        hook_failed.push(vol.clone());
                    }
                }
            }
            match run_backup_container(&docker, vol, &backup_dir).await {
                Ok(()) => {
                    println!("done");
                    if !hook_failed.contains(vol) {
                        report.volumes_ok += 1;
                    }
                }
                Err(e) => {
                    println!("FAILED");
                    report.fail(format!("tar of {vol} failed: {e}"));
                }
            }
        }

        println!(
            "Volume backup complete: {}/{} volumes, {} pre-hook(s) run.",
            report.volumes_ok,
            volumes.len(),
            report.hooks_run
        );
        if !hook_failed.is_empty() {
            println!(
                "WARNING: pre-hook failed for {}: those tarballs are raw copies of live \
                 data, not a reliable database backup.",
                hook_failed.join(", ")
            );
        }

        upload_volumes_to_s3(backup_cfg, &volumes, &backup_dir, report);
    }

    // Host bind mounts (#185; #83 only warned). Run this even when there are
    // no named volumes: a service can have bind mounts and no volume.
    backup_bind_mounts(&docker, backup_cfg, &backup_dir, report).await;
}

/// Archive bind-mount sources into this snapshot and upload them. The
/// summary is printed last, so it becomes the run's reported message.
async fn backup_bind_mounts(
    docker: &Docker,
    cfg: &orca_core::backup::BackupConfig,
    backup_dir: &str,
    report: &mut BackupReport,
) {
    let mounts = bind_mounts::list_unbacked_bind_mounts(docker).await;
    if mounts.is_empty() {
        return;
    }
    let max_bytes = cfg.bind_mount_max_mb.saturating_mul(1024 * 1024);
    match bind_archive::archive(&mounts, std::path::Path::new(backup_dir), max_bytes) {
        Ok((entries, archive)) => {
            for e in entries.iter().filter(|e| e.decision.is_gap()) {
                println!(
                    "WARNING: NOT backed up: {} ({:?}), mounted by {}",
                    e.host_path,
                    e.decision,
                    e.mounted_by.join(", ")
                );
            }
            if let Some(path) = archive {
                match bind_archive::store(cfg, &path, &node_hostname()) {
                    Ok(stored) => println!("Bind mounts archived to {}", stored.display()),
                    Err(e) => report.fail(format!("storing the bind-mount archive failed: {e:#}")),
                }
            }
            // Too-large or unreadable sources stay warnings (in the summary),
            // not failures: a known-oversized mount would otherwise fail
            // every night.
            report.bind_mounts = Some(bind_archive::summary(&entries));
        }
        Err(e) => report.fail(format!(
            "bind-mount archive failed ({e:#}): {} bind mount(s) not backed up",
            mounts.len()
        )),
    }
}

/// Upload each volume tarball from a completed local backup to all S3 targets.
/// Uses `{vol}_{epoch}.tar.gz` as the S3 key so daily backups don't overwrite each other.
pub(super) fn upload_volumes_to_s3(
    config: &orca_core::backup::BackupConfig,
    volumes: &[String],
    backup_dir: &str,
    report: &mut BackupReport,
) {
    use orca_core::backup::BackupTarget;

    let s3_targets: Vec<_> = config
        .targets
        .iter()
        .filter(|t| matches!(t, BackupTarget::S3 { .. }))
        .collect();

    if s3_targets.is_empty() {
        return;
    }

    let hostname = node_hostname();
    let date = chrono::Utc::now().format("%Y-%m-%d");

    for vol in volumes {
        let local_path = std::path::Path::new(backup_dir).join(format!("{vol}.tar.gz"));
        if !local_path.exists() {
            continue;
        }
        let s3_name = format!("agents/{hostname}/{date}/{vol}.tar.gz");
        for target in &s3_targets {
            report.s3_total += 1;
            match orca_core::backup::s3::upload(&local_path, target, &s3_name) {
                Ok(()) => {
                    report.s3_ok += 1;
                    tracing::info!("Uploaded {vol} to S3");
                }
                Err(e) => report.fail(format!("S3 upload of {vol} failed: {e}")),
            }
        }
    }
}

fn node_hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

fn load_service_hooks() -> std::collections::HashMap<String, String> {
    std::env::var("ORCA_SERVICE_HOOKS_JSON")
        .ok()
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

async fn run_pre_hook(docker: &Docker, container: &str, hook: &str) -> anyhow::Result<()> {
    use bollard::exec::{CreateExecOptions, StartExecResults};
    use futures_util::StreamExt;

    tracing::info!("Running pre-hook in {container}: {hook}");
    let exec = docker
        .create_exec(
            container,
            CreateExecOptions {
                cmd: Some(vec!["sh".to_string(), "-c".to_string(), hook.to_string()]),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                ..Default::default()
            },
        )
        .await?;

    let mut text = String::new();
    if let StartExecResults::Attached { mut output, .. } = docker.start_exec(&exec.id, None).await?
    {
        while let Some(Ok(chunk)) = output.next().await {
            text.push_str(&chunk.to_string());
        }
    }

    let inspect = docker.inspect_exec(&exec.id).await?;
    let code = inspect.exit_code.unwrap_or(-1);
    // Keep the tail of the hook's output: "exit code 1" alone says nothing
    // about a wrong password or a missing binary.
    let tail: String = text
        .trim()
        .chars()
        .rev()
        .take(300)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    anyhow::ensure!(code == 0, "pre-hook exited with code {code}: {tail}");
    Ok(())
}
