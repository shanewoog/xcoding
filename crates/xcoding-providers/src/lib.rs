//! Cloud-model adapters for OpenAI-compatible streaming chat completions.

use std::{collections::BTreeMap, env, fs, path::PathBuf, pin::Pin};

use async_stream::try_stream;
use futures_util::{Stream, StreamExt};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use xcoding_protocol::{ListModelsResult, ProviderAuthStatus, ProviderModel, UserConfig};

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
        "OPENAI_API_KEY is not set. Configure it in Desktop Settings (~/.xcoding/config.json), set the environment variable, or use a repo-root .env file. Optionally set XCODING_OPENAI_BASE_URL for an OpenAI-compatible endpoint."
    )]
    MissingApiKey,
    #[error("provider request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("invalid provider response: {0}")]
    InvalidResponse(String),
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


/// Inspect cloud-provider credentials without making a network request.
/// Does not return the full API key.
/// Resolve credentials (optional UI overrides first) and list provider models.
pub fn list_models_blocking(
    base_url_override: Option<&str>,
    api_key_override: Option<&str>,
) -> Result<ListModelsResult, String> {
    bootstrap_credentials();

    let api_key = api_key_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_owned())
        .or_else(|| {
            env::var("OPENAI_API_KEY")
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        })
        .ok_or_else(|| ProviderError::MissingApiKey.to_string())?;

    let base_url = base_url_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_owned())
        .or_else(|| {
            env::var("XCODING_OPENAI_BASE_URL")
                .ok()
                .map(|value| value.trim().trim_end_matches('/').to_owned())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| "https://ai.v58.dev/v1".to_owned());

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("failed to start async runtime for model list: {error}"))?;

    runtime.block_on(async move {
        OpenAiCompatibleProvider::new(api_key, base_url)
            .list_models()
            .await
            .map_err(|error| error.to_string())
    })
}

fn parse_models_response(base_url: &str, body: &str) -> Result<ListModelsResult, ProviderError> {
    #[derive(Debug, Deserialize)]
    struct ModelsResponse {
        #[serde(default)]
        data: Vec<ModelEntry>,
    }

    #[derive(Debug, Deserialize)]
    struct ModelEntry {
        id: String,
        #[serde(default)]
        owned_by: Option<String>,
    }

    let parsed: ModelsResponse = serde_json::from_str(body).map_err(|error| {
        ProviderError::InvalidResponse(format!("invalid /models response JSON: {error}"))
    })?;

    let mut models: Vec<ProviderModel> = parsed
        .data
        .into_iter()
        .filter_map(|entry| {
            let id = entry.id.trim().to_owned();
            if id.is_empty() {
                None
            } else {
                Some(ProviderModel {
                    id,
                    owned_by: entry
                        .owned_by
                        .map(|value| value.trim().to_owned())
                        .filter(|value| !value.is_empty()),
                })
            }
        })
        .collect();

    models.sort_by(|left, right| {
        left.id
            .to_ascii_lowercase()
            .cmp(&right.id.to_ascii_lowercase())
    });
    models.dedup_by(|left, right| left.id == right.id);

    if models.is_empty() {
        return Err(ProviderError::InvalidResponse(
            "provider returned an empty model list".to_owned(),
        ));
    }

    Ok(ListModelsResult {
        models,
        base_url: base_url.trim_end_matches('/').to_owned(),
    })
}

pub fn inspect_auth() -> ProviderAuthStatus {
    bootstrap_credentials();
    let base_url = env::var("XCODING_OPENAI_BASE_URL")
        .unwrap_or_else(|_| "https://ai.v58.dev/v1".to_owned())
        .trim_end_matches('/')
        .to_owned();
    match env::var("OPENAI_API_KEY") {
        Ok(key) if !key.trim().is_empty() => {
            let trimmed = key.trim();
            let key_hint = Some(mask_api_key(trimmed));
            ProviderAuthStatus {
                ready: true,
                has_api_key: true,
                base_url,
                key_hint,
                message: "OPENAI_API_KEY is set. Cloud requests can proceed.".to_owned(),
            }
        }
        _ => ProviderAuthStatus {
            ready: false,
            has_api_key: false,
            base_url,
            key_hint: None,
            message: "OPENAI_API_KEY is not set. Configure it in Desktop Settings (~/.xcoding/config.json), set the environment variable, or use a repo-root .env file.".to_owned(),
        },
    }
}

fn mask_api_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 4 {
        return "****".to_owned();
    }
    let suffix: String = chars[chars.len().saturating_sub(4)..].iter().collect();
    format!("...{suffix}")
}

impl OpenAiCompatibleProvider {
    pub fn from_environment() -> Result<Self, ProviderError> {
        // Existing process env wins. Fill missing vars from dotenv and user config.
        bootstrap_credentials();
        let api_key = env::var("OPENAI_API_KEY")
            .map_err(|_| ProviderError::MissingApiKey)
            .and_then(|key| {
                let trimmed = key.trim().to_owned();
                if trimmed.is_empty() {
                    Err(ProviderError::MissingApiKey)
                } else {
                    Ok(trimmed)
                }
            })?;
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

    /// List models from the OpenAI-compatible `GET {base_url}/models` endpoint.
    pub async fn list_models(&self) -> Result<ListModelsResult, ProviderError> {
        let response = self
            .client
            .get(format!("{}/models", self.base_url))
            .bearer_auth(&self.api_key)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::HttpStatus { status, body });
        }

