//! S3 backup storage via rclone subprocess.
//!
//! Uses `rclone` for S3-compatible providers (AWS, Hetzner, Minio, R2, B2, …).
//! No rclone config file is required: the region and endpoint go on the command
//! line, and credentials through the child's environment (see [`apply_s3_flags`]).

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use tracing::{error, info};

use super::config::BackupTarget;

/// Build the rclone on-the-fly remote path for an S3 target.
/// Format: `:s3:bucket/prefix/name` or `:s3:bucket/name`
fn s3_path(bucket: &str, prefix: &str, name: &str) -> String {
    let prefix = prefix.trim_end_matches('/');
    if prefix.is_empty() {
        format!(":s3:{bucket}/{name}")
    } else {
        format!(":s3:{bucket}/{prefix}/{name}")
    }
}

/// Configure a rclone command for an S3 target.
///
/// Region and endpoint are not secret and go on the command line. The access
/// key and secret key go in the child's environment as
/// `RCLONE_S3_ACCESS_KEY_ID` / `RCLONE_S3_SECRET_ACCESS_KEY`, which rclone
/// reads exactly like the matching flags. As flags they sat in
/// `/proc/<pid>/cmdline`, readable by every local user for the whole upload
/// (#205); a process's environment is readable only by its owner and root.
///
/// Never add `-vv` to these commands: at debug level rclone logs every value
/// it takes from the environment, secret key included.
fn apply_s3_flags(
    cmd: &mut Command,
    region: &str,
    endpoint: &Option<String>,
    access_key: &Option<String>,
    secret_key: &Option<String>,
) {
    cmd.arg("--s3-region").arg(region);
    if let Some(ep) = endpoint {
        cmd.arg("--s3-endpoint").arg(ep);
    }
    if let Some(key) = access_key {
        cmd.env("RCLONE_S3_ACCESS_KEY_ID", key);
    }
    if let Some(secret) = secret_key {
        cmd.env("RCLONE_S3_SECRET_ACCESS_KEY", secret);
    }
}

/// Upload a file to S3.
pub fn upload(data_path: &Path, target: &BackupTarget, name: &str) -> Result<()> {
    let (bucket, region, prefix, endpoint, access_key, secret_key) = match target {
        BackupTarget::S3 {
            bucket,
            region,
            prefix,
            endpoint,
            access_key,
            secret_key,
        } => (
            bucket,
            region,
            prefix.as_deref().unwrap_or(""),
            endpoint,
            access_key,
            secret_key,
        ),
        _ => anyhow::bail!("upload called with non-S3 target"),
    };

    let dest = s3_path(bucket, prefix, name);
    info!("Uploading backup to {dest}");

    let mut cmd = Command::new("rclone");
    cmd.arg("copyto").arg(data_path).arg(&dest);
    apply_s3_flags(&mut cmd, region, endpoint, access_key, secret_key);

    let output = cmd
        .output()
        .context("failed to run `rclone copyto` — is rclone installed?")?;

    if output.status.success() {
        info!("Uploaded backup to {dest}");
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        error!("S3 upload failed: {stderr}");
        anyhow::bail!("S3 upload failed: {stderr}")
    }
}

/// Download a single file from S3 to a local path.
pub fn download(target: &BackupTarget, name: &str, dest_path: &Path) -> Result<()> {
    let (bucket, region, prefix, endpoint, access_key, secret_key) = match target {
        BackupTarget::S3 {
            bucket,
            region,
            prefix,
            endpoint,
            access_key,
            secret_key,
        } => (
            bucket,
            region,
            prefix.as_deref().unwrap_or(""),
            endpoint,
            access_key,
            secret_key,
        ),
        _ => anyhow::bail!("download called with non-S3 target"),
    };

    let src = s3_path(bucket, prefix, name);
    info!("Downloading {src} → {}", dest_path.display());
    // Keys are nested (`master/<date>/…`), so the destination's parent may
    // not exist yet (#200).
    if let Some(parent) = dest_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }

    let mut cmd = Command::new("rclone");
    cmd.arg("copyto").arg(&src).arg(dest_path);
    apply_s3_flags(&mut cmd, region, endpoint, access_key, secret_key);

    let output = cmd
        .output()
        .context("failed to run `rclone copyto` — is rclone installed?")?;

    if output.status.success() {
        info!("Downloaded {src}");
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        error!("S3 download failed: {stderr}");
        anyhow::bail!("S3 download failed: {stderr}")
    }
}

