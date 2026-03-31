use std::fs;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};
use tracing::info;

/// Returns the path to `~/.orca/`.
fn orca_dir() -> Result<PathBuf> {
    let home = dirs_next::home_dir().context("could not determine home directory")?;
    Ok(home.join(".orca"))
}

/// Returns the path to `~/.orca/orca.pid`.
fn pid_path() -> Result<PathBuf> {
    Ok(orca_dir()?.join("orca.pid"))
}

/// Returns the path to `~/.orca/orca.log`.
fn log_path() -> Result<PathBuf> {
    Ok(orca_dir()?.join("orca.log"))
}

/// Fork to background: re-exec the current binary with the same args minus `--daemon`/`-d`,
/// redirect stdout/stderr to a log file, write the child PID, and exit.
pub fn daemonize(args: &[String]) -> Result<()> {
    let dir = orca_dir()?;
    fs::create_dir_all(&dir)?;

    let log_file_path = log_path()?;
    let log_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file_path)
        .context("failed to open log file")?;
    let log_stderr = log_file.try_clone()?;

    // Strip --daemon / -d from args (args[0] is typically the subcommand or first real arg)
    let exe = std::env::current_exe().context("could not determine current executable")?;
    let filtered_args: Vec<&String> = args
        .iter()
        .filter(|a| *a != "--daemon" && *a != "-d")
        .collect();

    // Preserve the current working directory so the child finds cluster.toml
    let cwd = std::env::current_dir().context("could not determine working directory")?;

    let child = Command::new(exe)
        .args(&filtered_args)
        .current_dir(&cwd)
        .stdout(std::process::Stdio::from(log_file))
        .stderr(std::process::Stdio::from(log_stderr))
        .stdin(std::process::Stdio::null())
        .spawn()
        .context("failed to spawn daemon process")?;

    let pid = child.id();

    // Write PID file
    let pid_file = pid_path()?;
    fs::write(&pid_file, pid.to_string()).context("failed to write PID file")?;

    println!(
        "Orca running in background (PID: {pid})\n  Log: {}",
        log_file_path.display()
    );
    Ok(())
}

/// Read the PID from `~/.orca/orca.pid`, returning `None` if the file doesn't exist.
pub fn read_pid() -> Option<u32> {
    let path = pid_path().ok()?;
    let contents = fs::read_to_string(path).ok()?;
    contents.trim().parse().ok()
}

/// Stop the daemon: send SIGTERM and remove the PID file.
pub fn handle_shutdown() -> Result<()> {
    let pid = read_pid().context("no PID file found — is the daemon running?")?;
    info!("Sending SIGTERM to PID {pid}");

    // Send SIGTERM via kill(2)
    let ret = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    if ret != 0 {
        let err = std::io::Error::last_os_error();
        anyhow::bail!("failed to send SIGTERM to PID {pid}: {err}");
    }

    // Remove PID file
    let path = pid_path()?;
    let _ = fs::remove_file(path);

    println!("Orca daemon (PID {pid}) has been stopped.");
    Ok(())
}
