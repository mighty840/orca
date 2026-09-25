//! Which cluster token `orca join` uses (#210).
//!
//! The agent used to require `--token` / `ORCA_TOKEN` and copied it into
//! `~/.orca/cluster.token` on every start. An operator who rotated by editing
//! that file, as on the master, had the change silently reverted by the next
//! restart. Now the file is a fallback when no flag or env value is given,
//! and a flag that disagrees with the file is reported instead of silently
//! winning.

use std::path::Path;

use anyhow::{Result, bail};

/// Where the join token came from. On rotation (#210) the agent saves a new
/// token to `~/.orca/cluster.token` itself only when it came from there.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TokenSource {
    Explicit,
    File,
}

impl TokenSource {
    /// The `ORCA_TOKEN_SOURCE` value `orca_agent::token::rotate` reads.
    pub(crate) fn env_value(&self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::File => "file",
        }
    }
}

/// The token to join with: `explicit` (from `--token` or `ORCA_TOKEN`) if
/// given, otherwise the token file at `file`. Never logs the value.
pub(crate) fn resolve_token(
    explicit: Option<String>,
    file: &Path,
) -> Result<(String, TokenSource)> {
    let from_file = std::fs::read_to_string(file)
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    match (explicit.map(|t| t.trim().to_string()), from_file) {
        (Some(t), _) if t.is_empty() => {
            bail!("the cluster token given by --token/ORCA_TOKEN is empty")
        }
        (Some(t), Some(f)) if t != f => {
            tracing::warn!(
                "the cluster token from --token/ORCA_TOKEN differs from {}; the flag wins and \
                 the file is updated to match. To rotate the token, change the unit's \
                 ORCA_TOKEN (or --token), not only this file",
                file.display()
            );
            Ok((t, TokenSource::Explicit))
        }
        (Some(t), _) => Ok((t, TokenSource::Explicit)),
        (None, Some(f)) => Ok((f, TokenSource::File)),
        (None, None) => bail!(
            "no cluster token: pass --token, set ORCA_TOKEN, or put the token in {}",
            file.display()
        ),
    }
}

/// `~/.orca/cluster.token`.
pub(crate) fn token_file() -> std::path::PathBuf {
    dirs_next::home_dir()
        .unwrap_or_else(|| ".".into())
        .join(".orca/cluster.token")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_with(content: Option<&str>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        if let Some(c) = content {
            std::fs::write(dir.path().join("cluster.token"), c).unwrap();
        }
        dir
    }

    #[test]
    fn the_file_is_used_when_no_flag_or_env_is_given() {
        let dir = file_with(Some("from-file\n"));
        let t = resolve_token(None, &dir.path().join("cluster.token")).unwrap();
        assert_eq!(t, ("from-file".into(), TokenSource::File));
    }

    #[test]
    fn an_explicit_token_wins() {
        let dir = file_with(Some("old"));
        let t = resolve_token(Some("new".into()), &dir.path().join("cluster.token")).unwrap();
        assert_eq!(t, ("new".into(), TokenSource::Explicit));
        let empty = file_with(None);
        let t = resolve_token(Some("new".into()), &empty.path().join("cluster.token")).unwrap();
        assert_eq!(t.0, "new");
    }

    #[test]
    fn no_token_anywhere_is_an_error_naming_the_file() {
        let dir = file_with(Some("   \n"));
        let err = resolve_token(None, &dir.path().join("cluster.token")).unwrap_err();
        assert!(err.to_string().contains("cluster.token"), "{err}");
        assert!(resolve_token(Some(" ".into()), &dir.path().join("cluster.token")).is_err());
    }
}
