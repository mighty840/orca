//! Realistic alert scenarios for judging diagnosis quality.
//!
//! Shared by `tests/alert_scenarios_test.rs` (checks the evidence reaches the
//! prompt, no model) and `examples/alert_eval.rs` (sends each scenario to a
//! real model and scores the answer).

#![allow(dead_code)]

use chrono::{Duration, Utc};
use orca_ai::context::{ClusterContext, NodeSummary, ServiceSummary};
use orca_core::api_types::FailureInfo;

pub struct Scenario {
    pub name: &'static str,
    pub service: &'static str,
    pub trigger: String,
    pub ctx: ClusterContext,
    /// Must appear in the prompt: the evidence the diagnosis depends on.
    pub evidence: &'static [&'static str],
    /// A good answer mentions each of these (case-insensitive); `a|b`
    /// accepts either.
    pub answer_keywords: &'static [&'static str],
    /// A good answer mentions none of these.
    pub answer_forbidden: &'static [&'static str],
    /// The right answer to "Resolves on its own?", when there is one.
    pub self_resolving: Option<bool>,
}

const MASTER: &str = "46.225.100.82";
const AGENT: &str = "178.105.159.224";

fn nodes(agent_heartbeat_secs: i64, agent_status: &str) -> Vec<NodeSummary> {
    vec![
        NodeSummary {
            id: "master".into(),
            address: MASTER.into(),
            status: "healthy".into(),
            cpu_percent: 23.0,
            memory_percent: 64.0,
            heartbeat_age_secs: None,
            ..Default::default()
        },
        NodeSummary {
            id: "4412".into(),
            address: AGENT.into(),
            status: agent_status.into(),
            cpu_percent: 41.0,
            memory_percent: 45.0,
            heartbeat_age_secs: Some(agent_heartbeat_secs),
            ..Default::default()
        },
    ]
}

fn healthy(name: &str, node: &str) -> ServiceSummary {
    ServiceSummary {
        name: name.into(),
        runtime: "container".into(),
        replicas_running: 1,
        replicas_desired: 1,
        status: "healthy".into(),
        node: Some(node.into()),
        ..Default::default()
    }
}

fn background() -> Vec<ServiceSummary> {
    vec![
        healthy("gitea", MASTER),
        healthy("keycloak", MASTER),
        healthy("harbor-core", MASTER),
        healthy("litellm", MASTER),
        healthy("breakpilot-nextcloud", AGENT),
        healthy("breakpilot-collabora", AGENT),
        healthy("jitsi-web", AGENT),
        healthy("signoz", AGENT),
        healthy("signoz-clickhouse", AGENT),
    ]
}

fn cluster(mut services: Vec<ServiceSummary>, nodes: Vec<NodeSummary>) -> ClusterContext {
    services.extend(background());
    ClusterContext {
        cluster_name: "breakpilot".into(),
        nodes,
        services,
        recent_events: Vec::new(),
        active_alerts: Vec::new(),
    }
}

fn down_trigger(svc: &str, restarts: u32, errors: u64) -> String {
    format!(
        "Service '{svc}' has 0/1 replicas running. Restarts in 24h: {restarts}. Recent errors: {errors}"
    )
}

fn failure(
    reason: &str,
    exit: Option<i64>,
    restarts: u32,
    mins_ago: i64,
    msg: &str,
) -> Option<FailureInfo> {
    Some(FailureInfo {
        reason: reason.into(),
        message: msg.into(),
        exit_code: exit,
        restart_count: restarts,
        observed_at: Utc::now() - Duration::minutes(mins_ago),
    })
}

fn lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_string).collect()
}

pub fn all() -> Vec<Scenario> {
    vec![
        oom(),
        image_typo(),
        missing_env(),
        db_down(),
        port_in_use(),
        agent_gone(),
        upstream_errors(),
        deploy_blip(),
    ]
}

