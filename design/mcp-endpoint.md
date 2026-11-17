# MCP endpoint: design and implementation plan (#80)

Status: draft for maintainer review, 2026-11-17. Umbrella: #80. Related: #259 (web
console), #265 (TUI CLI parity), #82 (RBAC/audit), #79 (OaaS tenancy), #309 / #315
(stop/rm semantics, v0.3.2 batch).

Line numbers are against `main` @ `31c36c0` (v0.3.1). The v0.3.2 work on
`fix/309-stop-agent-services` touches `reconciler.rs`, `operations/lifecycle.rs`,
`state.rs` and `handlers/mod.rs`, and shifts some of them.

## 1. Goals and non-goals

**Goals**

- Let any MCP client (Claude Desktop, Claude Code, custom agents) inspect an orca cluster
  natively: services, logs, nodes, alerts, backups, networks, secret *inventory*.
- `/api/v1` and its RBAC stay the only implementation of every operation. MCP is a
  client of the API, never a second code path into the reconciler.
- Ship read-only resources, read tools and template-only prompts first; add a few safe
  mutating tools after the v0.3.2 lifecycle fixes land.
- Let the in-orca AI monitor *propose* a remediation that a human approves and orca
  executes through the same tool layer (the notify -> act loop from #80).

**Non-goals (first releases)**

- No new validation or business logic in the MCP layer. Anything a tool needs that the API
  lacks becomes an API endpoint first, and #259/#265 reuse it.
- No secret values, ever: not via resources, tools, prompts, logs or env dumps.
- No `exec` (admin-only for a reason, `crates/orca-control/src/auth.rs:55-56`).
- No live config editing (`update_service_config` from the issue). #259 already decided
  config changes go through the GitOps repo as a PR (Q4).
- No server-side LLM calls inside MCP prompts or tools (section 6).
- No tenant scoping. `ApiToken` has only `name`/`value`/`role`
  (`crates/orca-core/src/config/cluster.rs:268-278`), so this waits for #79/#82.
- No autonomous remediation. The monitor loopback always has a human approval gate.

## 2. What exists today

### 2.1 HTTP API surface

The router is `crates/orca-control/src/api/mod.rs:14-93`. Everything except the agent
WebSocket (`api/mod.rs:18`) is behind `auth_middleware` (`api/mod.rs:86-89`). Webhook and
cluster routes are merged in at `api/mod.rs:84-85` (`webhook.rs:78-88`,
`cluster_handlers.rs:16-25`). Handler paths below are relative to
`crates/orca-control/src/api/handlers/` unless qualified.

Read endpoints:

| Route | Handler | Response | Action |
|---|---|---|---|
| `GET /api/v1/status?project=` | `mod.rs:156` | `StatusResponse` (`orca-core/src/api_types.rs:28`) | status |
| `GET /services/{name}/logs?tail=&follow=` | `ops/cluster.rs:21` | text; `LogsQuery` (`api_types.rs:127`) | logs |
| `GET /cluster/info` | `cluster_handlers.rs:28` | untyped `json!` (`cluster_handlers.rs:64`) | cluster_info |
| `GET /cluster/networks` | `ops/networks.rs:35` | `ClusterNetworksResponse` (`api_types.rs:273`) | status |
| `GET /cluster/backups` | `ops/backups.rs:30` | `ClusterBackupsResponse` (`api_types.rs:143`) | status |
| `GET /alerts?all=`, `/alerts/{id}` | `alerts.rs:79`, `:95` | `AlertConversation` (`orca-core/src/types/alert.rs:10`) | status |
| `GET /secrets`, `/secrets/usage` | `secrets.rs:49`, `:97` | key names / `SecretsUsageResponse`, never values | secrets (admin) |
| `GET /webhooks`, `/webhooks/{id}/invocations` | `webhook.rs:272` | `WebhookEntry` exposes only `has_secret` (`api_types.rs:200-211`) | status |

Mutating endpoints:

| Route | Handler | Action |
|---|---|---|
| `POST /deploy` (full `ServicesConfig`) | `mod.rs:32` | deploy |
| `POST /services/{name}/redeploy` · `promote` · `start` | `ops/deploy.rs:56`, `:69`, `:91` | deploy |
| `POST /services/{name}/scale` · `rollback` | `ops/deploy.rs:18`, `:40` | scale, rollback |
| `DELETE /services/{name}` (**pauses**) · `DELETE /projects/{p}` · `POST /stop` | `ops/deploy.rs:79`, `:102`, `:122` | stop |
| `POST /cluster/backups/trigger` | `ops/backups.rs:186` | admin |
| `POST /cluster/nodes/{id}/drain` · `undrain` | `cluster_handlers.rs:206`, `:213` | admin |
| `POST /alerts/{id}/reply` · `dismiss` · `resolve` | `alerts.rs:109`, `:136`, `:143` | admin |
| `POST`/`DELETE /secrets/{key}` | `secrets.rs:64`, `:181` | secrets (admin) |
| `POST /ask` (server-side LLM call) | `ask.rs:34` | status |
| webhooks, token rotation, node register/heartbeat | `auth.rs:63-79` | admin |

### 2.2 Clients

`crates/orca-cli/src/client.rs` (348 lines) covers cluster_info (:43), deploy (:61),
status (:78), logs/logs_follow (:87, :104), scale (:124), stop (:138), start (:150),
redeploy (:161), rollback (:172), alerts list/view/reply/dismiss/resolve (:183-230),
promote (:242), stop_all (:253), webhooks (:261-294) and token_rotation (:310). That is
not the whole surface. Secrets, backups, networks, drain and exec are called from
`crates/orca-cli/src/handlers/*` directly (e.g. `handlers/exec.rs:52`,
`handlers/token_rotate.rs:14-26`). The TUI has a second client,
`crates/orca-tui/src/api.rs` (369 lines), with its own `ClusterInfo`/`NodeInfo`
(`:77`, `:88`), because `/cluster/info` is untyped. An MCP stdio server would make a third.

### 2.3 Auth

- `ROUTE_POLICY` (`auth.rs:30-83`) maps (method, route template) to an action. Unknown
  routes are admin-only (`auth.rs:89-97`), so it fails closed.
- Roles are `Admin | Deployer | Viewer` (`cluster.rs:282-290`). `Role::can`
  (`cluster.rs:297-306`) gives viewer `status|logs|cluster_info`; deployer adds
  `deploy|stop|scale|rollback`; admin gets everything (`secrets`, `admin`).
- `resolve_token` (`auth.rs:126-145`) returns only a `Role`. Legacy `api_tokens`, which
  include the **cluster token agents join with**, resolve to admin and have no name
  (`auth.rs:130-137`, `state.rs:40-42`). The middleware does not attach the principal to
  the request, so nothing can log who acted.
- With no tokens configured, everything is allowed (`auth.rs:163-166`).
- The API is plain HTTP (`api_listen.rs:77`) on port 6880 (`cluster.rs:367-369`), bound to
  `0.0.0.0` by default (`cluster.rs:329-335`). The issue's `https://master:6880/mcp`
  doesn't match this.
- There is no audit log. `webhook_invocations` is "not durable audit" (`state.rs:97-100`).
  Rate limiting exists only in the proxy, per IP (`orca-proxy/src/rate_limit.rs:15`).

### 2.4 AI layer: today's "act" paths

- `AiMonitor` (`crates/orca-ai/src/monitor.rs:22`) runs `analyze_cycle_with`
  (`monitor.rs:93-178`): plan, ask the model with no lock held, record, dispatch to
  channels. Data comes through `ContextProvider` (`monitor.rs:184-192`), which
  `orca-control` implements (`alert_context.rs:29-153`).
- An alert auto-closes as remediated once replicas match and error counts are low
  (`monitor.rs:112-124`, `:167-176`), whatever caused the recovery.
- The model's reply is scanned for an `orca ...` command (`command_parser.rs:6`,
  `conversation.rs:129`, `:309`) and stored as `suggested_command`. **Nothing executes
  it.** `orca alerts fix` prints it and says "approve once interactive confirm is wired"
  (`crates/orca-cli/src/handlers/ops.rs:131-148`).
