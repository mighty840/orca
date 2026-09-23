//! Tests for route-level RBAC (#202).

use std::collections::HashSet;

use super::*;

/// Actions `Role::can` understands. A typo in `ROUTE_POLICY` would otherwise
/// silently make a route admin-only.
const KNOWN_ACTIONS: &[&str] = &[
    "status",
    "logs",
    "cluster_info",
    "deploy",
    "stop",
    "scale",
    "rollback",
    "secrets",
    ADMIN_ONLY,
];

/// Routes that authenticate outside the middleware and so are not in the
/// policy table: exempt paths, and the agent WebSocket (admin-gated itself).
fn outside_policy() -> HashSet<&'static str> {
    SKIP_AUTH_PATHS
        .iter()
        .copied()
        .chain(["/api/v1/ws/agent"])
        .collect()
}

/// Every `.route("<path>"` literal in the mounted router sources, including
/// ones whose path sits on the line after `.route(`.
fn mounted_route_paths() -> HashSet<String> {
    let sources = [
        include_str!("api/mod.rs"),
        include_str!("webhook.rs"),
        include_str!("cluster_handlers.rs"),
    ];
    let mut paths = HashSet::new();
    for src in sources {
        let mut rest = src;
        while let Some(at) = rest.find(".route(") {
            rest = &rest[at + ".route(".len()..];
            let trimmed = rest.trim_start();
            if let Some(after_quote) = trimmed.strip_prefix('"')
                && let Some(end) = after_quote.find('"')
            {
                paths.insert(after_quote[..end].to_string());
            }
        }
    }
    paths
}

// --- lookups ----------------------------------------------------------------

#[test]
fn static_routes_resolve_to_their_action() {
    assert_eq!(required_action("POST", Some("/api/v1/deploy")), "deploy");
    assert_eq!(required_action("GET", Some("/api/v1/status")), "status");
    assert_eq!(required_action("GET", Some("/api/v1/secrets")), "secrets");
}

#[test]
fn parameterised_routes_resolve_by_template_not_substring() {
    assert_eq!(
        required_action("POST", Some("/api/v1/services/{name}/scale")),
        "scale"
    );
    assert_eq!(
        required_action("POST", Some("/api/v1/services/{name}/redeploy")),
        "deploy"
    );
    assert_eq!(
        required_action("DELETE", Some("/api/v1/services/{name}")),
        "stop"
    );
}

#[test]
fn an_unmatched_request_is_admin_only() {
    // The fallback has no MatchedPath; it must not be a way around the table.
    assert_eq!(required_action("GET", None), ADMIN_ONLY);
}

#[test]
fn an_unlisted_route_or_method_is_admin_only() {
    assert_eq!(
        required_action("GET", Some("/api/v1/brand-new")),
        ADMIN_ONLY
    );
    // Known path, method nobody classified.
    assert_eq!(
        required_action("DELETE", Some("/api/v1/status")),
        ADMIN_ONLY
    );
}

// --- the routes #202 reported -----------------------------------------------

#[test]
fn the_three_reported_routes_are_admin_only() {
    for (method, route) in [
        ("GET", "/api/v1/services/{name}/exec"),
        ("POST", "/api/v1/webhooks"),
        ("GET", "/api/v1/secrets/usage"),
    ] {
        let action = required_action(method, Some(route));
        assert!(Role::Admin.can(action), "{method} {route}");
        assert!(!Role::Deployer.can(action), "deployer: {method} {route}");
        assert!(!Role::Viewer.can(action), "viewer: {method} {route}");
    }
}

#[test]
fn viewer_keeps_its_read_only_routes() {
    for (method, route) in [
        ("GET", "/metrics"),
        ("GET", "/api/v1/status"),
        ("GET", "/api/v1/services/{name}/logs"),
        ("GET", "/api/v1/cluster/info"),
        ("GET", "/api/v1/alerts"),
        ("GET", "/api/v1/webhooks"),
    ] {
        assert!(
            Role::Viewer.can(required_action(method, Some(route))),
            "{method} {route}"
        );
    }
}

#[test]
fn viewer_cannot_change_anything() {
    for (method, route, _) in ROUTE_POLICY {
        if *method != "GET" && *route != "/api/v1/ask" {
            assert!(
                !Role::Viewer.can(required_action(method, Some(route))),
                "viewer may {method} {route}"
            );
        }
    }
}

// --- the table itself -------------------------------------------------------

#[test]
fn no_route_is_classified_twice() {
    let mut seen = HashSet::new();
    for (method, route, _) in ROUTE_POLICY {
        assert!(seen.insert((method, route)), "duplicate: {method} {route}");
    }
}

#[test]
fn every_action_is_one_role_understands() {
    for (method, route, action) in ROUTE_POLICY {
        assert!(
            KNOWN_ACTIONS.contains(action),
            "{method} {route} uses unknown action {action:?}"
        );
    }
}

#[test]
fn every_mounted_route_is_classified() {
    // Adding a route without a policy entry fails here, instead of shipping
    // as admin-only by accident (or, before #202, as viewer-accessible).
    let classified: HashSet<&str> = ROUTE_POLICY.iter().map(|(_, p, _)| *p).collect();
    let outside = outside_policy();
    let unclassified: Vec<_> = mounted_route_paths()
        .into_iter()
        .filter(|p| !classified.contains(p.as_str()) && !outside.contains(p.as_str()))
        .collect();
    assert!(
        unclassified.is_empty(),
        "mounted but not in ROUTE_POLICY: {unclassified:?}"
    );
}

#[test]
fn every_policy_entry_is_a_mounted_route() {
    let mounted = mounted_route_paths();
    for (method, route, _) in ROUTE_POLICY {
        assert!(
            mounted.contains(*route),
            "{method} {route} is in ROUTE_POLICY but not mounted"
        );
    }
}

#[test]
fn the_route_scanner_finds_multi_line_routes() {
    // Guards the scanner the completeness tests rely on: these paths are
    // written on the line after `.route(`.
    let mounted = mounted_route_paths();
    for route in [
        "/api/v1/services/{name}/exec",
        "/api/v1/secrets/usage",
        "/api/v1/cluster/nodes/{node_id}/undrain",
        "/api/v1/webhooks/{id}/invocations",
    ] {
        assert!(mounted.contains(route), "scanner missed {route}");
    }
}
