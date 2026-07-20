//! Unit tests for ServiceConfig validation and spec_matches.

use super::*;
use super::*;

fn base_config() -> ServiceConfig {
    ServiceConfig {
        restart_policy: None,
        name: "test-svc".into(),
        project: None,
        runtime: RuntimeKind::Container,
        image: Some("nginx:latest".into()),
        module: None,
        replicas: Replicas::Fixed(1),
        port: Some(80),
        host_port: None,
        domain: Some("test.example.com".into()),
        domains: vec![],
        routes: vec!["/*".into()],
        health: Some("/healthz".into()),
        readiness: None,
        liveness: None,
        env: HashMap::from([("KEY".into(), "val".into())]),
        resources: None,
        volume: None,
        deploy: None,
        placement: None,
        network: Some("web".into()),
        aliases: vec!["test".into()],
        mounts: vec!["/host:/container".into()],
        triggers: vec![],
        assets: None,
        build: None,
        tls_cert: None,
        tls_key: None,
        internal: false,
        depends_on: vec![],
        cmd: vec![],
        extra_ports: vec!["8080:80".into()],
        strip_prefix: Some("/api".into()),
        pull_policy: Default::default(),
        backup: None,
    }
}

#[test]
fn identical_configs_match() {
    let a = base_config();
    let b = base_config();
    assert!(a.spec_matches(&b));
}

#[test]
fn image_change_detected() {
    let a = base_config();
    let mut b = base_config();
    b.image = Some("nginx:1.27".into());
    assert!(!a.spec_matches(&b));
}

#[test]
fn env_change_detected() {
    let a = base_config();
    let mut b = base_config();
    b.env.insert("NEW_KEY".into(), "new_val".into());
    assert!(!a.spec_matches(&b));
}

#[test]
fn extra_ports_change_detected() {
    let a = base_config();
    let mut b = base_config();
    b.extra_ports = vec!["9090:90".into()];
    assert!(!a.spec_matches(&b));
}

#[test]
fn mounts_change_detected() {
    let a = base_config();
    let mut b = base_config();
    b.mounts.push("/extra:/path".into());
    assert!(!a.spec_matches(&b));
}

#[test]
fn volume_change_detected() {
    let a = base_config();
    let mut b = base_config();
    b.volume = Some(crate::types::VolumeSpec {
        path: "/data".into(),
        size: None,
    });
    assert!(!a.spec_matches(&b));
}

#[test]
fn domain_change_detected() {
    let a = base_config();
    let mut b = base_config();
    b.domain = Some("new.example.com".into());
    assert!(!a.spec_matches(&b));
}

#[test]
fn all_domains_normalizes_both_forms() {
    let mut c = base_config();
    // Single `domain` → 1-element list.
    assert_eq!(c.all_domains(), vec!["test.example.com".to_string()]);
    // `domains` wins when set.
    c.domain = None;
    c.domains = vec!["a.com".into(), "b.com".into()];
    assert_eq!(c.all_domains(), vec!["a.com".to_string(), "b.com".into()]);
    assert_eq!(c.primary_domain().as_deref(), Some("a.com"));
    // Neither set → empty.
    c.domains = vec![];
    assert!(c.all_domains().is_empty());
    assert_eq!(c.primary_domain(), None);
}

#[test]
fn multi_domain_change_detected() {
    let mut a = base_config();
    let mut b = base_config();
    a.domain = None;
    b.domain = None;
    a.domains = vec!["a.com".into(), "b.com".into()];
    b.domains = vec!["a.com".into()];
    assert!(!a.spec_matches(&b));
}

#[test]
fn validate_rejects_both_domain_and_domains() {
    let mut c = base_config();
    c.domain = Some("a.com".into());
    c.domains = vec!["b.com".into()];
    assert!(c.validate().is_err());
    // Either alone is fine.
    c.domains = vec![];
    assert!(c.validate().is_ok());
    c.domain = None;
    c.domains = vec!["b.com".into()];
    assert!(c.validate().is_ok());
}

