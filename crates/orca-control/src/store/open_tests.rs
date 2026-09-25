//! #179: the master must refuse to start when its store is unusable.

use super::open_or_refuse;

/// The case seen twice on breakpilot: a second orca holds the lock.
#[test]
fn a_locked_store_is_refused_with_an_explanation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cluster.db");
    let _first = open_or_refuse(&path).unwrap();

    let err = open_or_refuse(&path).err().expect("second open must fail");

    let msg = format!("{err:#}");
    assert!(msg.contains("another orca process"), "{msg}");
    assert!(msg.contains("Refusing to start"), "{msg}");
}
