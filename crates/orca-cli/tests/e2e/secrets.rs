//! E2E test: `orca secrets list` shows secrets configured via the API.
//!
//! Set a unique secret via the API, run `orca secrets list`, assert the key
//! appears in stdout. Cleanup via DELETE at the end so the developer's
//! `~/.orca/secrets.json` isn't polluted permanently — same pattern as the
//! existing in-process secret-env interpolation E2E.

use serde_json::json;

use crate::harness::{OrcaServer, require_e2e_env};

const SECRET_KEY: &str = "E2E_CLI_SECRETS_LIST_KEY";
const SECRET_VALUE: &str = "list-me-please";

#[tokio::test]
#[ignore]
async fn orca_secrets_list_shows_configured_key() {
    require_e2e_env();
    let server = OrcaServer::start().await;
    let client = server.client();

    let resp = client
        .post(format!("{}/api/v1/secrets/{SECRET_KEY}", server.api_url))
        .json(&json!({ "value": SECRET_VALUE }))
        .send()
        .await
        .expect("set_secret");
    assert!(resp.status().is_success());

    let out = server.run_cli(&["secrets", "list"]).await;
    assert!(
        out.status.success(),
        "orca secrets list exited non-zero: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(SECRET_KEY),
        "expected `{SECRET_KEY}` in `orca secrets list` output, got:\n{stdout}"
    );

    // Cleanup so re-runs don't accumulate.
    let _ = client
        .delete(format!("{}/api/v1/secrets/{SECRET_KEY}", server.api_url))
        .send()
        .await;
}
