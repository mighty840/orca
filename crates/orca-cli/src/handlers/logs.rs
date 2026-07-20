//! `orca logs` — one-shot fetch, optional AI summary, and `--follow`
//! (streaming for local services, poll fallback for agent-pinned ones).

use crate::client::OrcaClient;

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
