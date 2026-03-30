//! Test utilities for orca crates.
//!
//! Provides [`MockRuntime`] for testing components that need a [`Runtime`]
//! without requiring Docker or wasmtime.

mod mock_runtime;

pub use mock_runtime::MockRuntime;
