//! Execute a command inside a running container — local (docker exec) or
//! remote (WS exec channel via master).

use anyhow::Result;

/// Execute a command inside a running container.
/// For services on a remote agent node, connects via the master WS exec API.
pub async fn handle_exec(service: &str, cmd: &[String], api: String) -> Result<()> {
    let cmd_vec: Vec<String> = if cmd.is_empty() {
        vec!["/bin/sh".to_string()]
    } else {
        cmd.to_vec()
    };

    if detect_remote_node(service, &api).await {
        exec_remote(service, &cmd_vec, &api).await
    } else {
        exec_local(service, &cmd_vec)
    }
}

/// Return true if the service is placed on a remote agent node.
async fn detect_remote_node(service: &str, api: &str) -> bool {
    let client = crate::client::OrcaClient::new(api.to_string());
    let Ok(status) = client.status().await else {
        return false;
    };
    status
        .services
        .iter()
        .find(|s| s.name == service)
        .and_then(|s| s.node.as_ref())
        .is_some()
}

/// Open a raw-terminal WS exec session with the master for a remote service.
async fn exec_remote(service: &str, cmd: &[String], api: &str) -> Result<()> {
    use crossterm::terminal;
    use futures_util::{SinkExt, StreamExt};
    use std::io::Write as _;
    use tokio::io::AsyncReadExt as _;
    use tokio_tungstenite::tungstenite;

    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let cmd_str = cmd.join(",");
    let token = crate::handlers::server::read_token(None).unwrap_or_default();

    let ws_base = api
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1);
    let ws_url = format!(
        "{ws_base}/api/v1/services/{service}/exec?cmd={}&cols={cols}&rows={rows}",
        urlencoding::encode(&cmd_str)
    );

    let mut req = tungstenite::client::IntoClientRequest::into_client_request(ws_url.as_str())?;
    if !token.is_empty() {
        req.headers_mut().insert(
            "Authorization",
            tungstenite::http::HeaderValue::from_str(&format!("Bearer {token}"))?,
        );
    }

    let (mut ws, _) = tokio_tungstenite::connect_async(req).await?;

    terminal::enable_raw_mode()?;
    let mut stdin = tokio::io::stdin();
    let mut stdout = std::io::stdout();
    let mut buf = vec![0u8; 1024];

    loop {
        tokio::select! {
            n = stdin.read(&mut buf) => {
                let n = n?;
                if n == 0 { break; }
                ws.send(tungstenite::Message::Binary(buf[..n].to_vec().into())).await?;
            }
            msg = ws.next() => {
                match msg {
                    Some(Ok(tungstenite::Message::Binary(data))) => {
                        stdout.write_all(&data)?;
                        stdout.flush()?;
                    }
                    None | Some(Ok(tungstenite::Message::Close(_))) => break,
                    Some(Err(e)) => {
                        terminal::disable_raw_mode()?;
                        return Err(e.into());
                    }
                    _ => {}
                }
            }
        }
    }

    terminal::disable_raw_mode()?;
    println!();
    Ok(())
}

/// Run docker exec locally (service is on this node).
fn exec_local(service: &str, cmd: &[String]) -> Result<()> {
    let container = format!("orca-{service}");

    let check = std::process::Command::new("docker")
        .args(["inspect", "--format", "{{.State.Running}}", &container])
        .output()?;

    if !check.status.success() || String::from_utf8_lossy(&check.stdout).trim() != "true" {
        anyhow::bail!("Container '{container}' is not running");
    }

    let mut docker_cmd = std::process::Command::new("docker");
    docker_cmd.arg("exec");
    if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        docker_cmd.arg("-it");
    }
    let status = docker_cmd.arg(&container).args(cmd).status()?;

    if !status.success() {
        anyhow::bail!("Command exited with status: {status}");
    }

    Ok(())
}
