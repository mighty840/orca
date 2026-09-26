//! Instance creation and readiness helpers for the reconciler.

use std::time::Duration;

use orca_core::runtime::Runtime;
use orca_core::types::{WorkloadSpec, WorkloadStatus};

use crate::state::InstanceState;

/// Create, start, and wait for a workload instance to be ready.
pub(crate) async fn create_and_start_instance(
    runtime: &dyn Runtime,
    spec: &WorkloadSpec,
) -> anyhow::Result<InstanceState> {
    // Keeps an existing same-name container aside and restores it if this
    // one fails to create or start (#174).
    let handle = runtime.create_and_start(spec).await?;

    let host_port = if let Some(port) = spec.port {
        runtime
            .resolve_host_port(&handle, port)
            .await
            .ok()
            .flatten()
    } else {
        None
    };

    let container_address = if let Some(port) = spec.port {
        let network = crate::routes::service_network_name(spec);
        runtime
            .resolve_container_address(&handle, port, &network)
            .await
            .ok()
            .flatten()
    } else {
        None
    };

    // Wait for container to be ready before registering routes: via its host
    // port, or its network address when it publishes none (#193: those used
    // to be routed the instant they started). Uses the readiness probe if
    // configured, falls back to the health path, then to `/`.
    let probe_addr = host_port
        .map(|p| format!("127.0.0.1:{p}"))
        .or_else(|| container_address.clone());
    if let Some(addr) = probe_addr {
        let (path, delay, strict) = if let Some(probe) = &spec.readiness {
            (probe.path.as_str(), probe.initial_delay_secs, true)
        } else {
            match spec.health.as_deref() {
                Some(path) => (path, 2, true),
                None => ("/", 2, false),
            }
        };
        if delay > 0 {
            tokio::time::sleep(Duration::from_secs(delay)).await;
        }
        wait_for_ready(&addr, path, strict).await;
    }

    // If no health/liveness probe is configured, mark as NoCheck so the
    // instance is immediately routable. If probes exist, the health checker
    // will update the state after its first check.
    let initial_health = if spec.health.is_none() && spec.liveness.is_none() {
        orca_core::types::HealthState::NoCheck
    } else {
        orca_core::types::HealthState::Healthy
    };

    Ok(InstanceState {
        handle,
        status: WorkloadStatus::Running,
        host_port,
        container_address,
        health: initial_health,
        is_canary: false,
        started_at: std::time::Instant::now(),
    })
}

/// Wait for a container to answer HTTP before registering routes.
///
/// With `strict` (an explicit readiness or health path), only a 2xx or 3xx
/// counts. Without it, the probe is `/`, and **any** HTTP response means the
/// server is up: a service that doesn't serve `/` answers 404 or 401, and
/// used to wait out the whole budget on every deploy (#193: 428 times in two
/// weeks on breakpilot).
pub(crate) async fn wait_for_ready(addr: &str, path: &str, strict: bool) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .no_proxy()
        .build()
        .unwrap();
    let url = format!("http://{addr}{path}");
    let started = std::time::Instant::now();

    for attempt in 1..=30 {
        match client.get(&url).send().await {
            Ok(resp) if ready(resp.status(), strict) => {
                tracing::debug!("Container ready at {addr} (attempt {attempt})");
                return;
            }
            _ => {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
    tracing::warn!(
        "Container at {addr} not ready after {}s, registering route anyway",
        started.elapsed().as_secs()
    );
}

/// Whether a probe response means ready; see [`wait_for_ready`].
fn ready(status: reqwest::StatusCode, strict: bool) -> bool {
    !strict || status.is_success() || status.is_redirection()
}

#[cfg(test)]
mod tests {
    use reqwest::StatusCode;

    use super::ready;

    #[test]
    fn without_a_health_path_any_answer_means_ready() {
        for status in [
            StatusCode::OK,
            StatusCode::NOT_FOUND,
            StatusCode::UNAUTHORIZED,
        ] {
            assert!(ready(status, false), "{status}");
        }
    }

    /// A server that answers 404 to everything, like a service without `/`.
    async fn not_found_server() -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = s.read(&mut buf).await;
                    let _ = s
                        .write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n")
                        .await;
                });
            }
        });
        addr
    }

    /// #193: this used to wait out the whole budget on every deploy.
    #[tokio::test]
    async fn a_service_answering_404_is_ready_at_once_without_a_health_path() {
        let addr = not_found_server().await;
        let waited = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            super::wait_for_ready(&addr, "/", false),
        )
        .await;
        assert!(waited.is_ok(), "must not wait for a 2xx");

        // With an explicit health path, a 404 is still not ready.
        let strict = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            super::wait_for_ready(&addr, "/healthz", true),
        )
        .await;
        assert!(strict.is_err(), "a health path must answer 2xx/3xx");
    }

    #[test]
    fn a_health_path_must_answer_2xx_or_3xx() {
        assert!(ready(StatusCode::OK, true));
        assert!(ready(StatusCode::FOUND, true));
        assert!(!ready(StatusCode::NOT_FOUND, true));
        assert!(!ready(StatusCode::SERVICE_UNAVAILABLE, true));
    }
}
