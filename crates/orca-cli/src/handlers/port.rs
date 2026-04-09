//! Port redirect helpers for binding to privileged ports without root.
//!
//! When the proxy cannot bind to port 80 or 443 due to permission errors,
//! these helpers set up iptables PREROUTING and OUTPUT rules to redirect
//! traffic from the privileged port to a high port (8080 or 8443).
//!
//! On shutdown (or next startup), [`cleanup_port_redirects`] removes any
//! rules that were added, preventing stale NAT entries from accumulating.

use std::sync::Mutex;

use tracing::info;

/// Tracks which port redirects are currently active so they can be cleaned up.
static ACTIVE_REDIRECTS: Mutex<Vec<(u16, u16)>> = Mutex::new(Vec::new());

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

    let rules = redirect_rules(target_port, high_port);

    for rule in &rules {
        if !run_iptables_rule(rule) {
            return target_port;
        }
        info!("iptables redirect: {target_port} -> {high_port} (rule applied)");
    }

    if let Ok(mut active) = ACTIVE_REDIRECTS.lock() {
        active.push((target_port, high_port));
    }

    info!("Port redirect {target_port} -> {high_port} set up via iptables");
    high_port
}

/// Remove all iptables redirect rules that were set up by [`setup_port_redirect`].
///
/// Called on shutdown to prevent stale NAT entries from leaking across restarts.
pub fn cleanup_port_redirects() {
    let pairs: Vec<(u16, u16)> = ACTIVE_REDIRECTS
        .lock()
        .map(|mut v| v.drain(..).collect())
        .unwrap_or_default();

    if pairs.is_empty() {
        return;
    }

    for (target_port, high_port) in &pairs {
        let rules = [
            format!(
                "-t nat -D PREROUTING -p tcp --dport {target_port} -j REDIRECT --to-port {high_port}"
            ),
            format!(
                "-t nat -D OUTPUT -o lo -p tcp --dport {target_port} -j REDIRECT --to-port {high_port}"
            ),
        ];
        for rule in &rules {
            run_iptables_rule(rule);
        }
        info!("Cleaned up iptables redirect {target_port} -> {high_port}");
    }
}

/// Clean up any stale orca redirect rules left over from a previous run.
///
/// Checks for the specific PREROUTING rules we create (80→8080, 443→8443)
/// and deletes them if found. Safe to call on every startup.
pub fn cleanup_stale_redirects() {
    for (target, high) in [(80u16, 8080u16), (443, 8443)] {
        let check =
            format!("-t nat -C PREROUTING -p tcp --dport {target} -j REDIRECT --to-port {high}");
        if run_iptables_rule(&check) {
            info!("Found stale iptables redirect {target} -> {high}, removing");
            let rules = [
                format!(
                    "-t nat -D PREROUTING -p tcp --dport {target} -j REDIRECT --to-port {high}"
                ),
                format!(
                    "-t nat -D OUTPUT -o lo -p tcp --dport {target} -j REDIRECT --to-port {high}"
                ),
            ];
            for rule in &rules {
                run_iptables_rule(rule);
            }
        }
    }
}

fn redirect_rules(target_port: u16, high_port: u16) -> [String; 2] {
    [
        format!(
            "-t nat -A PREROUTING -p tcp --dport {target_port} -j REDIRECT --to-port {high_port}"
        ),
        format!(
            "-t nat -A OUTPUT -o lo -p tcp --dport {target_port} -j REDIRECT --to-port {high_port}"
        ),
    ]
}

fn run_iptables_rule(rule: &str) -> bool {
    let status = std::process::Command::new("sudo")
        .arg("-n")
        .arg("iptables")
        .args(rule.split_whitespace())
        .status();

    match status {
        Ok(s) if s.success() => true,
        Ok(s) => {
            tracing::warn!(
                "iptables rule failed (exit {}): iptables {rule}",
                s.code().unwrap_or(-1)
            );
            false
        }
        Err(e) => {
            tracing::warn!("Failed to run sudo iptables: {e}");
            false
        }
    }
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

/// Return all currently tracked active redirect pairs (for testing).
#[cfg(test)]
pub fn active_redirect_count() -> usize {
    ACTIVE_REDIRECTS.lock().map(|v| v.len()).unwrap_or(0)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_permission_denied_catches_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let err = anyhow::Error::from(io_err);
        assert!(is_permission_denied(&err));
    }

    #[test]
    fn is_permission_denied_catches_string_match() {
        let err = anyhow::anyhow!("Address binding: Permission denied (os error 13)");
        assert!(is_permission_denied(&err));
    }

    #[test]
    fn is_permission_denied_rejects_unrelated() {
        let err = anyhow::anyhow!("Connection refused");
        assert!(!is_permission_denied(&err));
    }

    #[test]
    fn redirect_rules_format() {
        let rules = redirect_rules(80, 8080);
        assert!(rules[0].contains("PREROUTING"));
        assert!(rules[0].contains("--dport 80"));
        assert!(rules[0].contains("--to-port 8080"));
        assert!(rules[1].contains("OUTPUT"));
        assert!(rules[1].contains("-o lo"));
    }

    #[test]
    fn redirect_rules_443() {
        let rules = redirect_rules(443, 8443);
        assert!(rules[0].contains("--dport 443"));
        assert!(rules[0].contains("--to-port 8443"));
    }

    #[test]
    fn cleanup_with_no_active_redirects_is_noop() {
        // Should not panic or error
        cleanup_port_redirects();
    }

    #[test]
    fn active_redirects_tracks_entries() {
        // Clear state
        ACTIVE_REDIRECTS.lock().unwrap().clear();
        assert_eq!(active_redirect_count(), 0);

        // Simulate adding a redirect (without calling iptables)
        ACTIVE_REDIRECTS.lock().unwrap().push((80, 8080));
        assert_eq!(active_redirect_count(), 1);

        // cleanup_port_redirects drains the list (iptables -D will fail
        // in test env but the list still gets drained)
        cleanup_port_redirects();
        assert_eq!(active_redirect_count(), 0);
    }

    #[test]
    fn check_privileged_port_skips_non_privileged() {
        // Should not warn for non-privileged ports
        check_privileged_port(8080);
        check_privileged_port(3000);
    }
}
