use super::*;
use std::collections::HashMap;

use orca_core::config::ServiceConfig;
use orca_core::runtime::WorkloadHandle;
use orca_core::types::{Replicas, WorkloadStatus};

fn minimal_config(replicas: Replicas) -> ServiceConfig {
    ServiceConfig {
        adopted: false,
        restart_policy: None,
        name: "test-svc".to_string(),
        project: None,
        runtime: Default::default(),
        image: Some("nginx:latest".to_string()),
        module: None,
        replicas,
        port: Some(8080),
        domain: None,
        domains: vec![],
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
        cmd: vec![],
        extra_ports: vec![],
        strip_prefix: None,
        pull_policy: Default::default(),
        backup: None,
    }
}

fn make_instance(status: WorkloadStatus) -> InstanceState {
    InstanceState {
        handle: WorkloadHandle {
            runtime_id: "test-id".to_string(),
            name: "test-instance".to_string(),
            metadata: HashMap::new(),
        },
        status,
        host_port: None,
        container_address: None,
        health: HealthState::Unknown,
        is_canary: false,
        started_at: std::time::Instant::now(),
    }
}

#[test]
fn from_config_fixed_sets_desired_replicas() {
    let state = ServiceState::from_config(minimal_config(Replicas::Fixed(3)));
    assert_eq!(state.desired_replicas, 3);
}

#[test]
fn from_config_auto_defaults_to_one() {
    let state = ServiceState::from_config(minimal_config(Replicas::Auto));
    assert_eq!(state.desired_replicas, 1);
}

#[test]
fn running_count_with_mixed_statuses() {
    let mut state = ServiceState::from_config(minimal_config(Replicas::Fixed(4)));
    state.instances = vec![
        make_instance(WorkloadStatus::Running),
        make_instance(WorkloadStatus::Stopped),
        make_instance(WorkloadStatus::Running),
        make_instance(WorkloadStatus::Failed),
    ];
    assert_eq!(state.running_count(), 2);
}

#[test]
fn instance_event_log_counts_inside_window() {
    // Inject test data directly so we can simulate timestamps across the
    // window boundary. Real callers go through `record_*` which always
    // stamps with `Utc::now()` (monotonically increasing).
    let now = Utc::now();
    let mut log = InstanceEventLog::default();
    log.failures.push_back(now - Duration::minutes(30));
    log.failures.push_back(now - Duration::minutes(45));
    log.failures.push_back(now - Duration::hours(2));
    log.restarts.push_back(now - Duration::hours(48));
    log.restarts.push_back(now - Duration::hours(20));
    log.restarts.push_back(now - Duration::minutes(10));

    assert_eq!(log.failures_in(now, Duration::hours(1)), 2);
    assert_eq!(log.failures_in(now, Duration::hours(24)), 3);
    assert_eq!(log.restarts_in(now, Duration::hours(1)), 1);
    assert_eq!(log.restarts_in(now, Duration::hours(24)), 2);
}

#[test]
fn instance_event_log_prunes_on_append() {
    // A real append at `now` should drop anything earlier than 24h.
    let now = Utc::now();
    let mut log = InstanceEventLog::default();
    log.restarts.push_back(now - Duration::hours(48)); // pre-existing stale
    log.restarts.push_back(now - Duration::hours(20));
    log.record_restart(now);
    assert_eq!(
        log.restarts.len(),
        2,
        "the 48h-old entry must be pruned when a fresh `now` arrives"
    );
}
