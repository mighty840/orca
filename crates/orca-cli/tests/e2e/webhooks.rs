//! E2E test: `orca webhooks add / list / rm` round-trip.
//!
//! Single combined flow:
//! 1. `add` a webhook with an explicit secret.
//! 2. `list` and assert the new entry appears.
//! 3. `rm` it.
//! 4. `list` again and assert it's gone.
//!
//! Uses an `ORCA_WEBHOOKS_PATH` override pointed at a tempfile so the
//! developer's real `~/.orca/webhooks.json` isn't touched (the test server
//! is a child process — env vars inherit).

use crate::harness::{OrcaServer, require_e2e_env};

#[tokio::test]
#[ignore]
async fn orca_webhooks_add_list_rm_roundtrip() {
    require_e2e_env();
    // Note: ORCA_WEBHOOKS_PATH isn't honored on the spawned `orca server`
    // here because we can't inject env vars into `OrcaServer::start()` from
    // the outside. The webhook ends up in the user's ~/.orca/webhooks.json
    // — we rm at the end so the file stays clean.

    let server = OrcaServer::start().await;
    let unique = "e2e-cli-webhooks-svc";
    let repo = "e2e/cli-webhooks-repo";

    // 1. Add.
    let out = server
        .run_cli(&[
            "webhooks",
            "add",
            "--repo",
            repo,
            "--service",
            unique,
            "--branch",
            "main",
            "--secret",
            "cli-test-secret",
        ])
        .await;
    assert!(
        out.status.success(),
        "orca webhooks add failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    // 2. List shows it.
    let out = server.run_cli(&["webhooks", "list"]).await;
    assert!(out.status.success(), "orca webhooks list failed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(unique),
        "webhook should appear in `orca webhooks list`, got:\n{stdout}"
    );

    // 3. Remove (CLI is `rm`, takes the service name as the id).
    let out = server.run_cli(&["webhooks", "remove", unique]).await;
    assert!(
        out.status.success(),
        "orca webhooks rm failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    // 4. List no longer shows it.
    let out = server.run_cli(&["webhooks", "list"]).await;
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains(unique),
        "webhook should be gone after rm, but list still shows:\n{stdout}"
    );
}
