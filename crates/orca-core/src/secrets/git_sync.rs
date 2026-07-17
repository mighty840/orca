//! Best-effort git write-back for the encrypted secrets file (#109).
//!
//! The encrypted store lives in the config repo, so a mutation that stays
//! uncommitted drifts from git and a later pull could silently revert it.
//! After each save we commit and push. Every step is best-effort: the
//! secret is already persisted locally, so failures warn loudly instead of
//! failing the mutation.

use std::path::Path;
use std::process::Command;

use tracing::{debug, info, warn};

/// Commit `file` in its containing repo and push. No-op outside a git
/// work tree or when the file is unchanged.
pub(crate) fn autocommit_and_push(file: &Path) {
    let Some(dir) = file.parent() else {
        return;
    };
    if !run(dir, &["rev-parse", "--is-inside-work-tree"]) {
        debug!(
            ?file,
            "secrets file not in a git work tree — skipping write-back"
        );
        return;
    }
    let Some(name) = file.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return;
    };
    if !run(dir, &["add", "--", &name]) {
        warn!(?file, "git add failed — secrets change is NOT committed");
        return;
    }
    // `diff --cached --quiet` exits 0 when nothing is staged.
    if run(dir, &["diff", "--cached", "--quiet", "--", &name]) {
        debug!(?file, "secrets file unchanged — nothing to commit");
        return;
    }
    if !run(dir, &["commit", "-m", "orca: secrets update"]) {
        warn!(?file, "git commit failed — secrets change is NOT committed");
        return;
    }
    let has_remote = output(dir, &["remote"]).is_some_and(|o| !o.trim().is_empty());
    if !has_remote {
        info!(
            ?file,
            "secrets change committed (no remote configured — not pushed)"
        );
        return;
    }
    if run(dir, &["push"]) {
        info!(?file, "secrets change committed and pushed");
        return;
    }
    warn!("git push rejected — retrying after pull --rebase");
    if run(dir, &["pull", "--rebase"]) && run(dir, &["push"]) {
        info!(?file, "secrets change pushed after rebase");
        return;
    }
    warn!(
        ?file,
        "secrets change committed locally but NOT pushed — the config repo \
         has drifted; push manually before the next git pull/reconcile"
    );
}

fn run(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn output(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}
