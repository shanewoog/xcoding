//! Cloud-model adapters. Phase 1 starts with the OpenAI-compatible chat-completions stream.

use std::{env, pin::Pin};

use async_stream::try_stream;
use futures_util::{Stream, StreamExt};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub type ProviderEventStream = Pin<Box<dyn Stream<Item = Result<String, ProviderError>> + Send>>;

#[derive(Clone, Debug, Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_owned(),
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_owned(),
            content: content.into(),
        }
    }
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("OPENAI_API_KEY is not set")]
    MissingApiKey,
    #[error("provider request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("provider returned HTTP {status}: {body}")]
    HttpStatus { status: StatusCode, body: String },
    #[error("invalid UTF-8 in provider stream: {0}")]
    Utf8(#[from] std::str::Utf8Error),
    #[error("invalid OpenAI-compatible stream event: {0}")]
    StreamJson(#[from] serde_json::Error),
}

pub struct OpenAiCompatibleProvider {
    api_key: String,
    base_url: String,
    client: Client,
}

impl OpenAiCompatibleProvider {
    pub fn from_environment() -> Result<Self, ProviderError> {
        let api_key = env::var("OPENAI_API_KEY").map_err(|_| ProviderError::MissingApiKey)?;
        let base_url = env::var("XCODING_OPENAI_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_owned());
        Ok(Self::new(api_key, base_url))
    }

    pub fn new(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            client: Client::new(),
        }
    }

    pub async fn stream_chat(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
    ) -> Result<ProviderEventStream, ProviderError> {
        let response = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({
                "model": model,
                "messages": messages,
                "stream": true
            }))
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::HttpStatus { status, body });
        }

        let stream = try_stream! {
            let mut bytes = response.bytes_stream();
            let mut buffer = Vec::new();

            while let Some(chunk) = bytes.next().await {
                buffer.extend_from_slice(&chunk?);

                while let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
                    let line: Vec<u8> = buffer.drain(..=newline).collect();
                    let line = std::str::from_utf8(&line)?.trim();
                    let Some(data) = line.strip_prefix("data:") else {
                        continue;
                    };
                    let data = data.trim();

                    if data == "[DONE]" {
                        return;
                    }

                    if let Some(delta) = parse_delta(data)? {
                        yield delta;
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }
}

#[derive(Deserialize)]
struct ChatCompletionChunk {
    choices: Vec<ChatCompletionChoice>,
}

#[derive(Deserialize)]
struct ChatCompletionChoice {
    delta: ChatCompletionDelta,
}

#[derive(Deserialize)]
struct ChatCompletionDelta {
    content: Option<String>,
}

fn parse_delta(data: &str) -> Result<Option<String>, ProviderError> {
    let chunk: ChatCompletionChunk = serde_json::from_str(data)?;
    Ok(chunk
        .choices
        .into_iter()
        .find_map(|choice| choice.delta.content))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_delta() {
        let delta =
            parse_delta(r#"{"choices":[{"delta":{"content":"Hello"}}]}"#).expect("event parses");
        assert_eq!(delta.as_deref(), Some("Hello"));
    }

    #[test]
    fn ignores_non_text_delta() {
        let delta =
            parse_delta(r#"{"choices":[{"delta":{"role":"assistant"}}]}"#).expect("event parses");
        assert_eq!(delta, None);
    }
}
