//! Prompts: templates that gather cluster data for the client's model.
//!
//! They make no LLM call inside orca. Logs and failure text come from
//! containers, so they are redacted and fenced as data, not instructions.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::backend::ApiBackend;
use crate::catalog::op;
use crate::output::fenced;

/// One prompt argument: name, description, required.
pub type Arg = (&'static str, &'static str, bool);

pub struct PromptDef {
    pub name: &'static str,
    pub description: &'static str,
    pub args: &'static [Arg],
}

pub const PROMPTS: &[PromptDef] = &[
    PromptDef {
        name: "diagnose",
        description: "Work out why a service is unhealthy from its status, logs and alerts.",
        args: &[("service", "Service name", true)],
    },
    PromptDef {
        name: "failing_services",
        description: "Review every service that is degraded or stopped.",
        args: &[("project", "Only this project's services", false)],
    },
    PromptDef {
        name: "capacity_review",
        description: "Review node CPU, memory and disk against what the services use.",
        args: &[],
    },
    PromptDef {
        name: "explain_alert",
        description: "Explain an alert from the AI monitor, with its service's state.",
        args: &[("id", "Alert id", true)],
    },
];

/// How the read-only server asks the model to treat changes.
const READ_ONLY: &str = "This connection is read-only. Propose any change as a command for \
                         the operator to run (for example `orca redeploy <service>`); do not \
                         say it has been done.";

/// The prompt text for `name` with `args`, or why it cannot be built.
pub async fn render(
    backend: &dyn ApiBackend,
    name: &str,
    args: &BTreeMap<String, String>,
) -> Result<String, String> {
    let arg = |k: &str| args.get(k).cloned().unwrap_or_default();
    let need = |k: &str| match args.get(k).filter(|v| !v.is_empty()) {
        Some(v) => Ok(v.clone()),
        None => Err(format!("prompt '{name}' needs the argument '{k}'")),
    };
    match name {
        "diagnose" => {
            let svc = need("service")?;
            Ok(diagnose(backend, &svc).await)
        }
        "failing_services" => {
            let project = arg("project");
            let status = fetch(backend, "get_status", &[("project", &project)]).await;
            let failing = match serde_json::from_str::<Value>(&status) {
                Ok(v) => only_failing(&v),
                Err(_) => status,
            };
            Ok(format!(
                "List the orca services below that are not running as declared, group them \
                 by likely cause, and say what to check first for each. {READ_ONLY}\n\n{}",
                fenced("Degraded or stopped services", &failing)
            ))
        }
        "capacity_review" => Ok(format!(
            "Review this orca cluster's capacity: which nodes are close to their CPU, memory \
             or disk limits, which services use the most, and where a service could move. \
             {READ_ONLY}\n\n{}\n\n{}",
            fenced("Nodes", &fetch(backend, "get_cluster", &[]).await),
            fenced("Services", &fetch(backend, "get_status", &[]).await),
        )),
        "explain_alert" => {
            let id = need("id")?;
            let alert = fetch(backend, "get_alert", &[("id", &id)]).await;
            let service = serde_json::from_str::<Value>(&alert)
                .ok()
                .and_then(|v| v["service"].as_str().map(str::to_owned));
            let mut text = format!(
                "Explain this alert from orca's AI monitor: what happened, whether it is \
                 still happening, and what to do. {READ_ONLY}\n\n{}",
                fenced("Alert", &alert)
            );
            if let Some(svc) = service {
                text.push_str("\n\n");
                text.push_str(&fenced(
                    &format!("Service {svc}"),
                    &fetch(backend, "get_service", &[("name", &svc)]).await,
                ));
            }
            Ok(text)
        }
        other => Err(format!("unknown prompt '{other}'")),
    }
}

async fn diagnose(backend: &dyn ApiBackend, svc: &str) -> String {
    format!(
        "Diagnose the orca service `{svc}`: what is wrong, the likely cause, and a fix. \
         {READ_ONLY}\n\n{}\n\n{}\n\n{}",
        fenced(
            "Status and configuration",
            &fetch(backend, "get_service", &[("name", svc)]).await
        ),
        fenced(
            "Last log lines",
            &fetch(backend, "get_logs", &[("name", svc), ("tail", "200")]).await
        ),
        fenced("Alerts", &fetch(backend, "list_alerts", &[]).await),
    )
}

/// The result of a read operation, processed as its tool would be; an
/// error becomes the text in its place, so one failed fetch does not lose
/// the rest of the prompt.
async fn fetch(backend: &dyn ApiBackend, tool: &str, args: &[(&str, &str)]) -> String {
    let Some(op) = op(tool) else {
        return format!("(unknown operation {tool})");
    };
    let args = args
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    match crate::server::run_op(backend, op, &args).await {
        Ok(text) => text,
        Err(e) => format!("(not available: {e})"),
    }
}

/// The `services` entries whose status is degraded or stopped.
fn only_failing(status: &Value) -> String {
    let failing: Vec<&Value> = status["services"]
        .as_array()
        .map(|s| {
            s.iter()
                .filter(|svc| matches!(svc["status"].as_str(), Some("degraded" | "stopped")))
                .collect()
        })
        .unwrap_or_default();
    if failing.is_empty() {
        return "(none: every service is running or paused)".into();
    }
    serde_json::to_string_pretty(&failing).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_degraded_and_stopped_services_are_listed() {
        let status = serde_json::json!({ "services": [
            { "name": "a", "status": "running" },
            { "name": "b", "status": "degraded" },
            { "name": "c", "status": "paused" },
            { "name": "d", "status": "stopped" },
        ]});
        let out = only_failing(&status);
        assert!(out.contains("\"b\"") && out.contains("\"d\""), "{out}");
        assert!(!out.contains("\"a\"") && !out.contains("\"c\""), "{out}");
        let healthy = serde_json::json!({ "services": [{ "name": "a", "status": "running" }] });
        assert!(only_failing(&healthy).starts_with("(none"));
    }
}