- `[ai.auto_remediate]` (`orca-core/src/config/ai.rs:19`, `:101-111`) is parsed but has
  **no consumer** outside config tests. `mark_remediated` (`conversation.rs:348`) has no
  caller. `AlertState::Acknowledged` (`alert.rs:36`) is never set, and no ack endpoint
  exists.
- Conversations live only in memory (`crates/orca-control/src/alerts.rs:8-9`).
- `alerts::reply` holds the engine write lock while `operator_reply` calls the LLM
  (`handlers/alerts.rs:117-119`), the pattern the monitor dropped in #181
  (`monitor.rs:82-85`). dismiss/resolve take the same path, but the engine special-cases
  them (no LLM) and they still dispatch to delivery channels.
- `redact_line` (`orca-ai/src/redact.rs:31`) is applied only in the alert prompt builder
  (`alert_prompt.rs:157-171`). `GET .../logs` returns raw lines (`ops/cluster.rs:37-43`),
  and `FailureInfo` (`api_types.rs:39`) often carries raw log lines inside `/status`.

### 2.5 Secrets facts

- Env is stored templated (`${secrets.X}`) in `svc.config`. It is resolved only into the
  `WorkloadSpec` (`crates/orca-control/src/routes.rs:12-20`, `:205`). Literal env values
  in `service.toml` can still be credentials.