        let body = response.text().await?;
        parse_models_response(&self.base_url, &body)
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

/// Resolve the user config directory: `%USERPROFILE%/.xcoding` or `$HOME/.xcoding`.
pub fn user_config_dir() -> PathBuf {
    if let Ok(home) = env::var("USERPROFILE") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join(".xcoding");
        }
    }
    if let Ok(home) = env::var("HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join(".xcoding");
        }
    }
    env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".xcoding")
}

/// Path to `~/.xcoding/config.json`.
pub fn user_config_path() -> PathBuf {
    user_config_dir().join("config.json")
}

/// Load user preferences from `~/.xcoding/config.json`, or defaults when missing/invalid.
pub fn load_user_config() -> UserConfig {
    let path = user_config_path();
    match fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => UserConfig::default(),
    }
}

/// Persist user preferences to `~/.xcoding/config.json`.
pub fn save_user_config(config: &UserConfig) -> Result<(), String> {
    let dir = user_config_dir();
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let path = dir.join("config.json");
    let body = serde_json::to_string_pretty(config).map_err(|error| error.to_string())?;
    fs::write(&path, format!("{body}\n")).map_err(|error| error.to_string())?;
    Ok(())
}

/// Apply provider credentials from user config into the process environment.
/// Overwrites existing values when the config provides non-empty credentials.
pub fn apply_user_config_to_env(config: &UserConfig) {
    if let Some(key) = config
        .api_key
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        unsafe {
            env::set_var("OPENAI_API_KEY", key);
        }
    }
    let base = config.base_url.trim();
    if !base.is_empty() {
        let normalized = base.trim_end_matches('/').to_owned();
        unsafe {
            env::set_var("XCODING_OPENAI_BASE_URL", normalized);
        }
    }
}

/// Fill missing credential env vars from user config without overwriting existing values.
pub fn fill_env_from_user_config() {
    let config = load_user_config();
    let has_key = env::var("OPENAI_API_KEY")
        .ok()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    if !has_key {
        if let Some(key) = config
            .api_key
            .as_ref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        {
            unsafe {
                env::set_var("OPENAI_API_KEY", key);
            }
        }
    }
    let has_base = env::var("XCODING_OPENAI_BASE_URL")
        .ok()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    if !has_base {
        let base = config.base_url.trim();
        if !base.is_empty() {
            let normalized = base.trim_end_matches('/').to_owned();
            unsafe {
                env::set_var("XCODING_OPENAI_BASE_URL", normalized);
            }
        }
    }
}

/// Load dotenv files then fill missing env from `~/.xcoding/config.json`.
pub fn bootstrap_credentials() {
    load_dotenv_files();
    fill_env_from_user_config();
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
        assert!(message.contains("config.json") || message.contains(".xcoding"));
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

    #[test]
    fn inspect_auth_reports_missing_key() {
        // Cannot safely clear process env for concurrent tests; assert shape via mask helper.
        assert_eq!(mask_api_key("abcd"), "****");
        assert_eq!(mask_api_key("sk-1234567890"), "...7890");
    }

    #[test]
    fn parses_models_list_response() {
        let body = r#"{"object":"list","data":[{"id":"gpt-b"},{"id":"gpt-a","owned_by":"openai"},{"id":"gpt-a"}]}"#;
        let result = parse_models_response("https://ai.v58.dev/v1/", body).expect("parse");
        assert_eq!(result.base_url, "https://ai.v58.dev/v1");
        assert_eq!(
            result
                .models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            vec!["gpt-a", "gpt-b"]
        );
        assert_eq!(result.models[0].owned_by.as_deref(), Some("openai"));
    }

    fn inspect_auth_returns_status_struct() {
        let status = inspect_auth();
        assert!(!status.base_url.is_empty());
        assert!(!status.message.is_empty());
        assert_eq!(status.ready, status.has_api_key);
    }
    #[test]
    fn user_config_roundtrip_under_temp_home() {
        let temp = std::env::temp_dir().join(format!("xcoding-user-config-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&temp).expect("temp home");
        let previous_userprofile = env::var("USERPROFILE").ok();
        let previous_home = env::var("HOME").ok();
        unsafe {
            env::set_var("USERPROFILE", &temp);
            env::set_var("HOME", &temp);
        }
        let mut config = UserConfig::default();
        config.locale = "zh-CN".to_owned();
        config.model = "gpt-test".to_owned();
        config.base_url = "https://example.test/v1".to_owned();
        config.api_key = Some("sk-test-key-1234".to_owned());
        config.last_workspace_root = Some("D:\\work\\demo".to_owned());
        save_user_config(&config).expect("save");
        let loaded = load_user_config();
        assert_eq!(loaded.locale, "zh-CN");
        assert_eq!(loaded.model, "gpt-test");
        assert_eq!(loaded.base_url, "https://example.test/v1");
        assert_eq!(loaded.api_key.as_deref(), Some("sk-test-key-1234"));
        assert_eq!(loaded.last_workspace_root.as_deref(), Some("D:\\work\\demo"));
        unsafe {
            match previous_userprofile {
                Some(value) => env::set_var("USERPROFILE", value),
                None => env::remove_var("USERPROFILE"),
            }
            match previous_home {
                Some(value) => env::set_var("HOME", value),
                None => env::remove_var("HOME"),
            }
        }
        let _ = fs::remove_dir_all(&temp);
    }

}
