//! E2E test: `orca backup all` actually backs up master's own Docker volumes.
//!
//! Regression coverage for commit 03945a7 ("scheduler now backs up master's
//! own volumes"). Before the fix the cron scheduler only persisted
//! `cluster.db` + `secrets.json` inline; master-local Docker volumes were
//! silently skipped. The fix replaced the inline handler with a spawned
//! `orca backup all` subprocess.
//!
//! This test exercises the spawned subprocess end-to-end: create an
//! `orca-*` volume on the host, run `target/debug/orca backup all` with
//! `HOME` overridden to a tempdir, then assert the tempdir contains a
//! tar.gz for the volume.
//!
//! Run with: `cargo test -p orca-control --test e2e_master_backup_test -- --ignored`

use std::path::{Path, PathBuf};
use std::time::Duration;

use bollard::Docker;
use bollard::image::CreateImageOptions;
use futures_util::StreamExt;

/// Volume name must start with `orca-` so the backup tool's prefix filter
/// picks it up.
const VOLUME: &str = "orca-e2e-master-bkp-vol";

async fn ensure_image(docker: &Docker, image: &str) {
    if docker.inspect_image(image).await.is_ok() {
        return;
    }
    let opts = CreateImageOptions {
        from_image: image,
        ..Default::default()
    };
    let mut stream = docker.create_image(Some(opts), None, None);
    while let Some(item) = stream.next().await {
        item.unwrap_or_else(|e| panic!("failed to pull {image}: {e}"));
    }
}

/// Locate the freshly-built `orca` binary regardless of which crate's tests
/// CARGO_MANIFEST_DIR points at. Walks up two levels to find the workspace
/// `target/`.
fn orca_binary() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root");
    for rel in ["target/debug/orca", "target/release/orca"] {
        let p = workspace.join(rel);
        if p.exists() {
            return p;
        }
    }
    panic!("orca binary not found — run `cargo build` first");
}

/// Find the most recent `<tempdir>/.orca/backups/<unix-ts>/` directory created
/// by the subprocess.
fn latest_backup_dir(home: &Path) -> Option<PathBuf> {
    let base = home.join(".orca/backups");
    let mut entries: Vec<_> = std::fs::read_dir(&base)
        .ok()?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    entries.last().map(|e| e.path())
}

#[tokio::test]
#[ignore]
async fn e2e_master_backup_includes_master_volumes() {
    let docker = Docker::connect_with_local_defaults().unwrap();
    ensure_image(&docker, "busybox:latest").await;

    // Reset volume state (idempotent across re-runs).
    let _ = docker.remove_volume(VOLUME, None).await;
    docker
        .create_volume(bollard::volume::CreateVolumeOptions {
            name: VOLUME,
            ..Default::default()
        })
        .await
        .expect("create_volume");

    // Seed the volume with a known file so we can assert the tar is non-empty.
    let seed = bollard::container::Config {
        image: Some("busybox:latest"),
        cmd: Some(vec![
            "sh",
            "-c",
            "echo 'master-vol-marker' > /data/mark.txt",
        ]),
        host_config: Some(bollard::models::HostConfig {
            binds: Some(vec![format!("{VOLUME}:/data")]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let c = docker
        .create_container::<&str, &str>(None, seed)
        .await
        .unwrap();
    docker.start_container::<&str>(&c.id, None).await.unwrap();
    docker.wait_container::<&str>(&c.id, None).next().await;
    let _ = docker
        .remove_container(
            &c.id,
            Some(bollard::container::RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await;

    // Spawn `orca backup all` with HOME pointing at a tempdir so the backup
    // output goes there instead of polluting the developer's ~/.orca.
    let tmp = tempfile::tempdir().expect("tempdir");
    let exe = orca_binary();
    let status = tokio::process::Command::new(&exe)
        .args(["backup", "all"])
        .env("HOME", tmp.path())
        // Block any AWS credentials in the caller's env from triggering S3
        // upload paths. Local backup only is what we want to exercise.
        .env_remove("AWS_ACCESS_KEY_ID")
        .env_remove("AWS_SECRET_ACCESS_KEY")
        .env_remove("ORCA_BACKUP_CONFIG_JSON")
        .output()
        .await
        .expect("spawn orca backup all");

    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        let stdout = String::from_utf8_lossy(&status.stdout);
        panic!("orca backup all failed:\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}");
    }

    // Find the timestamped backup directory and the per-volume tar within it.
    let dir = latest_backup_dir(tmp.path())
        .unwrap_or_else(|| panic!("no backup dir created under {}", tmp.path().display()));
    let tar = dir.join(format!("{VOLUME}.tar.gz"));

    // The whole point of the regression test: the master's own Docker volume
    // must appear in the backup output. If a future refactor stops spawning
    // the subprocess (or stops calling backup_all_volumes from `backup all`),
    // this assertion is what catches it.
    assert!(
        tar.exists(),
        "expected {} after `orca backup all` — got files: {:?}",
        tar.display(),
        std::fs::read_dir(&dir).ok().map(|it| it
            .filter_map(|e| e.ok().map(|e| e.file_name()))
            .collect::<Vec<_>>())
    );
    let len = std::fs::metadata(&tar).unwrap().len();
    assert!(
        len > 0,
        "tar exists but is empty — backup wrote no data for {VOLUME}"
    );

    // Best-effort: poll a brief window for the volume to free up before we
    // remove it (busybox containers exit fast, but Docker's volume refcount
    // can lag by milliseconds).
    for _ in 0..10 {
        if docker.remove_volume(VOLUME, None).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
