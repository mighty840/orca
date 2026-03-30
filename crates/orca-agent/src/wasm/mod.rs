//! Wasm runtime implementing the [`Runtime`] trait via wasmtime.
//!
//! Wasm workloads are lightweight — ~5ms cold start, ~1-5MB memory each.
//! They run inside the orca process (no container overhead) with WASI P2
//! sandbox isolation.

mod runtime;
mod runtime_impl;
mod state;

pub use runtime::WasmRuntime;