/// #89: `network == name` silently broke registration; validate must
/// refuse it loudly.
#[test]
fn validate_restart_policy_formats() {
    let mut c = base_config();
    for ok in [
        "no",
        "always",
        "unless-stopped",
        "on-failure",
        "on-failure:5",
    ] {
        c.restart_policy = Some(ok.into());
        assert!(c.validate().is_ok(), "{ok} should be valid");
    }
    for bad in ["sometimes", "on-failure:", "on-failure:x", "Always"] {
        c.restart_policy = Some(bad.into());
        assert!(c.validate().is_err(), "{bad} should be rejected");
    }
    c.restart_policy = None;
    assert!(c.validate().is_ok());
}

/// restart_policy changes must trigger a redeploy.
#[test]
fn spec_matches_detects_restart_policy_change() {
    let a = base_config();
    let mut b = base_config();
    b.restart_policy = Some("on-failure:5".into());
    assert!(!a.spec_matches(&b));
}

#[test]
fn validate_allows_network_equal_to_name() {
    // #89: the collision is legal (prod runs several such services,
    // including agent-pinned ones) — the breadcrumb is a load-time
    // warning, never a rejection.
    let mut c = base_config();
    c.network = Some(c.name.clone());
    assert!(c.validate().is_ok());
}

#[test]
fn parse_domains_array_from_toml() {
    let toml = r#"
            [[service]]
            name = "web"
            image = "nginx:latest"
            domains = ["apex.example.com", "www.example.com"]
        "#;
    let cfg: ServicesConfig = toml::from_str(toml).unwrap();
    assert_eq!(cfg.service.len(), 1);
    assert_eq!(
        cfg.service[0].all_domains(),
        vec![
            "apex.example.com".to_string(),
            "www.example.com".to_string()
        ]
    );
}

#[test]
fn aliases_change_detected() {
    let a = base_config();
    let mut b = base_config();
    b.aliases.push("new-alias".into());
    assert!(!a.spec_matches(&b));
}

#[test]
fn strip_prefix_change_detected() {
    let a = base_config();
    let mut b = base_config();
    b.strip_prefix = None;
    assert!(!a.spec_matches(&b));
}

#[test]
fn network_change_detected() {
    let a = base_config();
    let mut b = base_config();
    b.network = Some("internal".into());
    assert!(!a.spec_matches(&b));
}

#[test]
fn internal_flag_change_detected() {
    let a = base_config();
    let mut b = base_config();
    b.internal = true;
    assert!(!a.spec_matches(&b));
}

#[test]
fn port_change_detected() {
    let a = base_config();
    let mut b = base_config();
    b.port = Some(8080);
    assert!(!a.spec_matches(&b));
}

#[test]
fn cmd_change_detected() {
    let a = base_config();
    let mut b = base_config();
    b.cmd = vec!["--debug".into()];
    assert!(!a.spec_matches(&b));
}

#[test]
fn non_spec_fields_ignored() {
    let a = base_config();
    let mut b = base_config();
    // These changes should NOT trigger a recreate
    b.name = "different-name".into();
    b.project = Some("other-project".into());
    b.replicas = Replicas::Fixed(5);
    assert!(a.spec_matches(&b));
}

#[test]
fn unresolved_secret_templates_match() {
    let mut a = base_config();
    let mut b = base_config();
    // Both have the same unresolved template — should match
    a.env.insert("TOKEN".into(), "${secrets.MY_TOKEN}".into());
    b.env.insert("TOKEN".into(), "${secrets.MY_TOKEN}".into());
    assert!(a.spec_matches(&b));
}

#[test]
fn resolved_vs_unresolved_differs() {
    let mut a = base_config();
    let mut b = base_config();
    // a has the template, b has a resolved value — should NOT match
    a.env.insert("TOKEN".into(), "${secrets.MY_TOKEN}".into());
    b.env
        .insert("TOKEN".into(), "actual-secret-value-123".into());
    assert!(!a.spec_matches(&b));
}
