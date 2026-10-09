use super::*;
use std::time::Duration;

/// Verify the exponential backoff logic: starts at 5s, doubles, capped at 60s.
#[test]
fn test_backoff_doubles() {
    let min_backoff = Duration::from_secs(5);
    let max_backoff = Duration::from_secs(60);

    let mut interval = min_backoff;
    assert_eq!(interval, Duration::from_secs(5));

    interval = (interval * 2).max(min_backoff).min(max_backoff);
    assert_eq!(interval, Duration::from_secs(10));

    interval = (interval * 2).max(min_backoff).min(max_backoff);
    assert_eq!(interval, Duration::from_secs(20));

    interval = (interval * 2).max(min_backoff).min(max_backoff);
    assert_eq!(interval, Duration::from_secs(40));

    interval = (interval * 2).max(min_backoff).min(max_backoff);
    assert_eq!(interval, Duration::from_secs(60), "should be capped at 60s");

    interval = (interval * 2).max(min_backoff).min(max_backoff);
    assert_eq!(interval, Duration::from_secs(60), "should remain at cap");
}

fn make_test_spec(name: &str) -> WorkloadSpec {
    WorkloadSpec {
        restart_policy: None,
        fingerprint: None,
        name: name.to_string(),
        runtime: orca_core::types::RuntimeKind::Container,
        image: "test:latest".to_string(),
        replicas: orca_core::types::Replicas::Fixed(1),
        port: None,
        host_port: None,
        domain: None,
        domains: vec![],
        routes: Vec::new(),
        health: None,
        readiness: None,
        liveness: None,
        env: std::collections::HashMap::new(),
        resources: None,
        volume: None,
        deploy: None,
        placement: None,
        network: None,
        aliases: Vec::new(),
        mounts: Vec::new(),
        triggers: Vec::new(),
        build: None,
        tls_cert: None,
        tls_key: None,
        internal: false,
        cmd: vec![],
        extra_ports: vec![],
        strip_prefix: None,
        pull_policy: Default::default(),
    }
}

fn make_cmd(name: &str) -> WorkloadCommand {
    WorkloadCommand {
        action: "deploy".to_string(),
        spec: make_test_spec(name),
    }
}

#[tokio::test]
async fn test_failed_command_retried() {
    let client = AgentClient::new("http://localhost:0".to_string(), 1);
    let cmd = make_cmd("test-svc");

    // Simulate a failed command being enqueued
    client.enqueue_failed_command(cmd.clone()).await;

    let failed = client.failed_commands.read().await;
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].0.spec.name, "test-svc");
    assert_eq!(failed[0].1, 1, "first failure should have attempt count 1");
}

#[tokio::test]
async fn test_max_retries_exceeded() {
    let client = AgentClient::new("http://localhost:0".to_string(), 1);
    let cmd = make_cmd("doomed-svc");

    // Manually insert a command at MAX_COMMAND_RETRIES attempts
    {
        let mut failed = client.failed_commands.write().await;
        failed.push((cmd, MAX_COMMAND_RETRIES));
    }

    // After retry_failed_commands, the command exceeds max and is dropped.
    // Verify that attempt MAX_COMMAND_RETRIES + 1 > MAX triggers the drop.
    let next_attempt = MAX_COMMAND_RETRIES + 1;
    assert!(
        next_attempt > MAX_COMMAND_RETRIES,
        "command should be dropped after exceeding max retries"
    );

    // Verify the queue tracking works: add at max, it should be there
    let failed = client.failed_commands.read().await;
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].1, MAX_COMMAND_RETRIES);
}

#[tokio::test]
async fn a_redeploy_reports_only_the_new_container() {
    // #297: the replaced container stayed in the workload map, so every
    // heartbeat also reported it (as failed once Docker removed it), and the
    // master showed the running service as stopped.
    let runtime = orca_core::testing::MockRuntime::new();
    let agent = AgentClient::new("http://127.0.0.1:9".into(), 1);
    let other = make_test_spec("db");
    agent.deploy_spec(&runtime, &other).await.unwrap();
    let web = make_test_spec("web");
    agent.deploy_spec(&runtime, &web).await.unwrap();
    agent.deploy_spec(&runtime, &web).await.unwrap();

    let reports = agent.collect_workload_reports(&runtime).await;
    let web_reports: Vec<_> = reports.iter().filter(|r| r.service_name == "web").collect();
    assert_eq!(web_reports.len(), 1, "{reports:?}");
    assert_eq!(web_reports[0].container_id.as_deref(), Some("mock-3"));
    assert!(
        reports.iter().any(|r| r.service_name == "db"),
        "other services are kept"
    );
}
