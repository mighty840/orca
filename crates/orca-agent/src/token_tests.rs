use super::*;

#[test]
fn a_file_sourced_token_is_saved_to_the_token_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("cluster.token"), "old\n").unwrap();

    let r = persist(dir.path(), true, "", "old", "new");

    assert!(r.persisted, "{r:?}");
    let saved = std::fs::read_to_string(dir.path().join("cluster.token")).unwrap();
    assert_eq!(saved.trim(), "new");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join("cluster.token"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[test]
fn agent_env_holding_the_old_token_is_rewritten() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("agent.env"), "OTHER=1\nORCA_TOKEN=old\n").unwrap();

    let r = persist(dir.path(), false, "old", "old", "new");

    assert!(r.persisted, "{r:?}");
    let env = std::fs::read_to_string(dir.path().join("agent.env")).unwrap();
    assert_eq!(env, "OTHER=1\nORCA_TOKEN=new\n");
}

/// The breakpilot and platform agents: `--token` in a root-owned ExecStart.
#[test]
fn a_token_orca_cannot_rewrite_is_reported_not_saved() {
    let dir = tempfile::tempdir().unwrap();
    // agent.env exists but holds a different token: not ours to touch.
    std::fs::write(dir.path().join("agent.env"), "ORCA_TOKEN=something-else\n").unwrap();

    let r = persist(dir.path(), false, "old", "old", "new");

    assert!(!r.persisted);
    assert!(r.detail.unwrap().contains("Remove it from the unit"));
    let env = std::fs::read_to_string(dir.path().join("agent.env")).unwrap();
    assert_eq!(env, "ORCA_TOKEN=something-else\n", "left alone");
    // The CLI mirror is still updated.
    let mirror = std::fs::read_to_string(dir.path().join("cluster.token")).unwrap();
    assert_eq!(mirror.trim(), "new");
}

/// After the operator put the new token in the unit and restarted, a resent
/// rotation must count as saved, or `--finish` could never succeed.
#[test]
fn an_agent_started_with_the_new_token_counts_as_saved() {
    let dir = tempfile::tempdir().unwrap();

    let r = persist(dir.path(), false, "new", "new", "new");

    assert!(r.persisted, "{r:?}");
}

#[test]
fn set_replaces_the_token_in_memory() {
    set("rotated".into());
    assert_eq!(current(), "rotated");
}
