//! Cloud-model adapters for OpenAI-compatible streaming chat completions.

use std::{collections::BTreeMap, env, pin::Pin};

use async_stream::try_stream;
use futures_util::{Stream, StreamExt};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

pub type ProviderEventStream =
    Pin<Box<dyn Stream<Item = Result<ProviderEvent, ProviderError>> + Send>>;

#[derive(Clone, Debug, PartialEq)]
pub enum ProviderEvent {
    TextDelta(String),
    ToolCall(ProviderToolCall),
}

#[derive(Clone, Debug, Serialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ProviderToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self::content("system", content)
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::content("user", content)
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::content("assistant", content)
    }

    pub fn assistant_tool_calls(tool_calls: Vec<ProviderToolCall>) -> Self {
        Self {
            role: "assistant".to_owned(),
            content: None,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".to_owned(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }

    fn content(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ProviderToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ProviderFunctionCall,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ProviderFunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error(
        "OPENAI_API_KEY is not set. Set it in the environment or a repo-root .env file, and optionally set XCODING_OPENAI_BASE_URL for an OpenAI-compatible endpoint."
    )]
    MissingApiKey,
    #[error("provider request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("{}", format_http_status_message(status, body))]
    HttpStatus { status: StatusCode, body: String },
    #[error("invalid UTF-8 in provider stream: {0}")]
    Utf8(#[from] std::str::Utf8Error),
    #[error("invalid OpenAI-compatible stream event: {0}")]
    StreamJson(#[from] serde_json::Error),
    #[error("invalid tool call from provider: {0}")]
    InvalidToolCall(String),
}

fn format_http_status_message(status: &StatusCode, body: &str) -> String {
    let truncated = truncate_provider_body(body, 280);
    if *status == StatusCode::UNAUTHORIZED || *status == StatusCode::FORBIDDEN {
        format!(
            "Cloud provider authentication failed (HTTP {}). Check OPENAI_API_KEY and XCODING_OPENAI_BASE_URL. Provider response: {}",
            status.as_u16(),
            truncated
        )
    } else {
        format!(
            "Cloud provider request failed (HTTP {}). Check OPENAI_API_KEY and XCODING_OPENAI_BASE_URL if this looks like an auth or endpoint issue. Provider response: {}",
            status.as_u16(),
            truncated
        )
    }
}

fn truncate_provider_body(body: &str, max_chars: usize) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return "(empty body)".to_owned();
    }
    let mut truncated = trimmed.chars().take(max_chars).collect::<String>();
    if trimmed.chars().count() > max_chars {
        truncated.push_str("...");
    }
    truncated
}

pub struct OpenAiCompatibleProvider {
    api_key: String,
    base_url: String,
    client: Client,
}

impl OpenAiCompatibleProvider {
    pub fn from_environment() -> Result<Self, ProviderError> {
        // Existing process env wins. Fill missing vars from nearby .env files.
        load_dotenv_files();
        let api_key = env::var("OPENAI_API_KEY").map_err(|_| ProviderError::MissingApiKey)?;
        let base_url = env::var("XCODING_OPENAI_BASE_URL")
            .unwrap_or_else(|_| "https://ai.v58.dev/v1".to_owned());
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
        tools: &[ToolDefinition],
    ) -> Result<ProviderEventStream, ProviderError> {
        let mut body = json!({
            "model": model,
            "messages": messages,
            "stream": true
        });
        if !tools.is_empty() {
            body["tools"] = Value::Array(
                tools
                    .iter()
                    .map(|tool| {
                        json!({
                            "type": "function",
                            "function": tool
                        })
                    })
                    .collect(),
            );
        }

        let response = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&body)
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
            let mut tool_calls = BTreeMap::new();

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
                        for tool_call in completed_tool_calls(std::mem::take(&mut tool_calls))? {
                            yield ProviderEvent::ToolCall(tool_call);
                        }
                        return;
                    }

                    let parsed = parse_chunk(data)?;
                    if let Some(content) = parsed.content {
                        yield ProviderEvent::TextDelta(content);
                    }
                    for delta in parsed.tool_calls {
                        tool_calls
                            .entry(delta.index)
                            .or_insert_with(ToolCallAccumulator::default)
                            .merge(delta);
                    }
                }
            }

            for tool_call in completed_tool_calls(tool_calls)? {
                yield ProviderEvent::ToolCall(tool_call);
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

#[derive(Default, Deserialize)]
struct ChatCompletionDelta {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ToolCallDelta>,
}

#[derive(Default, Deserialize)]
struct ToolCallDelta {
    index: usize,
    id: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    function: ToolFunctionDelta,
}

#[derive(Default, Deserialize)]
struct ToolFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

struct ParsedChunk {
    content: Option<String>,
    tool_calls: Vec<ToolCallDelta>,
}

#[derive(Default)]
struct ToolCallAccumulator {
    id: Option<String>,
    kind: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl ToolCallAccumulator {
    fn merge(&mut self, delta: ToolCallDelta) {
        if let Some(id) = delta.id {
            if !id.is_empty() {
                self.id = Some(id);
            }
        }
        if let Some(kind) = delta.kind {
            if !kind.is_empty() {
                self.kind = Some(kind);
            }
        }
        if let Some(name) = delta.function.name {
            if !name.is_empty() {
                self.name = Some(name);
            }
        }
        if let Some(arguments) = delta.function.arguments {
            self.arguments.push_str(&arguments);
        }
    }

    fn finish(self) -> Result<ProviderToolCall, ProviderError> {
        Ok(ProviderToolCall {
            id: self
                .id
                .ok_or_else(|| ProviderError::InvalidToolCall("missing id".to_owned()))?,
            kind: self.kind.unwrap_or_else(|| "function".to_owned()),
            function: ProviderFunctionCall {
                name: self.name.ok_or_else(|| {
                    ProviderError::InvalidToolCall("missing function name".to_owned())
                })?,
                arguments: self.arguments,
            },
        })
    }
}

fn load_dotenv_files() {
    // dotenvy does not override existing process environment values.
    let _ = dotenvy::dotenv();
    if let Ok(cwd) = env::current_dir() {
        let mut dir = cwd;
        loop {
            let candidate = dir.join(".env");
            if candidate.is_file() {
                let _ = dotenvy::from_path(&candidate);
                break;
            }
            if !dir.pop() {
                break;
            }
        }
    }
}
fn parse_chunk(data: &str) -> Result<ParsedChunk, ProviderError> {
    let chunk: ChatCompletionChunk = serde_json::from_str(data)?;
    let mut choices = chunk.choices.into_iter();
    let delta = choices
        .next()
        .map(|choice| choice.delta)
        .unwrap_or_default();
    Ok(ParsedChunk {
        content: delta.content,
        tool_calls: delta.tool_calls,
    })
}

fn completed_tool_calls(
    tool_calls: BTreeMap<usize, ToolCallAccumulator>,
) -> Result<Vec<ProviderToolCall>, ProviderError> {
    tool_calls
        .into_values()
        .map(ToolCallAccumulator::finish)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_delta() {
        let parsed =
            parse_chunk(r#"{"choices":[{"delta":{"content":"Hello"}}]}"#).expect("event parses");
        assert_eq!(parsed.content.as_deref(), Some("Hello"));
    }

    #[test]
    fn parses_incremental_tool_call() {
        let first = parse_chunk(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"read_file","arguments":"{\"path\":"}}]}}]}"#)
            .expect("first event parses");
        let second = parse_chunk(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"src/lib.rs\"}"}}]}}]}"#)
            .expect("second event parses");
        let mut accumulator = ToolCallAccumulator::default();
        accumulator.merge(first.tool_calls.into_iter().next().expect("first call"));
        accumulator.merge(second.tool_calls.into_iter().next().expect("second call"));

        assert_eq!(
            accumulator.finish().expect("tool call completes"),
            ProviderToolCall {
                id: "call_1".to_owned(),
                kind: "function".to_owned(),
                function: ProviderFunctionCall {
                    name: "read_file".to_owned(),
                    arguments: r#"{"path":"src/lib.rs"}"#.to_owned(),
                },
            }
        );
    }

    #[test]
    fn ignores_empty_tool_name_fragments() {
        let first = parse_chunk(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"list_dir","arguments":""}}]}}]}"#)
            .expect("first event parses");
        let second = parse_chunk(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"","arguments":"{\"path\":\".\"}"}}]}}]}"#)
            .expect("second event parses");
        let mut accumulator = ToolCallAccumulator::default();
        accumulator.merge(first.tool_calls.into_iter().next().expect("first call"));
        accumulator.merge(second.tool_calls.into_iter().next().expect("second call"));

        assert_eq!(
            accumulator.finish().expect("tool call completes"),
            ProviderToolCall {
                id: "call_1".to_owned(),
                kind: "function".to_owned(),
                function: ProviderFunctionCall {
                    name: "list_dir".to_owned(),
                    arguments: r#"{"path":"."}"#.to_owned(),
                },
            }
        );
    }

    #[test]
    fn missing_api_key_message_is_actionable() {
        let message = ProviderError::MissingApiKey.to_string();
        assert!(message.contains("OPENAI_API_KEY is not set"));
        assert!(message.contains(".env"));
        assert!(message.contains("XCODING_OPENAI_BASE_URL"));
    }

    #[test]
    fn unauthorized_status_message_is_actionable() {
        let message = ProviderError::HttpStatus {
            status: StatusCode::UNAUTHORIZED,
            body: r#"{"code":"INVALID_API_KEY","message":"Invalid API key"}"#.to_owned(),
        }
        .to_string();
        assert!(message.contains("Cloud provider authentication failed (HTTP 401)"));
        assert!(message.contains("OPENAI_API_KEY"));
        assert!(message.contains("XCODING_OPENAI_BASE_URL"));
        assert!(message.contains("INVALID_API_KEY"));
    }

    #[test]
    fn non_auth_status_message_includes_truncated_body() {
        let long_body = "x".repeat(400);
        let message = ProviderError::HttpStatus {
            status: StatusCode::BAD_GATEWAY,
            body: long_body,
        }
        .to_string();
        assert!(message.contains("Cloud provider request failed (HTTP 502)"));
        assert!(message.contains("OPENAI_API_KEY"));
        assert!(message.ends_with("..."));
        assert!(message.len() < 500);
    }
}