- The store is flat. Project scoping is a key prefix `<project>.KEY` (`routes.rs:13-14`,
  `orca-core/src/secrets/store.rs:191`), so the issue's `secrets/{project}/` is a prefix
  filter over `/secrets/usage`.

### 2.6 Rust MCP SDK (`rmcp`)

- `rmcp` is not in `Cargo.lock`. The local registry cache (a crates.io mirror) has 0.16.0,
  1.8.0, 3.3.0 and 3.9.0. 3.9.0 is the newest *cached*, not necessarily the newest
  published. Its repository is `github.com/modelcontextprotocol/rust-sdk`, and it needs
  Rust 1.88. CI uses `dtolnay/rust-toolchain@stable` (`.github/workflows/ci.yml:32`).
- Verified in the 3.9.0 source:
  - features `server` (default), `macros`, `transport-io` (stdio, `transport/io.rs:16`),
    `transport-streamable-http-server`, `client`, `auth`, `elicitation`;
  - macros `#[tool]`, `#[tool_router]`, `#[tool_handler]`, `#[prompt]`,
    `#[prompt_router]`;
  - `ServerHandler` with `list_resources`, `list_resource_templates`, `read_resource`,
    `list_prompts`, `get_prompt` and `call_tool`;
  - tool annotations `read_only_hint` and `destructive_hint` (`model/tool.rs:63`, `:73`).
- HTTP (verified):
  - `StreamableHttpService` implements `tower_service::Service<Request<_>>`
    (`transport/streamable_http_server/tower.rs:1473-1495`).
  - It injects the inbound `http::request::Parts` into the handler's request context
    extensions (`tower.rs:1417-1467`), so a tool can read the caller's `Authorization`
    header per request.
  - It serves `POST`, plus `GET`/`DELETE` in legacy-session or replay modes
    (`tower.rs:1973-1977`).
  - `StreamableHttpServerConfig` (`tower.rs:219`) accepts **only loopback `Host`** by
    default and serves protocol `2026-07-28` statelessly.
  - *Unverified*: the exact `nest_service` wiring into axum 0.8.8, and whether the `http`
    and `tower` versions unify without duplicates.
- Schemas come from `schemars` 1.x (`schemars 1.2.1` is already in `Cargo.lock`
  transitively), but **no workspace type derives `JsonSchema`**. Either add derives
  behind an `orca-core` feature, or define small MCP-side arg structs.

## 3. Architecture

### 3.1 A new `orca-mcp` leaf crate

```
orca-core  <-  orca-mcp (rmcp, reqwest)       depends only on core (Q6: + ai for redact)
   ^             ^          ^
orca-agent       |          |
   ^             |          |
orca-control ----+          |    mounts /mcp with InProcessBackend
   ^                        |
orca-cli -------------------+    `orca mcp` (stdio) with HttpBackend
```

`orca-mcp` holds the `ServerHandler`, the resource/tool/prompt tables, the arg types and
redaction. It reaches orca only through:

```rust
#[async_trait]
pub trait ApiBackend: Send + Sync {
    async fn call(&self, method: Method, path_and_query: &str, body: Option<Value>)
        -> Result<ApiReply, McpBackendError>;          // status + bytes
    async fn whoami(&self) -> Result<Principal, McpBackendError>;
}
```

