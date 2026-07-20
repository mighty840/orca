use std::path::PathBuf;

use crate::client::OrcaClient;
use crate::commands::{AlertsAction, WebhookAction};

/// Find the orca project directory by walking up from `base` looking for
/// `cluster.toml` or `services/`. Falls back to `~/.orca/`.
pub fn find_orca_dir_from(base: &std::path::Path) -> Option<PathBuf> {
    let mut dir = base.to_path_buf();
    loop {
        if dir.join("cluster.toml").exists() || dir.join("services").is_dir() {
            return Some(dir);
        }
        if !dir.pop() {
            break;
        }
    }
    let home_orca = dirs_next::home_dir()?.join("orca");
    if home_orca.join("cluster.toml").exists() {
        return Some(home_orca);
    }
    None
}

/// Find the orca project directory starting from the current working directory.
pub fn find_orca_dir() -> Option<PathBuf> {
    find_orca_dir_from(&std::env::current_dir().ok()?)
}

/// Resolve the API URL: on agent nodes, fall back to the saved leader URL
/// if the default localhost:6880 isn't reachable.
pub fn resolve_api(api: &str) -> String {
    if api != "http://127.0.0.1:6880" {
        return api.to_string();
    }
    let leader_file = dirs_next::home_dir()
        .unwrap_or_else(|| ".".into())
        .join(".orca/leader.url");
    if let Ok(url) = std::fs::read_to_string(&leader_file) {
        let url = url.trim().to_string();
        if !url.is_empty() {
            return url;
        }
    }
    api.to_string()
}

pub async fn handle_stop(service: Option<String>, api: String) -> anyhow::Result<()> {
    let client = OrcaClient::new(api);
    match service {
        Some(name) => {
            client.stop(&name).await?;
            println!("Paused service: {name} (still defined; `orca start {name}` to resume)");
        }
        None => {
            client.stop_all().await?;
            println!("Paused all services.");
        }
    }
    Ok(())
}

/// Resume a paused service to its configured replica count.
pub async fn handle_start(service: &str, api: String) -> anyhow::Result<()> {
    let client = OrcaClient::new(api);
    client.start(service).await?;
    println!("Started service: {service}");
    Ok(())
}

pub async fn handle_logs(
    service: String,
    tail: u64,
    follow: bool,
    summarize: bool,
    api: String,
) -> anyhow::Result<()> {
    let client = OrcaClient::new(api);

    if follow {
        if summarize {
            eprintln!("(--summarize ignored with --follow)");
        }
        use std::io::Write;
        let mut stdout = std::io::stdout();
        // Stream live. For a master-local service the server sends a chunked
        // body and this blocks until Ctrl-C. For an agent-pinned service the
        // server returns a one-shot body (agent-side streaming isn't wired
        // yet), so this prints the current tail and returns — we then poll.
        client.logs_follow(&service, tail, &mut stdout).await?;

        // Poll fallback (remote services). Seed the anchor from what was just
        // printed so we don't reprint it, then emit only new trailing lines.
        let mut anchor = client
            .logs(&service, tail)
            .await
            .ok()
            .and_then(|s| s.lines().last().map(str::to_string))
            .unwrap_or_default();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let cur = match client.logs(&service, tail).await {
                Ok(c) => c,
                Err(_) => continue,
            };
            if let Some(delta) = new_after_anchor(&anchor, &cur) {
                print!("{delta}");
                stdout.flush().ok();
                if let Some(l) = delta.lines().last() {
                    anchor = l.to_string();
                }
            }
        }
    }

    match client.logs(&service, tail).await {
        Ok(logs) => {
            if summarize {
                let ai_config = crate::handlers::ai_ops::load_ai_config();
                match ai_config {
                    Some(config) => {
                        let prompt = format!(
                            "Analyze and summarize these logs for the service '{service}'. \
                             Highlight errors, warnings, and anomalies:\n\n{logs}"
                        );
                        match orca_ai::ops::ask(&config, &prompt, "", "").await {
                            Ok(summary) => println!("{summary}"),
                            Err(e) => {
                                tracing::error!("AI summarization failed: {e}");
                                print!("{logs}");
                            }
                        }
                    }
                    None => {
                        println!("No AI configuration found. Configure [ai] in cluster.toml.");
                        print!("{logs}");
                    }
                }
            } else {
                print!("{logs}");
            }
        }
        Err(e) => {
            tracing::error!("Failed to get logs for '{service}': {e}");
            std::process::exit(1);
        }
    }
    Ok(())
}

