//! Integration test: verify the example config files in the repo root parse correctly.

use std::path::Path;

use orca_core::config::{ClusterConfig, ServicesConfig};

#[test]
fn parse_cluster_example() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("cluster.toml.example");

    let config = ClusterConfig::load(&path)
        .unwrap_or_else(|e| panic!("Failed to parse cluster.toml.example: {e}"));
    assert_eq!(config.cluster.name, "my-cluster");
    assert_eq!(config.cluster.domain.as_deref(), Some("example.com"));
}

#[test]
fn parse_services_example() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("services.toml.example");

    let config = ServicesConfig::load(&path)
        .unwrap_or_else(|e| panic!("Failed to parse services.toml.example: {e}"));
    assert!(!config.service.is_empty());
    // Check that GPU service parsed correctly
    let llm = config.service.iter().find(|s| s.name == "llm-inference");
    assert!(llm.is_some());
    let gpu = llm
        .unwrap()
        .resources
        .as_ref()
        .unwrap()
        .gpu
        .as_ref()
        .unwrap();
    assert_eq!(gpu.count, 1);
}
