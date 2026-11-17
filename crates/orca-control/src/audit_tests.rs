use super::*;

fn event(n: u64) -> AuditEvent {
    AuditEvent {
        at: chrono::Utc::now(),
        principal: Some("ci".into()),
        method: "POST".into(),
        route: "/api/v1/deploy".into(),
        target: None,
        status: 200,
        latency_ms: n,
        client: None,
    }
}

#[test]
fn mutations_and_exec_are_audited_reads_and_agent_chatter_are_not() {
    assert!(audited("POST", "/api/v1/services/{name}/redeploy"));
    assert!(audited("DELETE", "/api/v1/secrets/{key}"));
    assert!(audited("GET", "/api/v1/services/{name}/exec"));
    assert!(!audited("GET", "/api/v1/status"));
    assert!(!audited("POST", "/api/v1/cluster/heartbeat"));
    assert!(!audited("POST", "/api/v1/cluster/register"));
}

#[test]
fn the_buffer_keeps_the_newest_events() {
    let log = AuditLog::default();
    for n in 0..(CAPACITY as u64 + 5) {
        log.record(event(n));
    }
    let recent = log.recent(usize::MAX);
    assert_eq!(recent.len(), CAPACITY);
    assert_eq!(recent[0].latency_ms, CAPACITY as u64 + 4, "newest first");
    assert_eq!(
        recent[CAPACITY - 1].latency_ms,
        5,
        "the oldest five dropped"
    );
}
