use std::os::unix::fs::PermissionsExt;

use super::*;

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn a_new_file_is_owner_only_with_the_right_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("example.key.pem");
    write_private(&path, b"-----BEGIN PRIVATE KEY-----").unwrap();
    assert_eq!(mode(&path), 0o600);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"-----BEGIN PRIVATE KEY-----"
    );
}

#[test]
fn a_world_readable_file_is_replaced_not_rewritten_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("example.key.pem");
    std::fs::write(&path, b"old").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

    write_private(&path, b"new").unwrap();

    assert_eq!(mode(&path), 0o600);
    assert_eq!(std::fs::read(&path).unwrap(), b"new");
}

#[test]
fn no_temporary_file_is_left_behind() {
    let dir = tempfile::tempdir().unwrap();
    write_private(&dir.path().join("a.key.pem"), b"x").unwrap();
    write_private(&dir.path().join("a.key.pem"), b"y").unwrap();
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["a.key.pem"]);
}

#[test]
fn missing_parent_directories_are_created() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("certs/nested/b.key.pem");
    write_private(&path, b"x").unwrap();
    assert_eq!(mode(&path), 0o600);
}

#[test]
fn restrict_tightens_only_what_is_looser() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f");
    std::fs::write(&path, b"x").unwrap();

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(restrict(&path, 0o600).unwrap(), "0644 is looser than 0600");
    assert_eq!(mode(&path), 0o600);

    assert!(!restrict(&path, 0o600).unwrap(), "already 0600");

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
    assert!(
        !restrict(&path, 0o600).unwrap(),
        "0400 is stricter; leave it"
    );
    assert_eq!(mode(&path), 0o400);
}

#[test]
fn a_private_dir_is_owner_only() {
    let dir = tempfile::tempdir().unwrap();
    let certs = dir.path().join("certs");
    std::fs::create_dir(&certs).unwrap();
    std::fs::set_permissions(&certs, std::fs::Permissions::from_mode(0o775)).unwrap();
    create_private_dir(&certs).unwrap();
    assert_eq!(mode(&certs), 0o700);
}

#[test]
fn create_private_new_creates_once_and_never_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("master.key");
    assert!(create_private_new(&path, b"first").unwrap());
    assert!(!create_private_new(&path, b"second").unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), b"first");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    // No temporaries left behind.
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty());
}
