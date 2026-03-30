//! WASI state, instance tracking, and context builder.

use std::collections::HashMap;

use wasmtime::component::{Component, ResourceTable};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiView};

use orca_core::types::{WorkloadSpec, WorkloadStatus};

/// Per-instance WASI state.
pub(crate) struct WasmState {
    ctx: WasiCtx,
    table: ResourceTable,
}

impl WasiView for WasmState {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }

    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.ctx
    }
}

/// Tracks a running Wasm instance.
pub(crate) struct WasmInstance {
    /// The compiled component (shared across instances of the same module).
    pub component: Component,
    /// Current status.
    pub status: WorkloadStatus,
    /// Spec used to create this instance.
    pub spec: WorkloadSpec,
    /// Accumulated log output.
    pub logs: Vec<String>,
    /// When this instance was started.
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Invocation count (for HTTP-triggered workloads).
    pub invocation_count: u64,
}

/// Create a WASI context with the given environment variables.
pub(crate) fn build_wasi_ctx(env: &HashMap<String, String>) -> WasmState {
    let mut builder = WasiCtxBuilder::new();

    // Inherit stdio for logging
    builder.inherit_stdio();

    // Set environment variables
    for (key, value) in env {
        builder.env(key, value);
    }

    WasmState {
        ctx: builder.build(),
        table: ResourceTable::new(),
    }
}
