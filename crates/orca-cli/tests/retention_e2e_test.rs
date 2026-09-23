//! #204 end to end against a local S3 server (`rclone serve s3`): S3
//! retention keeps a floor per artifact, never touches foreign objects, is
//! opt-in, and never runs after a failed backup.
//!
//! Skipped (with a message) where rclone isn't installed.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
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

/// A bucket holding 5 old nights of one config artifact and one volume
/// tarball (August 2026, far past a 7-day retention), plus a foreign object.
fn seed_bucket(root: &Path) -> PathBuf {
    let bucket = root.join(BUCKET);
    for d in 1..=5 {
        let date = format!("2026-08-0{d}");
        let master = bucket.join(format!("master/{date}"));
        std::fs::create_dir_all(&master).unwrap();
        std::fs::write(
            master.join(format!("secrets_2026080{d}T030000Z.json")),
            "old",
        )
        .unwrap();
        let agent = bucket.join(format!("agents/host1/{date}"));
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::write(agent.join("orca-db-data.tar.gz"), "old").unwrap();
    }
    std::fs::write(bucket.join("manual-dump.sql"), "operator's own file").unwrap();
    bucket
}

fn exists(bucket: &Path, key: &str) -> bool {
    bucket.join(key).exists()
}

fn run_basic(endpoint: &str, prune_s3: bool, extra_local_target: Option<&Path>) -> (bool, String) {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".orca")).unwrap();
    std::fs::write(home.path().join(".orca/secrets.json"), r#"{"secrets":{}}"#).unwrap();
    let mut targets = vec![serde_json::json!({
        "type": "s3", "bucket": BUCKET, "region": "us-east-1",
        "endpoint": endpoint, "access_key": AK, "secret_key": SK
    })];
    if let Some(p) = extra_local_target {
        targets.push(serde_json::json!({ "type": "local", "path": p.display().to_string() }));
    }
    let cfg = serde_json::json!({
        "retention_days": 7, "keep_min": 2, "prune_s3": prune_s3, "targets": targets
    });
    let out = Command::new(env!("CARGO_BIN_EXE_orca"))
        .args(["backup", "basic"])
        .current_dir(home.path())
        .env("HOME", home.path())
        .env("ORCA_BACKUP_CONFIG_JSON", cfg.to_string())
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
fn s3_retention_keeps_a_floor_per_artifact_and_leaves_foreign_objects() {
    if !rclone_available() {
        eprintln!("SKIP: rclone is not installed");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let bucket = seed_bucket(root.path());
    let (_stub, endpoint) = start_stub(root.path());

    let (ok, out) = run_basic(&endpoint, true, None);
    assert!(ok, "{out}");
    assert!(out.contains("S3 object(s) pruned"), "{out}");

    // secrets: today's new copy + the newest old one (08-05) are the floor of 2.
    for d in 1..=4 {
        assert!(
            !exists(
                &bucket,
                &format!("master/2026-08-0{d}/secrets_2026080{d}T030000Z.json")
            ),
            "day {d}"
        );
    }
    assert!(exists(
        &bucket,
        "master/2026-08-05/secrets_20260805T030000Z.json"
    ));
    // Volume tarballs: no new copy this run, so the newest 2 old ones stay.
    for d in 1..=3 {
        assert!(
            !exists(
                &bucket,
                &format!("agents/host1/2026-08-0{d}/orca-db-data.tar.gz")
            ),
            "day {d}"
        );
    }
    assert!(exists(
        &bucket,
        "agents/host1/2026-08-04/orca-db-data.tar.gz"
    ));
    assert!(exists(
        &bucket,
        "agents/host1/2026-08-05/orca-db-data.tar.gz"
    ));
    assert!(
        exists(&bucket, "manual-dump.sql"),
        "foreign objects are never pruned"
    );
}

#[test]
fn s3_retention_is_opt_in_and_never_runs_after_a_failed_backup() {
    if !rclone_available() {
        eprintln!("SKIP: rclone is not installed");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let bucket = seed_bucket(root.path());
    let (_stub, endpoint) = start_stub(root.path());
    let all_old = |b: &Path| {
        (1..=5).all(|d| exists(b, &format!("agents/host1/2026-08-0{d}/orca-db-data.tar.gz")))
    };

    // Off by default: nothing deleted, and the summary says so.
    let (ok, out) = run_basic(&endpoint, false, None);
    assert!(ok, "{out}");
    assert!(out.contains("S3 not pruned (prune_s3 = false)"), "{out}");
    assert!(all_old(&bucket));

    // On, but the run fails (a local target that is a file): keep everything.
    let blocker = tempfile::NamedTempFile::new().unwrap();
    let (ok, out) = run_basic(&endpoint, true, Some(blocker.path()));
    assert!(!ok, "{out}");
    assert!(out.contains("retention skipped"), "{out}");
    assert!(all_old(&bucket), "a failed run must not prune");
}
