use ratatui::text::Line;

use super::render::{plural, render_node_ascii};
use crate::api::{DockerNetwork, DomainRoute, NetworkService, NodeNetworks, NodeRole};

/// `plural` is what keeps "1 node" from rendering as "1 nodes". Trivial
/// but easy to silently break by inverting the comparison.
#[test]
fn plural_is_empty_for_one() {
    assert_eq!(plural(0), "s");
    assert_eq!(plural(1), "");
    assert_eq!(plural(2), "s");
    assert_eq!(plural(99), "s");
}

fn line_text(line: &Line) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn fixture_node() -> NodeNetworks {
    NodeNetworks {
        node_id: Some(1),
        hostname: "worker-1".into(),
        role: NodeRole::Agent,
        networks: vec![
            DockerNetwork {
                name: "orca-web".into(),
                services: vec![
                    NetworkService {
                        name: "api".into(),
                        aliases: vec!["api".into()],
                        missing_aliases: vec!["db".into()],
                    },
                    NetworkService {
                        name: "web".into(),
                        aliases: vec!["web".into(), "frontend".into()],
                        missing_aliases: vec![],
                    },
                ],
            },
            DockerNetwork {
                name: "orca-db".into(),
                services: vec![NetworkService {
                    name: "postgres".into(),
                    aliases: vec![],
                    missing_aliases: vec![],
                }],
            },
        ],
        domains: vec![
            DomainRoute {
                domain: "example.com".into(),
                service: "web".into(),
                resolved_ip: Some("1.2.3.4".into()),
            },
            DomainRoute {
                domain: "api.example.com".into(),
                service: "api".into(),
                resolved_ip: None,
            },
        ],
        reachable: true,
    }
}

fn rendered() -> Vec<String> {
    let mut lines = Vec::new();
    render_node_ascii(&fixture_node(), &mut lines);
    lines.iter().map(line_text).collect()
}

/// Locks the tree shape: branch connectors on every child, `└` only on the
/// last sibling, and vertical continuation lines only while siblings follow.
/// This is exactly the geometry that regressed when the prefix logic was
/// first written per-level instead of composed.
#[test]
fn tree_prefixes_compose_across_levels() {
    let lines = rendered();

    // Domains keep the domain → service mapping plus the A-record.
    assert!(lines.contains(&"    ├─ example.com → web  [A: 1.2.3.4]".to_string()));
    assert!(lines.contains(&"    └─ api.example.com → api  [A: ?]".to_string()));

    // Services under a non-last network carry its continuation line.
    assert!(lines.contains(&"    │   ├── api  aliases: api".to_string()));
    assert!(lines.contains(&"    │   └── web  aliases: web, frontend".to_string()));

    // The warning row aligns under the service name, keeping both the
    // network's and the (non-last) service's continuation lines.
    assert!(
        lines.contains(&"    │   │   ⚠ referenced as: db  (add to network aliases)".to_string())
    );

    // The last network gets `└─` and its children a blank continuation
    // column — but still real branch connectors.
    assert!(lines.iter().any(|l| l.starts_with("    └─ orca-db")));
    assert!(lines.contains(&"        └── postgres  aliases: (no aliases)".to_string()));
}

#[test]
fn unreachable_node_renders_placeholder_only() {
    let mut node = fixture_node();
    node.reachable = false;
    let mut lines = Vec::new();
    render_node_ascii(&node, &mut lines);
    let texts: Vec<String> = lines.iter().map(line_text).collect();
    assert_eq!(texts.len(), 2, "header + placeholder, nothing else");
    assert!(texts[1].contains("agent unreachable"));
}
