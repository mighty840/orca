//! The prompt for diagnosing one alert: the affected service in full detail,
//! the rest of the cluster in one line each.
//!
//! The generic cluster prompt gave the model counts only ("0/1 replicas"),
//! so diagnoses were generic too. This one hands it the evidence an operator
//! would look at first: the recorded failure, the log tail, the image and its
//! predecessor, memory against its limit, and the node.

use chrono::{DateTime, Utc};

use crate::context::{ClusterContext, NodeSummary, ServiceSummary, push_command_reference};
use crate::redact::redact_line;

/// Log lines included for the affected service.
pub const ALERT_LOG_LINES: usize = 40;

/// Lines of the recorded failure message included (it is often a log tail).
const FAILURE_MESSAGE_LINES: usize = 30;

/// The sections the model must answer in.
pub const ANSWER_FORMAT: &str = "Answer in exactly these sections, in Markdown:\n\
    **What happened**: one or two concrete sentences (numbers, times, exit codes).\n\
    **Evidence**: the log lines, exit codes and metrics you rely on, quoted from above.\n\
    **Likely cause**: with confidence (high, medium or low). If another service is the real cause, name it.\n\
    **Other possible causes**: only if your confidence is not high.\n\
    **Fix**: the exact `orca` commands or the service.toml change, in order.\n\
    **Verify**: how to confirm the fix worked.\n\
    **Resolves on its own?**: yes or no, and why.\n\
    Use only the evidence above. If something you need is missing, say what to check instead of guessing. \
    Stay under 250 words.";

impl ClusterContext {
    /// System prompt for diagnosing an alert on `service`.
    pub fn alert_prompt(&self, service: &str) -> String {
        let mut out = String::with_capacity(8192);
        out.push_str(&format!(
            "You are Orca AI, the operations assistant for cluster '{}'. An alert just \
             opened for service '{service}'. Diagnose it from the evidence below and \
             quote the evidence you rely on.\n\n",
            self.cluster_name
        ));
        push_command_reference(&mut out);

        match self.services.iter().find(|s| s.name == service) {
            Some(svc) => push_affected(&mut out, svc, &self.nodes),
            None => out.push_str(&format!(
                "## Affected service: {service}\n(not in the cluster state)\n\n"
            )),
        }

        out.push_str("## Nodes\n");
        for n in &self.nodes {
            out.push_str(&format!("- {}\n", node_line(n)));
        }

        let others: Vec<&ServiceSummary> =
            self.services.iter().filter(|s| s.name != service).collect();
        let (healthy, unhealthy): (Vec<_>, Vec<_>) =
            others.into_iter().partition(|s| s.status == "healthy");
        if !unhealthy.is_empty() {
            out.push_str("\n## Other services with problems\n");
            for s in unhealthy {
                out.push_str(&format!("- {}\n", service_line(s)));
            }
        }
        if !healthy.is_empty() {
            let names: Vec<&str> = healthy.iter().map(|s| s.name.as_str()).collect();
            out.push_str(&format!(
                "\n## Healthy services ({})\n{}\n",
                names.len(),
                names.join(", ")
            ));
        }

        if !self.recent_events.is_empty() {
            out.push_str("\n## Recent events\n");
            for e in self.recent_events.iter().take(20) {
                out.push_str(&format!("- {e}\n"));
            }
        }
        out
    }
}

