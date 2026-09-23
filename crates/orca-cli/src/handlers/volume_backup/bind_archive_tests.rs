//! #185: bind-mount sources are archived unless they are system paths, clean
//! in git, too large, or unreadable, and the archive restores them in place.

use std::path::Path;
use std::process::Command;

use super::super::bind_mounts::UnbackedMount;
use super::*;

fn mount(service: &str, host: &Path, ctr: &str) -> UnbackedMount {
    UnbackedMount {
        service: service.into(),
        host_path: host.display().to_string(),
        container_path: ctr.into(),
    }
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.email=t@t", "-c", "user.name=t"])
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

#[test]
fn system_paths_and_sockets_are_never_archived() {
    for p in [
        "/",
        "/etc/hostname",
        "/var/run/docker.sock",
        "/var/lib/docker/containers",
    ] {
        assert_eq!(classify(Path::new(p), u64::MAX), Decision::System, "{p}");
    }
    #[cfg(unix)]
    {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("s.sock");
        let _l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        assert_eq!(classify(&sock, u64::MAX), Decision::System);
    }
}

#[test]
fn clean_tracked_files_are_in_git_but_edits_and_ignored_data_are_not() {
    let repo = tempfile::tempdir().unwrap();
    let r = repo.path();
    git(r, &["init", "-q"]);
    std::fs::write(r.join("nginx.conf"), "server {}").unwrap();
    std::fs::write(r.join(".gitignore"), "data/\n").unwrap();
    std::fs::create_dir(r.join("conf")).unwrap();
    std::fs::write(r.join("conf/a.yml"), "a: 1").unwrap();
    git(r, &["add", "."]);
    git(r, &["commit", "-qm", "init"]);

    assert_eq!(classify(&r.join("nginx.conf"), u64::MAX), Decision::InGit);
    assert_eq!(classify(&r.join("conf"), u64::MAX), Decision::InGit);

    // A local edit (like the master's uncommitted cluster.toml) must be kept.
    std::fs::write(r.join("nginx.conf"), "server { listen 81; }").unwrap();
    assert_eq!(classify(&r.join("nginx.conf"), u64::MAX), Decision::Archive);

    // Ignored or untracked data beside tracked config isn't in git either.
    std::fs::write(r.join("conf/generated.key"), "secret").unwrap();
    assert_eq!(classify(&r.join("conf"), u64::MAX), Decision::Archive);
    std::fs::create_dir(r.join("data")).unwrap();
    std::fs::write(r.join("data/db"), "x").unwrap();
    assert_eq!(classify(&r.join("data"), u64::MAX), Decision::Archive);
}

#[test]
fn oversized_and_missing_sources_are_gaps() {
    let dir = tempfile::tempdir().unwrap();
    let big = dir.path().join("big");
    std::fs::write(&big, vec![0u8; 4096]).unwrap();
    let d = classify(&big, 1024);
    assert!(
        matches!(d, Decision::TooLarge { .. }) && d.is_gap(),
        "{d:?}"
    );
    let d = classify(&dir.path().join("missing"), u64::MAX);
    assert!(
        matches!(d, Decision::Unreadable { .. }) && d.is_gap(),
        "{d:?}"
    );
}

#[cfg(unix)]
#[test]
fn unreadable_file_is_reported_not_half_archived() {
    use std::os::unix::fs::PermissionsExt;
    if nix_is_root() {
        return; // root reads everything; the check is meaningless there
    }
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("secret");
    std::fs::create_dir(&src).unwrap();
    std::fs::write(src.join("key.pem"), "k").unwrap();
    std::fs::set_permissions(src.join("key.pem"), std::fs::Permissions::from_mode(0o000)).unwrap();
    let d = classify(&src, u64::MAX);
    assert!(matches!(d, Decision::Unreadable { .. }), "{d:?}");
}

fn nix_is_root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .map(|o| o.stdout.starts_with(b"0"))
        .unwrap_or(false)
}

#[test]
fn archive_deduplicates_writes_a_manifest_and_restores_in_place() {
    let src = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let certs = src.path().join("trust-certificates");
    std::fs::create_dir(&certs).unwrap();
    std::fs::write(certs.join("ca.crt"), "CA").unwrap();
    let site = src.path().join("index.html");
    std::fs::write(&site, "<h1>hi</h1>").unwrap();

    // The same source mounted by two services is archived once.
    let mounts = vec![
        mount("harbor-core", &certs, "/harbor_cust_cert"),
        mount("harbor-nginx", &certs, "/harbor_cust_cert"),
        mount("landing", &site, "/usr/share/nginx/html/index.html"),
        mount(
            "vuln-watch",
            Path::new("/var/run/docker.sock"),
            "/var/run/docker.sock",
        ),
    ];
    let (entries, archive) = archive(&mounts, dest.path(), u64::MAX).unwrap();
    assert_eq!(entries.len(), 3);
    let certs_entry = entries
        .iter()
        .find(|e| e.host_path.ends_with("trust-certificates"))
        .unwrap();
    assert_eq!(certs_entry.mounted_by.len(), 2);
    assert!(summary(&entries).starts_with("Bind mounts: 2 archived, 0 in git, 1 system"));

    // Extract into a scratch root and find both sources at their absolute paths.
    let root = tempfile::tempdir().unwrap();
    let ok = Command::new("tar")
        .arg("-xzf")
        .arg(archive.unwrap())
        .arg("-C")
        .arg(root.path())
        .status()
        .unwrap()
        .success();
    assert!(ok);
    let rel = |p: &Path| root.path().join(p.strip_prefix("/").unwrap());
    assert_eq!(
        std::fs::read_to_string(rel(&certs).join("ca.crt")).unwrap(),
        "CA"
    );
    assert_eq!(std::fs::read_to_string(rel(&site)).unwrap(), "<h1>hi</h1>");
    let manifest = std::fs::read_to_string(root.path().join(MANIFEST)).unwrap();
    assert!(manifest.contains("\"decision\": \"system\""), "{manifest}");
}

#[test]
fn nothing_archivable_writes_no_archive_but_still_a_manifest() {
    let dest = tempfile::tempdir().unwrap();
    let mounts = vec![mount("x", Path::new("/"), "/hostfs")];
    let (entries, archive) = archive(&mounts, dest.path(), u64::MAX).unwrap();
    assert!(archive.is_none());
    assert_eq!(entries[0].decision, Decision::System);
    assert!(dest.path().join(MANIFEST).exists());
}
