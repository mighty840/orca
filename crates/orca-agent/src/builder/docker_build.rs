//! Docker image builder: clones git repos and builds images from Dockerfiles.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use orca_core::config::BuildConfig;
use tokio::process::Command;
use tracing::{debug, info};

/// Builds Docker images from source repositories.
pub struct DockerBuilder {
    /// Root directory for build artifacts (e.g., `~/.orca/builds/`).
    builds_dir: PathBuf,
}

impl DockerBuilder {
    /// Create a new builder with the given builds directory.
    pub fn new(builds_dir: PathBuf) -> Self {
        Self { builds_dir }
    }

    /// Create a builder using the default `~/.orca/builds/` directory.
    pub fn default_dir() -> Result<Self> {
        let home = dirs_next::home_dir().context("cannot determine home directory")?;
        let builds_dir = home.join(".orca").join("builds");
        Ok(Self { builds_dir })
    }

    /// Build a service image from its build config. Returns the image tag.
    ///
    /// Steps: clone/pull the repo, then run `docker build`.
    pub async fn build_service(&self, config: &BuildConfig, service_name: &str) -> Result<String> {
        let dest = self.builds_dir.join(service_name);
        std::fs::create_dir_all(&dest)
            .with_context(|| format!("failed to create build dir: {}", dest.display()))?;

        let commit = self
            .clone_or_pull(&config.repo, config.branch_or_default(), &dest)
            .await?;

        let image_tag = build_image_tag(service_name, &commit);

        let dockerfile = config.dockerfile_or_default();
        let context = config.context_or_default();

        self.build_image(&image_tag, dockerfile, context, &dest)
            .await?;

        info!("Built image {image_tag} for service {service_name}");
        Ok(image_tag)
    }

    /// Clone a repo, or pull latest if already cloned. Returns the HEAD commit hash.
    async fn clone_or_pull(&self, repo: &str, branch: &str, dest: &Path) -> Result<String> {
        if dest.join(".git").exists() {
            info!("Pulling latest for {repo} (branch {branch})");
            run_git(dest, &["fetch", "origin", branch]).await?;
            run_git(dest, &["checkout", branch]).await?;
            run_git(dest, &["reset", "--hard", &format!("origin/{branch}")]).await?;
        } else {
            info!("Cloning {repo} (branch {branch}) into {}", dest.display());
            let dest_str = dest.to_string_lossy();
            run_git(
                dest.parent().unwrap_or(dest),
                &[
                    "clone",
                    "--branch",
                    branch,
                    "--single-branch",
                    repo,
                    &dest_str,
                ],
            )
            .await?;
        }

        let commit = get_head_commit(dest).await?;
        debug!("HEAD commit: {commit}");
        Ok(commit)
    }

    /// Run `docker build` and return the image tag on success.
    async fn build_image(
        &self,
        image_tag: &str,
        dockerfile: &str,
        context: &str,
        build_dir: &Path,
    ) -> Result<()> {
        let context_path = build_dir.join(context);
        let dockerfile_path = build_dir.join(dockerfile);

        info!("Building image {image_tag} (dockerfile: {dockerfile}, context: {context})");

        let output = Command::new("docker")
            .arg("build")
            .arg("-t")
            .arg(image_tag)
            .arg("-f")
            .arg(&dockerfile_path)
            .arg(&context_path)
            .output()
            .await
            .context("failed to run docker build")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("docker build failed for {image_tag}:\n{stderr}");
        }

        Ok(())
    }
}

/// Run a git command in the given directory.
async fn run_git(cwd: &Path, args: &[&str]) -> Result<()> {
    debug!("git {} (in {})", args.join(" "), cwd.display());
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git {} failed:\n{stderr}", args.join(" "));
    }
    Ok(())
}

/// Get the HEAD commit hash of a git repo.
async fn get_head_commit(repo_dir: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_dir)
        .output()
        .await
        .context("failed to run git rev-parse HEAD")?;

    if !output.status.success() {
        anyhow::bail!("git rev-parse HEAD failed");
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Generate the image tag for a service build.
pub fn build_image_tag(service_name: &str, commit_hash: &str) -> String {
    let short = &commit_hash[..commit_hash.len().min(12)];
    format!("orca-build-{service_name}:{short}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_tag_format() {
        let tag = build_image_tag("api", "abc123def456789");
        assert_eq!(tag, "orca-build-api:abc123def456");
    }

    #[test]
    fn image_tag_short_hash() {
        let tag = build_image_tag("web", "abc");
        assert_eq!(tag, "orca-build-web:abc");
    }

    #[test]
    fn build_config_defaults() {
        let config = BuildConfig {
            repo: "git@github.com:org/repo.git".to_string(),
            branch: None,
            dockerfile: None,
            context: None,
        };
        assert_eq!(config.branch_or_default(), "main");
        assert_eq!(config.dockerfile_or_default(), "Dockerfile");
        assert_eq!(config.context_or_default(), ".");
    }
}
