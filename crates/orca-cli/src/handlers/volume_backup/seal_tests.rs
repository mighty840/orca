//! #231: volume tarballs were stored and uploaded in plaintext even with
//! `age_recipients` set.

use super::*;

fn keypair() -> (String, String) {
    let id = age::x25519::Identity::generate();
    (
        id.to_public().to_string(),
        age::secrecy::ExposeSecret::expose_secret(&id.to_string()).to_string(),
    )
}

fn tarball_in(dir: &Path) -> Vec<u8> {
    let data = b"pretend-this-is-a-postgres-dump".repeat(1000);
    std::fs::write(plain_path(dir, "orca-db-data"), &data).unwrap();
    data
}

#[test]
fn sealing_leaves_only_ciphertext() {
    let dir = tempfile::tempdir().unwrap();
    let data = tarball_in(dir.path());
    let (public, private) = keypair();

    seal(&[public], dir.path(), "orca-db-data").unwrap();

    assert!(
        !plain_path(dir.path(), "orca-db-data").exists(),
        "plaintext removed"
    );
    let sealed = sealed_path(dir.path(), "orca-db-data");
    assert_eq!(tarball(dir.path(), "orca-db-data"), Some(sealed.clone()));
    let raw = std::fs::read(&sealed).unwrap();
    assert!(
        !raw.windows(8).any(|w| w == b"postgres"),
        "no plaintext inside"
    );
    assert_eq!(encrypt::decrypt_file(&sealed, &private).unwrap(), data);
}

#[test]
fn without_recipients_the_tarball_stays_as_is() {
    let dir = tempfile::tempdir().unwrap();
    tarball_in(dir.path());

    seal(&[], dir.path(), "orca-db-data").unwrap();

    let plain = plain_path(dir.path(), "orca-db-data");
    assert_eq!(tarball(dir.path(), "orca-db-data"), Some(plain));
}

#[test]
fn a_failed_seal_keeps_no_plaintext() {
    let dir = tempfile::tempdir().unwrap();
    tarball_in(dir.path());

    let err = seal(&["not-an-age-key".into()], dir.path(), "orca-db-data");

    assert!(err.is_err());
    assert_eq!(
        tarball(dir.path(), "orca-db-data"),
        None,
        "nothing left behind"
    );
}

#[test]
fn restoring_a_sealed_tarball_needs_an_identity() {
    let dir = tempfile::tempdir().unwrap();
    tarball_in(dir.path());
    let (public, _) = keypair();
    seal(&[public], dir.path(), "orca-db-data").unwrap();

    let err = plaintext_dir(dir.path(), "orca-db-data", None).unwrap_err();
    assert!(err.to_string().contains("--identity"), "{err}");

    // A plaintext tarball restores from where it is.
    let plain_dir = tempfile::tempdir().unwrap();
    tarball_in(plain_dir.path());
    assert_eq!(
        plaintext_dir(plain_dir.path(), "orca-db-data", None).unwrap(),
        plain_dir.path()
    );
}
