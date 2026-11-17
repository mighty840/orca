use super::*;

const SENTINEL: &str = "s3cr3t-sentinel";

fn config_with_secrets_everywhere() -> ServiceConfig {
    serde_json::from_value(serde_json::json!({
        "name": "api",
        "image": "registry.example.com/api:1",
        "env": {
            "PLAIN": format!("{SENTINEL}-plain"),
            "REF": "${secrets.DB_PASSWORD}",
            "SCOPED": "${secrets.acme.API_KEY}",
            "EMBEDDED": format!("postgres://u:{SENTINEL}@${{secrets.DB_HOST}}/db"),
            "EMPTY": "",
        },
        "cmd": ["--password", format!("{SENTINEL}-cmd")],
        "backup": { "pre_hook": format!("pg_dump -W {SENTINEL}-hook") },
        "build": { "repo": format!("https://x:{SENTINEL}-build@git.example.com/r") },
        "tls_key": format!("/etc/{SENTINEL}-key.pem"),
    }))
    .expect("valid config")
}

#[test]
fn no_secret_reaches_the_view() {
    let view = ServiceConfigView::from(&config_with_secrets_everywhere());
    let json = serde_json::to_string(&view).unwrap();
    assert!(!json.contains(SENTINEL), "{json}");
}

#[test]
fn env_keys_stay_and_only_whole_references_are_shown() {
    let view = ServiceConfigView::from(&config_with_secrets_everywhere());
    assert_eq!(view.env["PLAIN"], MASKED);
    assert_eq!(view.env["EMBEDDED"], MASKED);
    assert_eq!(view.env["REF"], "${secrets.DB_PASSWORD}");
    assert_eq!(view.env["SCOPED"], "${secrets.acme.API_KEY}");
    assert_eq!(view.env["EMPTY"], "");
    assert_eq!(view.secret_refs, ["DB_HOST", "DB_PASSWORD", "acme.API_KEY"]);
    assert!(view.cmd && view.backup_hook && view.build);
}

#[test]
fn a_value_that_only_looks_like_one_reference_is_masked() {
    for v in ["${secrets.A}${secrets.B}", "${secrets.A}x}", "${secrets.}"] {
        assert_eq!(mask_value(v), MASKED, "{v}");
    }
}
