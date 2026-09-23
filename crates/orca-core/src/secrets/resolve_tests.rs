use std::collections::HashMap;

use super::super::SecretStore;

fn store(pairs: &[(&str, &str)]) -> (SecretStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut s =
        SecretStore::open_with_key(dir.path().join("s.json"), &dir.path().join("k")).unwrap();
    for (k, v) in pairs {
        s.set(*k, *v).unwrap();
    }
    (s, dir)
}

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn checked_resolution_names_every_missing_secret() {
    // The live example: coturn ran with TURN_SECRET=${secrets.TURN_SECRET}
    // verbatim because the secret was never created.
    let (s, _d) = store(&[("PG_PASS", "hunter2")]);
    let err = s
        .resolve_env_checked(
            &env(&[
                ("TURN_SECRET", "${secrets.TURN_SECRET}"),
                (
                    "URL",
                    "postgres://u:${secrets.PG_PASS}@db/${secrets.DB_NAME}",
                ),
            ]),
            None,
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("DB_NAME, TURN_SECRET"), "{err}");
    assert!(
        !err.contains("hunter2"),
        "values must never appear in errors"
    );
}

#[test]
fn checked_resolution_substitutes_when_everything_exists() {
    let (s, _d) = store(&[("PG_PASS", "hunter2"), ("web.TOKEN", "scoped")]);
    let out = s
        .resolve_env_checked(
            &env(&[
                ("URL", "postgres://u:${secrets.PG_PASS}@db/app"),
                ("TOKEN", "${secrets.TOKEN}"),
                ("PLAIN", "no refs here"),
            ]),
            Some("web"),
        )
        .unwrap();
    assert_eq!(out["URL"], "postgres://u:hunter2@db/app");
    assert_eq!(out["TOKEN"], "scoped");
    assert_eq!(out["PLAIN"], "no refs here");
}

#[test]
fn unchecked_resolution_still_leaves_unknown_refs_verbatim() {
    let (s, _d) = store(&[]);
    let out = s.resolve_env_scoped(&env(&[("A", "x-${secrets.NOPE}-y")]), None);
    assert_eq!(out["A"], "x-${secrets.NOPE}-y");
}

#[test]
fn a_secret_value_containing_a_reference_is_not_expanded_again() {
    // The old scanner re-read substituted text: a value referencing itself
    // looped forever, and one referencing another secret leaked it.
    let (s, _d) = store(&[("LOOP", "${secrets.LOOP}"), ("OTHER", "leak")]);
    let out = s
        .resolve_env_checked(&env(&[("A", "${secrets.LOOP}")]), None)
        .unwrap();
    assert_eq!(out["A"], "${secrets.LOOP}");
    let (s2, _d2) = store(&[("REF", "${secrets.OTHER}"), ("OTHER", "leak")]);
    let out = s2.resolve_env_scoped(&env(&[("B", "${secrets.REF}")]), None);
    assert_eq!(out["B"], "${secrets.OTHER}");
}

#[test]
fn unterminated_reference_is_left_alone() {
    let (s, _d) = store(&[("A", "1")]);
    let out = s
        .resolve_env_checked(&env(&[("X", "${secrets.A} and ${secrets.")]), None)
        .unwrap();
    assert_eq!(out["X"], "1 and ${secrets.");
}