pub async fn handle_scale(service: String, replicas: u32, api: String) -> anyhow::Result<()> {
    let client = OrcaClient::new(api);
    match client.scale(&service, replicas).await {
        Ok(resp) => {
            println!("Scaled {} to {} replicas", resp.service, resp.replicas);
        }
        Err(e) => {
            tracing::error!("Failed to scale '{service}': {e}");
            std::process::exit(1);
        }
    }
    Ok(())
}

pub async fn handle_alerts(action: AlertsAction, api: String) -> anyhow::Result<()> {
    let client = OrcaClient::new(api);
    match action {
        AlertsAction::List { all } => {
            let alerts = client.alerts_list(all).await?;
            if alerts.is_empty() {
                let scope = if all { "all" } else { "active" };
                println!("No {scope} alert conversations.");
                return Ok(());
            }
            println!(
                "{:<36}  {:<20}  {:<9}  {:<16}  Started",
                "ID", "Service", "Severity", "State"
            );
            for a in alerts {
                println!(
                    "{:<36}  {:<20}  {:<9?}  {:<16?}  {}",
                    a.id,
                    truncate(&a.service, 20),
                    a.severity,
                    a.state,
                    a.started_at.format("%Y-%m-%d %H:%M:%S")
                );
            }
        }
        AlertsAction::View { id } => {
            let conv = client.alerts_view(&id).await?;
            print_conversation(&conv);
        }
        AlertsAction::Reply { id, message } => {
            let msg = message.join(" ");
            if msg.trim().is_empty() {
                anyhow::bail!("reply message cannot be empty");
            }
            let conv = client.alerts_reply(&id, &msg).await?;
            println!("Reply sent. Latest exchange:");
            print_latest_exchange(&conv);
        }
        AlertsAction::Dismiss { id } => {
            client.alerts_dismiss(&id).await?;
            println!("Dismissed alert {id}.");
        }
        AlertsAction::Resolve { id } => {
            client.alerts_resolve(&id).await?;
            println!("Resolved alert {id}.");
        }
        AlertsAction::Fix { id } => {
            let conv = client.alerts_view(&id).await?;
            let cmd = conv
                .messages
                .iter()
                .rev()
                .find_map(|m| m.suggested_command.as_deref());
            match cmd {
                Some(c) => {
                    println!("Suggested fix for alert {id}:\n  {c}");
                    println!(
                        "\nReview before running. To apply: `orca alerts reply {id} approve` once interactive\n\
                         confirm is wired, or run the command directly."
                    );
                }
                None => println!("No suggested command on alert {id}."),
            }
        }
    }
    Ok(())
}

fn print_conversation(conv: &orca_core::types::AlertConversation) {
    println!("Alert {} — {}", conv.id, conv.service);
    println!(
        "Severity: {:?}  State: {:?}  Started: {}",
        conv.severity,
        conv.state,
        conv.started_at.format("%Y-%m-%d %H:%M:%S")
    );
    if let Some(resolved) = conv.resolved_at {
        println!("Resolved: {}", resolved.format("%Y-%m-%d %H:%M:%S"));
    }
    println!("\nConversation:");
    for msg in &conv.messages {
        let who = match msg.sender {
            orca_core::types::AlertSender::Orca => "orca",
            orca_core::types::AlertSender::Operator => "you",
            orca_core::types::AlertSender::System => "system",
        };
        println!(
            "  [{} {}] {}",
            msg.timestamp.format("%H:%M:%S"),
            who,
            msg.content
        );
        if let Some(cmd) = &msg.suggested_command {
            println!("    fix: {cmd}");
        }
    }
}

fn print_latest_exchange(conv: &orca_core::types::AlertConversation) {
    for msg in conv.messages.iter().rev().take(2).rev() {
        let who = match msg.sender {
            orca_core::types::AlertSender::Orca => "orca",
            orca_core::types::AlertSender::Operator => "you",
            orca_core::types::AlertSender::System => "system",
        };
        println!("  [{who}] {}", msg.content);
        if let Some(cmd) = &msg.suggested_command {
            println!("    fix: {cmd}");
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max.saturating_sub(1)])
    }
}

/// Generate a random 32-byte hex secret for webhook HMAC signing.
fn generate_secret() -> String {
    format!("{:x}{:x}", rand::random::<u128>(), rand::random::<u128>())
}

