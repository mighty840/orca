use std::path::Path;

use orca_core::backup::BackupTarget;

use super::*;

#[test]
fn parse_reads_every_artifact_shape_we_write() {
    let p = parse("master/2026-09-23/secrets_20260923T030000Z.json").unwrap();
    assert_eq!(
        (p.name.as_str(), p.ext.as_str(), p.encrypted),
        ("secrets", "json", false)
    );
    assert_eq!(p.timestamp, "20260923T030000Z");

    let p = parse("master-key_20260923T030000Z.key.age").unwrap();
    assert_eq!(
        (p.name.as_str(), p.ext.as_str(), p.encrypted),
        ("master-key", "key", true)
    );

    // certs/ is stored as <name>_<ts>.gz(.age) from its tarball.
    let p = parse("certs_20260923T030000Z.gz.age").unwrap();
    assert_eq!((p.name.as_str(), p.encrypted), ("certs", true));

    // Volume tarballs have no timestamped name; they're not config artifacts.
    assert!(parse("agents/host/2026-09-23/orca-gitea-db-data.tar.gz").is_none());
    assert!(parse("garbage").is_none());
}

#[test]
fn destinations_are_absolute_and_where_the_server_reads_them() {
    // #200: restore-basic used to copy into the current directory.
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let file = |p: &Path| Some(Destination::File(p.to_path_buf()));
    assert_eq!(
        destination("master-key", h),
        file(&h.join(".orca/master.key"))
    );
    assert_eq!(
        destination("secrets", h),
        file(&h.join(".orca/secrets.json"))
    );
    assert_eq!(
        destination("webhooks", h),
        file(&h.join(".orca/webhooks.json"))
    );
    assert_eq!(
        destination("certs", h),
        Some(Destination::ExtractInto(h.join(".orca")))
    );
    assert_eq!(destination("unknown", h), None);

    // cluster.toml follows the server: into the ~/orca checkout if present.
    assert_eq!(
        destination("cluster", h),
        file(&h.join(".orca/cluster.toml"))
    );
    std::fs::create_dir(h.join("orca")).unwrap();
    assert_eq!(
        destination("cluster", h),
        file(&h.join("orca/cluster.toml"))
    );
}

#[test]
fn restore_order_puts_the_key_before_the_secrets() {
    let pos = |n| RESTORE_ORDER.iter().position(|x| *x == n).unwrap();
    assert!(pos("master-key") < pos("secrets"));
    assert_eq!(RESTORE_ORDER[0], "master-key");
}

#[test]
fn latest_per_name_picks_the_newest_across_targets() {
    let local = BackupTarget::Local { path: "/b".into() };
    let found = |loc: &str| Found {
        parsed: parse(loc).unwrap(),
        location: loc.into(),
        target: local.clone(),
    };
    let latest = latest_per_name(vec![
        found("secrets_20260921T030000Z.json"),
        found("secrets_20260923T030000Z.json"),
        found("secrets_20260922T030000Z.json"),
        found("cluster_20260920T030000Z.toml"),
    ]);
    assert_eq!(latest["secrets"].parsed.timestamp, "20260923T030000Z");
    assert_eq!(latest["cluster"].parsed.timestamp, "20260920T030000Z");
}

#[test]
fn install_moves_the_existing_file_aside_and_writes_owner_only() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("secrets.json");
    assert_eq!(install_file(&dest, b"new").unwrap(), None);

    std::fs::write(&dest, "live").unwrap();
    let aside = install_file(&dest, b"restored").unwrap().unwrap();
    assert_eq!(std::fs::read_to_string(&dest).unwrap(), "restored");
    assert_eq!(std::fs::read_to_string(&aside).unwrap(), "live");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[test]
fn encrypted_artifacts_need_an_identity_and_decrypt_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("plain");
    std::fs::write(&src, "key bytes").unwrap();
    let id = age::x25519::Identity::generate();
    let enc = encrypt::encrypt_to_temp(&src, &[id.to_public().to_string()]).unwrap();

    let err = plaintext(enc.path(), true, None).unwrap_err().to_string();
    assert!(err.contains("--identity"), "{err}");

    // age-keygen writes comment lines above the key; read_identity skips them.
    let key_file = dir.path().join("key.txt");
    let secret = age::secrecy::ExposeSecret::expose_secret(&id.to_string()).to_string();
    std::fs::write(
        &key_file,
        format!("# created: now\n# public key: x\n{secret}\n"),
    )
    .unwrap();
    let identity = read_identity(&key_file).unwrap();
    assert_eq!(
        plaintext(enc.path(), true, Some(&identity)).unwrap(),
        b"key bytes"
    );
}

#[test]
fn server_detection_sees_a_listener() {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    // The guard that matters: a live server is detected. (A just-closed
    // port is not asserted: the kernel may still complete a handshake.)
    assert!(server_running(port));
}
