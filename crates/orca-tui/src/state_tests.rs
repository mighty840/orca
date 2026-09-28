use super::*;

fn svc(name: &str) -> ServiceStatus {
    ServiceStatus {
        name: name.into(),
        image: String::new(),
        runtime: "container".into(),
        desired_replicas: 1,
        running_replicas: 1,
        status: "running".into(),
        domain: None,
        domains: vec![],
        project: None,
        memory_usage: None,
        cpu_percent: None,
        node: None,
        memory_limit_bytes: None,
        last_failure: None,
    }
}

fn status(names: &[&str]) -> StatusResponse {
    StatusResponse {
        cluster_name: "c".into(),
        services: names.iter().map(|n| svc(n)).collect(),
    }
}

#[test]
fn the_selection_follows_its_service_when_a_row_appears_above_it() {
    // Before #262 the cursor stayed on row 1, which became "api": `x` then
    // stopped a service the operator never selected.
    let mut state = AppState::new();
    state.update_status(status(&["db", "web", "worker"]));
    state.selected_service = 1;
    assert_eq!(state.selected_service_name(), Some("web"));

    state.update_status(status(&["api", "db", "web", "worker"]));
    assert_eq!(state.selected_service_name(), Some("web"));
}

#[test]
fn the_selection_follows_its_service_when_a_row_above_it_goes() {
    let mut state = AppState::new();
    state.update_status(status(&["api", "db", "web"]));
    state.selected_service = 2;
    state.update_status(status(&["db", "web"]));
    assert_eq!(state.selected_service_name(), Some("web"));
}

#[test]
fn a_removed_selection_clamps_to_the_last_row() {
    let mut state = AppState::new();
    state.update_status(status(&["api", "db", "web"]));
    state.selected_service = 2;
    state.update_status(status(&["api", "db"]));
    assert_eq!(state.selected_service_name(), Some("db"));
}

fn node(id: u64) -> crate::api::NodeInfo {
    serde_json::from_value(serde_json::json!({
        "node_id": id,
        "address": format!("10.0.0.{id}:6880"),
        "labels": {},
        "last_heartbeat": "2026-09-28T07:00:00Z",
        "drain": false,
    }))
    .expect("node info")
}

fn cluster(ids: &[u64]) -> ClusterInfo {
    ClusterInfo {
        cluster_name: "c".into(),
        node_count: ids.len() as u64,
        nodes: ids.iter().map(|&id| node(id)).collect(),
        version: None,
        commit: None,
    }
}

#[test]
fn the_node_selection_follows_its_node() {
    // `x` drains the selected node: it must not shift to a neighbour when
    // a node joins above it.
    let mut state = AppState::new();
    state.update_cluster(cluster(&[2, 3]));
    state.selected_node = 1;
    state.update_cluster(cluster(&[1, 2, 3]));
    assert_eq!(state.nodes[state.selected_node].node_id, 3);
    state.update_cluster(cluster(&[1]));
    assert_eq!(state.selected_node, 0);
}
