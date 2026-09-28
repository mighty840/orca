//! Suspending the TUI for an interactive shell, and restoring the terminal
//! whatever happens.

use std::io;

use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

/// Suspend ratatui and run `docker exec -it` (or `orca exec` for remote
/// services), blocking until the child exits. The TUI comes back even when
/// the child can't be started.
pub(crate) fn run_container_shell(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    api_url: &str,
    service: &str,
    node: Option<&str>,
    cmd: &[String],
) -> anyhow::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    let status = shell_command(api_url, service, node, cmd).status();

    // Restore before looking at the result: an early return here left the
    // TUI drawing on the normal screen in cooked mode (#262).
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    terminal.clear()?;
    terminal.hide_cursor()?;
    let status = status?;
    if !status.success() {
        anyhow::bail!("exit status {status}");
    }
    Ok(())
}

/// Remote services go through this very binary's `orca exec`, aimed at the
/// cluster the TUI is showing: a bare `orca` from `PATH` without `--api`
/// reached the default cluster instead (#262). Local services use
/// `docker exec` directly.
fn shell_command(
    api_url: &str,
    service: &str,
    node: Option<&str>,
    cmd: &[String],
) -> std::process::Command {
    let mut c = if node.is_some() {
        let exe = std::env::current_exe().unwrap_or_else(|_| "orca".into());
        let mut c = std::process::Command::new(exe);
        c.args(["--api", api_url, "exec", service]);
        c
    } else {
        let mut c = std::process::Command::new("docker");
        c.args(["exec", "-it", &format!("orca-{service}")]);
        c
    };
    c.args(cmd);
    c
}

/// Restore the terminal before a panic message prints; otherwise it lands in
/// raw mode on the alternate screen and the shell is left unusable.
pub(crate) fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remote_shell_targets_the_tuis_cluster_with_this_binary() {
        let cmd = shell_command(
            "http://10.0.0.1:6880",
            "api",
            Some("node-2"),
            &["sh".into()],
        );
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["--api", "http://10.0.0.1:6880", "exec", "api", "sh"]);
        assert_eq!(
            cmd.get_program(),
            std::env::current_exe().unwrap().as_os_str()
        );
    }

    #[test]
    fn a_local_shell_is_docker_exec() {
        let cmd = shell_command("http://x", "api", None, &["sh".into()]);
        assert_eq!(cmd.get_program(), "docker");
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["exec", "-it", "orca-api", "sh"]);
    }
}
