//! Path-based routing utilities for Wasm trigger matching.

use crate::WasmTrigger;

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
}
