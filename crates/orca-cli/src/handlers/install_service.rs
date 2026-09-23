//! Install orca as a systemd service for auto-start on boot.

use anyhow::{Context, Result};

const SERVER_TEMPLATE: &str = r#"[Unit]
Description=Orca Container + Wasm Orchestrator
After=network-online.target docker.service
Wants=network-online.target
Requires=docker.service

[Service]
Type=simple
User={user}
WorkingDirectory={workdir}
ExecStart={exe} server
Restart=on-failure
RestartSec=5
AmbientCapabilities=CAP_NET_BIND_SERVICE
SecureBits=keep-caps
LimitNOFILE=65536
LimitNPROC=4096
StandardOutput=journal
StandardError=journal
SyslogIdentifier=orca

[Install]
WantedBy=multi-user.target
"#;

const AGENT_TEMPLATE: &str = r#"[Unit]
Description=Orca Agent (joined node)
After=network-online.target docker.service
Wants=network-online.target
Requires=docker.service

[Service]
Type=simple
User={user}
WorkingDirectory={workdir}
# The cluster token is read from this file (ORCA_TOKEN=..., mode 0600), not
# given on the command line: a unit file is world-readable and ExecStart
# appears in every local user's process list.
EnvironmentFile={env_file}
ExecStart={exe} join {leader}
Restart=on-failure
RestartSec=5
AmbientCapabilities=CAP_NET_BIND_SERVICE
SecureBits=keep-caps
LimitNOFILE=65536
LimitNPROC=4096
StandardOutput=journal
StandardError=journal
SyslogIdentifier=orca-agent

[Install]
WantedBy=multi-user.target
"#;

/// Handle the `orca install-service` command.
pub fn handle_install_service(leader: Option<String>, token: Option<String>) -> Result<()> {
    let user = std::env::var("USER").unwrap_or_else(|_| "root".into());
    let exe = std::env::current_exe()
        .context("cannot determine binary path")?
        .display()
        .to_string();
    let workdir = default_workdir(&user);

    let is_agent = leader.is_some();

    let unit = if let Some(leader) = &leader {
        // --token, or the token `orca join` already saved on this node.
        let token = match &token {
            Some(token) => token.clone(),
            None => read_token_file(&user)?,
        };
        let env_file = agent_env_path(&user);
        write_agent_env(std::path::Path::new(&env_file), &token)?;
        render_agent_unit(&user, &workdir, &exe, leader, &env_file)
    } else {
        SERVER_TEMPLATE
            .replace("{user}", &user)
            .replace("{workdir}", &workdir)
            .replace("{exe}", &exe)
    };

    let unit_path = if is_agent {
        "/etc/systemd/system/orca-agent.service"
    } else {
        "/etc/systemd/system/orca.service"
    };

    // Write to a temp file then sudo mv, since /etc/systemd needs root.
    let tmp = std::env::temp_dir().join("orca.service");
    std::fs::write(&tmp, &unit).context("failed to write temp unit file")?;

    let status = std::process::Command::new("sudo")
        .args(["cp", &tmp.display().to_string(), unit_path])
        .status()
        .context("failed to run sudo cp")?;

    if !status.success() {
        anyhow::bail!("failed to install unit file to {unit_path}");
    }
    let _ = std::fs::remove_file(&tmp);

    // Reload systemd and enable the service
    let service_name = if is_agent {
        "orca-agent.service"
    } else {
        "orca.service"
    };
    run_systemctl(&["daemon-reload"])?;
    run_systemctl(&["enable", service_name])?;

    println!("Installed systemd unit: {unit_path}");
    println!("  User: {user}");
    println!("  WorkingDirectory: {workdir}");
    println!("  Binary: {exe}");
    if let Some(leader) = &leader {
        println!("  Leader: {leader}");
        println!("  Token: {} (mode 0600)", agent_env_path(&user));
    }
    println!();
    println!("Start now with:  sudo systemctl start {service_name}");
    println!("View logs with:  journalctl -u {service_name} -f");
    Ok(())
}

/// Where an agent's cluster token lives for systemd.
fn agent_env_path(user: &str) -> String {
    if user == "root" {
        "/root/.orca/agent.env".to_string()
    } else {
        format!("/home/{user}/.orca/agent.env")
    }
}

/// The agent unit file. It carries no credential.
fn render_agent_unit(user: &str, workdir: &str, exe: &str, leader: &str, env_file: &str) -> String {
    AGENT_TEMPLATE
        .replace("{user}", user)
        .replace("{workdir}", workdir)
        .replace("{exe}", exe)
        .replace("{leader}", leader)
        .replace("{env_file}", env_file)
}

