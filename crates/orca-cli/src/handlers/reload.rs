use anyhow::Result;
use tracing::info;

/// Reload: shutdown daemon, restart, and redeploy all services.
pub async fn handle_reload() -> Result<()> {
    let cwd = std::env::current_dir()?;

    // Read current server args from the running process
    let pid = super::daemon::read_pid();

    if let Some(pid) = pid {
        info!("Stopping current daemon (PID: {pid})");
        super::daemon::handle_shutdown()?;
        // Wait for clean shutdown
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }

    // Restart the server daemon
    let exe = std::env::current_exe()?;
    info!("Restarting orca server...");
    let output = std::process::Command::new(&exe)
        .args(["server", "--daemon"])
        .current_dir(&cwd)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to restart server: {stderr}");
    }
    print!("{}", String::from_utf8_lossy(&output.stdout));

    // Wait for API to be ready
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Redeploy all services
    info!("Redeploying services...");
    let output = std::process::Command::new(&exe)
        .args(["deploy"])
        .current_dir(&cwd)
        .output()?;

    print!("{}", String::from_utf8_lossy(&output.stdout));
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Deploy failed: {stderr}");
    }

    println!("Reload complete.");
    Ok(())
}
