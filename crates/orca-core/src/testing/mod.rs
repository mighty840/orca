//! Test utilities for orca crates.
//!
//! Provides [`MockRuntime`] for testing components that need a
//! [`Runtime`](crate::runtime::Runtime) without requiring Docker or wasmtime,
//! and [`spec`] for building a [`WorkloadSpec`](crate::types::WorkloadSpec)
//! without enumerating thirty fields.

mod mock_runtime;
mod spec;

pub use mock_runtime::{MockOp, MockOpKind, MockRuntime};
pub use spec::{spec, spec_with_image};
