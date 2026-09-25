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
