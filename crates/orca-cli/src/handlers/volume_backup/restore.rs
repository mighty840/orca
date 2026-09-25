//! `orca backup restore-volume`: restore a Docker volume from the latest
//! local snapshot or from an S3 key, decrypting `.age` tarballs (#231).

use std::path::{Path, PathBuf};

use bollard::Docker;

use super::helpers::{find_latest_backup_dir, run_restore_container};
use super::seal;

/// Restore a Docker volume from the latest local backup directory, or from
/// the S3 object `from_s3` (#200: a fresh host has no local backups, so the
/// tarballs in S3 were unreachable through the CLI). Encrypted tarballs need
/// `identity`, an age key file. Returns success.
pub async fn restore_volume(
    volume_name: &str,
    from_s3: Option<&str>,
    identity: Option<&Path>,
) -> bool {
    match restore(volume_name, from_s3, identity).await {
        Ok(()) => {
            println!("Restored volume '{volume_name}' successfully.");
            true
        }
        Err(e) => {
            eprintln!("Restore of '{volume_name}' failed: {e:#}");
            false
        }
    }
}

async fn restore(
    volume_name: &str,
    from_s3: Option<&str>,
    identity: Option<&Path>,
) -> anyhow::Result<()> {
    let docker = Docker::connect_with_local_defaults()
        .map_err(|e| anyhow::anyhow!("cannot connect to Docker: {e}"))?;
    let identity = identity
        .map(crate::handlers::restore::read_identity)
        .transpose()?;

    let source = match from_s3 {
        Some(key) => stage_from_s3(volume_name, key)?,
        None => PathBuf::from(find_latest_backup_dir().ok_or_else(|| {
            anyhow::anyhow!(
                "no backup directories found in ~/.orca/backups/. To restore from \
                 S3, pass --from-s3 <key> (see `orca backup list`)."
            )
        })?),
    };
    let dir = seal::plaintext_dir(&source, volume_name, identity.as_deref())?;

    println!("Restoring {volume_name} from {} ...", source.display());
    run_restore_container(&docker, volume_name, &dir.display().to_string())
        .await
        .map_err(|e| anyhow::anyhow!("restore container failed: {e}"))
}

/// Download `key` from the first S3 target into a fresh staging directory,
/// as `<volume>.tar.gz` or, for an encrypted key, `<volume>.tar.gz.age`.
fn stage_from_s3(volume_name: &str, key: &str) -> anyhow::Result<PathBuf> {
    let cfg = crate::handlers::backup::load_backup_config();
    let target = cfg
        .targets
        .iter()
        .find(|t| matches!(t, orca_core::backup::BackupTarget::S3 { .. }))
        .ok_or_else(|| anyhow::anyhow!("no S3 backup target configured"))?;
    let dir = seal::staging_dir(volume_name)?;
    let dest = if key.ends_with(orca_core::backup::encrypt::AGE_SUFFIX) {
        seal::sealed_path(&dir, volume_name)
    } else {
        seal::plain_path(&dir, volume_name)
    };
    orca_core::backup::s3::download(target, key, &dest)?;
    Ok(dir)
}
