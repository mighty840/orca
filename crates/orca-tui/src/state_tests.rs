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

fn webhook(repo: &str, service: &str) -> crate::api::WebhookEntry {
    serde_json::from_value(serde_json::json!({
        "repo": repo,
        "branch": "main",
        "service_name": service,
    }))
    .expect("webhook entry")
}

#[test]
fn a_webhook_filter_narrows_the_list_the_actions_index() {
    // `x` deletes `visible_webhooks()[selected_webhook]`: with a filter set,
    // row 0 must be the first match, not the first webhook overall.
    let mut state = AppState::new();
    state.webhooks = vec![
        webhook("org/site", "site"),
        webhook("org/api", "api"),
        webhook("org/api-docs", "docs"),
    ];
    state.webhook_filter = "API".into();
    let shown: Vec<_> = state
        .visible_webhooks()
        .iter()
        .map(|w| w.service_name.clone())
        .collect();
    assert_eq!(shown, ["api", "docs"]);
    state.webhook_filter.clear();
    assert_eq!(state.visible_webhooks().len(), 3);
}

#[test]
fn slash_edits_the_current_views_own_filter() {
    use crossterm::event::KeyCode;
    let mut state = AppState::new();
    state.view = View::Webhooks;
    state.selected_webhook = 4;
    crate::input_keys::handle_filter_key(&mut state, KeyCode::Char('a'));
    assert_eq!(state.webhook_filter, "a");
    assert!(state.filter.is_empty(), "the services filter is untouched");
    assert_eq!(
        state.selected_webhook, 0,
        "the cursor starts at the first match"
    );
}