fn oom() -> Scenario {
    let svc = ServiceSummary {
        name: "signoz-otel-collector".into(),
        runtime: "container".into(),
        replicas_running: 0,
        replicas_desired: 1,
        status: "degraded".into(),
        restart_count_24h: 0,
        error_count_1h: 412,
        image: Some("signoz/signoz-otel-collector:v0.129.7".into()),
        last_deploy_at: Some(Utc::now() - Duration::days(3)),
        node: Some(AGENT.into()),
        memory_limit: Some("512Mi".into()),
        last_failure: failure(
            "OOMKilled",
            Some(137),
            0,
            50,
            "2026-09-26T10:19:58Z warn exporterhelper: Exporting failed. Will retry the request after interval. {\"kind\": \"exporter\", \"name\": \"clickhouselogsexporter\", \"error\": \"code: 210, message: Connection refused (signoz-clickhouse:9000)\", \"interval\": \"28.4s\"}\n2026-09-26T10:20:01Z warn batchprocessor: Sender failed {\"error\": \"sending queue is full\"}\n2026-09-26T10:20:03Z info memorylimiter: not configured",
        ),
        recent_logs: lines(
            "2026-09-26T10:17:03Z error clickhouselogsexporter: code: 210, message: Connection refused (signoz-clickhouse:9000)\n2026-09-26T10:18:40Z warn exporterhelper: Exporting failed. Will retry the request after interval. {\"queue_size\": 4812}\n2026-09-26T10:19:58Z warn exporterhelper: Exporting failed. {\"queue_size\": 9644}\n2026-09-26T10:20:01Z warn batchprocessor: Sender failed {\"error\": \"sending queue is full\"}",
        ),
        ..Default::default()
    };
    Scenario {
        name: "oom-killed",
        service: "signoz-otel-collector",
        trigger: down_trigger("signoz-otel-collector", 0, 412),
        ctx: cluster(vec![svc], nodes(4, "healthy")),
        evidence: &[
            "OOMKilled",
            "exit code: 137",
            "512Mi",
            "Connection refused (signoz-clickhouse:9000)",
        ],
        answer_keywords: &["137", "memory", "512"],
        answer_forbidden: &["orca service", "--replicas", "memory_limiter ="],
        self_resolving: Some(false),
    }
}

fn image_typo() -> Scenario {
    let svc = ServiceSummary {
        name: "breakpilot-portal".into(),
        runtime: "container".into(),
        replicas_running: 0,
        replicas_desired: 1,
        status: "degraded".into(),
        image: Some("repo.breakpilot.com/platform/portal:v1.2.30".into()),
        previous_image: Some("repo.breakpilot.com/platform/portal:v1.2.3".into()),
        last_deploy_at: Some(Utc::now() - Duration::minutes(6)),
        node: Some(AGENT.into()),
        last_failure: failure(
            "ImagePullError",
            None,
            0,
            5,
            "pull repo.breakpilot.com/platform/portal:v1.2.30: manifest unknown: manifest unknown",
        ),
        ..Default::default()
    };
    Scenario {
        name: "image-tag-typo",
        service: "breakpilot-portal",
        trigger: down_trigger("breakpilot-portal", 0, 0),
        ctx: cluster(vec![svc], nodes(3, "healthy")),
        evidence: &[
            "ImagePullError",
            "manifest unknown",
            "v1.2.30",
            "previous image: repo.breakpilot.com/platform/portal:v1.2.3",
        ],
        answer_keywords: &["v1.2.30", "v1.2.3", "tag"],
        answer_forbidden: &["orca service"],
        self_resolving: Some(false),
    }
}

fn missing_env() -> Scenario {
    let svc = ServiceSummary {
        name: "breakpilot-erp-backend".into(),
        runtime: "container".into(),
        replicas_running: 0,
        replicas_desired: 1,
        status: "degraded".into(),
        restart_count_24h: 14,
        image: Some("repo.breakpilot.com/erp/backend:2026.09.27".into()),
        previous_image: Some("repo.breakpilot.com/erp/backend:2026.09.20".into()),
        last_deploy_at: Some(Utc::now() - Duration::minutes(22)),
        node: Some(AGENT.into()),
        memory_limit: Some("2Gi".into()),
        memory_usage: Some("0B".into()),
        last_failure: failure(
            "CrashLoopBackOff",
            Some(1),
            14,
            1,
            "Traceback (most recent call last):\n  File \"/app/settings.py\", line 41, in <module>\n    DATABASE_URL = os.environ[\"DATABASE_URL\"]\nKeyError: 'DATABASE_URL'",
        ),
        recent_logs: lines(
            "[entrypoint] running migrations\nTraceback (most recent call last):\n  File \"/app/settings.py\", line 41, in <module>\n    DATABASE_URL = os.environ[\"DATABASE_URL\"]\nKeyError: 'DATABASE_URL'\n[entrypoint] exited with 1",
        ),
        ..Default::default()
    };
    Scenario {
        name: "crashloop-missing-env",
        service: "breakpilot-erp-backend",
        trigger: down_trigger("breakpilot-erp-backend", 14, 0),
        ctx: cluster(vec![svc], nodes(2, "healthy")),
        evidence: &[
            "CrashLoopBackOff",
            "KeyError: 'DATABASE_URL'",
            "exit code: 1",
            "2026.09.20",
        ],
        answer_keywords: &["DATABASE_URL", "rollback|2026.09.20"],
        answer_forbidden: &["memory", "orca service", "env = ["],
        self_resolving: Some(false),
    }
}

