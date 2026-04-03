use super::*;
use std::collections::HashMap;

use orca_core::config::ServiceConfig;
use orca_core::types::Replicas;

fn minimal_config(image: Option<String>, module: Option<String>) -> ServiceConfig {
    ServiceConfig {
        name: "test-svc".to_string(),
        project: None,
        runtime: Default::default(),
        image,
        module,
        replicas: Replicas::Fixed(1),
        port: Some(8080),
        domain: Some("test.example.com".to_string()),
        health: None,
        readiness: None,
        liveness: None,
        env: HashMap::new(),
        resources: None,
        volume: None,
        deploy: None,
        placement: None,
        network: None,
        aliases: vec![],
        mounts: vec![],
        routes: vec![],
        host_port: None,
        triggers: Vec::new(),
        assets: None,
        build: None,
        tls_cert: None,
        tls_key: None,
        internal: false,
        depends_on: vec![],
    }
}

#[test]
fn config_to_spec_with_image() {
    let config = minimal_config(Some("nginx:latest".to_string()), None);
    let spec = service_config_to_spec(&config).unwrap();
    assert_eq!(spec.name, "test-svc");
    assert_eq!(spec.image, "nginx:latest");
    assert_eq!(spec.port, Some(8080));
    assert_eq!(spec.domain.as_deref(), Some("test.example.com"));
}

#[test]
fn config_to_spec_errors_without_image_or_module() {
    let config = minimal_config(None, None);
    let result = service_config_to_spec(&config);
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("no image, module, or build config"),
        "unexpected error: {err_msg}"
    );
}

#[test]
fn config_to_spec_with_build_config() {
    let mut config = minimal_config(None, None);
    config.build = Some(orca_core::config::BuildConfig {
        repo: "git@github.com:org/repo.git".to_string(),
        branch: None,
        dockerfile: None,
        context: None,
    });
    let spec = service_config_to_spec(&config).unwrap();
    assert!(spec.image.starts_with("orca-build-test-svc:"));
    assert!(spec.build.is_some());
}

/// Env vars pass through to spec (secret resolution happens client-side in load_dir).
#[test]
fn config_to_spec_passes_env_through() {
    let mut config = minimal_config(Some("nginx:latest".into()), None);
    config.env.insert("KEY".into(), "value".into());
    config.env.insert("SECRET".into(), "${secrets.FOO}".into());

    let spec = service_config_to_spec(&config).unwrap();
    assert_eq!(spec.env["KEY"], "value");
    // Without a secrets.json in cwd, patterns pass through unchanged
    assert_eq!(spec.env["SECRET"], "${secrets.FOO}");
}
