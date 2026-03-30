//! Wasm runtime implementing the [`Runtime`] trait via wasmtime.
//!
//! Wasm workloads are lightweight — ~5ms cold start, ~1-5MB memory each.
//! They run inside the orca process (no container overhead) with WASI P2
//! sandbox isolation.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use tokio::io::AsyncRead;
use tokio::sync::RwLock;
use tracing::{debug, info};
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Engine, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiView};

use orca_core::error::{OrcaError, Result};
use orca_core::runtime::{ExecResult, LogOpts, LogStream, Runtime, WorkloadHandle};
use orca_core::types::{ResourceStats, WorkloadSpec, WorkloadStatus};

/// Per-instance WASI state.
struct WasmState {
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
struct WasmInstance {
    /// The compiled component (shared across instances of the same module).
    component: Component,
    /// Current status.
    status: WorkloadStatus,
    /// Spec used to create this instance.
    spec: WorkloadSpec,
    /// Accumulated log output.
    logs: Vec<String>,
    /// When this instance was started.
    started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Invocation count (for HTTP-triggered workloads).
    invocation_count: u64,
}

/// Wasm runtime backed by wasmtime with WASI P2 support.
pub struct WasmRuntime {
    engine: Engine,
    /// Running instances keyed by runtime_id.
    instances: Arc<RwLock<HashMap<String, WasmInstance>>>,
    /// Compiled component cache keyed by module path/OCI ref.
    component_cache: Arc<RwLock<HashMap<String, Component>>>,
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
    async fn load_component(&self, module_ref: &str) -> Result<Component> {
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

    /// Create a WASI context with the given environment variables.
    fn build_wasi_ctx(env: &HashMap<String, String>) -> WasmState {
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
        let state = Self::build_wasi_ctx(&env);
        let mut store = Store::new(&self.engine, state);

        let mut linker = Linker::new(&self.engine);
        wasmtime_wasi::add_to_linker_async::<WasmState>(&mut linker)
            .map_err(|e| OrcaError::Runtime(format!("linker setup failed: {e}")))?;

        let instance_obj = linker
            .instantiate_async(&mut store, &component)
            .await
            .map_err(|e| OrcaError::Runtime(format!("instantiation failed: {e}")))?;

        // Try to call the "handle" export with (method, path, body) -> string
        // This is a simplified calling convention for M1
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

#[async_trait]
impl Runtime for WasmRuntime {
    fn name(&self) -> &str {
        "wasm"
    }

    async fn create(&self, spec: &WorkloadSpec) -> Result<WorkloadHandle> {
        let component = self.load_component(&spec.image).await?;

        let runtime_id = format!("wasm-{}", uuid::Uuid::now_v7());
        info!("Created Wasm instance {} for {}", runtime_id, spec.name);

        let instance = WasmInstance {
            component,
            status: WorkloadStatus::Creating,
            spec: spec.clone(),
            logs: Vec::new(),
            started_at: None,
            invocation_count: 0,
        };

        let mut instances = self.instances.write().await;
        instances.insert(runtime_id.clone(), instance);

        Ok(WorkloadHandle {
            runtime_id,
            name: format!("orca-wasm-{}", spec.name),
            metadata: HashMap::new(),
        })
    }

    async fn start(&self, handle: &WorkloadHandle) -> Result<()> {
        let mut instances = self.instances.write().await;
        let instance =
            instances
                .get_mut(&handle.runtime_id)
                .ok_or_else(|| OrcaError::WorkloadNotFound {
                    name: handle.runtime_id.clone(),
                })?;

        // For Wasm, "starting" means the component is ready to receive invocations.
        // Unlike containers, there's no long-running process — each HTTP trigger
        // creates a fresh Store and calls the component's export.
        instance.status = WorkloadStatus::Running;
        instance.started_at = Some(Utc::now());
        instance.logs.push(format!(
            "{} Instance started (trigger-based, no background process)",
            Utc::now().format("%H:%M:%S")
        ));

        info!("Wasm instance {} is ready", handle.name);
        Ok(())
    }

    async fn stop(&self, handle: &WorkloadHandle, _timeout: Duration) -> Result<()> {
        let mut instances = self.instances.write().await;
        if let Some(instance) = instances.get_mut(&handle.runtime_id) {
            instance.status = WorkloadStatus::Stopped;
            instance.logs.push(format!(
                "{} Instance stopped (invocations: {})",
                Utc::now().format("%H:%M:%S"),
                instance.invocation_count
            ));
            info!("Stopped Wasm instance {}", handle.name);
        }
        Ok(())
    }

    async fn remove(&self, handle: &WorkloadHandle) -> Result<()> {
        let mut instances = self.instances.write().await;
        instances.remove(&handle.runtime_id);
        info!("Removed Wasm instance {}", handle.name);
        Ok(())
    }

    async fn status(&self, handle: &WorkloadHandle) -> Result<WorkloadStatus> {
        let instances = self.instances.read().await;
        let instance =
            instances
                .get(&handle.runtime_id)
                .ok_or_else(|| OrcaError::WorkloadNotFound {
                    name: handle.runtime_id.clone(),
                })?;
        Ok(instance.status)
    }

    async fn logs(&self, handle: &WorkloadHandle, opts: &LogOpts) -> Result<LogStream> {
        let instances = self.instances.read().await;
        let instance =
            instances
                .get(&handle.runtime_id)
                .ok_or_else(|| OrcaError::WorkloadNotFound {
                    name: handle.runtime_id.clone(),
                })?;

        let tail = opts.tail.unwrap_or(100) as usize;
        let logs: Vec<String> = instance
            .logs
            .iter()
            .rev()
            .take(tail)
            .rev()
            .cloned()
            .collect();

        let text = logs.join("\n") + "\n";
        let cursor = std::io::Cursor::new(text.into_bytes());
        Ok(Box::pin(cursor) as Pin<Box<dyn AsyncRead + Send>>)
    }

    async fn exec(&self, _handle: &WorkloadHandle, _cmd: &[String]) -> Result<ExecResult> {
        Err(OrcaError::Runtime(
            "exec is not supported for Wasm workloads".to_string(),
        ))
    }

    async fn stats(&self, handle: &WorkloadHandle) -> Result<ResourceStats> {
        let instances = self.instances.read().await;
        let _instance =
            instances
                .get(&handle.runtime_id)
                .ok_or_else(|| OrcaError::WorkloadNotFound {
                    name: handle.runtime_id.clone(),
                })?;

        // Wasm instances are ephemeral — stats are approximate.
        // Memory is per-invocation and freed after each call.
        Ok(ResourceStats {
            cpu_percent: 0.0,
            memory_bytes: 0,
            network_rx_bytes: 0,
            network_tx_bytes: 0,
            gpu_stats: Vec::new(),
            timestamp: Utc::now(),
        })
    }

    async fn resolve_host_port(
        &self,
        _handle: &WorkloadHandle,
        _container_port: u16,
    ) -> Result<Option<u16>> {
        // Wasm workloads don't bind ports — they're invoked via HTTP triggers
        // routed through the proxy.
        Ok(None)
    }
}
