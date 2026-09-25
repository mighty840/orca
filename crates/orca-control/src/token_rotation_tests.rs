//! #210: rotating the cluster token without locking agents out.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use orca_core::config::ClusterConfig;
use orca_core::testing::MockRuntime;
use orca_core::ws_types::MasterMessage;
use tokio::sync::{Notify, RwLock, mpsc};

use super::*;
use crate::auth::resolve_token;
use crate::session::AgentSession;
use crate::state::RegisteredNode;

const AGENT: u64 = 7;

fn state_with_token(dir: &Path, token: &str) -> AppState {
    std::fs::write(dir.join("cluster.token"), format!("{token}\n")).unwrap();
    AppState::new(
        ClusterConfig {
            api_tokens: vec![token.into()],
            ..Default::default()
        },
        Arc::new(MockRuntime::new()),
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    )
}

/// Register agent `AGENT` with a live session; returns its message stream.
async fn connect_agent(state: &AppState) -> mpsc::Receiver<MasterMessage> {
    state.registered_nodes.write().await.insert(
        AGENT,
        RegisteredNode {
            node_id: AGENT,
            address: "agent-a:6881".into(),
            labels: HashMap::new(),
            peer_ip: None,
            last_heartbeat: chrono::Utc::now(),
            drain: false,
            cpu_percent: 0.0,
            memory_bytes: 0,
            memory_total: 0,
            disk_used: 0,
            disk_total: 0,
            net_rx: 0,
            net_tx: 0,
        },
    );
    let (tx, rx) = mpsc::channel(8);
    state.ws_agents.write().await.insert(
        AGENT,
        AgentSession {
            tx,
            session_id: 1,
            shutdown: Arc::new(Notify::new()),
        },
    );
    rx
}

fn file(dir: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(name))
        .ok()
        .map(|s| s.trim().to_string())
}

fn pushed_token(rx: &mut mpsc::Receiver<MasterMessage>) -> String {
    match rx.try_recv() {
        Ok(MasterMessage::RotateToken { token }) => token.0,
        other => panic!("expected RotateToken, got {other:?}"),
    }
}

#[tokio::test]
async fn start_accepts_both_tokens_and_pushes_the_new_one() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_with_token(dir.path(), "old-token");
    let mut rx = connect_agent(&state).await;

    let status = start(&state, dir.path()).await.unwrap();

    let new = file(dir.path(), "cluster.token").unwrap();
    assert_ne!(new, "old-token");
    assert_eq!(new.len(), 32);
    assert_eq!(file(dir.path(), PREVIOUS).as_deref(), Some("old-token"));
    assert!(resolve_token(&state, &new).is_some(), "new token accepted");
    assert!(
        resolve_token(&state, "old-token").is_some(),
        "old still accepted"
    );
    assert_eq!(pushed_token(&mut rx), new);
    assert!(status.in_progress);
    assert_eq!(status.nodes.len(), 1);
    assert!(!status.nodes[0].state.rotated);
    assert!(
        !serde_json::to_string(&status).unwrap().contains(&new),
        "the status never carries a token"
    );
}

#[tokio::test]
async fn finish_waits_for_every_agent_to_be_on_the_new_token_for_good() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_with_token(dir.path(), "old-token");
    let _rx = connect_agent(&state).await;
    start(&state, dir.path()).await.unwrap();
    let new = file(dir.path(), "cluster.token").unwrap();

    let err = finish(&state, dir.path(), false).await.unwrap_err();
    assert!(err.to_string().contains("node 7"), "{err}");

    // In memory only (token in a root-owned ExecStart): still refused.
    on_rotated(&state, AGENT, false, Some("pass --token".into())).await;
    let err = finish(&state, dir.path(), false).await.unwrap_err();
    assert!(err.to_string().contains("not saved"), "{err}");

    on_rotated(&state, AGENT, true, Some("saved".into())).await;
    let status = finish(&state, dir.path(), false).await.unwrap();

    assert!(!status.in_progress);
    assert!(
        resolve_token(&state, "old-token").is_none(),
        "old token retired"
    );
    assert!(resolve_token(&state, &new).is_some());
    assert_eq!(file(dir.path(), PREVIOUS), None);
}

