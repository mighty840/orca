//! Port redirect helpers for binding to privileged ports without root.
//!
//! When the proxy cannot bind to port 80 or 443 due to permission errors,
//! these helpers set up iptables PREROUTING and OUTPUT rules to redirect
//! traffic from the privileged port to a high port (8080 or 8443).

use tracing::info;

/// Check if an error is a permission denied error.
pub fn is_permission_denied(e: &anyhow::Error) -> bool {
    if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
        return io_err.kind() == std::io::ErrorKind::PermissionDenied;
    }
    // Check the chain for nested io::Error
    let msg = e.to_string().to_lowercase();
    msg.contains("permission denied")
}

/// Set up iptables port redirect from a privileged port to a high port.
///
/// Returns the high port to bind if iptables setup succeeds, or the original
/// port if it fails (so the caller can emit a helpful error).
pub fn setup_port_redirect(target_port: u16) -> u16 {
    let high_port = if target_port == 80 { 8080 } else { 8443 };

    let rules = [
        // Redirect external traffic
        format!(
            "-t nat -A PREROUTING -p tcp --dport {target_port} -j REDIRECT --to-port {high_port}"
        ),
        // Redirect localhost traffic
        format!(
            "-t nat -A OUTPUT -o lo -p tcp --dport {target_port} -j REDIRECT --to-port {high_port}"
        ),
    ];

    for rule in &rules {
        let status = std::process::Command::new("sudo")
            .arg("-n") // non-interactive, fail if password needed
            .arg("iptables")
            .args(rule.split_whitespace())
            .status();

        match status {
            Ok(s) if s.success() => {
                info!("iptables redirect: {target_port} -> {high_port} (rule applied)");
            }
            Ok(s) => {
                tracing::warn!(
                    "iptables rule failed (exit {}): iptables {rule}",
                    s.code().unwrap_or(-1)
                );
                return target_port;
            }
            Err(e) => {
                tracing::warn!("Failed to run sudo iptables: {e}");
                return target_port;
            }
        }
    }

    info!("Port redirect {target_port} -> {high_port} set up via iptables");
    high_port
}

/// Check if the current binary has `cap_net_bind_service` capability.
/// Returns `true` if the capability is detected or cannot be determined.
fn has_net_bind_capability() -> bool {
    // Try reading /proc/self/status CapEff field
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            if let Some(hex) = line.strip_prefix("CapEff:\t")
                && let Ok(caps) = u64::from_str_radix(hex.trim(), 16)
            {
                // CAP_NET_BIND_SERVICE is bit 10
                return caps & (1 << 10) != 0;
            }
        }
    }
    // Fallback: try `getcap` on the binary
    if let Ok(exe) = std::env::current_exe()
        && let Ok(output) = std::process::Command::new("getcap").arg(&exe).output()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        return stdout.contains("cap_net_bind_service");
    }
    // Cannot determine — assume capable (will fail at bind time with a clear error)
    true
}

/// If using a privileged port, check capabilities upfront and print guidance.
pub fn check_privileged_port(proxy_port: u16) {
    if (proxy_port == 80 || proxy_port == 443) && !has_net_bind_capability() {
        let exe = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "orca".to_string());
        tracing::warn!(
            "Port {proxy_port} requires elevated privileges. Run once:\n  \
             sudo setcap 'cap_net_bind_service=+ep' {exe}"
        );
    }
}
