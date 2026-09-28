# AI Ops

Orca includes an AI operations assistant that can diagnose issues, analyze logs, and suggest fixes using any OpenAI-compatible LLM.

## Setup

Add an `[ai]` section to your `cluster.toml`:

::: code-group

```toml [LiteLLM]
[ai]
provider = "litellm"
endpoint = "https://llm.example.com"
model = "qwen3-30b"
api_key = "${secrets.ai_api_key}"
```

```toml [Ollama]
[ai]
provider = "ollama"
endpoint = "http://localhost:11434"
model = "qwen3:30b"
```

```toml [OpenAI]
[ai]
provider = "openai"
model = "gpt-4o"
api_key = "${secrets.openai_key}"
```

:::

## Ask Your Cluster

Query the AI assistant with full cluster context:

```bash
orca ask "why is the API slow?"
orca ask "which services are using the most memory?"
orca ask "should I scale the worker service?"
```

The assistant has access to service status, logs, metrics, and configuration to provide informed answers.

## Summarise Logs

Pipe a service's recent log buffer through the AI backend for a concise digest
of what's happening, likely root causes, and suggested next steps:

```bash
orca logs api --summarize
orca logs api --summarize --tail 500
```

The default tail is enough context for most issues; bump `--tail` when you need
a longer window. The same `[ai]` provider used for `orca ask` is used here, so
no additional configuration is required.

## Generate Configs

Let AI generate service configurations from natural language:

```bash
orca generate "deploy a postgres database with 10GB storage in zone eu-1"
```

## Conversational Alerts

Configure AI-powered alert analysis:

```toml
[ai.alerts]
enabled = true
analysis_interval_secs = 60
# A "service down" alert only fires once a service has been at 0 running
# replicas for this many seconds. Deploys/rollouts briefly drop to 0 replicas
# while the new container pulls and starts; the grace window keeps a normal
# rollout from opening (and then auto-remediating) an alert on every deploy.
alert_grace_secs = 120

[ai.alerts.channels]
slack = "https://hooks.slack.com/services/..."
webhook = "https://my-pagerduty-webhook/..."
```

When an alert fires, the AI investigates the root cause, suggests fixes, and tracks resolution.

### What an alert diagnosis sees

Each alert's prompt carries the evidence for the affected service:

- The failure orca recorded: reason (`OOMKilled`, `ImagePullError`,
  `CrashLoopBackOff`, ...), exit code, restart count and its log tail.
- Its last 40 log lines, fetched from its node when the alert opens, also from
  a crashed container.
- Its image and the previous one, when it was deployed, memory use against its
  limit, CPU, its health checks, and its node with heartbeat age. A node
  without a heartbeat for 60 s shows as unreachable.

The rest of the cluster is summarized as one line per service with a problem
and a list of the healthy ones. The prompt also lists the valid `orca`
commands and `service.toml` keys, so suggested fixes use real syntax (for
example `orca scale <service> <N>`).

### Redaction

Secrets are masked in every log line before it leaves the cluster:
`key=value` pairs whose name looks like a password, token or key, bearer
tokens, passwords in URLs, and long opaque tokens. Digests and hashes are
kept.

### Answer format

The model answers in fixed sections:

| Section | Content |
|---------|---------|
| **What happened** | One or two concrete sentences |
| **Evidence** | The log lines, exit codes and metrics it relies on |
| **Likely cause** | With confidence (high, medium, low); names another service if that is the real cause |
| **Other possible causes** | Only when confidence isn't high |
| **Fix** | The exact `orca` commands or `service.toml` change |
| **Verify** | How to confirm the fix worked |
| **Resolves on its own?** | Yes or no, and why |

The suggested command is taken from the Fix section. The model call has a 30 s
timeout; when it fails, the alert is still delivered without a diagnosis.

### Working with alerts

```bash
orca alerts list                 # Open alerts (--all includes resolved)
orca alerts view <id>
orca alerts reply <id> "did the database restart?"
orca alerts fix <id>             # Print the suggested command
orca alerts dismiss <id>
orca alerts resolve <id>
```

`orca alerts fix` prints the command for review; it doesn't run it. In the
TUI, open view `7`.

`alert_grace_secs` (default 120) debounces transient outages: a service must stay down longer than the grace window before it pages, so a webhook deploy doesn't spam open/remediated notifications. Raise it for services whose image pulls or health checks routinely take longer than two minutes; a genuine sustained outage still opens exactly one alert.

## Auto-Remediation

::: warning
Auto-remediation is powerful but should be enabled cautiously in production. Start with `restart_crashed` only.
:::

```toml
[ai.auto_remediate]
restart_crashed = true            # Restart crashed containers
scale_on_pressure = false         # Auto-scale under load
rollback_on_failure = false       # Rollback failed deploys
```

## GPU Monitoring

On nodes with GPUs, the AI monitor tracks thermal and VRAM utilization:

```bash
orca ask "what's the GPU utilization on the inference node?"
```

## Supported Providers

Any OpenAI-compatible API works:

| Provider | Local/Remote | Notes |
|----------|-------------|-------|
| **Ollama** | Local | Best for air-gapped setups |
| **LiteLLM** | Proxy | Route to any backend model |
| **vLLM** | Self-hosted | High-throughput inference |
| **OpenAI** | Remote | GPT-4o, GPT-4o-mini |
