//! Build per-service `FailureInfo` records so `orca status` can explain *why* a
//! service is degraded — both deploy-time errors and runtime container crashes.

use orca_core::api_types::FailureInfo;

/// Classify a deploy error string into a short, K8s-style reason.
pub fn from_deploy_error(message: &str) -> FailureInfo {
    let lower = message.to_lowercase();
    let reason = if lower.contains("not found") || lower.contains("pull") {
        "ImagePullError"
    } else if lower.contains("did not acknowledge")
        || lower.contains("unreachable")
        || lower.contains("not connected")
    {
        "AgentUnreachable"
    } else if lower.contains("did not complete")
        || lower.contains("timed out")
        || lower.contains("timeout")
    {
        "DeployTimeout"
    } else {
        "DeployError"
    };
    FailureInfo {
        reason: reason.to_string(),
        message: message.to_string(),
        exit_code: None,
        restart_count: 0,
        observed_at: chrono::Utc::now(),
    }
}

/// Build a `FailureInfo` from a crashed container's heartbeat detail.
pub fn from_crash(
    exit_code: Option<i64>,
    restart_count: u32,
    last_logs: Option<&str>,
) -> FailureInfo {
    let reason = if restart_count >= 3 {
        "CrashLoopBackOff"
    } else if exit_code.is_some_and(|c| c != 0) {
        "Error"
    } else {
        "Stopped"
    };
    let message = last_logs
        .map(str::to_string)
        .unwrap_or_else(|| match exit_code {
            Some(c) => format!("container exited with code {c}"),
            None => "container is not running".to_string(),
        });
    FailureInfo {
        reason: reason.to_string(),
        message,
        exit_code,
        restart_count,
        observed_at: chrono::Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_deploy_errors() {
        assert_eq!(
            from_deploy_error("pull access denied for x, not found").reason,
            "ImagePullError"
        );
        assert_eq!(
            from_deploy_error(
                "agent 5 did not acknowledge deploy of x within 10s — agent may be unreachable"
            )
            .reason,
            "AgentUnreachable"
        );
        assert_eq!(
            from_deploy_error("deploy of x did not complete within 600s on agent 5").reason,
            "DeployTimeout"
        );
        assert_eq!(
            from_deploy_error("something else broke").reason,
            "DeployError"
        );
    }

    #[test]
    fn classifies_crashes() {
        assert_eq!(from_crash(Some(1), 0, None).reason, "Error");
        assert_eq!(from_crash(Some(1), 5, None).reason, "CrashLoopBackOff");
        assert_eq!(from_crash(None, 0, None).reason, "Stopped");
        // Log tail becomes the message when present.
        assert_eq!(
            from_crash(Some(1), 0, Some("panic: boom")).message,
            "panic: boom"
        );
        // Otherwise a synthesized message.
        assert_eq!(
            from_crash(Some(137), 0, None).message,
            "container exited with code 137"
        );
    }
}
