use std::collections::HashMap;

use super::sops_store::SopsBackend;

/// One combined scenario instead of several tests: the age identity is
/// supplied via the process-global `ROPS_AGE` env var, and parallel tests
/// mutating the environment would race.
#[test]
fn sops_backend_round_trip_edit_and_diff_stability() {
    let identity = age::x25519::Identity::generate();
    let recipient = identity.to_public().to_string();
    // SAFETY: this is the only test in the binary touching ROPS_AGE, and
    // it sets the var before any rops call reads it.
    unsafe {
        std::env::set_var(
            "ROPS_AGE",
            age::secrecy::ExposeSecret::expose_secret(&identity.to_string()),
        )
    };

    let dir = std::env::temp_dir().join(format!("orca-sops-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("secrets.enc.json");

    // git_autocommit=false: the scratch dir is intentionally not a repo,
    // and the write-back is exercised as a no-op via the missing-repo path.
    let backend = SopsBackend::new(path.clone(), vec![recipient], false);

    // Missing file loads as empty.
    assert_eq!(backend.load().unwrap(), HashMap::new());

    // Create: first save encrypts to the configured recipient.
    let mut secrets = HashMap::from([
        ("db_password".to_string(), "hunter2".to_string()),
        ("api_token".to_string(), "tok-123".to_string()),
    ]);
    backend.save(&secrets).unwrap();

    let raw = std::fs::read_to_string(&path).unwrap();
    let on_disk: serde_json::Value = serde_json::from_str(&raw).unwrap();
    // SOPS format: keys plaintext, values ENC[...], metadata present.
    assert!(on_disk.get("sops").is_some(), "sops metadata section");
    let db_cipher = on_disk["db_password"].as_str().unwrap().to_string();
    assert!(db_cipher.starts_with("ENC["), "value must be encrypted");
    assert!(!raw.contains("hunter2"), "plaintext must not hit disk");

    // Round trip.
    assert_eq!(backend.load().unwrap(), secrets);

    // Edit: change one key, add one, remove one.
    secrets.insert("api_token".to_string(), "tok-456".to_string());
    secrets.insert("new_key".to_string(), "fresh".to_string());
    secrets.remove("db_password");
    backend.save(&secrets).unwrap();
    assert_eq!(backend.load().unwrap(), secrets);

    // Diff stability: an untouched value keeps its exact ciphertext across
    // an unrelated mutation (saved data key + nonces are reused), so git
    // diffs show only the keys that actually changed.
    let raw2 = std::fs::read_to_string(&path).unwrap();
    let on_disk2: serde_json::Value = serde_json::from_str(&raw2).unwrap();
    secrets.insert("another".to_string(), "x".to_string());
    backend.save(&secrets).unwrap();
    let on_disk3: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        on_disk2["new_key"], on_disk3["new_key"],
        "untouched value must stay byte-identical across saves"
    );

    // A fresh file with no recipients configured must refuse, not panic.
    let empty = SopsBackend::new(dir.join("other.enc.json"), vec![], false);
    let err = empty.save(&secrets).unwrap_err().to_string();
    assert!(err.contains("age_recipients"), "got: {err}");

    // Missing identity: with no matching age key available, decryption
    // must fail with a hint pointing at the key configuration. (Last in
    // this test — it clears the process-global ROPS_AGE.)
    // SAFETY: same single-test env discipline as the set_var above.
    unsafe { std::env::remove_var("ROPS_AGE") };
    let err = backend.load().unwrap_err().to_string();
    assert!(
        err.contains("age identity") || err.contains("age_key_file"),
        "missing-identity error should point at key config, got: {err}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
