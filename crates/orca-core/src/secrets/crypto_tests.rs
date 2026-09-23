use super::*;

fn key(b: u8) -> Vec<u8> {
    vec![b; KEY_LEN]
}

#[test]
fn hex_decode_rejects_odd_length_instead_of_panicking() {
    // Every AES value is "<24 hex>:<even hex>", 83 chars for a 13-byte
    // secret. Fed whole to the old hex_decode, it sliced past the end.
    let aes = aes_encrypt(b"13-byte-value", &key(1)).unwrap();
    assert_eq!(aes.len(), 83);
    assert!(hex_decode(&aes).is_err());
    assert!(hex_decode("abc").is_err());
    assert!(hex_decode("zz").is_err());
    assert_eq!(hex_decode("00ff").unwrap(), [0x00, 0xff]);
    assert_eq!(hex_decode("").unwrap(), Vec::<u8>::new());
}

#[test]
fn aes_round_trips_and_fails_cleanly_with_the_wrong_key() {
    let enc = aes_encrypt(b"hunter2", &key(1)).unwrap();
    assert_eq!(aes_decrypt(&enc, &key(1)).unwrap(), b"hunter2");
    let err = aes_decrypt(&enc, &key(2)).unwrap_err().to_string();
    assert!(err.contains("authentication failed"), "{err}");
}

#[test]
fn wrong_length_keys_and_nonces_are_errors_not_panics() {
    // Key::from_slice / Nonce::from_slice panic on a length mismatch.
    assert!(aes_encrypt(b"x", &[0u8; 31]).is_err());
    assert!(aes_encrypt(b"x", &[]).is_err());
    let enc = aes_encrypt(b"x", &key(1)).unwrap();
    assert!(aes_decrypt(&enc, &[0u8; 16]).is_err());
    let (_, ct) = enc.split_once(':').unwrap();
    assert!(aes_decrypt(&format!("abcd:{ct}"), &key(1)).is_err());
}

#[test]
fn format_is_decided_by_shape() {
    let aes = aes_encrypt(b"v", &key(1)).unwrap();
    assert_eq!(classify(&aes), StoredFormat::Aes);
    assert_eq!(classify("6d792d736563726574"), StoredFormat::LegacyXor);
    assert_eq!(classify("not hex"), StoredFormat::Unknown);
    assert_eq!(classify("abc"), StoredFormat::Unknown);
}

#[test]
fn read_key_rejects_a_truncated_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("master.key");
    std::fs::write(&path, [7u8; 16]).unwrap();
    let err = read_key(&path).unwrap_err().to_string();
    assert!(err.contains("16 bytes, expected 32"), "{err}");
    std::fs::write(&path, []).unwrap();
    assert!(read_key(&path).is_err());
}

#[test]
fn create_key_is_owner_only_and_never_replaces_an_existing_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sub").join("master.key");
    let first = create_key(&path).unwrap();
    assert_eq!(first.len(), KEY_LEN);
    // A second creator (e.g. a concurrent process) gets the existing key.
    assert_eq!(create_key(&path).unwrap(), first);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
