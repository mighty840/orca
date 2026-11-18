//! The read operations the MCP server offers, each as a tool and as a
//! resource, mapped to the `/api/v1` route that serves it.
//!
//! `action` is what the route requires in the master's `ROUTE_POLICY`;
//! orca-control's tests check the two agree. The master enforces it on
//! every call; here it only hides what the caller's role cannot use.

/// One argument of an operation.
#[derive(Debug, Clone, Copy)]
pub struct Param {
    pub name: &'static str,
    pub description: &'static str,
    pub required: bool,
    pub kind: Kind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Str,
    Int,
    Bool,
}

/// One read operation.
#[derive(Debug, Clone, Copy)]
pub struct Op {
    /// Tool name.
    pub tool: &'static str,
    pub description: &'static str,
    /// Route template; `{x}` segments are filled from the argument `x`, the
    /// other arguments go to the query string.
    pub route: &'static str,
    /// Fixed query parameters, e.g. `follow=false`.
    pub fixed_query: &'static str,
    pub action: &'static str,
    pub params: &'static [Param],
    /// Resource URI template (`orca://...`), with the same `{x}` names.
    pub resource: &'static str,
    /// The response is plain text (logs), not JSON.
    pub text: bool,
}

const NAME: Param = Param {
    name: "name",
    description: "Service name",
    required: true,
    kind: Kind::Str,
};

pub const OPS: &[Op] = &[
    Op {
        tool: "whoami",
        description: "The token this server uses: its name, role and allowed actions.",
        route: "/api/v1/whoami",
        fixed_query: "",
        action: "status",
        params: &[],
        resource: "orca://whoami",
        text: false,
    },
    Op {
        tool: "get_status",
        description: "Every service: replicas, status (running, degraded, stopped, paused), \
                      domains, node, CPU and memory, and the last failure.",
        route: "/api/v1/status",
        fixed_query: "",
        action: "status",
        params: &[Param {
            name: "project",
            description: "Only this project's services",
            required: false,
            kind: Kind::Str,
        }],
        resource: "orca://services",
        text: false,
    },
    Op {
        tool: "get_service",
        description: "One service's status and configuration (env values masked).",
        route: "/api/v1/services/{name}",
        fixed_query: "",
        action: "status",
        params: &[NAME],
        resource: "orca://services/{name}",
        text: false,
    },
    Op {
        tool: "get_history",
        description: "A service's recorded deploys, newest first (kept in memory by the master).",
        route: "/api/v1/services/{name}/history",
        fixed_query: "",
        action: "status",
        params: &[NAME],
        resource: "orca://services/{name}/history",
        text: false,
    },
    Op {
        tool: "get_logs",
        description: "The last lines of a service's container logs, with credentials masked.",
        route: "/api/v1/services/{name}/logs",
        fixed_query: "follow=false",
        action: "logs",
        params: &[
            NAME,
            Param {
                name: "tail",
                description: "Number of lines (default 200, at most 2000)",
                required: false,
                kind: Kind::Int,
            },
        ],
        resource: "orca://services/{name}/logs",
        text: true,
    },
    Op {
        tool: "get_cluster",
        description: "The cluster and its nodes: CPU, memory, disk, network, drain state, GPUs.",
        route: "/api/v1/cluster/info",
        fixed_query: "",
        action: "cluster_info",
        params: &[],
        resource: "orca://cluster",
        text: false,
    },
    Op {
        tool: "list_networks",
        description: "Docker networks per node, and which domains route where.",
        route: "/api/v1/cluster/networks",
        fixed_query: "",
        action: "status",
        params: &[],
        resource: "orca://networks",
        text: false,
    },
    Op {
        tool: "list_alerts",
        description: "Alert conversations opened by orca's AI monitor.",
        route: "/api/v1/alerts",
        fixed_query: "",
        action: "status",
        params: &[Param {
            name: "all",
            description: "Include resolved and dismissed alerts",
            required: false,
            kind: Kind::Bool,
        }],
        resource: "orca://alerts",
        text: false,
    },
    Op {
        tool: "get_alert",
        description: "One alert conversation.",
        route: "/api/v1/alerts/{id}",
        fixed_query: "",
        action: "status",
        params: &[Param {
            name: "id",
            description: "Alert id",
            required: true,
            kind: Kind::Str,
        }],
        resource: "orca://alerts/{id}",
        text: false,
    },
    Op {
        tool: "list_backups",
        description: "Backup status and snapshots per node.",
        route: "/api/v1/cluster/backups",
        fixed_query: "",
        action: "status",
        params: &[],
        resource: "orca://backups",
        text: false,
    },
    Op {
        tool: "list_webhooks",
        description: "Registered git webhooks (never their secrets).",
        route: "/api/v1/webhooks",
        fixed_query: "",
        action: "status",
        params: &[],
        resource: "orca://webhooks",
        text: false,
    },
    Op {
        tool: "secrets_inventory",
        description: "Secret names, where they are referenced and whether they are set. \
                      Never values. Admin only.",
        route: "/api/v1/secrets/usage",
        fixed_query: "",
        action: "secrets",
        params: &[],
        resource: "orca://secrets",
        text: false,
    },
];

/// Default and maximum log tail.
pub const TAIL_DEFAULT: u64 = 200;
pub const TAIL_MAX: u64 = 2000;

/// The operation called `tool`.
pub fn op(tool: &str) -> Option<&'static Op> {
    OPS.iter().find(|o| o.tool == tool)
}

mod paths;

pub use paths::match_resource;

#[cfg(test)]
mod tests;
