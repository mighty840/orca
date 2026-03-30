//! [`WasmRuntime`] struct and core methods (construction, component loading, HTTP invocation).

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::RwLock;
use tracing::{debug, info};
use wasmtime::component::{Component, Linker};
use wasmtime::{Engine, Store};

use orca_core::error::{OrcaError, Result};
use orca_core::types::WorkloadStatus;

use super::state::{WasmInstance, WasmState, build_wasi_ctx};

/// Wasm runtime backed by wasmtime with WASI P2 support.
pub struct WasmRuntime {
    pub(crate) engine: Engine,
    /// Running instances keyed by runtime_id.
    pub(crate) instances: Arc<RwLock<HashMap<String, WasmInstance>>>,
    /// Compiled component cache keyed by module path/OCI ref.
    pub(crate) component_cache: Arc<RwLock<HashMap<String, Component>>>,
}

impl WasmRuntime {
    /// Create a new Wasm runtime.
    ///
    /// # Errors
    ///
    /// Returns an error if the wasmtime engine fails to initialize.
    pub fn new() -> Result<Self> {
        let mut config = wasmtime::Config::new();
        config.async_support(true);
        config.wasm_component_model(true);

        let engine = Engine::new(&config)
            .map_err(|e| OrcaError::Runtime(format!("failed to create wasm engine: {e}")))?;

        Ok(Self {
            engine,
            instances: Arc::new(RwLock::new(HashMap::new())),
            component_cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Load and compile a Wasm component, using the cache if available.
    pub(crate) async fn load_component(&self, module_ref: &str) -> Result<Component> {
        // Check cache first
        {
            let cache = self.component_cache.read().await;
            if let Some(component) = cache.get(module_ref) {
                debug!("Using cached component: {module_ref}");
                return Ok(component.clone());
            }
        }

        // Load from disk or OCI
        let component = if module_ref.starts_with("oci://") {
            // TODO: M1+ — pull OCI artifact
            return Err(OrcaError::Runtime(format!(
                "OCI module refs not yet supported: {module_ref}"
            )));
        } else if module_ref.starts_with("builtin:") {
            // TODO: M1+ — built-in modules (static-server, etc.)
            return Err(OrcaError::Runtime(format!(
                "built-in modules not yet supported: {module_ref}"
            )));
        } else {
            // Load from local filesystem
            info!("Compiling Wasm component: {module_ref}");
            Component::from_file(&self.engine, module_ref)
                .map_err(|e| OrcaError::Runtime(format!("failed to load {module_ref}: {e}")))?
        };

        // Cache it
        {
            let mut cache = self.component_cache.write().await;
            cache.insert(module_ref.to_string(), component.clone());
        }

        Ok(component)
    }

    /// Invoke a Wasm component's exported `handle` function with HTTP-like request data.
    ///
    /// This is the M1 approach: a simple string-in/string-out calling convention.
    /// The component exports `handle(method: string, path: string, body: string) -> string`.
    ///
    /// Future milestones will add wasi:http/incoming-handler support.
    pub async fn invoke_http(
        &self,
        runtime_id: &str,
        method: &str,
        path: &str,
        body: &str,
    ) -> Result<String> {
        let instances = self.instances.read().await;
        let instance = instances
            .get(runtime_id)
            .ok_or_else(|| OrcaError::WorkloadNotFound {
                name: runtime_id.to_string(),
            })?;

        if instance.status != WorkloadStatus::Running {
            return Err(OrcaError::Runtime(format!(
                "wasm instance {} is not running",
                runtime_id
            )));
        }

        let component = instance.component.clone();
        let env = instance.spec.env.clone();
        drop(instances);

        // Create fresh store + linker for this invocation
        let state = build_wasi_ctx(&env);
        let mut store = Store::new(&self.engine, state);

        let mut linker = Linker::new(&self.engine);
        wasmtime_wasi::add_to_linker_async::<WasmState>(&mut linker)
            .map_err(|e| OrcaError::Runtime(format!("linker setup failed: {e}")))?;

        let instance_obj = linker
            .instantiate_async(&mut store, &component)
            .await
            .map_err(|e| OrcaError::Runtime(format!("instantiation failed: {e}")))?;

        // Try to call the "handle" export with (method, path, body) -> string
        let handle_func = instance_obj
            .get_typed_func::<(&str, &str, &str), (String,)>(&mut store, "handle")
            .map_err(|e| {
                OrcaError::Runtime(format!(
                    "component does not export 'handle(string, string, string) -> string': {e}"
                ))
            })?;

        let (response,) = handle_func
            .call_async(&mut store, (method, path, body))
            .await
            .map_err(|e| OrcaError::Runtime(format!("wasm invocation failed: {e}")))?;

        // Update invocation count
        {
            let mut instances = self.instances.write().await;
            if let Some(inst) = instances.get_mut(runtime_id) {
                inst.invocation_count += 1;
                inst.logs.push(format!(
                    "{} {} {} -> {} bytes",
                    Utc::now().format("%H:%M:%S"),
                    method,
                    path,
                    response.len()
                ));
                // Keep last 1000 log entries
                if inst.logs.len() > 1000 {
                    inst.logs.drain(..inst.logs.len() - 1000);
                }
            }
        }

        Ok(response)
    }
}
