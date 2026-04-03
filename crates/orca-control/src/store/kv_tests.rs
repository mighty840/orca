use std::collections::HashMap;

use orca_core::config::ServiceConfig;
use orca_core::types::NodeStatus;

use super::*;

fn temp_store() -> ClusterStore {
    let dir = tempfile::tempdir().unwrap();
    ClusterStore::open(&dir.path().join("test.db")).unwrap()
}

#[test]
fn register_and_get_node() {
    let store = temp_store();
    let entry = RaftEntry::RegisterNode {
        node_id: 1,
        address: "10.0.0.1:6881".into(),
        labels: HashMap::new(),
    };
    store.apply(&entry).unwrap();

    let node = store.get_node(1).unwrap().unwrap();
    assert_eq!(node.address, "10.0.0.1:6881");
    assert_eq!(node.status, NodeStatus::Ready);
}

#[test]
fn set_and_get_service() {
    let store = temp_store();
    let config = ServiceConfig {
        name: "web".into(),
        project: None,
        runtime: Default::default(),
        image: Some("nginx:latest".into()),
        module: None,
        replicas: Default::default(),
        port: Some(80),
        domain: None,
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
    };
    store
        .apply(&RaftEntry::SetService(Box::new(config)))
        .unwrap();

    let svc = store.get_service("web").unwrap().unwrap();
    assert_eq!(svc.image.as_deref(), Some("nginx:latest"));
}

#[test]
fn assign_and_unassign_workload() {
    let store = temp_store();
    store
        .apply(&RaftEntry::AssignWorkload {
            service: "web".into(),
            replica_idx: 0,
            node_id: 1,
        })
        .unwrap();
    store
        .apply(&RaftEntry::AssignWorkload {
            service: "web".into(),
            replica_idx: 1,
            node_id: 2,
        })
        .unwrap();

    let assignments = store.get_assignments("web").unwrap();
    assert_eq!(assignments.len(), 2);

    store
        .apply(&RaftEntry::UnassignWorkload {
            service: "web".into(),
            replica_idx: 0,
            node_id: 1,
        })
        .unwrap();

    let assignments = store.get_assignments("web").unwrap();
    assert_eq!(assignments.len(), 1);
    assert_eq!(assignments[0].node_id, 2);
}

#[test]
fn snapshot_captures_all_state() {
    let store = temp_store();
    store
        .apply(&RaftEntry::RegisterNode {
            node_id: 1,
            address: "10.0.0.1:6881".into(),
            labels: HashMap::new(),
        })
        .unwrap();
    store
        .apply(&RaftEntry::SetService(Box::new(ServiceConfig {
            name: "api".into(),
            project: None,
            runtime: Default::default(),
            image: Some("myapp:latest".into()),
            module: None,
            replicas: Default::default(),
            port: None,
            domain: None,
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
        })))
        .unwrap();

    let snap = store.snapshot().unwrap();
    assert_eq!(snap.nodes.len(), 1);
    assert_eq!(snap.services.len(), 1);
}
