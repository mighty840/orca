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

[ai.alerts.channels]
slack = "https://hooks.slack.com/services/..."
webhook = "https://my-pagerduty-webhook/..."
```

When an alert fires, the AI investigates the root cause, suggests fixes, and tracks resolution.

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