fn db_down() -> Scenario {
    let api = ServiceSummary {
        name: "breakpilot-dsms-api".into(),
        runtime: "container".into(),
        replicas_running: 0,
        replicas_desired: 1,
        status: "degraded".into(),
        restart_count_24h: 3,
        error_count_1h: 220,
        image: Some("repo.breakpilot.com/dsms/api:1.8.2".into()),
        last_deploy_at: Some(Utc::now() - Duration::days(5)),
        node: Some(MASTER.into()),
        health_check: Some("readiness GET /healthz on port 8080 every 10s, 3 failures".into()),
        last_failure: failure(
            "DeployTimeout",
            None,
            3,
            4,
            "readiness GET /healthz returned 503 for 120s",
        ),
        recent_logs: lines(
            "INFO  starting dsms-api 1.8.2\nERROR db: connection to server at \"breakpilot-db\" (172.18.0.9), port 5432 failed: Connection refused\nERROR /healthz 503: database unavailable\nERROR db: connection to server at \"breakpilot-db\" (172.18.0.9), port 5432 failed: Connection refused",
        ),
        ..Default::default()
    };
    let db = ServiceSummary {
        name: "breakpilot-db".into(),
        runtime: "container".into(),
        replicas_running: 0,
        replicas_desired: 1,
        status: "degraded".into(),
        restart_count_24h: 1,
        image: Some("postgres:17-alpine".into()),
        node: Some(MASTER.into()),
        last_failure: failure(
            "Error",
            Some(1),
            1,
            9,
            "FATAL:  could not write lock file \"postmaster.pid\": No space left on device",
        ),
        ..Default::default()
    };
    Scenario {
        name: "dependency-db-down",
        service: "breakpilot-dsms-api",
        trigger: down_trigger("breakpilot-dsms-api", 3, 220),
        ctx: cluster(vec![api, db], nodes(3, "healthy")),
        evidence: &[
            "port 5432 failed: Connection refused",
            "breakpilot-db: degraded",
            "last failure Error",
            "/healthz",
        ],
        answer_keywords: &["breakpilot-db"],
        answer_forbidden: &["orca service"],
        self_resolving: Some(false),
    }
}

fn port_in_use() -> Scenario {
    let svc = ServiceSummary {
        name: "coturn".into(),
        runtime: "container".into(),
        replicas_running: 0,
        replicas_desired: 1,
        status: "degraded".into(),
        image: Some("coturn/coturn:4.7".into()),
        last_deploy_at: Some(Utc::now() - Duration::minutes(3)),
        node: Some(AGENT.into()),
        last_failure: failure(
            "Error",
            None,
            0,
            3,
            "create container orca-coturn: Error response from daemon: driver failed programming external connectivity on endpoint orca-coturn: Bind for 0.0.0.0:3478 failed: port is already allocated",
        ),
        ..Default::default()
    };
    Scenario {
        name: "port-already-allocated",
        service: "coturn",
        trigger: down_trigger("coturn", 0, 0),
        ctx: cluster(vec![svc], nodes(5, "healthy")),
        evidence: &["port is already allocated", "0.0.0.0:3478"],
        answer_keywords: &["3478"],
        answer_forbidden: &["memory", "orca service"],
        self_resolving: Some(false),
    }
}

