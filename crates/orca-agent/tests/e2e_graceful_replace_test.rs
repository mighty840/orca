//! #172: replacing a running container must stop it gracefully (SIGTERM with
//! a grace period), not `docker rm -f` it.
//!
//! Needs Docker. Run with:
//! `cargo test -p orca-agent --test e2e_graceful_replace_test -- --ignored`

use std::time::Duration;

use orca_agent::docker::ContainerRuntime;
use orca_core::runtime::Runtime;
use orca_core::testing::spec;

#[tokio::test]
#[ignore = "needs Docker"]
async fn replacing_a_container_lets_the_old_one_shut_down_cleanly() {
    let out = tempfile::tempdir().unwrap();
    // The container user must be able to write the marker.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(out.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
    }
    let runtime = ContainerRuntime::new().expect("Docker required");
    let mut s = spec(&format!("graceful-replace-{}", std::process::id()));
    s.cmd = vec![
        "sh".into(),
        "-c".into(),
        // Busy-wait in the shell itself so the trap runs promptly on SIGTERM.
        "trap 'echo graceful > /out/marker; exit 0' TERM; while :; do sleep 1 & wait $!; done"
            .into(),
    ];
    s.mounts = vec![format!("{}:/out", out.path().display())];

    let first = runtime.create(&s).await.unwrap();
    runtime.start(&first).await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;

    // A redeploy: same name, so create() must replace the running one.
    let second = runtime.create(&s).await.unwrap();

    let marker = std::fs::read_to_string(out.path().join("marker")).unwrap_or_default();
    let _ = runtime.remove(&second).await;
    assert_eq!(
        marker.trim(),
        "graceful",
        "the old container was killed without running its SIGTERM handler"
    );
}

fn docker_inspect(name: &str, format: &str) -> Option<String> {
    let out = std::process::Command::new("docker")
        .args(["inspect", "-f", format, name])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// #174: a replacement that can't start (here: its host port is taken) must
/// leave the old container running, with the same id, not delete it.
#[tokio::test]
#[ignore = "needs Docker"]
async fn a_replacement_that_fails_to_start_restores_the_old_container() {
    let runtime = ContainerRuntime::new().expect("Docker required");
    let name = format!("failed-replace-{}", std::process::id());
    let container = format!("orca-{name}");
    let mut s = spec(&name);
    s.cmd = vec!["sleep".into(), "300".into()];
    s.port = Some(8080);

    let old = runtime.create_and_start(&s).await.unwrap();

    // Hold a host port so the replacement's start fails.
    let held = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    let mut broken = s.clone();
    broken.host_port = Some(held.local_addr().unwrap().port());
    let result = runtime.create_and_start(&broken).await;

    let id = docker_inspect(&container, "{{.Id}}");
    let running = docker_inspect(&container, "{{.State.Running}}");
    let aside_left = docker_inspect(&format!("{container}.replaced"), "{{.Id}}").is_some();
    let _ = runtime.remove(&old).await;
    drop(held);

    assert!(result.is_err(), "the replacement must fail");
    assert_eq!(
        id.as_deref(),
        Some(old.runtime_id.as_str()),
        "the old container is back"
    );
    assert_eq!(running.as_deref(), Some("true"), "and running");
    assert!(!aside_left, "no aside container left behind");
}
