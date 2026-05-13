//! Extract `${secrets.KEY}` references from an env-var value.
//!
//! Mirror of the substring scan in `SecretStore::resolve_value`, but returning
//! the parsed references instead of substituting values. Used by the secrets
//! dashboard to build a key → referencing-services index.

/// One `${secrets.KEY}` or `${secrets.scope.KEY}` reference, parsed out of a
/// string. `scope` is `None` for the unprefixed form. Project-scoped secrets
/// don't exist yet (BACKLOG: "Project-level environment variables"), but we
/// parse the syntax so the dashboard surfaces them once they land.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretReference {
    pub scope: Option<String>,
    pub key: String,
}

/// Scan a value for every `${secrets...}` reference. Order is preserved; a
/// value that references the same key twice yields two entries — the dashboard
/// dedupes at the service-level so an env block like `URL=${secrets.X}/${secrets.X}`
/// still counts as one service reference.
pub fn extract_refs(value: &str) -> Vec<SecretReference> {
    const PREFIX: &str = "${secrets.";
    let mut out = Vec::new();
    let mut cursor = 0;
    while let Some(start) = value[cursor..].find(PREFIX) {
        let abs_start = cursor + start;
        let after_prefix = abs_start + PREFIX.len();
        let Some(end_rel) = value[after_prefix..].find('}') else {
            break;
        };
        let inside = &value[after_prefix..after_prefix + end_rel];
        cursor = after_prefix + end_rel + 1;
        // Reject inner braces / nested templates — those aren't a single ref.
        if inside.contains('{') {
            continue;
        }
        let (scope, key) = match inside.split_once('.') {
            Some((s, k)) => (Some(s.to_string()), k.to_string()),
            None => (None, inside.to_string()),
        };
        if key.is_empty() {
            continue;
        }
        out.push(SecretReference { scope, key });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_single_unscoped_ref() {
        let refs = extract_refs("${secrets.STRIPE_KEY}");
        assert_eq!(refs.len(), 1);
        assert!(refs[0].scope.is_none());
        assert_eq!(refs[0].key, "STRIPE_KEY");
    }

    /// Project-scoped form is parsed even though the resolver doesn't yet
    /// support it — so the dashboard surfaces the references the moment
    /// project-scoping lands.
    #[test]
    fn extracts_scoped_ref() {
        let refs = extract_refs("${secrets.prod.DATABASE_URL}");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].scope.as_deref(), Some("prod"));
        assert_eq!(refs[0].key, "DATABASE_URL");
    }

    /// Multiple references in one value all come back, in order.
    #[test]
    fn extracts_multiple_refs() {
        let refs =
            extract_refs("postgres://${secrets.PG_USER}:${secrets.PG_PASS}@db/${secrets.PG_DB}");
        assert_eq!(refs.len(), 3);
        assert_eq!(refs[0].key, "PG_USER");
        assert_eq!(refs[1].key, "PG_PASS");
        assert_eq!(refs[2].key, "PG_DB");
    }

    /// A literal value with no template is empty.
    #[test]
    fn no_refs_in_plain_value() {
        assert!(extract_refs("plain-text-value").is_empty());
    }

    /// Empty key (`${secrets.}`) is rejected — it would never resolve, so
    /// counting it as a reference would just be noise.
    #[test]
    fn empty_key_skipped() {
        assert!(extract_refs("${secrets.}").is_empty());
    }

    /// An unclosed template stops the scan rather than crashing.
    #[test]
    fn unclosed_template_is_safe() {
        assert!(extract_refs("${secrets.UNCLOSED").is_empty());
        let refs = extract_refs("${secrets.OK} then ${secrets.UNCLOSED");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].key, "OK");
    }
}
