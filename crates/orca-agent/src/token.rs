//! The cluster token this agent authenticates with (#210).
//!
//! It used to be read from `ORCA_TOKEN` on every request, so it could only
//! change with a restart. It now lives here: set from `ORCA_TOKEN` at start,
//! and replaced in place when the master rotates it. A restart is exactly
//! what rotation must avoid (#209: a restarting agent can take its sites down).

use std::io::Write;
use std::path::Path;
use std::sync::RwLock;

static TOKEN: RwLock<Option<String>> = RwLock::new(None);

/// The token to present to the master: the rotated one if any, else
/// `ORCA_TOKEN` from the environment `orca join` set up.
pub fn current() -> String {
    if let Some(t) = TOKEN.read().unwrap_or_else(|e| e.into_inner()).clone() {
        return t;
    }
    std::env::var("ORCA_TOKEN").unwrap_or_default()
}

/// Use `token` from now on, in this process.
pub fn set(token: String) {
    *TOKEN.write().unwrap_or_else(|e| e.into_inner()) = Some(token);
}

/// What rotating did, reported back to the master.
#[derive(Debug, PartialEq, Eq)]
pub struct Rotated {
    /// The agent's next start will use the new token too.
    pub persisted: bool,
    /// Where it was saved, or what the operator must change.
    pub detail: Option<String>,
}

/// Switch to `new`: in memory right away, and on disk where the next start
/// reads it. `ORCA_TOKEN_SOURCE=file` (set by `orca join`) means the token
/// came from `~/.orca/cluster.token`.
pub fn rotate(new: &str) -> Rotated {
    let old = current();
    set(new.to_string());
    let from_file = std::env::var("ORCA_TOKEN_SOURCE").is_ok_and(|s| s == "file");
    // What this process was started with; rotating never changes it.
    let started_with = std::env::var("ORCA_TOKEN").unwrap_or_default();
    let dir = dirs_next::home_dir()
        .unwrap_or_else(|| ".".into())
        .join(".orca");
    persist(&dir, from_file, &started_with, &old, new)
}

/// Save `new` in `dir`. `~/.orca/cluster.token` is always updated, since the
/// CLI on this node reads it. It is also where the token comes from when
/// `from_file`. Otherwise the token came from the unit: `agent.env` is
/// rewritten only if it holds exactly the old token, since anything else
/// (`--token` in `ExecStart`, a different file) is not orca's to edit.
pub(crate) fn persist(
    dir: &Path,
    from_file: bool,
    started_with: &str,
    old: &str,
    new: &str,
) -> Rotated {
    let token_file = dir.join("cluster.token");
    let mirrored = write_private(&token_file, &format!("{new}\n"));
    if from_file {
        return match mirrored {
            Ok(()) => saved(&token_file),
            Err(e) => not_saved(format!("could not save {}: {e}", token_file.display())),
        };
    }
    // The unit already passes the new token: the operator updated it and
    // restarted, so the next start uses it too.
    if !started_with.is_empty() && started_with == new {
        return Rotated {
            persisted: true,
            detail: Some("started with the new token".into()),
        };
    }

    let env_file = dir.join("agent.env");
    let old_line = format!("ORCA_TOKEN={old}");
    if let Ok(content) = std::fs::read_to_string(&env_file)
        && !old.is_empty()
        && content.lines().any(|l| l.trim() == old_line)
    {
        let updated: String = content
            .lines()
            .map(|l| {
                if l.trim() == old_line {
                    format!("ORCA_TOKEN={new}\n")
                } else {
                    format!("{l}\n")
                }
            })
            .collect();
        return match write_private(&env_file, &updated) {
            Ok(()) => saved(&env_file),
            Err(e) => not_saved(format!("could not update {}: {e}", env_file.display())),
        };
    }
    not_saved(
        "the unit passes the token with --token or ORCA_TOKEN, which orca cannot rewrite. \
         Remove it from the unit and restart the agent: the new token is already in \
         ~/.orca/cluster.token, which the agent then reads. Do this before the agent's \
         next restart."
            .into(),
    )
}

fn saved(path: &Path) -> Rotated {
    Rotated {
        persisted: true,
        detail: Some(format!("saved to {}", path.display())),
    }
}

fn not_saved(detail: String) -> Rotated {
    Rotated {
        persisted: false,
        detail: Some(detail),
    }
}

/// Write `content` to `path` via an owner-only temporary file in the same
/// directory, renamed into place, so a crash never leaves a half-written token.
fn write_private(path: &Path, content: &str) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("token")
    ));
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
#[path = "token_tests.rs"]
mod tests;
