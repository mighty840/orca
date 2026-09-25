//! Rotating the cluster token without an outage (#210).
//!
//! Rotating used to mean editing `~/.orca/cluster.token`, restarting the
//! master, and updating every agent's unit by hand. The master accepted only
//! the new token, so each agent was locked out until it was updated.
//!
//! Now `start` generates a new token and keeps accepting the old one, also
//! across a master restart (`cluster.token.previous`). It pushes the new token
//! to every connected agent, which switches in memory and reports whether its
//! next start will use it too. `finish` retires the old token, and refuses
//! while any agent is not on the new one for good, so a later agent restart
//! can't lock it out. Agents that were offline get the token when they
//! reconnect. No path prints a token.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use orca_core::ws_types::{MasterMessage, RedactedToken};
use serde::{Deserialize, Serialize};

use crate::state::AppState;

const PREVIOUS: &str = "cluster.token.previous";
/// Per-agent progress, so a master restart mid-rotation doesn't forget which
/// agents still need the new token. Node ids and states only, no token.
const PROGRESS: &str = "cluster.token.rotation.json";

/// An in-progress rotation.
pub struct Rotation {
    /// The token being retired. Still accepted until `finish`.
    previous: String,
    /// The new token, pushed to agents that haven't confirmed yet.
    current: String,
    /// Per agent node.
    nodes: BTreeMap<u64, NodeState>,
    /// Where the token files live (`~/.orca`).
    dir: PathBuf,
}

impl Rotation {
    fn save_progress(&self) {
        if let Ok(json) = serde_json::to_string(&self.nodes)
            && let Err(e) = write_private(&self.dir.join(PROGRESS), &json)
        {
            tracing::warn!("cannot save token rotation progress: {e:#}");
        }
    }
}

/// One agent's progress.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NodeState {
    /// It uses the new token now.
    pub rotated: bool,
    /// Its next start will use it too.
    pub persisted: bool,
    /// Where it saved the token, or what must be changed by hand.
    pub detail: Option<String>,
}

/// What `status`, `start` and `finish` report. Never contains a token.
#[derive(Debug, Serialize)]
pub struct RotationStatus {
    pub in_progress: bool,
    pub token_file: String,
    pub nodes: Vec<NodeStatus>,
}

#[derive(Debug, Serialize)]
pub struct NodeStatus {
    pub node_id: u64,
    pub address: String,
    #[serde(flatten)]
    pub state: NodeState,
}

/// `~/.orca`, where the master keeps `cluster.token`.
pub fn token_dir() -> PathBuf {
    dirs_next::home_dir()
        .unwrap_or_else(|| ".".into())
        .join(".orca")
}

/// Start a rotation: new token in `dir/cluster.token`, old one kept in
/// `cluster.token.previous` and still accepted, new one pushed to agents.
pub async fn start(state: &AppState, dir: &Path) -> Result<RotationStatus> {
    let mut rotation = state.token_rotation.write().await;
    if rotation.is_some() {
        bail!(
            "a token rotation is already in progress; finish it first (orca token rotate --finish)"
        );
    }
    let token_file = dir.join("cluster.token");
    let previous = read_token(&token_file)
        .ok_or_else(|| anyhow::anyhow!("no cluster token in {}", token_file.display()))?;
    if !accepted(state, &previous) {
        bail!(
            "the token in {} is not the cluster token this master accepts (are `api_tokens` \
             set in cluster.toml?); rotate those by editing cluster.toml",
            token_file.display()
        );
    }
    let current = format!("{:032x}", rand::random::<u128>());

    // Previous first: if writing the new token fails, nothing was lost.
    write_private(&dir.join(PREVIOUS), &previous)?;
    write_private(&token_file, &current)?;
    {
        let mut tokens = state.api_tokens.write().unwrap_or_else(|e| e.into_inner());
        for t in tokens.iter_mut() {
            if *t == previous {
                *t = current.clone();
            }
        }
        tokens.push(previous.clone());
    }

    let master = crate::master_node::master_node_id();
    let nodes = state
        .registered_nodes
        .read()
        .await
        .keys()
        .filter(|id| **id != master)
        .map(|id| (*id, NodeState::default()))
        .collect();
    let r = Rotation {
        previous,
        current: current.clone(),
        nodes,
        dir: dir.to_path_buf(),
    };
    r.save_progress();
    *rotation = Some(r);
    drop(rotation);

    let agents = state.ws_agents.read().await;
    for tx in agents.values() {
        let _ = tx
            .send(MasterMessage::RotateToken {
                token: RedactedToken(current.clone()),
            })
            .await;
    }
    drop(agents);
    tracing::info!(
        "cluster token rotation started; new token saved to {}",
        token_file.display()
    );
    Ok(status(state, dir).await)
}