pub async fn handle_webhooks(action: WebhookAction, api: String) -> anyhow::Result<()> {
    let client = OrcaClient::new(api);
    match action {
        WebhookAction::Add {
            repo,
            service,
            branch,
            secret,
            infra,
        } => {
            let (final_secret, generated) = match secret {
                Some(s) => (s, false),
                None => (generate_secret(), true),
            };
            client
                .add_webhook(&repo, &service, &branch, &final_secret, infra)
                .await?;
            println!("Webhook registered: {repo} -> {service} (branch: {branch})");
            if generated {
                println!();
                println!("Generated secret (save this — it won't be shown again):");
                println!("  {final_secret}");
            }
        }
        WebhookAction::List => {
            let resp = client.list_webhooks().await?;
            let webhooks = resp["webhooks"].as_array();
            match webhooks {
                Some(hooks) if hooks.is_empty() => println!("No webhooks configured."),
                Some(hooks) => {
                    let header = format!("{:<30} {:<20} {:<10}", "REPO", "SERVICE", "BRANCH");
                    println!("{header}");
                    for h in hooks {
                        println!(
                            "{:<30} {:<20} {:<10}",
                            h["repo"].as_str().unwrap_or("-"),
                            h["service_name"].as_str().unwrap_or("-"),
                            h["branch"].as_str().unwrap_or("-"),
                        );
                    }
                }
                None => println!("No webhooks configured."),
            }
        }
        WebhookAction::Remove { ids } => {
            // Resolve glob patterns against the live webhook list; plain names
            // pass through. Deletion keys on service_name server-side, so a
            // name removes every webhook registered for that service.
            let has_glob = ids.iter().any(|p| p.contains('*') || p.contains('?'));
            let targets: Vec<String> = if has_glob {
                let resp = client.list_webhooks().await?;
                let services: Vec<String> = resp["webhooks"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|h| h["service_name"].as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let mut matched: Vec<String> = Vec::new();
                for pat in &ids {
                    if pat.contains('*') || pat.contains('?') {
                        for s in &services {
                            if glob_match(pat, s) && !matched.contains(s) {
                                matched.push(s.clone());
                            }
                        }
                    } else if !matched.contains(pat) {
                        matched.push(pat.clone());
                    }
                }
                matched
            } else {
                let mut t: Vec<String> = Vec::new();
                for id in ids {
                    if !t.contains(&id) {
                        t.push(id);
                    }
                }
                t
            };
            if targets.is_empty() {
                println!("No webhooks matched.");
                return Ok(());
            }
            let mut removed = 0usize;
            for t in &targets {
                match client.remove_webhook(t).await {
                    Ok(_) => {
                        println!("Removed webhook(s) for service: {t}");
                        removed += 1;
                    }
                    Err(e) => eprintln!("Failed to remove {t}: {e}"),
                }
            }
            println!("Removed {removed}/{} target(s).", targets.len());
        }
    }
    Ok(())
}

pub async fn handle_nodes(gpus: bool, api: String) -> anyhow::Result<()> {
    // Authenticated + status-checked (was a raw client with no token, which
    // 401'd against a token-protected master and then failed to decode the
    // plain-text body as JSON — "expected value at line 1 column 1").
    let client = OrcaClient::new(api);
    let json = client.cluster_info().await?;
    println!("Cluster: {}", json["cluster_name"].as_str().unwrap_or("?"));

    let empty = Vec::new();
    let nodes = json["nodes"].as_array().unwrap_or(&empty);
    if nodes.is_empty() {
        println!("No nodes registered.");
        return Ok(());
    }

    let s = |v: &serde_json::Value| v.as_str().unwrap_or("").to_string();
    if gpus {
        println!("{:<22} {:<24} GPUs", "NODE", "ADDRESS");
        let mut any = false;
        for n in nodes {
            let list = n["gpus"].as_array().cloned().unwrap_or_default();
            let desc = if list.is_empty() {
                "-".to_string()
            } else {
                any = true;
                list.iter()
                    .map(|g| {
                        let model = g["model"].as_str().unwrap_or("gpu");
                        let count = g["count"].as_u64().unwrap_or(1);
                        let vendor = g["vendor"].as_str().unwrap_or("");
                        format!("{count}x {vendor} {model}").trim().to_string()
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            println!("{:<22} {:<24} {}", n["node_id"], s(&n["address"]), desc);
        }
        if !any {
            println!("\nNo GPUs declared. Add `[[node.gpus]]` to a node in cluster.toml.");
        }
        return Ok(());
    }

    println!("{:<22} {:<24} LAST HEARTBEAT", "NODE ID", "ADDRESS");
    for n in nodes {
        println!(
            "{:<22} {:<24} {}",
            n["node_id"],
            s(&n["address"]),
            s(&n["last_heartbeat"])
        );
    }
    Ok(())
}

pub async fn handle_redeploy(service: String, api: String) -> anyhow::Result<()> {
    let client = OrcaClient::new(api);
    client.redeploy(&service).await?;
    println!("Redeployed service: {service}");
    Ok(())
}

pub async fn handle_rollback(service: String, api: String) -> anyhow::Result<()> {
    let client = OrcaClient::new(api);
    client.rollback(&service).await?;
    println!("Rolled back service: {service}");
    Ok(())
}

pub async fn handle_promote(service: String, api: String) -> anyhow::Result<()> {
    let client = OrcaClient::new(api);
    client.promote(&service).await?;
    println!("Promoted canary to stable for: {service}");
    Ok(())
}

pub async fn handle_tui(api: &str) -> anyhow::Result<()> {
    let api = resolve_api(api);
    orca_tui::run_tui(&api).await
}
pub async fn handle_web(_port: u16) -> anyhow::Result<()> {
    println!("Use `orca tui` instead.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_orca_dir_finds_cluster_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("cluster.toml"),
            "[cluster]\nname=\"test\"\n",
        )
        .unwrap();
        let sub = dir.path().join("a").join("b");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(find_orca_dir_from(&sub), Some(dir.path().to_path_buf()));
    }

    #[test]
    fn find_orca_dir_finds_services_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("services")).unwrap();
        assert_eq!(
            find_orca_dir_from(dir.path()),
            Some(dir.path().to_path_buf())
        );
    }

    #[test]
    fn find_orca_dir_returns_none_when_nothing_found() {
        let dir = tempfile::tempdir().unwrap();
        assert_ne!(
            find_orca_dir_from(dir.path()),
            Some(dir.path().to_path_buf())
        );
    }
}

/// Minimal glob for webhook service-name matching: `*` matches any run
/// (including empty), `?` matches exactly one character. Anchored (whole
/// string must match), which is what `remove 'breakpilot-*'` expects.
fn glob_match(pattern: &str, text: &str) -> bool {
    fn m(p: &[u8], t: &[u8]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some(b'*') => m(&p[1..], t) || (!t.is_empty() && m(p, &t[1..])),
            Some(b'?') => !t.is_empty() && m(&p[1..], &t[1..]),
            Some(&c) => !t.is_empty() && t[0] == c && m(&p[1..], &t[1..]),
        }
    }
    m(pattern.as_bytes(), text.as_bytes())
}

