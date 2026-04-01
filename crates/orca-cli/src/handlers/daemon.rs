use std::fs;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};
use tracing::{info, warn};

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

/// Maximum log file size before rotation (10 MB).
const MAX_LOG_SIZE: u64 = 10 * 1024 * 1024;

/// Check if a process with the given PID is alive using `kill(pid, 0)`.
fn is_process_alive(pid: u32) -> bool {
    let ret = unsafe { libc::kill(pid as i32, 0) };
    ret == 0
}

/// Check for a stale PID file. Returns `Err` if the daemon is already running.
/// Removes a stale PID file (process no longer running) and returns `Ok`.
fn check_stale_pid() -> Result<()> {
    let path = pid_path()?;
    if !path.exists() {
        return Ok(());
    }
    if let Some(pid) = read_pid() {
        if is_process_alive(pid) {
            anyhow::bail!("Orca is already running (PID {pid})");
        }
        warn!("Removing stale PID file (PID {pid} is not running)");
        let _ = fs::remove_file(&path);
    }
    Ok(())
}

/// Rotate the log file if it exceeds [`MAX_LOG_SIZE`].
fn rotate_log_if_needed(log_file_path: &std::path::Path) -> Result<()> {
    if !log_file_path.exists() {
        return Ok(());
    }
    let meta = fs::metadata(log_file_path).context("failed to stat log file")?;
    if meta.len() > MAX_LOG_SIZE {
        let rotated = log_file_path.with_extension("log.1");
        fs::rename(log_file_path, &rotated).context("failed to rotate log file")?;
        info!(
            "Rotated log file ({} bytes) to {}",
            meta.len(),
            rotated.display()
        );
    }
    Ok(())
}

/// Fork to background: re-exec the current binary with the same args minus `--daemon`/`-d`,
/// redirect stdout/stderr to a log file, write the child PID, and exit.
pub fn daemonize(args: &[String]) -> Result<()> {
    let dir = orca_dir()?;
    fs::create_dir_all(&dir)?;

    // Check for existing daemon
    check_stale_pid()?;

    let log_file_path = log_path()?;
    rotate_log_if_needed(&log_file_path)?;

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

    if !is_process_alive(pid) {
        warn!("PID {pid} is not running, cleaning up stale PID file");
        let path = pid_path()?;
        let _ = fs::remove_file(path);
        println!("Cleaned up stale PID file (PID {pid} was not running).");
        return Ok(());
    }

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