fn agent_gone() -> Scenario {
    let mut services = Vec::new();
    for name in [
        "breakpilot-nextcloud-app",
        "breakpilot-erp-frontend",
        "jitsi-jvb",
    ] {
        services.push(ServiceSummary {
            name: name.into(),
            runtime: "container".into(),
            replicas_running: 0,
            replicas_desired: 1,
            status: "degraded".into(),
            node: Some(AGENT.into()),
            last_failure: failure(
                "AgentUnreachable",
                None,
                0,
                5,
                "agent 4412 (178.105.159.224) has not sent a heartbeat for 312s",
            ),
            ..Default::default()
        });
    }
    let mut ctx = cluster(services, nodes(312, "healthy"));
    // The background services on the agent are down too.
    for s in &mut ctx.services {
        if s.node.as_deref() == Some(AGENT) && s.status == "healthy" {
            s.replicas_running = 0;
            s.status = "degraded".into();
        }
    }
    Scenario {
        name: "agent-unreachable",
        service: "breakpilot-nextcloud-app",
        trigger: down_trigger("breakpilot-nextcloud-app", 0, 0),
        ctx,
        evidence: &[
            "AgentUnreachable",
            "last heartbeat 312s ago",
            "jitsi-jvb: degraded",
        ],
        answer_keywords: &["178.105.159.224", "agent"],
        answer_forbidden: &["memory limit", "orca service", "orca scale"],
        self_resolving: Some(false),
    }
}

fn upstream_errors() -> Scenario {
    let svc = ServiceSummary {
        name: "breakpilot-billing".into(),
        runtime: "container".into(),
        replicas_running: 2,
        replicas_desired: 2,
        status: "healthy".into(),
        error_count_1h: 318,
        image: Some("repo.breakpilot.com/billing/api:3.4.0".into()),
        last_deploy_at: Some(Utc::now() - Duration::days(12)),
        node: Some(MASTER.into()),
        memory_limit: Some("512Mi".into()),
        memory_usage: Some("141Mi".into()),
        cpu_percent: Some(4.0),
        recent_logs: lines(
            "ERROR payments: POST https://api.stripe.com/v1/payment_intents timed out after 30s\nERROR payments: POST https://api.stripe.com/v1/payment_intents timed out after 30s\nWARN  retrying payment_intent for invoice INV-2026-0931 (attempt 3/5)\nINFO  GET /health 200 2ms\nERROR payments: POST https://api.stripe.com/v1/payment_intents timed out after 30s",
        ),
        ..Default::default()
    };
    Scenario {
        name: "external-api-timeouts",
        service: "breakpilot-billing",
        trigger:
            "Service 'breakpilot-billing' has 318 errors in the last hour. Recent log lines:\n"
                .into(),
        ctx: cluster(vec![svc], nodes(2, "healthy")),
        evidence: &[
            "api.stripe.com",
            "timed out after 30s",
            "141Mi used of a 512Mi limit",
        ],
        answer_keywords: &["stripe", "external"],
        answer_forbidden: &["orca rollback", "orca service"],
        self_resolving: None,
    }
}

fn deploy_blip() -> Scenario {
    let svc = ServiceSummary {
        name: "gitea-runner".into(),
        runtime: "container".into(),
        replicas_running: 0,
        replicas_desired: 1,
        status: "degraded".into(),
        image: Some("gitea/act_runner:0.2.13".into()),
        previous_image: Some("gitea/act_runner:0.2.12".into()),
        last_deploy_at: Some(Utc::now() - Duration::seconds(95)),
        node: Some(MASTER.into()),
        recent_logs: lines(
            "time=\"2026-09-28T07:40:02Z\" level=info msg=\"Starting runner daemon\"\ntime=\"2026-09-28T07:40:03Z\" level=info msg=\"pulling image catthehacker/ubuntu:act-22.04 (1.1 GB)\"",
        ),
        ..Default::default()
    };
    let mut ctx = cluster(vec![svc], nodes(3, "healthy"));
    ctx.recent_events = vec![
        "07:39:55 deploy gitea-runner: gitea/act_runner:0.2.12 -> 0.2.13 (webhook, orca-infra@4e1c2a9)".into(),
        "07:40:01 gitea-runner: old container stopped, new container starting".into(),
    ];
    Scenario {
        name: "deploy-in-progress",
        service: "gitea-runner",
        trigger: down_trigger("gitea-runner", 0, 0),
        ctx,
        evidence: &["0.2.13", "Starting runner daemon", "deploy gitea-runner"],
        answer_keywords: &["deploy"],
        answer_forbidden: &["orca service"],
        self_resolving: Some(true),
    }
}