#[cfg(test)]
mod glob_tests {
    use super::glob_match;

    #[test]
    fn glob_matches() {
        assert!(glob_match("breakpilot-*", "breakpilot-erp"));
        assert!(glob_match("breakpilot-*", "breakpilot-"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("navidrome", "navidrome"));
        assert!(glob_match("svc-?", "svc-1"));
        assert!(glob_match("*-db", "kitchenasty-db"));
        assert!(!glob_match("breakpilot-*", "navidrome"));
        assert!(!glob_match("svc-?", "svc-12"));
        assert!(!glob_match("navidrome", "navidrome2"));
    }
}

/// The portion of `cur` that follows the last occurrence of `anchor` (the
/// last line we already printed). Used by `logs --follow`'s poll fallback so
/// each 2s re-fetch of the tail emits only genuinely new lines. If the
/// anchor scrolled out of the tail window, the whole window is re-emitted.
fn new_after_anchor(anchor: &str, cur: &str) -> Option<String> {
    if anchor.is_empty() {
        return (!cur.is_empty()).then(|| cur.to_string());
    }
    match cur.rfind(anchor) {
        Some(pos) => {
            let after = &cur[pos + anchor.len()..];
            let after = after.strip_prefix('\n').unwrap_or(after);
            (!after.is_empty()).then(|| after.to_string())
        }
        None => (!cur.is_empty()).then(|| cur.to_string()),
    }
}

#[cfg(test)]
mod follow_tests {
    use super::new_after_anchor;

    #[test]
    fn emits_only_new_lines() {
        // Nothing new since the anchor.
        assert_eq!(new_after_anchor("line2", "line1\nline2"), None);
        // One new line appended.
        assert_eq!(
            new_after_anchor("line2", "line1\nline2\nline3").as_deref(),
            Some("line3")
        );
        // Anchor scrolled out of the window -> re-emit all.
        assert_eq!(new_after_anchor("gone", "a\nb").as_deref(), Some("a\nb"));
        // Empty anchor (first poll) -> everything.
        assert_eq!(new_after_anchor("", "x\ny").as_deref(), Some("x\ny"));
    }
}