/// Where the rotation stands.
pub async fn status(state: &AppState, dir: &Path) -> RotationStatus {
    let rotation = state.token_rotation.read().await;
    let registered = state.registered_nodes.read().await;
    let nodes = rotation
        .as_ref()
        .map(|r| {
            r.nodes
                .iter()
                .map(|(id, s)| NodeStatus {
                    node_id: *id,
                    address: registered
                        .get(id)
                        .map(|n| n.address.clone())
                        .unwrap_or_default(),
                    state: s.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    RotationStatus {
        in_progress: rotation.is_some(),
        token_file: dir.join("cluster.token").display().to_string(),
        nodes,
    }
}

/// Retire the old token. Refuses while any agent isn't on the new one for
/// good (its next restart would be locked out), unless `force`.
pub async fn finish(state: &AppState, dir: &Path, force: bool) -> Result<RotationStatus> {
    let mut rotation = state.token_rotation.write().await;
    let Some(r) = rotation.as_ref() else {
        bail!("no token rotation is in progress");
    };
    // Every agent known to the rotation or registered now: after a master
    // restart the rotation's list may not have seen them all yet.
    let master = crate::master_node::master_node_id();
    let mut all = r.nodes.clone();
    for id in state.registered_nodes.read().await.keys() {
        if *id != master {
            all.entry(*id).or_default();
        }
    }
    let pending: Vec<String> = all
        .iter()
        .filter(|(_, s)| !(s.rotated && s.persisted))
        .map(|(id, s)| match (&s.rotated, &s.detail) {
            (false, _) => format!("node {id}: has not confirmed the new token"),
            (true, d) => format!("node {id}: not saved ({})", d.as_deref().unwrap_or("?")),
        })
        .collect();
    if !pending.is_empty() && !force {
        bail!(
            "not every agent is on the new token for good, so retiring the old one could lock \
             them out at their next restart: {}. Fix that, or pass --force",
            pending.join("; ")
        );
    }
    let previous = r.previous.clone();
    state
        .api_tokens
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|t| *t != previous);
    let _ = std::fs::remove_file(dir.join(PREVIOUS));
    let _ = std::fs::remove_file(dir.join(PROGRESS));
    *rotation = None;
    drop(rotation);
    tracing::info!("cluster token rotation finished; the old token is no longer accepted");
    Ok(status(state, dir).await)
}

/// An agent confirmed the new token.
pub async fn on_rotated(state: &AppState, node_id: u64, persisted: bool, detail: Option<String>) {
    if let Some(r) = state.token_rotation.write().await.as_mut() {
        r.nodes.insert(
            node_id,
            NodeState {
                rotated: true,
                persisted,
                detail,
            },
        );
        r.save_progress();
    }
}

/// An agent (re)connected during a rotation: send it the new token unless it
/// already confirmed it is on it for good. That covers an agent offline at
/// `start`, and one whose unit was fixed and restarted after it reported
/// "in memory only": it then confirms "saved".
pub async fn on_connect(state: &AppState, node_id: u64) {
    let token = {
        let mut rotation = state.token_rotation.write().await;
        let Some(r) = rotation.as_mut() else { return };
        let node = r.nodes.entry(node_id).or_default();
        if node.rotated && node.persisted {
            return;
        }
        r.current.clone()
    };
    if let Some(tx) = state.ws_agents.read().await.get(&node_id) {
        let _ = tx
            .send(MasterMessage::RotateToken {
                token: RedactedToken(token),
            })
            .await;
    }
}

/// At startup: an unfinished rotation left `cluster.token.previous`. Keep
/// accepting that token and resume, so agents not yet rotated can reconnect.
pub async fn resume(state: &AppState, dir: &Path) {
    let (Some(previous), Some(current)) = (
        read_token(&dir.join(PREVIOUS)),
        read_token(&dir.join("cluster.token")),
    ) else {
        return;
    };
    if previous == current {
        return;
    }
    if !accepted(state, &previous) {
        state
            .api_tokens
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .push(previous.clone());
    }
    let nodes = std::fs::read_to_string(dir.join(PROGRESS))
        .ok()
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default();
    *state.token_rotation.write().await = Some(Rotation {
        previous,
        current,
        nodes,
        dir: dir.to_path_buf(),
    });
    tracing::warn!(
        "an unfinished cluster token rotation was resumed; the old token is still accepted \
         until `orca token rotate --finish`"
    );
}

fn accepted(state: &AppState, token: &str) -> bool {
    state
        .api_tokens
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .any(|t| t == token)
}

fn read_token(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// Owner-only temp file in the same directory, renamed into place, so a
/// crash never leaves a half-written token.
fn write_private(path: &Path, token: &str) -> Result<()> {
    use std::io::Write;
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("token");
    let tmp = dir.join(format!(".{name}.tmp"));
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(token.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
        .map_err(|e| anyhow::anyhow!("cannot save {}: {e}", path.display()))?;
    Ok(())
}

#[cfg(test)]
#[path = "token_rotation_tests.rs"]
mod tests;