fn push_affected(out: &mut String, svc: &ServiceSummary, nodes: &[NodeSummary]) {
    out.push_str(&format!("## Affected service: {}\n", svc.name));
    out.push_str(&format!(
        "- status: {}, {}/{} replicas running ({})\n",
        svc.status, svc.replicas_running, svc.replicas_desired, svc.runtime
    ));
    if let Some(image) = &svc.image {
        out.push_str(&format!("- image: {image}"));
        if let Some(at) = svc.last_deploy_at {
            out.push_str(&format!(", deployed {}", ago(at)));
        }
        out.push('\n');
    }
    if let Some(prev) = &svc.previous_image {
        out.push_str(&format!("- previous image: {prev}\n"));
    }
    if let Some(node) = &svc.node {
        match nodes.iter().find(|n| &n.address == node || &n.id == node) {
            Some(n) => out.push_str(&format!("- node: {}\n", node_line(n))),
            None => out.push_str(&format!("- node: {node}\n")),
        }
    }
    let memory = match (&svc.memory_usage, &svc.memory_limit) {
        (Some(u), Some(l)) => Some(format!("{u} used of a {l} limit")),
        (None, Some(l)) => Some(format!("limit {l}, current usage unknown")),
        (Some(u), None) => Some(format!("{u} used, no limit set")),
        (None, None) => None,
    };
    if let Some(m) = memory {
        out.push_str(&format!("- memory: {m}\n"));
    }
    if let Some(cpu) = svc.cpu_percent {
        out.push_str(&format!("- cpu: {cpu:.0}%\n"));
    }
    if let Some(h) = &svc.health_check {
        out.push_str(&format!("- health check: {h}\n"));
    }
    out.push_str(&format!(
        "- restarts in the last 24 h: {}, errors in the last hour: {}\n",
        svc.restart_count_24h, svc.error_count_1h
    ));

    if let Some(f) = &svc.last_failure {
        out.push_str(&format!(
            "\n### Last recorded failure ({})\n",
            ago(f.observed_at)
        ));
        out.push_str(&format!("- reason: {}\n", f.reason));
        if let Some(code) = f.exit_code {
            out.push_str(&format!("- exit code: {code}\n"));
        }
        out.push_str(&format!("- restart count: {}\n", f.restart_count));
        push_block(out, "- detail:", f.message.lines(), FAILURE_MESSAGE_LINES);
    }

    if svc.recent_logs.is_empty() {
        out.push_str("\n### Recent logs\n(none available)\n");
    } else {
        out.push_str(&format!(
            "\n### Recent logs (last {} lines)\n",
            svc.recent_logs.len().min(ALERT_LOG_LINES)
        ));
        push_block(
            out,
            "",
            svc.recent_logs.iter().map(String::as_str),
            ALERT_LOG_LINES,
        );
    }
    out.push('\n');
}

/// Append the last `max` lines, indented and redacted.
fn push_block<'a>(
    out: &mut String,
    header: &str,
    lines: impl Iterator<Item = &'a str>,
    max: usize,
) {
    let lines: Vec<&str> = lines.filter(|l| !l.trim().is_empty()).collect();
    if !header.is_empty() {
        out.push_str(header);
        out.push('\n');
    }
    for line in &lines[lines.len().saturating_sub(max)..] {
        out.push_str("    ");
        out.push_str(&redact_line(line));
        out.push('\n');
    }
}

fn node_line(n: &NodeSummary) -> String {
    let mut line = format!(
        "{} ({}) status={} cpu={:.0}% mem={:.0}%",
        n.id, n.address, n.status, n.cpu_percent, n.memory_percent
    );
    if let Some(age) = n.heartbeat_age_secs {
        line.push_str(&format!(" last heartbeat {age}s ago"));
    }
    line
}

fn service_line(s: &ServiceSummary) -> String {
    let mut line = format!(
        "{}: {}, {}/{} replicas, restarts_24h={}, errors_1h={}",
        s.name,
        s.status,
        s.replicas_running,
        s.replicas_desired,
        s.restart_count_24h,
        s.error_count_1h
    );
    if let Some(f) = &s.last_failure {
        line.push_str(&format!(", last failure {}", f.reason));
    }
    line
}

fn ago(at: DateTime<Utc>) -> String {
    let secs = (Utc::now() - at).num_seconds().max(0);
    let rel = match secs {
        0..=119 => format!("{secs} s ago"),
        120..=7199 => format!("{} min ago", secs / 60),
        _ => format!("{} h ago", secs / 3600),
    };
    format!("{} UTC, {rel}", at.format("%Y-%m-%d %H:%M"))
}

#[cfg(test)]
#[path = "alert_prompt_tests.rs"]
mod tests;
