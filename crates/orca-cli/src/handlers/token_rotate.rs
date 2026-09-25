//! `orca token rotate` (#210): start a cluster-token rotation, watch it, and
//! finish it. Runs on the master (it reads `~/.orca/cluster.token` for auth,
//! and the master rewrites that file). Never prints a token.

use reqwest::Method;
use serde_json::{Value, json};

use crate::client::OrcaClient;

pub async fn handle(status: bool, finish: bool, force: bool, api: String) -> anyhow::Result<()> {
    let client = OrcaClient::new(api);
    let result = if status {
        client
            .token_rotation(Method::GET, "/api/v1/cluster/token/rotation", None)
            .await?
    } else if finish {
        client
            .token_rotation(
                Method::POST,
                "/api/v1/cluster/token/rotation/finish",
                Some(json!({ "force": force })),
            )
            .await?
    } else {
        let r = client
            .token_rotation(Method::POST, "/api/v1/cluster/token/rotate", None)
            .await?;
        println!(
            "New cluster token saved to {} on the master. The old one is still accepted.",
            r["token_file"].as_str().unwrap_or("~/.orca/cluster.token")
        );
        println!(
            "Update anything else that uses the cluster token (CI, laptops, scripts), then run \
             `orca token rotate --finish`."
        );
        r
    };
    print_status(&result);
    Ok(())
}

fn print_status(s: &Value) {
    if !s["in_progress"].as_bool().unwrap_or(false) {
        println!("No token rotation in progress.");
        return;
    }
    let nodes = s["nodes"].as_array().cloned().unwrap_or_default();
    if nodes.is_empty() {
        println!("Rotation in progress; no agents have reported yet.");
        return;
    }
    println!("{:<16} {:<28} STATE", "NODE", "ADDRESS");
    for n in nodes {
        let state = match (n["rotated"].as_bool(), n["persisted"].as_bool()) {
            (Some(true), Some(true)) => "on the new token".to_string(),
            (Some(true), _) => format!(
                "new token in memory only: {}",
                n["detail"].as_str().unwrap_or("")
            ),
            _ => "waiting (offline, or an agent too old to rotate)".to_string(),
        };
        println!(
            "{:<16} {:<28} {state}",
            n["node_id"],
            n["address"].as_str().unwrap_or("")
        );
    }
}
