//! High-level AI operations: ask questions and generate configs.
//!
//! These functions are used directly by the CLI — they create their own
//! backend from the provided config, so no running server is needed.

use orca_core::config::AiConfig;

use crate::backend::{ChatMessage, LlmBackend, OpenAiCompatibleBackend, Role};

const ASK_SYSTEM_PROMPT: &str = "\
You are Orca AI, the ops assistant for an orca container orchestrator cluster.
Orca is NOT Kubernetes — it has its own CLI. Available commands:
  orca status                  — show all services
  orca logs <service> --tail N — view container logs
  orca scale <service> <n>     — scale replicas
  orca stop <service>          — stop a service
  orca deploy                  — redeploy all services
  orca promote <service>       — promote canary to stable
  orca tui                     — terminal dashboard

Diagnose issues using the cluster status and logs provided. Suggest fixes as \
exact `orca` CLI commands. Never suggest kubectl, docker, or k8s commands. Be concise.";

const GENERATE_SYSTEM_PROMPT: &str = "\
Generate a valid orca service.toml. Output ONLY a ```toml code block.
Use this exact format:
```toml
[[service]]
name = \"my-service\"
image = \"image:tag\"
replicas = 1
port = 8080
domain = \"app.example.com\"
health = \"/healthz\"

[service.env]
KEY = \"value\"

[service.resources]
memory = \"512Mi\"
cpu = 1.0

[service.volume]
path = \"/data\"
```
Only use these fields: name, image, replicas, port, domain, health, env, \
resources (memory/cpu), volume (path), network, aliases, internal, placement (node). \
Do not invent fields. No text outside the code block.";

/// Build an LLM backend from the AI config section.
fn build_backend(config: &AiConfig) -> anyhow::Result<OpenAiCompatibleBackend> {
    let endpoint = config
        .endpoint
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("No AI endpoint configured in cluster.toml [ai] section"))?;
    let model = config
        .model
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("No AI model configured in cluster.toml [ai] section"))?;

    Ok(OpenAiCompatibleBackend::new(
        endpoint.to_string(),
        model.to_string(),
        config.api_key.clone(),
    )
    .with_max_tokens(4000))
}

/// Ask the AI a question about the cluster, providing status and log context.
pub async fn ask(
    config: &AiConfig,
    question: &str,
    status_text: &str,
    logs_text: &str,
) -> anyhow::Result<String> {
    let backend = build_backend(config)?;

    let user_content = format!(
        "Context:\n{status}\n\nLogs:\n{logs}\n\nQuestion: {question}",
        status = if status_text.is_empty() {
            "(no status available — server may not be running)"
        } else {
            status_text
        },
        logs = if logs_text.is_empty() {
            "(no recent logs)"
        } else {
            logs_text
        },
        question = question,
    );

    let messages = vec![
        ChatMessage {
            role: Role::System,
            content: ASK_SYSTEM_PROMPT.to_string(),
        },
        ChatMessage {
            role: Role::User,
            content: user_content,
        },
    ];

    let response = backend.chat(&messages).await?;
    Ok(response.content)
}

/// Generate a service.toml configuration from a natural language description.
pub async fn generate(config: &AiConfig, description: &str) -> anyhow::Result<String> {
    let backend = build_backend(config)?;

    let messages = vec![
        ChatMessage {
            role: Role::System,
            content: GENERATE_SYSTEM_PROMPT.to_string(),
        },
        ChatMessage {
            role: Role::User,
            content: description.to_string(),
        },
    ];

    let response = backend.chat(&messages).await?;
    // Extract TOML block if wrapped in ```toml ... ```
    Ok(extract_toml_block(&response.content))
}

/// Extract a ```toml``` fenced code block from LLM output, or return as-is.
fn extract_toml_block(content: &str) -> String {
    if let Some(start) = content.find("```toml") {
        let after_fence = &content[start + 7..];
        if let Some(end) = after_fence.find("```") {
            return after_fence[..end].trim().to_string();
        }
    }
    content.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_prompt_contains_sections() {
        assert!(ASK_SYSTEM_PROMPT.contains("orca"));
        assert!(ASK_SYSTEM_PROMPT.contains("Orca AI"));
        assert!(ASK_SYSTEM_PROMPT.contains("Diagnose"));
        assert!(GENERATE_SYSTEM_PROMPT.contains("orca"));
        assert!(GENERATE_SYSTEM_PROMPT.contains("service.toml"));
    }

    #[test]
    fn test_build_context_with_empty_status() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        // We can't actually call the LLM, but we can verify the message
        // construction works with empty inputs by checking build_backend
        // requires a valid config.
        let config = AiConfig {
            provider: "litellm".to_string(),
            endpoint: None,
            model: None,
            api_key: None,
            alerts: None,
            auto_remediate: None,
        };
        let result = rt.block_on(ask(&config, "test?", "", ""));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No AI endpoint configured")
        );
    }

    #[test]
    fn test_extract_toml_block_with_fence() {
        let input = "Here is the config:\n```toml\n[[service]]\nname = \"pg\"\n```\nDone.";
        let result = extract_toml_block(input);
        assert_eq!(result, "[[service]]\nname = \"pg\"");
    }

    #[test]
    fn test_extract_toml_block_no_fence() {
        let input = "[[service]]\nname = \"pg\"";
        let result = extract_toml_block(input);
        assert_eq!(result, input);
    }
}
