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
    let (bucket, region, prefix, endpoint) = match target {
        BackupTarget::S3 {
            bucket,
            region,
            prefix,
            endpoint,
            ..
        } => (bucket, region, prefix.as_deref().unwrap_or(""), endpoint),
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

/// List backups in an S3 prefix.
pub fn list_objects(target: &BackupTarget) -> Result<Vec<String>> {
    let (bucket, region, prefix, endpoint) = match target {
        BackupTarget::S3 {
            bucket,
            region,
            prefix,
            endpoint,
            ..
        } => (bucket, region, prefix.as_deref().unwrap_or(""), endpoint),
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
