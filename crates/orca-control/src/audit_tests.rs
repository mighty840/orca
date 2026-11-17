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
fn requests_without_a_valid_token_cannot_flush_the_buffer() {
    let log = AuditLog::default();
    log.record(event(1));
    for _ in 0..(CAPACITY * 2) {
        log.record(AuditEvent {
            principal: None,
            status: 401,
            ..event(0)
        });
    }
    let kept = log.recent(usize::MAX);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].principal.as_deref(), Some("ci"));
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
