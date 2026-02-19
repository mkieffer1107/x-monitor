use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::header;
use serde::{Deserialize, Serialize};

use crate::config::ResolvedAiProvider;

const DEFAULT_SYSTEM_PROMPT: &str = "You are an analyst for real-time Twitter monitoring. Provide concise, practical analysis based on the user's request.";
const DEFAULT_MONITOR_PROMPT: &str = "Summarize why this post matters and what to watch next.";
const USER_PROMPT_TEMPLATE: &str = "\
{{monitor_prompt}}

Twitter post:
{{post_text}}";

#[derive(Debug, Clone)]
pub struct AiClient {
    http: reqwest::Client,
}

#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    temperature: f32,
    messages: Vec<ChatMessage>,
}

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Option<Vec<ChatChoice>>,
    error: Option<ChatApiError>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatOutputMessage,
}

#[derive(Debug, Deserialize)]
struct ChatOutputMessage {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatApiError {
    message: Option<String>,
    r#type: Option<String>,
}

fn render_user_prompt(template: &str, monitor_prompt: &str, post_text: &str) -> String {
    template
        .replace("{{monitor_prompt}}", monitor_prompt.trim())
        .replace("{{post_text}}", post_text.trim())
}

pub fn prepare_prompts(prompt: &str, post_text: &str) -> (String, String, String) {
    let system_prompt = DEFAULT_SYSTEM_PROMPT.to_string();
    let monitor_prompt = if prompt.trim().is_empty() {
        DEFAULT_MONITOR_PROMPT.to_string()
    } else {
        prompt.trim().to_string()
    };
    let user_prompt = render_user_prompt(USER_PROMPT_TEMPLATE, &monitor_prompt, post_text);
    (system_prompt, monitor_prompt, user_prompt)
}

impl AiClient {
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .context("failed to construct ai http client")?;
        Ok(Self { http })
    }

    pub async fn analyze_post(
        &self,
        provider: ResolvedAiProvider,
        model: String,
        prompt: String,
        post_text: String,
    ) -> Result<String> {
        let base_url = provider.base_url.trim();
        if base_url.is_empty() {
            anyhow::bail!("AI endpoint is empty");
        }

        let model = model.trim().to_string();
        if model.is_empty() {
            anyhow::bail!("AI model ID is empty");
        }

        let endpoint = format!("{}/chat/completions", base_url.trim_end_matches('/'));

        let (system_prompt, _monitor_prompt, user_prompt) = prepare_prompts(&prompt, &post_text);

        let request = ChatCompletionRequest {
            model,
            temperature: 0.2,
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: system_prompt,
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: user_prompt,
                },
            ],
        };

        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {}", provider.api_key).parse()?,
        );
        headers.insert(header::CONTENT_TYPE, "application/json".parse()?);

        let response = self
            .http
            .post(endpoint)
            .headers(headers)
            .json(&request)
            .send()
            .await
            .context("failed to call ai endpoint")?;

        let status = response.status();
        let body = response
            .text()
            .await
            .context("failed to read ai response body")?;

        if !status.is_success() {
            anyhow::bail!("ai request failed ({status}): {body}");
        }

        let parsed = serde_json::from_str::<ChatCompletionResponse>(&body)
            .with_context(|| format!("failed to parse ai response: {body}"))?;

        if let Some(api_error) = parsed.error {
            let mut parts = Vec::new();
            if let Some(kind) = api_error.r#type {
                parts.push(kind);
            }
            if let Some(message) = api_error.message {
                parts.push(message);
            }
            let rendered = if parts.is_empty() {
                "unknown api error".to_string()
            } else {
                parts.join(": ")
            };
            anyhow::bail!("ai api error: {rendered}");
        }

        let output = parsed
            .choices
            .and_then(|choices| choices.into_iter().next())
            .and_then(|choice| choice.message.content)
            .map(|content| content.trim().to_string())
            .filter(|content| !content.is_empty())
            .context("ai response did not contain a message")?;

        Ok(output)
    }
}
