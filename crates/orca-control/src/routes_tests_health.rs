use std::collections::HashMap;

use crate::state::InstanceState;
use orca_core::runtime::WorkloadHandle;
use orca_core::types::{HealthState, WorkloadStatus};

fn make_instance(health: HealthState, port: Option<u16>) -> InstanceState {
    InstanceState {
        handle: WorkloadHandle {
            runtime_id: "r".into(),
            name: "n".into(),
            metadata: HashMap::new(),
        },
        status: WorkloadStatus::Running,
        host_port: port,
        container_address: None,
        health,
        is_canary: false,
    }
}

/// Only Healthy and NoCheck instances should be routable.
#[test]
fn health_filter_includes_healthy_and_nocheck() {
    let instances = [
        make_instance(HealthState::Healthy, Some(8080)),
        make_instance(HealthState::NoCheck, Some(8081)),
        make_instance(HealthState::Unhealthy, Some(8082)),
        make_instance(HealthState::Unknown, Some(8083)),
    ];
    let routable: Vec<_> = instances
        .iter()
        .filter(|i| i.status == WorkloadStatus::Running)
        .filter(|i| matches!(i.health, HealthState::Healthy | HealthState::NoCheck))
        .collect();
    assert_eq!(routable.len(), 2);
    assert_eq!(routable[0].host_port, Some(8080));
    assert_eq!(routable[1].host_port, Some(8081));
}

/// All-unhealthy instances should produce an empty route set.
#[test]
fn health_filter_excludes_all_unhealthy() {
    let instances = [
        make_instance(HealthState::Unhealthy, Some(8080)),
        make_instance(HealthState::Unknown, Some(8081)),
    ];
    let routable: Vec<_> = instances
        .iter()
        .filter(|i| i.status == WorkloadStatus::Running)
        .filter(|i| matches!(i.health, HealthState::Healthy | HealthState::NoCheck))
        .collect();
    assert!(routable.is_empty());
}
