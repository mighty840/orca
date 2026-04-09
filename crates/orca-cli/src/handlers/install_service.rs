//! Install orca as a systemd service for auto-start on boot.

use anyhow::{Context, Result};

const UNIT_TEMPLATE: &str = r#"[Unit]
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

/// Handle the `orca install-service` command.
pub fn handle_install_service() -> Result<()> {
    let user = std::env::var("USER").unwrap_or_else(|_| "root".into());
    let exe = std::env::current_exe()
        .context("cannot determine binary path")?
        .display()
        .to_string();
    let workdir = default_workdir(&user);

    let unit = UNIT_TEMPLATE
        .replace("{user}", &user)
        .replace("{workdir}", &workdir)
        .replace("{exe}", &exe);

    let unit_path = "/etc/systemd/system/orca.service";

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
    run_systemctl(&["daemon-reload"])?;
    run_systemctl(&["enable", "orca.service"])?;

    println!("Installed systemd unit: {unit_path}");
    println!("  User: {user}");
    println!("  WorkingDirectory: {workdir}");
    println!("  Binary: {exe}");
    println!();
    println!("Start now with:  sudo systemctl start orca");
    println!("View logs with:  journalctl -u orca -f");
    Ok(())
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
    fn unit_template_has_all_placeholders() {
        assert!(UNIT_TEMPLATE.contains("{user}"));
        assert!(UNIT_TEMPLATE.contains("{workdir}"));
        assert!(UNIT_TEMPLATE.contains("{exe}"));
    }

    #[test]
    fn unit_template_renders_correctly() {
        let unit = UNIT_TEMPLATE
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
    fn default_workdir_root() {
        assert_eq!(default_workdir("root"), "/root/orca");
    }

    #[test]
    fn default_workdir_user() {
        assert_eq!(default_workdir("sharang"), "/home/sharang/orca");
    }
}
