//! Prometheus metrics endpoint.
//!
//! Serves `/metrics` in Prometheus text exposition format.
//! This endpoint is unauthenticated so Prometheus can scrape it.

use std::fmt::Write;
use std::sync::Arc;

use axum::extract::State;
use axum::http::header;
use axum::response::IntoResponse;

use orca_core::types::WorkloadStatus;

use crate::state::AppState;

/// Handler for `GET /metrics` — returns Prometheus text format.
pub async fn metrics_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut out = String::with_capacity(1024);
    let services = state.services.read().await;

    // orca_services_total
    let _ = writeln!(out, "# HELP orca_services_total Number of services");
    let _ = writeln!(out, "# TYPE orca_services_total gauge");
    let _ = writeln!(out, "orca_services_total {}", services.len());

    // orca_instances_total per service and status
    let _ = writeln!(
        out,
        "# HELP orca_instances_total Instance counts by service and status"
    );
    let _ = writeln!(out, "# TYPE orca_instances_total gauge");
    for svc in services.values() {
        let running = svc
            .instances
            .iter()
            .filter(|i| i.status == WorkloadStatus::Running)
            .count();
        let stopped = svc.instances.len() - running;
        let name = &svc.config.name;
        let _ = writeln!(
            out,
            "orca_instances_total{{service=\"{name}\",status=\"running\"}} {running}"
        );
        let _ = writeln!(
            out,
            "orca_instances_total{{service=\"{name}\",status=\"stopped\"}} {stopped}"
        );
    }
    drop(services);

    // orca_nodes_total
    let nodes = state.registered_nodes.read().await;
    let _ = writeln!(out, "# HELP orca_nodes_total Number of cluster nodes");
    let _ = writeln!(out, "# TYPE orca_nodes_total gauge");
    let _ = writeln!(out, "orca_nodes_total {}", nodes.len());

    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], out)
}
