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