#[tokio::test]
async fn force_finishes_despite_pending_agents() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_with_token(dir.path(), "old-token");
    let _rx = connect_agent(&state).await;
    start(&state, dir.path()).await.unwrap();

    finish(&state, dir.path(), true).await.unwrap();

    assert!(resolve_token(&state, "old-token").is_none());
}

#[tokio::test]
async fn a_second_start_and_a_foreign_token_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_with_token(dir.path(), "old-token");
    start(&state, dir.path()).await.unwrap();
    assert!(start(&state, dir.path()).await.is_err());

    // The file doesn't hold a token this master accepts.
    let other = tempfile::tempdir().unwrap();
    let state = state_with_token(other.path(), "old-token");
    std::fs::write(other.path().join("cluster.token"), "unrelated").unwrap();
    let err = start(&state, other.path()).await.unwrap_err();
    assert!(err.to_string().contains("cluster.toml"), "{err}");
}

/// A master restart mid-rotation keeps the old token accepted, and an
/// agent that reconnects is sent the new one.
#[tokio::test]
async fn a_restart_resumes_and_reconnecting_agents_get_the_new_token() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(PREVIOUS), "old-token").unwrap();
    // After the restart the master loads only the new token.
    let state = state_with_token(dir.path(), "new-token");

    resume(&state, dir.path()).await;
    assert!(resolve_token(&state, "old-token").is_some());
    assert!(status(&state, dir.path()).await.in_progress);

    let mut rx = connect_agent(&state).await;
    on_connect(&state, AGENT).await;
    assert_eq!(pushed_token(&mut rx), "new-token");

    // Once it confirmed, a later reconnect sends nothing.
    on_rotated(&state, AGENT, true, None).await;
    on_connect(&state, AGENT).await;
    assert!(rx.try_recv().is_err());
}

/// Review finding 1: an agent with `--token` in its unit reports "in memory
/// only". Once the operator fixes the unit and restarts it, the reconnect
/// must resend the token so the agent can confirm "saved", or `--finish`
/// could only ever succeed with `--force`.
#[tokio::test]
async fn an_agent_fixed_after_an_in_memory_rotation_lets_finish_succeed() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_with_token(dir.path(), "old-token");
    let mut rx = connect_agent(&state).await;
    start(&state, dir.path()).await.unwrap();
    let new = pushed_token(&mut rx);
    on_rotated(&state, AGENT, false, Some("pass --token".into())).await;

    // Unit fixed, agent restarted: it reconnects and is sent the token again.
    on_connect(&state, AGENT).await;
    assert_eq!(pushed_token(&mut rx), new);
    on_rotated(
        &state,
        AGENT,
        true,
        Some("started with the new token".into()),
    )
    .await;

    finish(&state, dir.path(), false).await.unwrap();
    assert!(resolve_token(&state, "old-token").is_none());
}

/// Review finding 2: after a master restart, `--finish` must still wait for
/// agents that haven't confirmed, from the saved progress and from the
/// agents registered now.
#[tokio::test]
async fn finish_after_a_master_restart_still_waits_for_agents() {
    let dir = tempfile::tempdir().unwrap();
    let before = state_with_token(dir.path(), "old-token");
    let _rx = connect_agent(&before).await;
    start(&before, dir.path()).await.unwrap();
    let new = file(dir.path(), "cluster.token").unwrap();
    drop(before);

    // Restart: the master loads the new token and resumes.
    let after = state_with_token(dir.path(), &new);
    resume(&after, dir.path()).await;
    let err = finish(&after, dir.path(), false).await.unwrap_err();
    assert!(err.to_string().contains("node 7"), "saved progress: {err}");

    // Even without the progress file, a registered agent is checked.
    std::fs::remove_file(dir.path().join(PROGRESS)).unwrap();
    let fresh = state_with_token(dir.path(), &new);
    resume(&fresh, dir.path()).await;
    let _rx = connect_agent(&fresh).await;
    assert!(finish(&fresh, dir.path(), false).await.is_err());

    on_rotated(&fresh, AGENT, true, None).await;
    finish(&fresh, dir.path(), false).await.unwrap();
    assert!(!dir.path().join(PROGRESS).exists());
}
