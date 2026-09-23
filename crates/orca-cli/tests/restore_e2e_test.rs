//! #200 end to end: back up one host to S3 (age-encrypted), then restore onto
//! an empty host. It runs the real `orca` binary against a local S3 server
//! (`rclone serve s3`), with orca's own rclone invocation and credentials.
//!
//! Skipped (with a message) where rclone isn't installed.

use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const AK: &str = "orcatestaccesskey";
const SK: &str = "orcatestsecretkey12345";
const BUCKET: &str = "orca-bk";

struct Stub(Child);
impl Drop for Stub {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

fn rclone_available() -> bool {
    Command::new("rclone")
        .arg("version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn start_stub(root: &Path) -> (Stub, String) {
    std::fs::create_dir_all(root.join(BUCKET)).unwrap();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let child = Command::new("rclone")
        .args(["serve", "s3"])
        .arg(root)
        .args(["--addr", &format!("127.0.0.1:{port}")])
        .args(["--auth-key", &format!("{AK},{SK}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < deadline, "rclone serve s3 did not start");
        std::thread::sleep(Duration::from_millis(100));
    }
    (Stub(child), format!("http://127.0.0.1:{port}"))
}

fn config(endpoint: &str, recipient: &str) -> String {
    serde_json::json!({
        "age_recipients": [recipient],
        "targets": [{
            "type": "s3", "bucket": BUCKET, "region": "us-east-1",
            "endpoint": endpoint, "access_key": AK, "secret_key": SK
        }]
    })
    .to_string()
}

fn orca(home: &Path, cfg: &str, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_orca"))
        .args(args)
        .current_dir(home)
        .env("HOME", home)
        .env("ORCA_BACKUP_CONFIG_JSON", cfg)
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

#[test]
fn back_up_to_s3_then_restore_onto_an_empty_host() {
    if !rclone_available() {
        eprintln!("SKIP: rclone is not installed");
        return;
    }
    let s3_root = tempfile::tempdir().unwrap();
    let (_stub, endpoint) = start_stub(s3_root.path());

    let id = age::x25519::Identity::generate();
    let keys = tempfile::tempdir().unwrap();
    let key_file = keys.path().join("orca-backup.key");
    let secret = age::secrecy::ExposeSecret::expose_secret(&id.to_string()).to_string();
    std::fs::write(&key_file, format!("# created by age-keygen\n{secret}\n")).unwrap();
    let cfg = config(&endpoint, &id.to_public().to_string());

    // Host A: the state a master holds.
    let host_a = tempfile::tempdir().unwrap();
    let a = host_a.path().join(".orca");
    std::fs::create_dir_all(&a).unwrap();
    let files = [
        ("master.key", vec![42u8; 32]),
        ("secrets.json", br#"{"secrets":{"A":"x:y"}}"#.to_vec()),
        (
            "webhooks.json",
            br#"[{"repo":"r","service_name":"s"}]"#.to_vec(),
        ),
        ("cluster.db", b"redb-bytes".to_vec()),
    ];
    for (name, body) in &files {
        std::fs::write(a.join(name), body).unwrap();
    }
    let (ok, out) = orca(host_a.path(), &cfg, &["backup", "basic"]);
    assert!(ok, "backup failed:\n{out}");

    // The listing is recursive now: it shows real object keys.
    let (ok, out) = orca(host_a.path(), &cfg, &["backup", "list"]);
    assert!(
        ok && out.contains("master-key_") && out.contains(".age"),
        "{out}"
    );

    // Host B: empty. Without the identity, nothing encrypted can be restored.
    let host_b = tempfile::tempdir().unwrap();
    let (ok, out) = orca(host_b.path(), &cfg, &["backup", "restore-basic", "--force"]);
    assert!(!ok, "restore without identity must fail:\n{out}");
    assert!(out.contains("--identity"), "{out}");

    let (ok, out) = orca(
        host_b.path(),
        &cfg,
        &[
            "backup",
            "restore-basic",
            "--force",
            "--identity",
            key_file.to_str().unwrap(),
        ],
    );
    assert!(ok, "restore failed:\n{out}");
    for (name, body) in &files {
        let restored = std::fs::read(host_b.path().join(".orca").join(name))
            .unwrap_or_else(|e| panic!("{name} not restored ({e}):\n{out}"));
        assert_eq!(&restored, body, "{name} differs after restore");
    }
}
