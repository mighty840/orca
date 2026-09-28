//! Every view renders at any terminal size without panicking (#265).

use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::state::{AppState, View};

fn state_with_data() -> AppState {
    let mut state = AppState::new();
    let services: Vec<crate::api::ServiceStatus> = (0..30)
        .map(|i| {
            serde_json::from_value(serde_json::json!({
                "name": format!("service-with-a-long-name-{i}"),
                "image": "repo.example.com/group/image:1.2.3",
                "runtime": "container",
                "desired_replicas": 1,
                "running_replicas": i % 2,
                "status": if i % 2 == 0 { "stopped" } else { "running" },
                "project": format!("project-{}", i % 4),
                "domains": [format!("svc{i}.example.com")],
            }))
            .expect("service")
        })
        .collect();
    state.services = services;
    state.nodes = (1..=12)
        .map(|id| {
            serde_json::from_value(serde_json::json!({
                "node_id": id,
                "address": format!("10.0.0.{id}:6880"),
                "labels": {"role": "agent"},
                "last_heartbeat": "2026-09-28T07:00:00Z",
                "drain": id == 3,
            }))
            .expect("node")
        })
        .collect();
    state.logs = (0..300)
        .map(|i| format!("line {i} ERROR maybe\n"))
        .collect();
    state
}

#[test]
fn every_view_renders_at_every_size() {
    let views = [
        View::Chat,
        View::Services,
        View::Nodes,
        View::Logs {
            service: "service-with-a-long-name-1".into(),
        },
        View::Detail {
            service: "service-with-a-long-name-1".into(),
        },
        View::Help,
        View::Secrets,
        View::Backups,
        View::Webhooks,
        View::Networks,
        View::Alerts,
        View::Token,
    ];
    for (w, h) in [(20, 6), (40, 12), (80, 24), (120, 40), (220, 60)] {
        for view in &views {
            let mut state = state_with_data();
            state.view = view.clone();
            state.selected_node = 11;
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal
                .draw(|f| super::draw(f, &state))
                .unwrap_or_else(|e| panic!("{view:?} at {w}x{h}: {e}"));
        }
    }
}

#[test]
fn a_narrow_services_table_keeps_names_readable() {
    let mut state = state_with_data();
    state.view = View::Services;
    let mut terminal = Terminal::new(TestBackend::new(70, 20)).unwrap();
    terminal.draw(|f| super::draw(f, &state)).unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    let rows: Vec<String> = screen
        .chars()
        .collect::<Vec<_>>()
        .chunks(70)
        .map(|r| r.iter().collect())
        .collect();
    assert!(
        screen.contains("service-with-a-lo"),
        "names are cut, not squeezed away:\n{}",
        rows.join("\n")
    );
    assert!(
        !screen.contains("RUNTIME"),
        "low-priority columns are dropped"
    );
}
