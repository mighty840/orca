use std::collections::HashMap;
use std::sync::Arc;

use tracing::info;

/// Handle the `orca server` command.
pub async fn handle_server(config: &str, proxy_port: u16) -> anyhow::Result<()> {
    let cluster_config = orca_core::config::ClusterConfig::load(config.as_ref())?;
    info!(
        "Starting orca server '{}' (API: {}, Proxy: {})",
        cluster_config.cluster.name, cluster_config.cluster.api_port, proxy_port,
    );

    // Create runtimes
    let container_runtime = Arc::new(orca_agent::docker::ContainerRuntime::new()?);
    let wasm_runtime = match orca_agent::wasm::WasmRuntime::new() {
        Ok(r) => {
            info!("Wasm runtime initialized (wasmtime)");
            Some(Arc::new(r))
        }
        Err(e) => {
            tracing::warn!("Wasm runtime unavailable: {e}");
            None
        }
    };

    // Shared state: route table + wasm triggers
    let route_table = Arc::new(tokio::sync::RwLock::new(HashMap::new()));
    let wasm_triggers: orca_proxy::SharedWasmTriggers =
        Arc::new(tokio::sync::RwLock::new(Vec::new()));

    // Build Wasm invoker callback for the proxy (needs concrete WasmRuntime)
    let wasm_invoker: Option<orca_proxy::WasmInvoker> = wasm_runtime.as_ref().map(|wr| {
        let wr = wr.clone();
        Arc::new(
            move |runtime_id: String, method: String, path: String, body: String| {
                let wr = wr.clone();
                Box::pin(async move {
                    wr.invoke_http(&runtime_id, &method, &path, &body)
                        .await
                        .map_err(|e| e.to_string())
                }) as orca_proxy::WasmInvokeFuture
            },
        ) as orca_proxy::WasmInvoker
    });

    // Cast concrete WasmRuntime to dyn Runtime for the control plane
    let wasm_as_trait: Option<Arc<dyn orca_core::runtime::Runtime>> =
        wasm_runtime.map(|wr| wr as Arc<dyn orca_core::runtime::Runtime>);

    // Spawn proxy
    let proxy_routes = route_table.clone();
    let proxy_triggers = wasm_triggers.clone();
    tokio::spawn(async move {
        if let Err(e) =
            orca_proxy::run_proxy(proxy_routes, proxy_triggers, wasm_invoker, proxy_port, None)
                .await
        {
            tracing::error!("Proxy error: {e}");
        }
    });

    // Run the API server (blocks until shutdown)
    let container_runtime_cleanup = container_runtime.clone();
    orca_control::run_server(
        cluster_config,
        container_runtime,
        wasm_as_trait,
        route_table,
        wasm_triggers,
    )
    .await?;

    // Graceful cleanup
    info!("Shutting down, cleaning up containers...");
    container_runtime_cleanup.cleanup_all().await;
    info!("Shutdown complete");

    Ok(())
}
