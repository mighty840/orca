//! Key material must be owner-only on disk (#186).

use std::os::unix::fs::PermissionsExt;

use super::*;

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn set_mode(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

fn provider(cache: &Path) -> AcmeProvider {
    AcmeProvider::new(
        "ops@example.com".into(),
        cache.to_path_buf(),
        Arc::new(RwLock::new(HashMap::new())),
    )
}

// --- save_cert ------------------------------------------------------------

#[tokio::test]
async fn a_saved_key_and_its_directory_are_owner_only() {
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("certs");

    provider(&cache)
        .save_cert("cloud.example.com", b"CERT", b"KEY")
        .await
        .unwrap();

    assert_eq!(mode(&cache), 0o700);
    assert_eq!(mode(&cache.join("cloud.example.com.key.pem")), 0o600);
    assert_eq!(
        std::fs::read(cache.join("cloud.example.com.key.pem")).unwrap(),
        b"KEY"
    );
    assert_eq!(
        std::fs::read(cache.join("cloud.example.com.cert.pem")).unwrap(),
        b"CERT"
    );
}

#[tokio::test]
async fn renewing_replaces_a_world_readable_key_with_an_owner_only_one() {
    // What a renewal did on hosts upgraded from a version that wrote 0644.
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("certs");
    std::fs::create_dir(&cache).unwrap();
    let key = cache.join("git.example.com.key.pem");
    std::fs::write(&key, b"OLD").unwrap();
    set_mode(&key, 0o644);

    provider(&cache)
        .save_cert("git.example.com", b"CERT", b"NEW")
        .await
        .unwrap();

    assert_eq!(mode(&key), 0o600);
    assert_eq!(std::fs::read(&key).unwrap(), b"NEW");
}

// --- secure_existing_key_material ------------------------------------------

#[test]
fn startup_tightens_keys_directory_and_account_but_not_certificates() {
    // Laid out like the breakpilot master: 0775 directory, keys at 0644 and
    // 0664, ACME account at 0664.
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("certs");
    std::fs::create_dir(&cache).unwrap();
    set_mode(&cache, 0o775);
    for (name, m) in [
        ("a.example.com.key.pem", 0o644),
        ("b.example.com.key.pem", 0o664),
        ("a.example.com.cert.pem", 0o644),
    ] {
        std::fs::write(cache.join(name), b"x").unwrap();
        set_mode(&cache.join(name), m);
    }
    let account = root.path().join("acme-account.json");
    std::fs::write(&account, b"{}").unwrap();
    set_mode(&account, 0o664);

    let tightened = secure_existing_key_material(&cache, &account).unwrap();

    assert_eq!(tightened, 4, "directory, two keys and the account");
    assert_eq!(mode(&cache), 0o700);
    assert_eq!(mode(&cache.join("a.example.com.key.pem")), 0o600);
    assert_eq!(mode(&cache.join("b.example.com.key.pem")), 0o600);
    assert_eq!(mode(&account), 0o600);
    // Certificates are public; their mode is not our business.
    assert_eq!(mode(&cache.join("a.example.com.cert.pem")), 0o644);
}

#[test]
fn startup_tightening_is_idempotent() {
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("certs");
    std::fs::create_dir(&cache).unwrap();
    std::fs::write(cache.join("a.key.pem"), b"x").unwrap();
    set_mode(&cache.join("a.key.pem"), 0o644);
    let account = root.path().join("acme-account.json");

    assert!(secure_existing_key_material(&cache, &account).unwrap() > 0);
    assert_eq!(secure_existing_key_material(&cache, &account).unwrap(), 0);
}

#[test]
fn startup_tightening_skips_paths_that_do_not_exist() {
    let root = tempfile::tempdir().unwrap();
    let got = secure_existing_key_material(
        &root.path().join("no-certs-yet"),
        &root.path().join("no-account-yet.json"),
    )
    .unwrap();
    assert_eq!(got, 0);
}