/// List backups in an S3 prefix.
pub fn list_objects(target: &BackupTarget) -> Result<Vec<String>> {
    let (bucket, region, prefix, endpoint, access_key, secret_key) = match target {
        BackupTarget::S3 {
            bucket,
            region,
            prefix,
            endpoint,
            access_key,
            secret_key,
        } => (
            bucket,
            region,
            prefix.as_deref().unwrap_or(""),
            endpoint,
            access_key,
            secret_key,
        ),
        _ => return Ok(vec![]),
    };

    // List every object under the prefix, recursively (#200). Without `-R`,
    // `lsf` returns only the top-level "directories" (`agents/`, `master/`)
    // and no backup could ever be found; `--files-only` drops those entries.
    // `--format p` prints each object's path relative to the prefix.
    let remote_dir = if prefix.is_empty() {
        format!(":s3:{bucket}/")
    } else {
        format!(":s3:{bucket}/{prefix}/")
    };

    let mut cmd = Command::new("rclone");
    cmd.args(["lsf", "-R", "--files-only", &remote_dir, "--format", "p"]);
    apply_s3_flags(&mut cmd, region, endpoint, access_key, secret_key);

    let output = cmd
        .output()
        .context("failed to run `rclone lsf` — is rclone installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("S3 list failed: {stderr}");
    }

    let files: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s3_target(access_key: Option<&str>, secret_key: Option<&str>) -> BackupTarget {
        BackupTarget::S3 {
            bucket: "test-bucket".into(),
            region: "us-east-1".into(),
            prefix: None,
            endpoint: None,
            access_key: access_key.map(String::from),
            secret_key: secret_key.map(String::from),
        }
    }

    #[test]
    fn upload_non_s3_target_errors() {
        let local = BackupTarget::Local {
            path: "/tmp".into(),
        };
        let err = upload(std::path::Path::new("/tmp/x"), &local, "x.db").unwrap_err();
        assert!(err.to_string().contains("non-S3"));
    }

    #[test]
    fn download_non_s3_target_errors() {
        let local = BackupTarget::Local {
            path: "/tmp".into(),
        };
        let err = download(&local, "x.db", std::path::Path::new("/tmp/out")).unwrap_err();
        assert!(err.to_string().contains("non-S3"));
    }

    #[test]
    fn list_objects_non_s3_target_returns_empty() {
        let local = BackupTarget::Local {
            path: "/tmp".into(),
        };
        assert!(list_objects(&local).unwrap().is_empty());
    }

    /// Credentials in the target config must be preserved through the struct
    /// so upload/download/list_objects can pass them as --s3-* flags to rclone.
    #[test]
    fn s3_target_preserves_credentials() {
        let target = s3_target(Some("AKID123"), Some("SECRET456"));
        match &target {
            BackupTarget::S3 {
                access_key,
                secret_key,
                ..
            } => {
                assert_eq!(access_key.as_deref(), Some("AKID123"));
                assert_eq!(secret_key.as_deref(), Some("SECRET456"));
            }
            _ => panic!("expected S3 target"),
        }
    }

    /// A target without credentials is still valid (rclone falls back to its
    /// own credential discovery — env vars, config file, IAM role, etc.).
    #[test]
    fn s3_target_without_credentials_is_valid() {
        let target = s3_target(None, None);
        match &target {
            BackupTarget::S3 {
                access_key,
                secret_key,
                ..
            } => {
                assert!(access_key.is_none());
                assert!(secret_key.is_none());
            }
            _ => panic!("expected S3 target"),
        }
    }

    #[test]
    fn s3_path_without_prefix() {
        assert_eq!(
            s3_path("my-bucket", "", "file.tar.gz"),
            ":s3:my-bucket/file.tar.gz"
        );
    }

    #[test]
    fn s3_path_with_prefix() {
        // trailing slash in prefix must not produce a double slash
        assert_eq!(
            s3_path("my-bucket", "backups/", "file.tar.gz"),
            ":s3:my-bucket/backups/file.tar.gz"
        );
    }

    #[test]
    fn s3_path_with_prefix_no_trailing_slash() {
        assert_eq!(
            s3_path("my-bucket", "backups", "file.tar.gz"),
            ":s3:my-bucket/backups/file.tar.gz"
        );
    }

    /// upload() with a missing source file must return Err (rclone exits non-zero),
    /// not panic.
    #[test]
    fn upload_missing_source_file_returns_error() {
        let target = s3_target(Some("AKID"), Some("SECRET"));
        let result = upload(
            std::path::Path::new("/nonexistent/file.db"),
            &target,
            "file.db",
        );
        assert!(result.is_err(), "upload of a missing file must return Err");
    }

    // --- #205: credentials never on the command line ------------------------

    fn configured(access: Option<&str>, secret: Option<&str>) -> Command {
        let mut cmd = Command::new("rclone");
        apply_s3_flags(
            &mut cmd,
            "nbg1",
            &Some("https://nbg1.your-objectstorage.com".into()),
            &access.map(String::from),
            &secret.map(String::from),
        );
        cmd
    }

    fn env_of(cmd: &Command, key: &str) -> Option<String> {
        cmd.get_envs()
            .find(|(k, _)| *k == key)
            .and_then(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
    }

    #[test]
    fn credentials_are_not_in_the_arguments() {
        let cmd = configured(Some("AKID-PROBE"), Some("SECRET-PROBE"));
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(!args.iter().any(|a| a.contains("AKID-PROBE")), "{args:?}");
        assert!(!args.iter().any(|a| a.contains("SECRET-PROBE")), "{args:?}");
        assert!(!args.iter().any(|a| a.contains("access-key")), "{args:?}");
    }

    #[test]
    fn credentials_go_in_rclones_environment_variables() {
        let cmd = configured(Some("AKID-PROBE"), Some("SECRET-PROBE"));
        assert_eq!(
            env_of(&cmd, "RCLONE_S3_ACCESS_KEY_ID").as_deref(),
            Some("AKID-PROBE")
        );
        assert_eq!(
            env_of(&cmd, "RCLONE_S3_SECRET_ACCESS_KEY").as_deref(),
            Some("SECRET-PROBE")
        );
    }

    #[test]
    fn region_and_endpoint_stay_on_the_command_line() {
        let cmd = configured(None, None);
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "--s3-region",
                "nbg1",
                "--s3-endpoint",
                "https://nbg1.your-objectstorage.com"
            ]
        );
        assert_eq!(env_of(&cmd, "RCLONE_S3_ACCESS_KEY_ID"), None);
    }
}
