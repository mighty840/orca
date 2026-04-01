use anyhow::Result;

/// Execute a command inside a running container.
pub fn handle_exec(service: &str, cmd: &[String]) -> Result<()> {
    // Get the container name from service name
    let container = format!("orca-{service}");

    // Check if container is running
    let output = std::process::Command::new("docker")
        .args(["inspect", "--format", "{{.State.Running}}", &container])
        .output()?;

    if !output.status.success() || String::from_utf8_lossy(&output.stdout).trim() != "true" {
        anyhow::bail!("Container '{container}' is not running");
    }

    // Execute command interactively
    let status = std::process::Command::new("docker")
        .arg("exec")
        .arg("-it")
        .arg(&container)
        .args(cmd)
        .status()?;

    if !status.success() {
        anyhow::bail!("Command exited with status: {status}");
    }

    Ok(())
}
