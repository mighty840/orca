use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// A message in an LLM conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

/// Response from the LLM backend.
#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub content: String,
    pub tokens_used: Option<u64>,
}

/// Pluggable LLM backend. Supports any OpenAI-compatible API.
#[async_trait]
pub trait LlmBackend: Send + Sync + 'static {
    async fn chat(&self, messages: &[ChatMessage]) -> anyhow::Result<ChatResponse>;
    fn name(&self) -> &str;
}

/// OpenAI-compatible backend (works with LiteLLM, Ollama, vLLM, OpenAI, Anthropic proxy, etc.)
pub struct OpenAiCompatibleBackend {
    client: reqwest::Client,
    endpoint: String,
    model: String,
    api_key: Option<String>,
    max_tokens: u32,
}

impl OpenAiCompatibleBackend {
    pub fn new(endpoint: String, model: String, api_key: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoint,
            model,
            api_key,
            max_tokens: 4000,
        }
    }

    /// Set the max_tokens parameter for completions.
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    temperature: f64,
    max_tokens: u32,
}

#[derive(Deserialize)]
struct ChatApiResponse {
    choices: Vec<ChatChoice>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Deserialize)]
struct ChatChoiceMessage {
    content: String,
}

#[derive(Deserialize)]
struct Usage {
    total_tokens: Option<u64>,
}

#[async_trait]
impl LlmBackend for OpenAiCompatibleBackend {
    async fn chat(&self, messages: &[ChatMessage]) -> anyhow::Result<ChatResponse> {
        let url = format!(
            "{}/v1/chat/completions",
            self.endpoint.trim_end_matches('/')
        );

        let mut req = self.client.post(&url).json(&ChatRequest {
            model: &self.model,
            messages,
            temperature: 0.3,
            max_tokens: self.max_tokens,
        });

        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }

        let resp: ChatApiResponse = req.send().await?.error_for_status()?.json().await?;

        let content = resp
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .unwrap_or_default();

        Ok(ChatResponse {
            content,
            tokens_used: resp.usage.and_then(|u| u.total_tokens),
        })
    }

    fn name(&self) -> &str {
        "openai-compatible"
    }
}
