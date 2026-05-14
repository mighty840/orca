//! Integration test binary for orca-cli E2E tests.
//!
//! Run all E2E tests with:
//!   ORCA_E2E=1 cargo test -p orca-cli --test main -- --ignored
//!
//! Tests require Docker and a built `orca` binary (`cargo build` first).

#[path = "e2e/mod.rs"]
mod harness;

#[path = "e2e/deploy_container.rs"]
mod deploy_container;

#[path = "e2e/scale.rs"]
mod scale;

#[path = "e2e/status.rs"]
mod status;

#[path = "e2e/logs.rs"]
mod logs;

#[path = "e2e/stop.rs"]
mod stop;

#[path = "e2e/redeploy.rs"]
mod redeploy;

#[path = "e2e/rollback.rs"]
mod rollback;

#[path = "e2e/secrets.rs"]
mod secrets;

#[path = "e2e/webhooks.rs"]
mod webhooks;