/// Write `ORCA_TOKEN=<token>` to `path`, readable by its owner only.
///
/// The mode is set on the open handle before any token byte is written, so
/// a pre-existing file with looser permissions never holds the new token
/// while still readable by others. systemd reads the file as root before
/// dropping to the unit's `User=`, so owner-only is enough.
fn write_agent_env(path: &std::path::Path, token: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    // EnvironmentFile parses KEY=VALUE per line; keep the value unambiguous.
    if token.is_empty()
        || token
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '"' | '\'' | '\\'))
    {
        anyhow::bail!(
            "the cluster token contains characters a systemd EnvironmentFile cannot hold \
             unquoted (whitespace, control characters or quotes)"
        );
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("cannot write {}", path.display()))?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("cannot restrict {}", path.display()))?;
    writeln!(file, "ORCA_TOKEN={token}")
        .with_context(|| format!("cannot write {}", path.display()))?;
    Ok(())
}

fn read_token_file(user: &str) -> Result<String> {
    let path = if user == "root" {
        "/root/.orca/cluster.token".to_string()
    } else {
        format!("/home/{user}/.orca/cluster.token")
    };
    std::fs::read_to_string(&path)
        .map(|t| t.trim().to_string())
        .with_context(|| format!("cannot read token from {path}. Pass --token explicitly."))
}

fn default_workdir(user: &str) -> String {
    if user == "root" {
        "/root/orca".into()
    } else {
        format!("/home/{user}/orca")
    }
}

fn run_systemctl(args: &[&str]) -> Result<()> {
    let status = std::process::Command::new("sudo")
        .arg("systemctl")
        .args(args)
        .status()
        .with_context(|| format!("failed to run systemctl {}", args.join(" ")))?;

    if !status.success() {
        anyhow::bail!("systemctl {} failed", args.join(" "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_template_has_all_placeholders() {
        assert!(SERVER_TEMPLATE.contains("{user}"));
        assert!(SERVER_TEMPLATE.contains("{workdir}"));
        assert!(SERVER_TEMPLATE.contains("{exe}"));
    }

    #[test]
    fn server_template_renders_correctly() {
        let unit = SERVER_TEMPLATE
            .replace("{user}", "testuser")
            .replace("{workdir}", "/home/testuser/orca")
            .replace("{exe}", "/usr/local/bin/orca");
        assert!(unit.contains("User=testuser"));
        assert!(unit.contains("WorkingDirectory=/home/testuser/orca"));
        assert!(unit.contains("ExecStart=/usr/local/bin/orca server"));
        assert!(unit.contains("AmbientCapabilities=CAP_NET_BIND_SERVICE"));
        assert!(!unit.contains('{'));
    }

    #[test]
    fn agent_unit_renders_without_the_token() {
        let unit = render_agent_unit(
            "sharang",
            "/home/sharang/orca",
            "/home/sharang/.local/bin/orca",
            "http://100.80.5.14:6880",
            "/home/sharang/.orca/agent.env",
        );
        assert!(unit.contains("User=sharang"));
        assert!(unit.contains("EnvironmentFile=/home/sharang/.orca/agent.env"));
        assert!(
            unit.contains("ExecStart=/home/sharang/.local/bin/orca join http://100.80.5.14:6880\n")
        );
        assert!(!unit.contains("--token"), "a unit file is world-readable");
        assert!(unit.contains("AmbientCapabilities=CAP_NET_BIND_SERVICE"));
        assert!(unit.contains("SyslogIdentifier=orca-agent"));
        assert!(!unit.contains('{'));
    }

    #[test]
    fn agent_env_path_follows_the_user_home() {
        assert_eq!(agent_env_path("root"), "/root/.orca/agent.env");
        assert_eq!(agent_env_path("sharang"), "/home/sharang/.orca/agent.env");
    }

    #[test]
    fn agent_env_file_is_owner_only_and_holds_the_token() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".orca/agent.env");

        write_agent_env(&path, "3e6f00d").unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "ORCA_TOKEN=3e6f00d\n"
        );
    }

    #[test]
    fn agent_env_file_tightens_a_preexisting_loose_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.env");
        std::fs::write(&path, "stale").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        write_agent_env(&path, "newtoken").unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "ORCA_TOKEN=newtoken\n"
        );
    }

    #[test]
    fn agent_env_file_rejects_tokens_systemd_would_misparse() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.env");
        for bad in ["", "two words", "line\nbreak", "quo\"te", "back\\slash"] {
            assert!(write_agent_env(&path, bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn default_workdir_root() {
        assert_eq!(default_workdir("root"), "/root/orca");
    }

    #[test]
    fn default_workdir_user() {
        assert_eq!(default_workdir("sharang"), "/home/sharang/orca");
    }
}
