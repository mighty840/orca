//! Persisted TUI state at `~/.orca/tui-state.json`.
//!
//! Forward-compatible JSON: unknown fields are ignored, missing fields use
//! `Default`. All I/O is best-effort — a missing or unwritable home directory
//! must not break the TUI.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::state::AppState;

const ORCA_DIR: &str = ".orca";
const STATE_FILE: &str = "tui-state.json";

/// Snapshot of TUI state that survives across sessions.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedState {
    /// Project filter that was active when the TUI last exited. `None` means
    /// the user was viewing all services.
    #[serde(default)]
    pub last_project: Option<String>,
}

/// Load persisted state from the default path. Returns `Default::default()`
/// on any failure (no home, missing file, parse error).
pub fn load() -> PersistedState {
    state_path().as_deref().map(load_from).unwrap_or_default()
}

/// Persist the relevant slice of `AppState` to the default path. Errors are
/// swallowed — TUI behavior must not depend on the state file being writable.
pub fn save(state: &AppState) {
    let Some(path) = state_path() else {
        return;
    };
    let _ = save_state_to(&path, state);
}

/// Project the live `AppState` onto the persisted-state schema and write it.
/// Split from `save()` so tests can target the AppState → JSON mapping without
/// having to mock `dirs_next::home_dir`.
fn save_state_to(path: &Path, state: &AppState) -> std::io::Result<()> {
    save_to(
        path,
        &PersistedState {
            last_project: state.project_filter.clone(),
        },
    )
}

fn state_path() -> Option<PathBuf> {
    Some(dirs_next::home_dir()?.join(ORCA_DIR).join(STATE_FILE))
}

fn load_from(path: &Path) -> PersistedState {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return PersistedState::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

fn save_to(path: &Path, state: &PersistedState) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(state).unwrap_or_else(|_| "{}".to_string());
    std::fs::write(path, json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// A saved `last_project` survives a round trip through the JSON file.
    #[test]
    fn round_trips_last_project() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tui-state.json");
        let state = PersistedState {
            last_project: Some("compliance".into()),
        };
        save_to(&path, &state).unwrap();
        assert_eq!(load_from(&path), state);
    }

    /// A missing file is normal on first launch — `load_from` must return the
    /// default and never error.
    #[test]
    fn load_missing_file_returns_default() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("does-not-exist.json");
        assert_eq!(load_from(&path), PersistedState::default());
    }

    /// Corrupt JSON on disk (truncated write, manual edit) must be tolerated:
    /// the TUI silently falls back to the default rather than refusing to launch.
    #[test]
    fn load_corrupt_file_returns_default() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tui-state.json");
        std::fs::write(&path, "{ not valid json").unwrap();
        assert_eq!(load_from(&path), PersistedState::default());
    }

    /// A future binary writes a field this version doesn't know about; we must
    /// still decode the known fields rather than fail closed.
    #[test]
    fn load_with_extra_fields_decodes_known() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tui-state.json");
        std::fs::write(&path, r#"{"last_project":"frontend","future_field":42}"#).unwrap();
        assert_eq!(
            load_from(&path),
            PersistedState {
                last_project: Some("frontend".into()),
            },
        );
    }

    /// `save_to` creates `~/.orca/` (or any other missing parent) on demand
    /// so a fresh install doesn't need to mkdir manually.
    #[test]
    fn save_creates_parent_dir() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested/sub/tui-state.json");
        save_to(
            &path,
            &PersistedState {
                last_project: Some("x".into()),
            },
        )
        .unwrap();
        assert!(path.exists());
    }

    /// `save_state_to` must map `AppState::project_filter` onto
    /// `PersistedState::last_project`. This is the only place that mapping
    /// exists, so the test guards against silent renames on either side.
    #[test]
    fn save_state_to_round_trips_project_filter() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tui-state.json");
        let mut state = AppState::new();
        state.project_filter = Some("compliance".into());

        save_state_to(&path, &state).unwrap();
        assert_eq!(
            load_from(&path),
            PersistedState {
                last_project: Some("compliance".into()),
            },
        );
    }

    /// Clearing the filter (Esc / `:project` with no args) must persist as
    /// `last_project: null` so the next launch starts unfiltered. If we wrote
    /// nothing or skipped the call, the stale value from a prior session
    /// would silently come back.
    #[test]
    fn save_state_to_persists_none_to_clear_stale_value() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tui-state.json");

        // Seed the file with a stale value.
        save_to(
            &path,
            &PersistedState {
                last_project: Some("old".into()),
            },
        )
        .unwrap();

        // User has now cleared the filter — save must overwrite, not append.
        let state = AppState::new();
        save_state_to(&path, &state).unwrap();

        assert_eq!(load_from(&path), PersistedState::default());
    }
}
