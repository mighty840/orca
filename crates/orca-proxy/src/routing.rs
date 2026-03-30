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
}