- `HttpBackend` (in `orca-mcp`): reqwest plus a bearer token. Used by `orca mcp`.
- `InProcessBackend` (in `orca-control`): a clone of the authed API `Router`, called with
  `tower::ServiceExt::oneshot`, forwarding the MCP caller's `Authorization` header (taken
  from the injected `Parts`). Each inner request goes through `auth_middleware` and
  `ROUTE_POLICY`, with `MatchedPath` set by the router, exactly as an external call does.

This follows the dependency rule, and **every MCP operation is an `/api/v1` request**.
Some logic lives only in handlers, such as `restart_dependents` after a manual rollback
or redeploy (`ops/deploy.rs:44-52`, `:60-65`). Calling `reconciler::*` from MCP would
silently skip it.

Rejected alternatives:

- *MCP handlers in `orca-control` calling the reconciler.* RBAC would be duplicated per
  tool, handler logic skipped, and stdio would need a second implementation.
- *Loopback over TCP to 127.0.0.1:6880.* It depends on `api_bind` including loopback
  (`cluster.rs:329-335`) and adds a socket hop for nothing.

### 3.2 Transports, in order

1. **stdio, `orca mcp [--api URL] [--token T]`, first.** It adds no network surface and
   inherits `ROUTE_POLICY` unchanged. Two hazards:
   - **stdout must carry protocol only.** The CLI's default `tracing_subscriber::fmt::layer()`
     writes to stdout (`crates/orca-cli/src/main.rs:43-60`), so `orca mcp` must set up
     tracing to stderr before anything logs, and must not hit any `println!` path.
   - **Default token.** `read_token` falls back to `ORCA_TOKEN`, then `~/.orca/cluster.token`
     (`handlers/server.rs:303-315`). On the master that is the cluster token: legacy and
     admin, and the agents' join credential. `orca mcp` should refuse a token that
     `whoami` reports as legacy, unless `--allow-cluster-token` is given, and point the
     user at a named `[[token]]`.
2. **Streamable HTTP, `/mcp` on the master, second.**
   - Mounted only when `[mcp] enabled = true` **and** at least one token is configured.
     The open mode of `auth.rs:163-166` must not extend to an LLM-facing endpoint.
   - `ROUTE_POLICY` gets `POST`/`GET`/`DELETE /mcp` -> `status`, so a viewer can connect.
     Per-tool gating comes from the forwarded token on each inner request, plus
     `tools/list` filtering.
   - `allowed_hosts` comes from `[mcp]`, defaulting to loopback plus `cluster.domain`.
     Origin validation stays on.
   - Docs state that 6880 is plain HTTP. Remote clients should connect over the mesh or
     through orca-proxy TLS.

### 3.3 Authorization, audit and rate limits: server-side

stdio runs on the operator's machine, so anything enforced only in `orca-mcp` can be
bypassed by calling `/api/v1` directly. Security-relevant controls therefore go in the
API middleware:

- **`Principal { name, role, legacy }`** comes from `resolve_token` and is inserted as a
  request extension. Legacy tokens get the name `legacy-<n>`. `GET /api/v1/whoami` returns
  `{name, role, legacy, actions}`; MCP uses it to filter `tools/list`, #259 to pick the
  admin or read-only UI, and the TUI to hide actions.
- **Audit middleware**: one `tracing` event on target `orca::audit` for every non-GET
  request, with principal, route, status and latency, plus `X-Orca-Client: mcp-stdio|
  mcp-http|cli|tui` and `X-Orca-Mcp-Tool`. They go into a bounded ring buffer served at
  `GET /api/v1/audit` (admin). Durable storage in `cluster.db` is #82's job; this defines
  the event shape. #259's activity feed renders the same events.
- **Rate limits**: a per-principal token bucket on mutating actions in the middleware, so
  it applies to every client. `orca-mcp` adds advisory local limits (reads 60/min) and a
  per-target cooldown. The proxy's per-IP `RateLimiter` is the wrong key: every in-process
  call comes from one address.
- A unit test asserts that each MCP tool's declared action equals
  `required_action(method, template)` of the route it calls, so the MCP view cannot drift
  from `ROUTE_POLICY`. Enforcement is still the inner request.

## 4. Resources

The URI scheme is `orca://`. Resources return JSON (logs are text) and are capped at
`[mcp] max_resource_bytes` (default 256 KiB).

