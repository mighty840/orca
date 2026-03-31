//! Path-based routing utilities for Wasm trigger and container routing.

use crate::{RouteTarget, WasmTrigger};

/// Find a Wasm trigger matching the given path.
pub(crate) fn find_matching_trigger<'a>(
    triggers: &'a [WasmTrigger],
    path: &str,
) -> Option<&'a WasmTrigger> {
    triggers.iter().find(|t| path_matches(&t.pattern, path))
}

/// Simple glob-like path matching: "/api/edge/*" matches "/api/edge/foo/bar".
pub(crate) fn path_matches(pattern: &str, path: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        path.starts_with(prefix)
    } else {
        path == pattern
    }
}

/// Check whether a route's `path_pattern` matches the given request path.
///
/// - `None` pattern is a catch-all and matches everything.
/// - `Some(pattern)` delegates to glob-like `path_matches`.
pub fn path_matches_route(route: &RouteTarget, path: &str) -> bool {
    match &route.path_pattern {
        None => true,
        Some(pattern) => path_matches(pattern, path),
    }
}

/// Return the prefix length of a pattern for sorting (longest prefix wins).
fn pattern_prefix_len(pattern: &Option<String>) -> usize {
    match pattern {
        None => 0,
        Some(p) => p.strip_suffix('*').unwrap_or(p).len(),
    }
}

/// Select matching targets from a list for a given request path.
///
/// Finds all targets whose `path_pattern` matches the path, then keeps only
/// those with the longest (most specific) prefix. If no targets have a
/// `path_pattern` set (all `None`), returns all targets unchanged so the
/// caller can apply round-robin as before.
pub(crate) fn select_path_targets(targets: &[RouteTarget], path: &str) -> Vec<RouteTarget> {
    let matched: Vec<&RouteTarget> = targets
        .iter()
        .filter(|t| path_matches_route(t, path))
        .collect();

    if matched.is_empty() {
        return Vec::new();
    }

    // Find the longest prefix length among matches
    let max_len = matched
        .iter()
        .map(|t| pattern_prefix_len(&t.path_pattern))
        .max()
        .unwrap_or(0);

    matched
        .into_iter()
        .filter(|t| pattern_prefix_len(&t.path_pattern) == max_len)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_matches_exact() {
        assert!(path_matches("/api/health", "/api/health"));
        assert!(!path_matches("/api/health", "/api/status"));
    }

    #[test]
    fn test_path_matches_wildcard() {
        assert!(path_matches("/api/edge/*", "/api/edge/foo"));
        assert!(path_matches("/api/edge/*", "/api/edge/foo/bar"));
        assert!(path_matches("/api/edge/*", "/api/edge/"));
        assert!(!path_matches("/api/edge/*", "/api/other/foo"));
    }

    #[test]
    fn test_path_matches_root_wildcard() {
        assert!(path_matches("/*", "/anything"));
        assert!(path_matches("/*", "/"));
    }

    #[test]
    fn test_find_matching_trigger_returns_first_match() {
        let triggers = vec![
            WasmTrigger {
                pattern: "/api/edge/*".into(),
                runtime_id: "wasm-1".into(),
                service_name: "edge-a".into(),
            },
            WasmTrigger {
                pattern: "/api/edge/*".into(),
                runtime_id: "wasm-2".into(),
                service_name: "edge-b".into(),
            },
        ];
        let matched = find_matching_trigger(&triggers, "/api/edge/foo").unwrap();
        assert_eq!(matched.runtime_id, "wasm-1");
    }

    #[test]
    fn test_find_matching_trigger_no_match_returns_none() {
        let triggers = vec![WasmTrigger {
            pattern: "/api/edge/*".into(),
            runtime_id: "wasm-1".into(),
            service_name: "edge-a".into(),
        }];
        assert!(find_matching_trigger(&triggers, "/other/path").is_none());
    }

    #[test]
    fn test_find_matching_trigger_empty_list_returns_none() {
        let triggers: Vec<WasmTrigger> = vec![];
        assert!(find_matching_trigger(&triggers, "/any/path").is_none());
    }

    // --- path_matches_route tests ---

    fn target(addr: &str, pattern: Option<&str>) -> RouteTarget {
        RouteTarget {
            address: addr.to_string(),
            service_name: addr.to_string(),
            path_pattern: pattern.map(String::from),
        }
    }

    #[test]
    fn test_path_matches_route_none_is_catchall() {
        let t = target("a:80", None);
        assert!(path_matches_route(&t, "/anything"));
        assert!(path_matches_route(&t, "/"));
    }

    #[test]
    fn test_path_matches_route_pattern_matches() {
        let t = target("a:80", Some("/admin/*"));
        assert!(path_matches_route(&t, "/admin/login"));
        assert!(path_matches_route(&t, "/admin/"));
        assert!(!path_matches_route(&t, "/api/foo"));
    }

    #[test]
    fn test_path_matches_route_exact() {
        let t = target("a:80", Some("/health"));
        assert!(path_matches_route(&t, "/health"));
        assert!(!path_matches_route(&t, "/health/check"));
    }

    // --- select_path_targets tests ---

    #[test]
    fn test_select_all_none_returns_all() {
        let targets = vec![target("a:80", None), target("b:80", None)];
        let result = select_path_targets(&targets, "/anything");
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_select_longest_prefix_wins() {
        let targets = vec![
            target("api:3000", Some("/api/*")),
            target("api-v1:3000", Some("/api/v1/*")),
            target("storefront:80", None),
        ];
        let result = select_path_targets(&targets, "/api/v1/users");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].address, "api-v1:3000");
    }

    #[test]
    fn test_select_falls_back_to_catchall() {
        let targets = vec![
            target("api:3000", Some("/api/*")),
            target("storefront:80", None),
        ];
        let result = select_path_targets(&targets, "/about");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].address, "storefront:80");
    }

    #[test]
    fn test_select_no_match_returns_empty() {
        let targets = vec![
            target("api:3000", Some("/api/*")),
            target("admin:80", Some("/admin/*")),
        ];
        let result = select_path_targets(&targets, "/other");
        assert!(result.is_empty());
    }

    #[test]
    fn test_select_multiple_same_prefix_roundrobin() {
        let targets = vec![
            target("api-1:3000", Some("/api/*")),
            target("api-2:3000", Some("/api/*")),
        ];
        let result = select_path_targets(&targets, "/api/foo");
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_kitchenasty_example() {
        let targets = vec![
            target("kitchenasty-server:3000", Some("/api/*")),
            target("kitchenasty-admin:80", Some("/admin/*")),
            target("kitchenasty-storefront:80", None),
        ];

        let api = select_path_targets(&targets, "/api/products");
        assert_eq!(api.len(), 1);
        assert_eq!(api[0].address, "kitchenasty-server:3000");

        let admin = select_path_targets(&targets, "/admin/login");
        assert_eq!(admin.len(), 1);
        assert_eq!(admin[0].address, "kitchenasty-admin:80");

        let store = select_path_targets(&targets, "/");
        assert_eq!(store.len(), 1);
        assert_eq!(store[0].address, "kitchenasty-storefront:80");
    }
}
