//! S3 backup storage via AWS CLI subprocess.
//!
//! Uses `aws s3 cp` for reliability and broad compatibility
//! (AWS, Minio, R2, B2, any S3-compatible provider).

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use tracing::{error, info};

use super::config::BackupTarget;

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

    let s3_path = if prefix.is_empty() {
        format!("s3://{bucket}/{name}")
    } else {
        format!("s3://{bucket}/{prefix}/{name}")
    };

    info!("Uploading backup to {s3_path}");

    let mut cmd = Command::new("aws");
    cmd.args(["s3", "cp"])
        .arg(data_path)
        .arg(&s3_path)
        .arg("--region")
        .arg(region);

    if let Some(ep) = endpoint {
        cmd.arg("--endpoint-url").arg(ep);
    }
    if let Some(key) = access_key {
        cmd.env("AWS_ACCESS_KEY_ID", key);
    }
    if let Some(secret) = secret_key {
        cmd.env("AWS_SECRET_ACCESS_KEY", secret);
    }

    let output = cmd
        .output()
        .context("failed to run `aws s3 cp` — is AWS CLI installed?")?;

    if output.status.success() {
        info!("Uploaded backup to {s3_path}");
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

    let s3_path = if prefix.is_empty() {
        format!("s3://{bucket}/{name}")
    } else {
        format!("s3://{bucket}/{prefix}/{name}")
    };

    info!("Downloading {s3_path} → {}", dest_path.display());

    let mut cmd = Command::new("aws");
    cmd.args(["s3", "cp", &s3_path])
        .arg(dest_path)
        .arg("--region")
        .arg(region);

    if let Some(ep) = endpoint {
        cmd.arg("--endpoint-url").arg(ep);
    }
    if let Some(key) = access_key {
        cmd.env("AWS_ACCESS_KEY_ID", key);
    }
    if let Some(secret) = secret_key {
        cmd.env("AWS_SECRET_ACCESS_KEY", secret);
    }

    let output = cmd
        .output()
        .context("failed to run `aws s3 cp` — is AWS CLI installed?")?;

    if output.status.success() {
        info!("Downloaded {s3_path}");
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

    let s3_path = if prefix.is_empty() {
        format!("s3://{bucket}/")
    } else {
        format!("s3://{bucket}/{prefix}/")
    };

    let mut cmd = Command::new("aws");
    cmd.args(["s3", "ls", &s3_path, "--region", region]);

    if let Some(ep) = endpoint {
        cmd.arg("--endpoint-url").arg(ep);
    }
    if let Some(key) = access_key {
        cmd.env("AWS_ACCESS_KEY_ID", key);
    }
    if let Some(secret) = secret_key {
        cmd.env("AWS_SECRET_ACCESS_KEY", secret);
    }

    let output = cmd
        .output()
        .context("failed to run `aws s3 ls` — is AWS CLI installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("S3 list failed: {stderr}");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let files: Vec<String> = stdout
        .lines()
        .filter_map(|line| line.split_whitespace().last().map(String::from))
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
        let local = BackupTarget::Local { path: "/tmp".into() };
        let err = upload(std::path::Path::new("/tmp/x"), &local, "x.db").unwrap_err();
        assert!(err.to_string().contains("non-S3"));
    }

    #[test]
    fn download_non_s3_target_errors() {
        let local = BackupTarget::Local { path: "/tmp".into() };
        let err = download(&local, "x.db", std::path::Path::new("/tmp/out")).unwrap_err();
        assert!(err.to_string().contains("non-S3"));
    }

    #[test]
    fn list_objects_non_s3_target_returns_empty() {
        let local = BackupTarget::Local { path: "/tmp".into() };
        assert!(list_objects(&local).unwrap().is_empty());
    }

    /// Credentials in the target config must be preserved through the struct
    /// so upload/download/list_objects can set AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY.
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

    /// A target without credentials is still valid (relies on ambient AWS env/config).
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

    /// upload() with a missing source file must propagate an error from aws CLI,
    /// not panic. This also exercises the credential env-var path.
    #[test]
    fn upload_missing_source_file_returns_error() {
        let target = s3_target(Some("AKID"), Some("SECRET"));
        // /nonexistent definitely does not exist; aws cp will fail.
        // If aws CLI is not installed the context error fires instead — either way an Err.
        let result = upload(std::path::Path::new("/nonexistent/file.db"), &target, "file.db");
        assert!(result.is_err(), "upload of a missing file must return Err");
    }
}
