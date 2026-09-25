use super::*;

fn keypair() -> (String, String) {
    let id = age::x25519::Identity::generate();
    (
        id.to_public().to_string(),
        age::secrecy::ExposeSecret::expose_secret(&id.to_string()).to_string(),
    )
}

#[test]
fn round_trip_and_ciphertext_does_not_contain_the_plaintext() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("cluster.toml");
    let secret = b"[[token]]\nvalue = \"super-secret-cluster-token\"\n";
    std::fs::write(&src, secret).unwrap();
    let (public, private) = keypair();

    let enc = encrypt_to_temp(&src, &[public]).unwrap();
    let raw = std::fs::read(enc.path()).unwrap();
    assert!(raw.starts_with(b"age-encryption.org/v1"));
    assert!(
        !raw.windows(12).any(|w| w == b"super-secret"),
        "plaintext must not survive encryption"
    );
    assert_eq!(decrypt_file(enc.path(), &private).unwrap(), secret);
}

#[test]
fn every_recipient_can_decrypt() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("master.key");
    std::fs::write(&src, [7u8; 32]).unwrap();
    let (pub_a, priv_a) = keypair();
    let (pub_b, priv_b) = keypair();

    let enc = encrypt_to_temp(&src, &[pub_a, pub_b]).unwrap();
    assert_eq!(decrypt_file(enc.path(), &priv_a).unwrap(), [7u8; 32]);
    assert_eq!(decrypt_file(enc.path(), &priv_b).unwrap(), [7u8; 32]);
    let (_, stranger) = keypair();
    assert!(decrypt_file(enc.path(), &stranger).is_err());
}

#[test]
fn a_bad_recipient_is_an_error_not_a_plaintext_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("x");
    std::fs::write(&src, b"x").unwrap();
    let (good, _) = keypair();
    let err = encrypt_to_temp(&src, &[good, "age1typo".into()])
        .unwrap_err()
        .to_string();
    assert!(err.contains("invalid age recipient"), "{err}");
    assert!(encrypt_to_temp(&src, &[]).is_err());
}

/// #231: volume tarballs go through the file-to-file path, which streams.
#[test]
fn file_to_file_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("orca-db-data.tar.gz");
    let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&src, &data).unwrap();
    let (public, private) = keypair();

    let enc = dir.path().join("orca-db-data.tar.gz.age");
    encrypt_file(&src, &enc, &[public]).unwrap();
    assert!(
        std::fs::read(&enc)
            .unwrap()
            .starts_with(b"age-encryption.org/v1")
    );

    let out = dir.path().join("restored.tar.gz");
    decrypt_to_file(&enc, &private, &out).unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), data);

    let (_, stranger) = keypair();
    let bad = dir.path().join("bad.tar.gz");
    assert!(decrypt_to_file(&enc, &stranger, &bad).is_err());
    assert!(!bad.exists(), "no partial plaintext on failure");
}