| Resource | Backing endpoint | Role | Notes / gap |
|---|---|---|---|
| `orca://cluster`, `orca://nodes` | `/cluster/info` (`cluster_handlers.rs:28`) | viewer | **Gap**: typed `ClusterInfoResponse` in core (replaces TUI `api.rs:77-88`) |
| `orca://services` | `/status` (`handlers/mod.rs:156`) | viewer | `last_failure.detail` passed through `redact_line` |
| `orca://services/{name}` | **gap** | viewer | New `GET /api/v1/services/{name}`: status plus declared config, with **env keys only, values masked** (2.5) |
| `orca://services/{name}/logs{?tail}` | `.../logs?follow=false` (`ops/cluster.rs:21`) | viewer | `tail` default 200, max 2000; redacted; no follow |
| `orca://services/{name}/metrics` | `/status` cpu/memory (`api_types.rs:58`) | viewer | Point-in-time only; **gap**: no time series |
| `orca://services/{name}/history` | **gap** | viewer | `deploy_history.rs` (in memory, 20 per service, `:11-12`) has no route; #259 needs it |
| `orca://networks` | `/cluster/networks` (`ops/networks.rs:35`) | viewer | Includes domain routes (`network_domains.rs:71`) |
| `orca://routes` | **gap** | viewer | Proxy route table not exposed; `/networks` covers most of it |
| `orca://alerts`, `orca://alerts/{id}` | `alerts.rs:79`, `:95` | viewer | Returns 503 without `[ai]`, which maps to an empty list plus a note |
| `orca://backups` | `/cluster/backups` (`ops/backups.rs:30`) | viewer | |
| `orca://webhooks` | `/webhooks` (`webhook.rs:272`) | viewer | Only `has_secret`, never the secret |
| `orca://secrets{?prefix}` | `/secrets/usage` (`secrets.rs:97`) | **admin** | Names, references and `in_store` only |

Not exposed: `/metrics` (Prometheus text), token-rotation status, exec.

## 5. Tools

Every read resource gets a matching read tool, because some clients handle tools better
than resources. All tools set `read_only_hint`/`destructive_hint`.

### 5.1 Read-only (first release)

`get_status(project?)`, `get_service(name)`, `get_logs(name, tail?)`, `list_nodes()`,
`list_alerts(all?)`, `get_alert(id)`, `list_backups()`, `list_networks()`,
`list_webhooks()`, `secrets_inventory(prefix?)` (admin).

`ask` (`POST /api/v1/ask`) is **not** exposed. It is a server-side LLM call that spends
the cluster's model budget under a viewer-level action (`auth.rs:42-43`), and the MCP
client is already an LLM.

### 5.2 First mutating batch (PR 4)

| Tool | Endpoint | Role | Why it is acceptable |
|---|---|---|---|
| `dismiss_alert(id)`, `resolve_alert(id)` | `alerts.rs:136`, `:143` | admin | No LLM call; changes state and notifies channels |
| `trigger_backup(node_id?, master?)` | `ops/backups.rs:186` | admin | Additive; same path as the scheduler |
| `redeploy(name)` | `ops/deploy.rs:56` -> `operations/mod.rs:65` | deployer | Has #291 exclusivity and an explicit remote path. **Caveat**: re-reads `service.toml` from disk (`operations/mod.rs:79-84`), so it can apply uncommitted edits |
| `drain_node(id)`, `undrain_node(id)` | `cluster_handlers.rs:206-235` | admin | Only flips a flag that placement reads (`placement.rs:65`, `:81-84`, `:151`); the handler moves nothing |

Each mutating tool:

- takes `dry_run` (**default `true`**). Dry runs need server-side support, because
  `config_diff::changed_services` (`config_diff.rs:28`, `pub(crate)`, already shared by
  `declarative.rs:141` and `webhook.rs:371`) and `dependents.rs` live in `orca-control`.
  PR 4 adds `?dry_run=true` to `redeploy`. It returns the target node, the dependents that
  would restart, and whether the on-disk config differs from the stored one;
- requires `confirm: "<target>"` when `dry_run = false`, so a prompt-injected call has to
  name the exact target. MCP elicitation could replace this later; client support is
  *unverified*.

### 5.3 Held back until the v0.3.2 semantics settle

**Do not expose these until #309/#315 have shipped and the semantics are documented.**
Tool names must not encode today's behaviour (no `delete_service` that actually pauses).

