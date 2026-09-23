//! The webhook registry never turns an unreadable file into an empty list
//! that then gets persisted over it (#183).

use super::load_from;

fn entry() -> serde_json::Value {
    serde_json::json!([{
        "repo": "sharang/orca-infra",
        "service_name": "infra",
        "branch": "main",
        "secret": "s3cret"
    }])
}

#[test]
fn missing_file_is_a_first_run() {
    let dir = tempfile::tempdir().unwrap();
    assert!(load_from(&dir.path().join("webhooks.json")).is_empty());
}

#[test]
fn valid_file_loads() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("webhooks.json");
    std::fs::write(&path, entry().to_string()).unwrap();
    let loaded = load_from(&path);
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].repo, "sharang/orca-infra");
}

#[test]
fn unparseable_file_is_moved_aside_not_left_to_be_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("webhooks.json");
    // Truncated mid-write: the old non-atomic persist could leave this.
    let full = entry().to_string();
    let truncated = &full[..full.len() / 2];
    std::fs::write(&path, truncated).unwrap();

    assert!(load_from(&path).is_empty());
    assert!(
        !path.exists(),
        "the damaged file must not stay where persist writes"
    );
    let aside: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().contains("webhooks.json.corrupt-"))
        .collect();
    assert_eq!(aside.len(), 1, "exactly one preserved copy");
    assert_eq!(std::fs::read_to_string(&aside[0]).unwrap(), truncated);
}