| Operation | Why |
|---|---|
| `stop` (`ops/deploy.rs:79`) | Pauses rather than deletes (#315). On agent-placed services it leaves containers running while `status` says "paused" (#309; skip at `reconciler.rs:159-167`) |
| `stop_project`, `stop_all` (`ops/deploy.rs:102`, `:122`) | Same `reconciler::stop` path |
| `start` (`ops/deploy.rs:91`) | A no-op for agent-placed services whose placeholder is `Running` (#309) |
| `scale` (`ops/deploy.rs:18`) | Same remote-skip path (#309; untested for scale) |
| `rm` | Added in #319 as `POST /api/v1/services/{name}/remove` (admin only, `?force=`); `DELETE` keeps pausing. Expose only after it has shipped and been used in prod |
| `deploy` (`handlers/mod.rs:32`) | Takes a full `ServicesConfig`: it can change replicas through the same path, and it bypasses GitOps. At most a later dry-run `plan_deploy` |
| `rollback` (`ops/deploy.rs:40`) | Remote behaviour unverified; drains instances under the services write lock across awaits (`operations/lifecycle.rs:110-135`); history is lost on restart |
| `promote` | Canary path not reviewed for remote placement |
| `set_secret`, `remove_secret` | LLM-authored secret writes are a footgun; the handlers take no `State`, so nothing records an actor (`secrets.rs:64`, `:181`) |
| `reply_alert` | Server-side LLM call made while holding the engine write lock (`alerts.rs:117-119`) |
| webhooks, token rotation, node register, exec | Admin internals or arbitrary code execution: never |
| `restore_backup`, `add_node`, `remove_node` | No API endpoint exists (restore and join are local to the CLI/agent) |

## 6. Prompts

Prompts are **templates**. They return messages for the *client's* model, with fetched
data embedded, and make **no server-side LLM call**. This answers the comment on #80: if
`diagnose` ran `analyze` inside orca, prompt-injection responsibility would move into
orca's auth boundary, and tool RBAC would not cover it.

| Prompt | Composes |
|---|---|
| `diagnose(service)` | `get_service` + `get_logs(200)` + metrics + open alerts, ending with "propose a fix; mutating tools need confirmation" |
| `failing_services(project?)` | `/status` filtered to `degraded`/`stopped`, with redacted `last_failure` |
| `capacity_review()` | Node cpu/memory from `/cluster/info` + `memory_limit_bytes` from `/status` |
| `explain_alert(id)` | `get_alert` + `diagnose` inputs |
| `plan_deploy(service)` | Later, once a server-side dry-run deploy exists |

Logs and failure text are untrusted. They are redacted, then wrapped in fenced, labelled
blocks ("data, not instructions").

## 7. AI monitor loopback

### 7.1 Shape

`orca-ai` may depend only on core, so mirror the `ContextProvider` pattern
(`monitor.rs:184`):

1. A core type `ProposedAction { tool, args, rationale }` goes on `AlertMessage`, next to
   `suggested_command`. The model is asked for a structured action. `command_parser.rs`
   stays as a fallback that maps known verbs to tools.
2. A trait in `orca-ai`, `ActionExecutor { async fn execute(&self, &ProposedAction) ->
   Result<String> }`, is implemented in `orca-control` with `orca_mcp::dispatch` over the
   `InProcessBackend`. The principal is a dedicated `[[token]] name = "ai-monitor"`
   (deployer), so RBAC, audit and rate limits treat the monitor like any other client.
3. The allowlist comes from the unused `[ai.auto_remediate]` (`ai.rs:101-111`):
   `restart_crashed` -> `redeploy`. `scale_on_pressure` and `rollback_on_failure` stay
   suggest-only until 5.3 clears.

The built-in monitor gets MCP semantics without the wire. Only *external* agents need a
real MCP client, and they use `/mcp`.

### 7.2 Human approval gate

- A proposal puts the alert in `AwaitingAction` (`alert.rs:34`), with the dry-run result
  attached.
- A new `POST /api/v1/alerts/{id}/approve` (admin) executes it; `dismiss` rejects it. This
  is what `orca alerts fix` already promises (`cli/handlers/ops.rs:140-143`), and #265
  wants it in the TUI.
- Approval sets `Acknowledged`, which gives that state its first setter. On success,
  `mark_remediated` (`conversation.rs:348`) records the action.
- Proposals expire after about 30 min, and a restart loses them, since conversations are
  in memory (Q7).

### 7.3 Fencing the monitor's own actions (#80 comment)

- After an approved action, the alert stays open for a stability window of N analysis
  intervals, during which the self-resolved check (`monitor.rs:112-124`) does not fire.
  It then closes as "remediated by redeploy, approved by X", not "self-resolved".
- A recurrence inside the window **escalates** (higher severity, re-notify) instead of
  re-proposing. At most one automated action per service per window.
- While an action is in flight for a service, the planner does not treat it as newly down.
  `in_flight` is reused for this (`operations/mod.rs:66-70`).

## 8. Shared API work with #259 and #265 (build once)

| Endpoint / change | MCP | #259 web | #265 TUI |
|---|---|---|---|
| `Principal` + `GET /api/v1/whoami` | tools/list, stdio token check | admin vs read-only | hide actions |
| Audit middleware + `GET /api/v1/audit` | audit | activity feed | — |
| `GET /api/v1/services/{name}` (masked env) | service resource | service detail | detail pane |
| `GET /api/v1/services/{name}/history` | history | deploy history + rollback | rollback picker |
| Typed `ClusterInfoResponse` (core) | cluster/nodes | Overview, Nodes | replaces `api.rs:77-88` |
| `?dry_run=true` on redeploy (later on deploy) | dry-run, `plan_deploy` | "redeploy with progress" preview | — |
| `POST /api/v1/alerts/{id}/approve` | loopback | Alerts screen | `alerts fix` |
| Redaction in core (`?redact=true` on logs) | logs | optional | — |
| Later: log follow over SSE/WS, cert status, backup restore | — | listed in #259 | live logs, restore |

#259's gap list is partly stale: exec over WebSocket exists (`api/mod.rs:29-32`), and so
does backup listing (`api/mod.rs:44`).

## 9. Phased PR plan

New code goes in new files: `handlers/mod.rs` is already 253 lines, `api_types.rs` 384,
`client.rs` 348. The soft limit is 250 lines and the hard one 500, enforced by CI
(`.github/workflows/ci.yml:49`). Unit tests go in `#[cfg(test)] mod tests` or sibling
`*_tests.rs` files (`auth.rs:201-203`); integration tests in `crates/*/tests/`.

**PR 1: shared API prerequisites (no MCP).** Can ship in v0.4.0 with #259/#265.
- `Principal`, `GET /api/v1/whoami`, audit middleware + `GET /api/v1/audit`, and
  per-principal mutation rate limit (new `auth_principal.rs`, `audit.rs`).
- `GET /api/v1/services/{name}` and `.../history` in a new
  `handlers/ops/service_detail.rs`.
- Typed `ClusterInfoResponse` in a new `orca-core/src/api_types/cluster.rs`. This starts
  splitting `api_types.rs` into a directory, with re-exports kept.
- Tests:
  - `every_mounted_route_is_classified` still passes;
  - a `whoami` role matrix, including the legacy cluster token;
  - `crates/orca-control/tests/service_detail_test.rs` (`MockRuntime`) shows env values
    are masked and resolved secrets never appear;
  - audit events for each mutating route.

*Status (2026-11-17): implemented on `feat/80-api-prereqs`, stacked on the v0.3.2 PR
#319. It differs from the plan above in these ways:*
- *Which secrets a service references (`${secrets.KEY}` values and `secret_refs`) is the
  `secrets` inventory, admin-only in `ROUTE_POLICY`. Service detail and history mask
  them for any caller without the `secrets` action.*
- *Audit events without a valid token (401s, the signed GitHub webhook) are logged on
  `orca::audit` but not kept in the buffer, so unauthenticated traffic cannot flush it.*
- *The rate limit is `[api_rate_limit]` in `cluster.toml`, **off by default**, and
  `mutations_per_minute = 60` when enabled. It counts mutating requests per token name,
  excluding agent register and heartbeat, and answers 429 with `Retry-After`.*
- *The TUI still parses cluster info by hand; switching it to `ClusterInfoResponse` is
  left to #265.*

**PR 2: `orca-mcp` + `orca mcp` stdio, read resources, read tools and prompts.**
- `crates/orca-mcp/src/{lib, backend, http_backend, resources, tools_read, prompts,
  redact, limits}.rs`. The CLI gets `handlers/mcp.rs` and a small `Mcp` variant in
  `commands.rs` (246 lines). Tracing goes to stderr, and legacy tokens are refused by
  default.
- rmcp pinned, `default-features = false`, features `server`, `macros`, `transport-io`.
- Tests:
  - `MockBackend` tests per resource and tool;
  - a tool-action == `required_action` table test;
  - redaction (logs and `last_failure`) and the size cap;
  - prompt rendering with injected log text;
  - an rmcp stdio round trip in `crates/orca-mcp/tests/stdio_test.rs` (rmcp `client` as a
    dev-dependency), checking that nothing but protocol reaches stdout.

*Status (2026-11-17): implemented on `feat/80-mcp-stdio`, stacked on PR 1. Decisions:
a new `orca-mcp` crate (Q1), its own small HTTP client (Q2), and `redact.rs` moved to
`orca-core` with a re-export from `orca-ai` (Q6). rmcp is pinned to `=3.9.0` with
`server` + `transport-io`. Differences from the plan above: one table (`catalog`)
drives tools, resources and the `ROUTE_POLICY` test; a client-side read limit in `orca mcp`
(`limits.rs`, 60 API calls/min by default, `--reads-per-minute`), because the master's
rate limit and audit cover mutations and exec only, so MCP reads are not audited; no
`orca://services/{name}/metrics` (the numbers are in `get_status`/`get_service`).*

**PR 3: `/mcp` Streamable HTTP on the master.**
- `[mcp]` config (`enabled` false by default, `allowed_hosts`, `max_resource_bytes`) in a
  new `orca-core/src/config/mcp.rs`, and `InProcessBackend` in a new
  `orca-control/src/mcp_mount.rs`.
- Tests:
  - a viewer sees only read tools;
  - a deployer calling an admin tool gets 403 from the inner request;
  - no tokens configured means `/mcp` is unmounted;
  - a foreign `Host` is rejected;
  - audit events carry `X-Orca-Client: mcp-http`.

**PR 4: safe mutating tools (5.2)** plus server-side `?dry_run=true` on redeploy.
- Tests:
  - a default `dry_run` never mutates (the mock records no POST without `dry_run`);
  - a `confirm` mismatch is rejected;
  - the cooldown and the server-side rate limit both hold;
  - the dry run reports on-disk drift.

**PR 5: monitor loopback with approval.** Sequenced after the ai-ops runbook work (as #80
asks) and after v0.3.2.
- `ProposedAction` (core), `ActionExecutor` (ai), executor + `/alerts/{id}/approve`
  (control), and fencing in a new `orca-ai/src/monitor_fence.rs` (`monitor.rs` is 318
  lines). CLI/TUI `alerts fix` gets wired to `approve`.
- Tests (extend `monitor_resilience_tests.rs`):
  - a recovery inside the window is not "self-resolved";
  - a recurrence escalates;
  - nothing executes without approval;
  - only allowlisted tools are proposed.

**After v0.3.2:** revisit 5.3 (`stop`/`start`/`scale`/`rm`/`rollback`) once the `DELETE`
semantics are final.

## 10. Open questions

1. **Crate or module?** A new `orca-mcp` crate (one more to publish and version), or
   `orca-control::mcp` plus a thin CLI wrapper?
2. **Shared client.** Extract an `orca-client` crate in PR 1 (it would serve the CLI, TUI,
   MCP stdio and #259), or accept a third HTTP client?
3. **Open clusters.** With no tokens configured, is "`/mcp` never mounts, stdio is
   read-only" right, or should MCP mirror the API's open mode?
4. **Config changes.** Drop `update_service_config` in favour of #259's GitOps PR flow, or
   add a later `propose_config_change` that opens a PR on the infra repo?
5. **Monitor identity.** A dedicated `[[token]]`, or a synthetic in-process principal with
   no secret in `cluster.toml`?
6. **Redaction location.** Should `orca-mcp` depend on `orca-ai` for `redact_line`, or
   should `redact.rs` move to `orca-core` so the logs endpoint can offer `?redact=true` to
   every client?
7. **Pending approvals.** Are in-memory, expiring proposals acceptable, or should they
   persist?
8. **Approval channels.** API/CLI/TUI only, or Slack interactive buttons too (which needs
   an inbound, signature-verified endpoint)?
9. **Transport scope.** Is stdio enough for the first release, so `/mcp` (PR 3) can slip
   past v0.4.0?
10. **Roles.** Is `deployer` the right floor for an MCP-initiated `redeploy`, or should
    MCP mutation need its own permission (a natural fit for #82's permission sets)?
