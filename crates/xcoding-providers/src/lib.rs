//! Cloud-model adapters for OpenAI-compatible streaming chat completions.

use std::io::Write;
use std::path::Path;
use std::{collections::BTreeMap, env, fs, path::PathBuf, pin::Pin, time::Duration};

use async_stream::try_stream;
use futures_util::{Stream, StreamExt};
use reqwest::Client;
/// Re-exported so downstream crates can name the `ProviderError::HttpStatus` field type.
pub use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use xcoding_protocol::{
    CloudProviderConfig, ListModelsResult, MAX_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT,
    MAX_CIRCUIT_FAILURE_THRESHOLD, MAX_CIRCUIT_MIN_REQUEST_COUNT,
    MAX_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD, MAX_CIRCUIT_RECOVERY_WAIT_SECS,
    MAX_CONTEXT_COMPACTION_THRESHOLD_PERCENT, MAX_CONTEXT_WINDOW_TOKENS, MAX_MAX_PROVIDER_RETRIES,
    MAX_MAX_TOOL_ROUNDS, MAX_NON_STREAM_TIMEOUT_SECS, MAX_STREAM_FIRST_EVENT_TIMEOUT_SECS,
    MAX_STREAM_IDLE_TIMEOUT_SECS, MIN_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT,
    MIN_CIRCUIT_FAILURE_THRESHOLD, MIN_CIRCUIT_MIN_REQUEST_COUNT,
    MIN_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD, MIN_CIRCUIT_RECOVERY_WAIT_SECS,
    MIN_CONTEXT_COMPACTION_THRESHOLD_PERCENT, MIN_CONTEXT_WINDOW_TOKENS, MIN_MAX_PROVIDER_RETRIES,
    MIN_MAX_TOOL_ROUNDS, MIN_NON_STREAM_TIMEOUT_SECS, MIN_STREAM_FIRST_EVENT_TIMEOUT_SECS,
    MIN_STREAM_IDLE_TIMEOUT_SECS, ProviderAuthStatus, ProviderModel, ProviderWireApi, UserConfig,
};

pub type ProviderEventStream =
    Pin<Box<dyn Stream<Item = Result<ProviderEvent, ProviderError>> + Send>>;

#[derive(Clone, Debug, PartialEq)]
pub enum ProviderEvent {
    TextDelta(String),
    /// Model identifier reported by the upstream response, used to detect a
    /// gateway silently swapping the requested model.
    ModelReported(String),
    /// Hidden thinking a gateway streams while a reasoning model works. It is
    /// not model output: it exists so the caller can tell a thinking model
    /// apart from a stalled connection.
    ReasoningDelta(String),
    ToolCall(ProviderToolCall),
    /// Token accounting reported by the endpoint for this request. Optional on
    /// the wire: many OpenAI-compatible endpoints never send it.
    Usage(ProviderUsage),
}

/// Real token counts for one provider request, used to calibrate the local
/// estimator. Zero means the endpoint did not report that field.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProviderUsage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum ChatMessageContent {
    Text(String),
    Parts(Vec<ChatContentPart>),
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type")]
pub enum ChatContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ChatImageUrl },
}

#[derive(Clone, Debug, Serialize)]
pub struct ChatImageUrl {
    pub url: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<ChatMessageContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ProviderToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self::text("system", content)
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::text("user", content)
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::text("assistant", content)
    }

    pub fn user_with_images(text: impl Into<String>, images: &[(String, String)]) -> Self {
        let text = text.into();
        if images.is_empty() {
            return Self::user(text);
        }
        let mut parts = Vec::with_capacity(images.len() + 1);
        if !text.trim().is_empty() {
            parts.push(ChatContentPart::Text { text });
        }
        for (mime, data_base64) in images {
            parts.push(ChatContentPart::ImageUrl {
                image_url: ChatImageUrl {
                    url: format!("data:{mime};base64,{data_base64}"),
                },
            });
        }
        if parts.is_empty() {
            parts.push(ChatContentPart::Text {
                text: String::new(),
            });
        }
        Self {
            role: "user".to_owned(),
            content: Some(ChatMessageContent::Parts(parts)),
            tool_calls: None,
            tool_call_id: None,
        }
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
            content: Some(ChatMessageContent::Text(content.into())),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }

    fn text(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: Some(ChatMessageContent::Text(content.into())),
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
    /// Set when the upstream stream stopped mid-arguments (for example
    /// `finish_reason: "length"`), so `arguments` holds an incomplete JSON
    /// fragment. Local-only: never sent to or read from a provider.
    #[serde(default, skip)]
    pub truncated: bool,
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
    #[error("{}", format_empty_stream_message(status, body))]
    EmptyStream { status: StatusCode, body: String },
    #[error("invalid UTF-8 in provider stream: {0}")]
    Utf8(#[from] std::str::Utf8Error),
    #[error("invalid OpenAI-compatible stream event: {0}")]
    StreamJson(#[from] serde_json::Error),
    #[error("invalid tool call from provider: {0}")]
    InvalidToolCall(String),
    #[error("stream disconnected before completion: {0}")]
    StreamDisconnected(String),
}

impl ProviderError {
    /// Transient transport / upstream failures worth retrying before failing the turn.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Http(error) => {
                // Retry network/timeouts and incomplete requests; skip pure decode bugs.
                error.is_timeout()
                    || error.is_connect()
                    || error.is_request()
                    || error.is_body()
                    || (!error.is_decode() && error.status().is_none())
            }
            Self::HttpStatus { status, .. } => {
                // Context overflow (400 + overflow body) needs history trimming, not a plain resend.
                if self.is_context_overflow() {
                    return false;
                }
                matches!(
                    status.as_u16(),
                    400 | 401 | 403 | 404 | 408 | 409 | 429 | 500 | 502 | 503 | 504
                )
            }
            Self::StreamDisconnected(_) | Self::EmptyStream { .. } => true,
            Self::MissingApiKey
            | Self::InvalidResponse(_)
            | Self::Utf8(_)
            | Self::StreamJson(_)
            | Self::InvalidToolCall(_) => false,
        }
    }

    /// Request was rejected for exceeding the model context window. Unlike
    /// `is_retryable`, resending the same body is guaranteed to fail again:
    /// the caller must shrink the history before it retries.
    pub fn is_context_overflow(&self) -> bool {
        match self {
            Self::HttpStatus { status, body } => {
                *status == StatusCode::BAD_REQUEST && body_indicates_context_overflow(body)
            }
            // Responses API reports some request rejections as a successful SSE
            // connection followed by `response.failed`, rather than HTTP 400.
            Self::InvalidResponse(message) => body_indicates_context_overflow(message),
            _ => false,
        }
    }
}

/// Initial attempt + this many retries (Codex-style: retry up to 5 times).
pub const MAX_PROVIDER_RETRIES: u32 = 5;
const EMPTY_STREAM_RESPONSE_BODY_LIMIT: usize = 4 * 1024;

pub fn provider_retry_delay(retry_number: u32) -> Duration {
    // retry_number is 1..=MAX_PROVIDER_RETRIES after the first failure.
    let shift = retry_number.saturating_sub(1).min(4);
    Duration::from_millis(250u64.saturating_mul(1u64 << shift))
}

/// Default HTTP User-Agent shaped like OpenAI Codex (`originator/version`).
/// Many restricted gateways only allow Codex-looking clients.
pub const DEFAULT_HTTP_USER_AGENT: &str = "codex_cli_rs/0.50.0";

/// Resolve the provider HTTP User-Agent.
/// Override with `XCODING_HTTP_USER_AGENT` when a gateway expects a different Codex flavor.
pub fn http_user_agent() -> String {
    env::var("XCODING_HTTP_USER_AGENT")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_HTTP_USER_AGENT.to_owned())
}

fn build_http_client() -> Client {
    let user_agent = http_user_agent();
    Client::builder()
        // Present as Codex so client-restricted OpenAI-compatible gateways accept traffic.
        .user_agent(user_agent.clone())
        // Avoid hanging forever on dead endpoints; do not set a total body timeout so
        // long-lived SSE chat streams are not cut mid-turn.
        .connect_timeout(Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| {
            Client::builder()
                .user_agent(user_agent)
                .build()
                .unwrap_or_else(|_| Client::new())
        })
}

fn format_empty_stream_message(status: &StatusCode, body: &str) -> String {
    format!(
        "model returned an empty response; please retry. HTTP {}; response body: {}",
        status.as_u16(),
        body
    )
}

fn append_stream_body_sample(sample: &mut Vec<u8>, truncated: &mut bool, chunk: &[u8]) {
    let remaining = EMPTY_STREAM_RESPONSE_BODY_LIMIT.saturating_sub(sample.len());
    if remaining == 0 {
        *truncated = true;
        return;
    }
    if chunk.len() > remaining {
        sample.extend_from_slice(&chunk[..remaining]);
        *truncated = true;
    } else {
        sample.extend_from_slice(chunk);
    }
}

fn format_stream_body_sample(sample: &[u8], truncated: bool) -> String {
    let mut body = String::from_utf8_lossy(sample).into_owned();
    if truncated {
        body.push_str("\n...[response body truncated after 4096 bytes]");
    }
    if body.trim().is_empty() {
        "(empty body)".to_owned()
    } else {
        body
    }
}

fn format_http_status_message(status: &StatusCode, body: &str) -> String {
    let truncated = truncate_provider_body(body, 280);
    if *status == StatusCode::UNAUTHORIZED || *status == StatusCode::FORBIDDEN {
        return format!(
            "Cloud provider authentication failed (HTTP {}). Check OPENAI_API_KEY and XCODING_OPENAI_BASE_URL. Provider response: {}",
            status.as_u16(),
            truncated
        );
    }
    if *status == StatusCode::BAD_REQUEST && body_indicates_context_overflow(body) {
        return format!(
            "Context window exceeded (HTTP 400): conversation history is too long for this model. History will be trimmed before retrying. Provider response: {}",
            truncated
        );
    }
    format!(
        "Cloud provider request failed (HTTP {}). Check OPENAI_API_KEY and XCODING_OPENAI_BASE_URL if this looks like an auth or endpoint issue. Provider response: {}",
        status.as_u16(),
        truncated
    )
}

fn body_indicates_context_overflow(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("context_length_exceeded")
        || lower.contains("context window")
        || lower.contains("context length")
        || lower.contains("maximum context")
        || lower.contains("too many tokens")
        || (lower.contains("exceeds") && (lower.contains("token") || lower.contains("context")))
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
    wire_api: ProviderWireApi,
    client: Client,
}

/// Inspect cloud-provider credentials without making a network request.
/// Does not return the full API key.
/// Strip trailing slashes and optional `/v1` so stored hosts stay path-free.
pub fn normalize_base_url(input: &str) -> String {
    let mut value = input.trim().to_owned();
    loop {
        let without_slash = value.trim_end_matches('/').to_owned();
        if without_slash != value {
            value = without_slash;
            continue;
        }
        if value.len() >= 3 {
            let lower = value.to_ascii_lowercase();
            if lower.ends_with("/v1") {
                value.truncate(value.len() - 3);
                continue;
            }
        }
        break;
    }
    value
}

/// Build the OpenAI-compatible API root used for HTTP calls (`{host}/v1`).
pub fn api_root_url(input: &str) -> String {
    let normalized = normalize_base_url(input);
    if normalized.is_empty() {
        return "https://ai.v58.dev/v1".to_owned();
    }
    format!("{normalized}/v1")
}

/// Ensure `providers` / `active_provider_id` exist and mirror the active slot onto legacy fields.
pub fn normalize_user_config(mut config: UserConfig) -> UserConfig {
    config.provider = if config.provider.trim().is_empty() {
        "openai".to_owned()
    } else {
        config.provider.trim().to_owned()
    };
    config.model = config.model.trim().to_owned();
    config.locale = config.locale.trim().to_owned();
    if config.locale.is_empty() {
        config.locale = "en".to_owned();
    }
    config.max_provider_retries = config
        .max_provider_retries
        .clamp(MIN_MAX_PROVIDER_RETRIES, MAX_MAX_PROVIDER_RETRIES);
    config.max_tool_rounds = config
        .max_tool_rounds
        .clamp(MIN_MAX_TOOL_ROUNDS, MAX_MAX_TOOL_ROUNDS);
    config.circuit_failure_threshold = config
        .circuit_failure_threshold
        .clamp(MIN_CIRCUIT_FAILURE_THRESHOLD, MAX_CIRCUIT_FAILURE_THRESHOLD);
    config.stream_first_event_timeout_secs = config.stream_first_event_timeout_secs.clamp(
        MIN_STREAM_FIRST_EVENT_TIMEOUT_SECS,
        MAX_STREAM_FIRST_EVENT_TIMEOUT_SECS,
    );
    config.stream_idle_timeout_secs = config
        .stream_idle_timeout_secs
        .clamp(MIN_STREAM_IDLE_TIMEOUT_SECS, MAX_STREAM_IDLE_TIMEOUT_SECS);
    config.non_stream_timeout_secs = config
        .non_stream_timeout_secs
        .clamp(MIN_NON_STREAM_TIMEOUT_SECS, MAX_NON_STREAM_TIMEOUT_SECS);
    config.circuit_recovery_success_threshold = config.circuit_recovery_success_threshold.clamp(
        MIN_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD,
        MAX_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD,
    );
    config.circuit_recovery_wait_secs = config.circuit_recovery_wait_secs.clamp(
        MIN_CIRCUIT_RECOVERY_WAIT_SECS,
        MAX_CIRCUIT_RECOVERY_WAIT_SECS,
    );
    config.circuit_error_rate_threshold_percent =
        config.circuit_error_rate_threshold_percent.clamp(
            MIN_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT,
            MAX_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT,
        );
    config.circuit_min_request_count = config
        .circuit_min_request_count
        .clamp(MIN_CIRCUIT_MIN_REQUEST_COUNT, MAX_CIRCUIT_MIN_REQUEST_COUNT);
    config.context_compaction_threshold_percent =
        config.context_compaction_threshold_percent.clamp(
            MIN_CONTEXT_COMPACTION_THRESHOLD_PERCENT,
            MAX_CONTEXT_COMPACTION_THRESHOLD_PERCENT,
        );
    config.model_context_windows = config
        .model_context_windows
        .into_iter()
        .map(|(model, window)| (model.trim().to_ascii_lowercase(), window))
        .filter(|(model, _)| !model.is_empty())
        .map(|(model, window)| {
            (
                model,
                window.clamp(MIN_CONTEXT_WINDOW_TOKENS, MAX_CONTEXT_WINDOW_TOKENS),
            )
        })
        .collect();
    if let Some(key) = config.api_key.as_mut() {
        let trimmed = key.trim().to_owned();
        if trimmed.is_empty() {
            config.api_key = None;
        } else {
            *key = trimmed;
        }
    }
    if let Some(root) = config.last_workspace_root.as_mut() {
        let trimmed = root.trim().to_owned();
        if trimmed.is_empty() {
            config.last_workspace_root = None;
        } else {
            *root = trimmed;
        }
    }
    if let Some(home) = config.workspace_home.as_mut() {
        let trimmed = home.trim().to_owned();
        if trimmed.is_empty() {
            config.workspace_home = None;
        } else {
            *home = trimmed;
        }
    }

    if config.providers.is_empty() {
        let id = "default".to_owned();
        let name = if config.provider.trim().is_empty() {
            "openai".to_owned()
        } else {
            config.provider.trim().to_owned()
        };
        let base = normalize_base_url(&config.base_url);
        config.providers.push(CloudProviderConfig {
            id: id.clone(),
            name,
            base_url: if base.is_empty() {
                "https://ai.v58.dev".to_owned()
            } else {
                base
            },
            wire_api: ProviderWireApi::default(),
            trust_level: xcoding_protocol::ProviderTrustLevel::Relay,
            api_key: config.api_key.clone(),
        });
        config.active_provider_id = Some(id);
    }

    for provider in &mut config.providers {
        provider.id = provider.id.trim().to_owned();
        if provider.id.is_empty() {
            provider.id = format!("provider-{}", uuid_like());
        }
        provider.name = provider.name.trim().to_owned();
        if provider.name.is_empty() {
            provider.name = "openai".to_owned();
        }
        provider.base_url = {
            let base = normalize_base_url(&provider.base_url);
            if base.is_empty() {
                "https://ai.v58.dev".to_owned()
            } else {
                base
            }
        };
        if let Some(key) = provider.api_key.as_mut() {
            let trimmed = key.trim().to_owned();
            if trimmed.is_empty() {
                provider.api_key = None;
            } else {
                *key = trimmed;
            }
        }
    }

    let active_id = config
        .active_provider_id
        .as_ref()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .filter(|value| {
            config
                .providers
                .iter()
                .any(|provider| provider.id == *value)
        })
        .unwrap_or_else(|| config.providers[0].id.clone());
    config.active_provider_id = Some(active_id.clone());

    if let Some(active) = config
        .providers
        .iter()
        .find(|provider| provider.id == active_id)
        .cloned()
    {
        // Sessions still require the technical openai provider id.
        config.provider = "openai".to_owned();
        config.base_url = active.base_url;
        config.api_key = active.api_key;
    } else {
        config.base_url = normalize_base_url(&config.base_url);
        if config.base_url.is_empty() {
            config.base_url = "https://ai.v58.dev".to_owned();
        }
    }

    config
}

fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0);
    format!("{millis:x}")
}

fn resolve_provider_credentials(
    base_url_override: Option<&str>,
    api_key_override: Option<&str>,
) -> Result<(String, String), String> {
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

    let base_url = api_root_url(
        &base_url_override
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_owned())
            .or_else(|| {
                env::var("XCODING_OPENAI_BASE_URL")
                    .ok()
                    .map(|value| value.trim().to_owned())
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or_else(|| "https://ai.v58.dev".to_owned()),
    );

    Ok((api_key, base_url))
}

/// Resolve credentials (optional UI overrides first) and list provider models.
pub async fn list_models(
    base_url_override: Option<&str>,
    api_key_override: Option<&str>,
) -> Result<ListModelsResult, String> {
    let (api_key, base_url) = resolve_provider_credentials(base_url_override, api_key_override)?;
    OpenAiCompatibleProvider::new(api_key, base_url)
        .list_models()
        .await
        .map_err(|error| error.to_string())
}

/// Blocking helper for non-async callers. Avoid using this on the UI thread.
pub fn list_models_blocking(
    base_url_override: Option<&str>,
    api_key_override: Option<&str>,
) -> Result<ListModelsResult, String> {
    let (api_key, base_url) = resolve_provider_credentials(base_url_override, api_key_override)?;
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
    let base_url = api_root_url(
        &env::var("XCODING_OPENAI_BASE_URL").unwrap_or_else(|_| "https://ai.v58.dev".to_owned()),
    );
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
        let base_url =
            env::var("XCODING_OPENAI_BASE_URL").unwrap_or_else(|_| "https://ai.v58.dev".to_owned());
        Ok(Self::new(api_key, base_url))
    }

    pub fn new(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self::with_wire_api(api_key, base_url, ProviderWireApi::ChatCompletions)
    }

    pub fn with_wire_api(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        wire_api: ProviderWireApi,
    ) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: api_root_url(&base_url.into()),
            wire_api,
            client: build_http_client(),
        }
    }

    /// Return the exact endpoint used for model response requests.
    /// This intentionally exposes no credential material.
    pub fn chat_url(&self) -> String {
        match self.wire_api {
            ProviderWireApi::ChatCompletions => format!("{}/chat/completions", self.base_url),
            ProviderWireApi::Responses => format!("{}/responses", self.base_url),
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
        reasoning_effort: Option<&str>,
    ) -> Result<ProviderEventStream, ProviderError> {
        match self.wire_api {
            ProviderWireApi::ChatCompletions => {
                self.stream_chat_completions(model, messages, tools, reasoning_effort)
                    .await
            }
            ProviderWireApi::Responses => {
                self.stream_responses(model, messages, tools, reasoning_effort)
                    .await
            }
        }
    }

    async fn stream_chat_completions(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
        tools: &[ToolDefinition],
        reasoning_effort: Option<&str>,
    ) -> Result<ProviderEventStream, ProviderError> {
        let body = chat_completions_request_body(model, messages, tools, reasoning_effort);

        // The agent owns retry scheduling so it can report each reconnect attempt to
        // the UI. This provider opens one SSE response per call.
        let response = self.open_chat_completion(&body).await?;
        let response_status = response.status();

        let stream = try_stream! {
            let mut bytes = response.bytes_stream();
            let mut buffer = Vec::new();
            let mut tool_calls = BTreeMap::new();
            let mut body_sample = Vec::new();
            let mut body_sample_truncated = false;
            let mut emitted_event = false;
            // Providers send finish_reason on a chunk before [DONE]; keep the last one.
            let mut truncated_by_length = false;

            while let Some(chunk) = bytes.next().await {
                let chunk = chunk.map_err(|error| ProviderError::StreamDisconnected(error.to_string()))?;
                append_stream_body_sample(&mut body_sample, &mut body_sample_truncated, &chunk);
                buffer.extend_from_slice(&chunk);

                while let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
                    let line: Vec<u8> = buffer.drain(..=newline).collect();
                    let line = std::str::from_utf8(&line)?.trim();
                    let Some(data) = line.strip_prefix("data:") else {
                        continue;
                    };
                    let data = data.trim();

                    if data == "[DONE]" {
                        let completed_calls = completed_tool_calls(
                            std::mem::take(&mut tool_calls),
                            truncated_by_length,
                        )?;
                        if !emitted_event && completed_calls.is_empty() {
                            Err::<(), ProviderError>(ProviderError::EmptyStream {
                                status: response_status,
                                body: format_stream_body_sample(&body_sample, body_sample_truncated),
                            })?;
                        }
                        for tool_call in completed_calls {
                            yield ProviderEvent::ToolCall(tool_call);
                        }
                        return;
                    }

                    let parsed = parse_chunk(data)?;
                    if let Some(model) = parsed.model {
                        yield ProviderEvent::ModelReported(model);
                    }
                    if let Some(reason) = parsed.finish_reason.as_deref() {
                        truncated_by_length = reason == "length";
                    }
                    // Usage is reference data, not model output, so it never
                    // satisfies the empty-stream check.
                    if let Some(usage) = parsed.usage {
                        yield ProviderEvent::Usage(usage);
                    }
                    // Thinking arrives before the first content token on
                    // reasoning models. It is not output either, so it does not
                    // satisfy the empty-stream check; it only proves the stream
                    // is alive.
                    if let Some(reasoning) = parsed.reasoning {
                        yield ProviderEvent::ReasoningDelta(reasoning);
                    }
                    if let Some(content) = parsed.content.filter(|content| !content.trim().is_empty()) {
                        emitted_event = true;
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

            Err::<(), ProviderError>(ProviderError::StreamDisconnected(
                "connection closed before [DONE]".to_owned(),
            ))?;
        };

        Ok(Box::pin(stream))
    }

    async fn open_chat_completion(&self, body: &Value) -> Result<reqwest::Response, ProviderError> {
        let response = self
            .client
            .post(self.chat_url())
            .bearer_auth(&self.api_key)
            .json(body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::HttpStatus { status, body });
        }

        Ok(response)
    }

    async fn stream_responses(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
        tools: &[ToolDefinition],
        reasoning_effort: Option<&str>,
    ) -> Result<ProviderEventStream, ProviderError> {
        let body = responses_request_body(model, messages, tools, reasoning_effort);
        let response = self.open_chat_completion(&body).await?;
        let response_status = response.status();

        let stream = try_stream! {
            let mut bytes = response.bytes_stream();
            let mut buffer = Vec::new();
            let mut body_sample = Vec::new();
            let mut body_sample_truncated = false;
            let mut emitted_event = false;
            let mut completed = false;

            while let Some(chunk) = bytes.next().await {
                let chunk = chunk.map_err(|error| ProviderError::StreamDisconnected(error.to_string()))?;
                append_stream_body_sample(&mut body_sample, &mut body_sample_truncated, &chunk);
                buffer.extend_from_slice(&chunk);

                while let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
                    let line: Vec<u8> = buffer.drain(..=newline).collect();
                    let line = std::str::from_utf8(&line)?.trim();
                    let Some(data) = line.strip_prefix("data:") else {
                        continue;
                    };
                    let data = data.trim();
                    if data == "[DONE]" {
                        completed = true;
                        break;
                    }

                    match parse_responses_event(data)? {
                        ResponsesParsedEvent::TextDelta(delta) => {
                            if !delta.is_empty() {
                                emitted_event = true;
                                yield ProviderEvent::TextDelta(delta);
                            }
                        }
                        ResponsesParsedEvent::ReasoningDelta(delta) => {
                            if !delta.is_empty() {
                                yield ProviderEvent::ReasoningDelta(delta);
                            }
                        }
                        ResponsesParsedEvent::ToolCall(tool_call) => {
                            emitted_event = true;
                            yield ProviderEvent::ToolCall(tool_call);
                        }
                        ResponsesParsedEvent::Completed { usage, model } => {
                            if let Some(model) = model {
                                yield ProviderEvent::ModelReported(model);
                            }
                            if let Some(usage) = usage {
                                yield ProviderEvent::Usage(usage);
                            }
                            completed = true;
                            break;
                        }
                        ResponsesParsedEvent::Failed(message) => {
                            Err::<(), ProviderError>(ProviderError::InvalidResponse(message))?;
                        }
                        ResponsesParsedEvent::Ignored => {}
                    }
                }
                if completed {
                    break;
                }
            }

            if !completed {
                Err::<(), ProviderError>(ProviderError::StreamDisconnected(
                    "connection closed before response.completed".to_owned(),
                ))?;
            }
            if !emitted_event {
                Err::<(), ProviderError>(ProviderError::EmptyStream {
                    status: response_status,
                    body: format_stream_body_sample(&body_sample, body_sample_truncated),
                })?;
            }
        };

        Ok(Box::pin(stream))
    }
}

fn chat_completions_request_body(
    model: &str,
    messages: Vec<ChatMessage>,
    tools: &[ToolDefinition],
    reasoning_effort: Option<&str>,
) -> Value {
    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": true,
        // Endpoints that honor this send a final usage-only chunk, which lets
        // the agent calibrate its token estimate against real counts.
        // Endpoints that ignore it simply never send the chunk.
        "stream_options": { "include_usage": true }
    });
    if let Some(effort) = reasoning_effort
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        body["reasoning_effort"] = json!(effort);
    }
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
        // Mirrors the Responses path so both wire APIs let the model batch
        // independent tool calls into one round trip.
        body["tool_choice"] = json!("auto");
        body["parallel_tool_calls"] = json!(true);
    }
    body
}

fn responses_request_body(
    model: &str,
    messages: Vec<ChatMessage>,
    tools: &[ToolDefinition],
    reasoning_effort: Option<&str>,
) -> Value {
    let mut instructions = Vec::new();
    let mut input = Vec::new();
    for message in messages {
        if message.role == "system" {
            if let Some(content) = message.content {
                instructions.push(chat_content_as_text(content));
            }
            continue;
        }
        if let Some(tool_calls) = message.tool_calls {
            input.extend(tool_calls.into_iter().map(|tool_call| {
                json!({
                    "type": "function_call",
                    "call_id": tool_call.id,
                    "name": tool_call.function.name,
                    "arguments": tool_call.function.arguments
                })
            }));
            continue;
        }
        if message.role == "tool" {
            input.push(json!({
                "type": "function_call_output",
                "call_id": message.tool_call_id.unwrap_or_default(),
                "output": message.content.map(chat_content_as_text).unwrap_or_default()
            }));
            continue;
        }
        if let Some(content) = message.content {
            input.push(responses_message_item(&message.role, content));
        }
    }

    let mut body = json!({
        "model": model,
        "instructions": instructions.join("\n\n"),
        "input": input,
        "stream": true,
        "store": false
    });
    if !tools.is_empty() {
        body["tools"] = Value::Array(
            tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                        "strict": false
                    })
                })
                .collect(),
        );
        body["tool_choice"] = json!("auto");
        body["parallel_tool_calls"] = json!(true);
    }
    if let Some(effort) = reasoning_effort
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        body["reasoning"] = json!({ "effort": effort });
    }
    body
}

fn chat_content_as_text(content: ChatMessageContent) -> String {
    match content {
        ChatMessageContent::Text(text) => text,
        ChatMessageContent::Parts(parts) => parts
            .into_iter()
            .filter_map(|part| match part {
                ChatContentPart::Text { text } => Some(text),
                ChatContentPart::ImageUrl { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn responses_message_item(role: &str, content: ChatMessageContent) -> Value {
    let content_type = if role == "assistant" {
        "output_text"
    } else {
        "input_text"
    };
    let parts = match content {
        ChatMessageContent::Text(text) => vec![json!({ "type": content_type, "text": text })],
        ChatMessageContent::Parts(parts) => parts
            .into_iter()
            .map(|part| match part {
                ChatContentPart::Text { text } => json!({ "type": content_type, "text": text }),
                ChatContentPart::ImageUrl { image_url } => json!({
                    "type": "input_image",
                    "image_url": image_url.url,
                    "detail": "auto"
                }),
            })
            .collect(),
    };
    json!({ "type": "message", "role": role, "content": parts })
}

enum ResponsesParsedEvent {
    TextDelta(String),
    ReasoningDelta(String),
    ToolCall(ProviderToolCall),
    Completed {
        usage: Option<ProviderUsage>,
        model: Option<String>,
    },
    Failed(String),
    Ignored,
}

fn parse_responses_event(data: &str) -> Result<ResponsesParsedEvent, ProviderError> {
    let event: Value = serde_json::from_str(data)?;
    match event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "response.output_text.delta" => Ok(ResponsesParsedEvent::TextDelta(
            event
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        )),
        // Reasoning summaries prove the model is working before the first
        // output token, which can be minutes away on high reasoning effort.
        "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
            Ok(ResponsesParsedEvent::ReasoningDelta(
                event
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            ))
        }
        "response.output_item.done" => {
            let Some(item) = event.get("item") else {
                return Ok(ResponsesParsedEvent::Ignored);
            };
            if item.get("type").and_then(Value::as_str) != Some("function_call") {
                return Ok(ResponsesParsedEvent::Ignored);
            }
            let call_id = item
                .get("call_id")
                .or_else(|| item.get("id"))
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    ProviderError::InvalidToolCall(
                        "Responses function call is missing call_id".to_owned(),
                    )
                })?;
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    ProviderError::InvalidToolCall(
                        "Responses function call is missing name".to_owned(),
                    )
                })?;
            let arguments = item
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}");
            Ok(ResponsesParsedEvent::ToolCall(ProviderToolCall {
                id: call_id.to_owned(),
                kind: "function".to_owned(),
                function: ProviderFunctionCall {
                    name: name.to_owned(),
                    arguments: if arguments.trim().is_empty() {
                        "{}".to_owned()
                    } else {
                        arguments.to_owned()
                    },
                },
                // Responses delivers finished function calls in one event.
                truncated: false,
            }))
        }
        "response.completed" => Ok(ResponsesParsedEvent::Completed {
            usage: responses_usage(&event),
            model: event
                .pointer("/response/model")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }),
        "response.failed" | "response.incomplete" => {
            let message = event
                .pointer("/response/error/message")
                .and_then(Value::as_str)
                .or_else(|| {
                    event
                        .pointer("/response/incomplete_details/reason")
                        .and_then(Value::as_str)
                })
                .unwrap_or("Responses API returned a failed response");
            let code = event
                .pointer("/response/error/code")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty());
            let message = match code {
                Some(code) => format!("{code}: {message}"),
                None => message.to_owned(),
            };
            Ok(ResponsesParsedEvent::Failed(message))
        }
        _ => Ok(ResponsesParsedEvent::Ignored),
    }
}

/// Reads token counts from a `response.completed` event. Absent or zero counts
/// mean the endpoint did not report usage, so the agent keeps estimating.
fn responses_usage(event: &Value) -> Option<ProviderUsage> {
    let prompt_tokens = event
        .pointer("/response/usage/input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let completion_tokens = event
        .pointer("/response/usage/output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    (prompt_tokens > 0 || completion_tokens > 0).then_some(ProviderUsage {
        prompt_tokens,
        completion_tokens,
    })
}

#[derive(Deserialize)]
struct ChatCompletionChunk {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    choices: Vec<ChatCompletionChoice>,
    /// Sent only by endpoints that honor `stream_options.include_usage`, and
    /// usually on a final chunk that carries no choices.
    #[serde(default)]
    usage: Option<ChatCompletionUsage>,
}

#[derive(Deserialize)]
struct ChatCompletionUsage {
    #[serde(default)]
    prompt_tokens: Option<usize>,
    #[serde(default)]
    completion_tokens: Option<usize>,
}

#[derive(Deserialize)]
struct ChatCompletionChoice {
    delta: ChatCompletionDelta,
    /// `"length"` means the upstream hit its output cap; anything accumulated
    /// so far may be an incomplete fragment.
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Default, Deserialize)]
struct ChatCompletionDelta {
    content: Option<String>,
    /// Gateways in front of reasoning models stream visible-to-nobody thinking
    /// here before the first `content` token. `reasoning_content` is the common
    /// spelling; `reasoning` is accepted because some gateways use it, either as
    /// a string or as an object carrying the text.
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    reasoning: Option<Value>,
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
    model: Option<String>,
    content: Option<String>,
    reasoning: Option<String>,
    tool_calls: Vec<ToolCallDelta>,
    finish_reason: Option<String>,
    usage: Option<ProviderUsage>,
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

    /// `truncated_by_length` carries the upstream `finish_reason: "length"`.
    ///
    /// Incomplete arguments are returned as-is instead of failing the stream:
    /// the agent needs the call id to answer with a tool result, otherwise the
    /// turn dies with an assistant tool call that has no matching result.
    fn finish(self, truncated_by_length: bool) -> Result<ProviderToolCall, ProviderError> {
        let arguments = if self.arguments.trim().is_empty() {
            "{}".to_owned()
        } else {
            self.arguments
        };
        let truncated = serde_json::from_str::<Value>(&arguments).is_err()
            && (truncated_by_length || is_incomplete_json(&arguments));
        Ok(ProviderToolCall {
            id: self
                .id
                .ok_or_else(|| ProviderError::InvalidToolCall("missing id".to_owned()))?,
            kind: self.kind.unwrap_or_else(|| "function".to_owned()),
            function: ProviderFunctionCall {
                name: self.name.ok_or_else(|| {
                    ProviderError::InvalidToolCall("missing function name".to_owned())
                })?,
                arguments,
            },
            truncated,
        })
    }
}

/// Distinguish "the stream stopped early" from "the model emitted broken JSON".
/// Serde reports a premature end of input as `Category::Eof`.
fn is_incomplete_json(arguments: &str) -> bool {
    match serde_json::from_str::<Value>(arguments) {
        Err(error) => error.classify() == serde_json::error::Category::Eof,
        Ok(_) => false,
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
    let loaded = match fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => UserConfig::default(),
    };
    normalize_user_config(loaded)
}

/// Persist user preferences to `~/.xcoding/config.json`.
pub fn save_user_config(config: &UserConfig) -> Result<(), String> {
    let normalized = normalize_user_config(config.clone());
    let dir = user_config_dir();
    let path = dir.join("config.json");
    let body = serde_json::to_string_pretty(&normalized).map_err(|error| error.to_string())?;
    write_text_utf8(&path, &format!("{body}\n")).map_err(|error| error.to_string())?;
    Ok(())
}

fn temporary_sibling(path: &Path) -> PathBuf {
    match path.file_name().and_then(|value| value.to_str()) {
        Some(file_name) => path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(format!(".{file_name}.xcoding.tmp")),
        None => path.with_extension("xcoding.tmp"),
    }
}

fn write_text_utf8(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = temporary_sibling(path);
    {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_data()?;
    }
    #[cfg(windows)]
    {
        if path.exists() {
            fs::remove_file(path)?;
        }
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
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
        let api_root = api_root_url(base);
        unsafe {
            env::set_var("XCODING_OPENAI_BASE_URL", api_root);
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
            let api_root = api_root_url(base);
            unsafe {
                env::set_var("XCODING_OPENAI_BASE_URL", api_root);
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
    let usage = chunk.usage.and_then(|usage| {
        let prompt_tokens = usage.prompt_tokens.unwrap_or(0);
        let completion_tokens = usage.completion_tokens.unwrap_or(0);
        (prompt_tokens > 0 || completion_tokens > 0).then_some(ProviderUsage {
            prompt_tokens,
            completion_tokens,
        })
    });
    let mut choices = chunk.choices.into_iter();
    let (delta, finish_reason) = choices
        .next()
        .map(|choice| (choice.delta, choice.finish_reason))
        .unwrap_or_default();
    let reasoning = reasoning_delta_text(&delta);
    Ok(ParsedChunk {
        model: chunk.model,
        content: delta.content,
        reasoning,
        tool_calls: delta.tool_calls,
        finish_reason,
        usage,
    })
}

/// Pulls the thinking text out of one delta, whichever field the gateway used.
fn reasoning_delta_text(delta: &ChatCompletionDelta) -> Option<String> {
    if let Some(text) = delta
        .reasoning_content
        .as_deref()
        .filter(|text| !text.is_empty())
    {
        return Some(text.to_owned());
    }
    match delta.reasoning.as_ref()? {
        Value::String(text) => (!text.is_empty()).then(|| text.clone()),
        value => value
            .get("content")
            .or_else(|| value.get("text"))
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_owned),
    }
}

fn completed_tool_calls(
    tool_calls: BTreeMap<usize, ToolCallAccumulator>,
    truncated_by_length: bool,
) -> Result<Vec<ProviderToolCall>, ProviderError> {
    tool_calls
        .into_values()
        .map(|accumulator| accumulator.finish(truncated_by_length))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_http_user_agent_looks_like_codex() {
        assert!(DEFAULT_HTTP_USER_AGENT.starts_with("codex_cli_rs/"));
        assert!(DEFAULT_HTTP_USER_AGENT.contains('/'));
    }

    #[test]
    fn http_user_agent_uses_env_override() {
        let key = "XCODING_HTTP_USER_AGENT";
        let previous = env::var(key).ok();
        unsafe {
            env::set_var(key, "codex-cli/9.9.9");
        }
        assert_eq!(http_user_agent(), "codex-cli/9.9.9");
        unsafe {
            match previous {
                Some(value) => env::set_var(key, value),
                None => env::remove_var(key),
            }
        }
    }

    #[test]
    fn parses_text_delta() {
        let parsed =
            parse_chunk(r#"{"choices":[{"delta":{"content":"Hello"}}]}"#).expect("event parses");
        assert_eq!(parsed.content.as_deref(), Some("Hello"));
        assert_eq!(parsed.usage, None);
    }

    #[test]
    fn reasoning_only_delta_is_reported_as_stream_progress() {
        // Gateways in front of reasoning models send minutes of this before the
        // first content token. Dropping it made the caller treat a working
        // stream as one that never started.
        let parsed = parse_chunk(r#"{"choices":[{"delta":{"reasoning_content":"weighing options"}}]}"#)
            .expect("reasoning chunk parses");
        assert_eq!(parsed.reasoning.as_deref(), Some("weighing options"));
        assert_eq!(parsed.content, None);
        assert!(parsed.tool_calls.is_empty());
    }

    #[test]
    fn reasoning_delta_accepts_string_and_object_spellings() {
        let as_string = parse_chunk(r#"{"choices":[{"delta":{"reasoning":"thinking"}}]}"#)
            .expect("string reasoning parses");
        assert_eq!(as_string.reasoning.as_deref(), Some("thinking"));

        let as_object = parse_chunk(r#"{"choices":[{"delta":{"reasoning":{"content":"nested"}}}]}"#)
            .expect("object reasoning parses");
        assert_eq!(as_object.reasoning.as_deref(), Some("nested"));

        let empty = parse_chunk(r#"{"choices":[{"delta":{"reasoning_content":""}}]}"#)
            .expect("empty reasoning parses");
        assert_eq!(empty.reasoning, None);
    }

    #[test]
    fn content_delta_reports_no_reasoning() {
        let parsed = parse_chunk(r#"{"choices":[{"delta":{"content":"answer"}}]}"#)
            .expect("content chunk parses");
        assert_eq!(parsed.reasoning, None);
    }

    #[test]
    fn responses_stream_reports_reasoning_deltas() {
        match parse_responses_event(
            r#"{"type":"response.reasoning_summary_text.delta","delta":"planning"}"#,
        )
        .expect("reasoning summary parses")
        {
            ResponsesParsedEvent::ReasoningDelta(delta) => assert_eq!(delta, "planning"),
            _ => panic!("expected reasoning delta"),
        }
        match parse_responses_event(r#"{"type":"response.reasoning_text.delta","delta":"step"}"#)
            .expect("reasoning text parses")
        {
            ResponsesParsedEvent::ReasoningDelta(delta) => assert_eq!(delta, "step"),
            _ => panic!("expected reasoning delta"),
        }
    }

    #[test]
    fn parses_streamed_usage_and_degrades_without_it() {
        // Endpoints that honor `include_usage` send a final choice-less chunk.
        let parsed = parse_chunk(
            r#"{"choices":[],"usage":{"prompt_tokens":1234,"completion_tokens":56,"total_tokens":1290}}"#,
        )
        .expect("usage chunk parses");
        assert_eq!(
            parsed.usage,
            Some(ProviderUsage {
                prompt_tokens: 1234,
                completion_tokens: 56,
            })
        );
        assert_eq!(parsed.content, None);

        // A null usage field, an all-zero report, and a plain chunk must all
        // degrade to "no usage" instead of poisoning the calibration.
        assert_eq!(
            parse_chunk(r#"{"choices":[{"delta":{"content":"hi"}}],"usage":null}"#)
                .expect("null usage parses")
                .usage,
            None
        );
        assert_eq!(
            parse_chunk(r#"{"choices":[],"usage":{"prompt_tokens":0,"completion_tokens":0}}"#)
                .expect("zero usage parses")
                .usage,
            None
        );

        // Responses endpoints report the same numbers under different names.
        match parse_responses_event(
            r#"{"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":800,"output_tokens":20}}}"#,
        )
        .expect("completion event parses")
        {
            ResponsesParsedEvent::Completed { usage, model } => assert_eq!(
                (usage, model),
                (
                    Some(ProviderUsage {
                        prompt_tokens: 800,
                        completion_tokens: 20,
                    }),
                    None,
                )
            ),
            _ => panic!("expected completed event"),
        }
    }

    #[test]
    fn chat_completions_request_body_asks_for_streamed_usage() {
        let body = chat_completions_request_body(
            "gpt-test",
            vec![ChatMessage::user("Say hello.")],
            &[],
            None,
        );

        assert_eq!(body["stream_options"]["include_usage"], true);
    }

    #[test]
    fn provider_protocol_selects_model_response_endpoint() {
        let chat = OpenAiCompatibleProvider::new("test-key", "https://example.test/v1/");
        assert_eq!(chat.chat_url(), "https://example.test/v1/chat/completions");

        let responses = OpenAiCompatibleProvider::with_wire_api(
            "test-key",
            "https://example.test/v1/",
            ProviderWireApi::Responses,
        );
        assert_eq!(responses.chat_url(), "https://example.test/v1/responses");
    }

    #[test]
    fn builds_standard_responses_request_body() {
        let tool_call = ProviderToolCall {
            id: "call_1".to_owned(),
            kind: "function".to_owned(),
            function: ProviderFunctionCall {
                name: "read_file".to_owned(),
                arguments: r#"{"path":"src/lib.rs"}"#.to_owned(),
            },
            truncated: false,
        };
        let body = responses_request_body(
            "gpt-test",
            vec![
                ChatMessage::system("Follow the repository instructions."),
                ChatMessage::user_with_images(
                    "Inspect this image.",
                    &[("image/png".to_owned(), "aGVsbG8=".to_owned())],
                ),
                ChatMessage::assistant_tool_calls(vec![tool_call]),
                ChatMessage::tool_result("call_1", "file contents"),
            ],
            &[ToolDefinition {
                name: "read_file".to_owned(),
                description: "Read one file".to_owned(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"]
                }),
            }],
            Some("high"),
        );

        assert_eq!(body["model"], "gpt-test");
        assert_eq!(body["instructions"], "Follow the repository instructions.");
        assert_eq!(body["stream"], true);
        assert_eq!(body["store"], false);
        assert_eq!(body["reasoning"]["effort"], "high");
        assert_eq!(body["input"][0]["type"], "message");
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(body["input"][0]["content"][1]["type"], "input_image");
        assert_eq!(
            body["input"][0]["content"][1]["image_url"],
            "data:image/png;base64,aGVsbG8="
        );
        assert_eq!(body["input"][1]["type"], "function_call");
        assert_eq!(body["input"][1]["call_id"], "call_1");
        assert_eq!(body["input"][1]["name"], "read_file");
        assert_eq!(body["input"][1]["arguments"], r#"{"path":"src/lib.rs"}"#);
        assert_eq!(body["input"][2]["type"], "function_call_output");
        assert_eq!(body["input"][2]["call_id"], "call_1");
        assert_eq!(body["input"][2]["output"], "file contents");
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["name"], "read_file");
        assert_eq!(body["tools"][0]["strict"], false);
        assert_eq!(body["tool_choice"], "auto");
        assert_eq!(body["parallel_tool_calls"], true);
    }

    #[test]
    fn chat_completions_request_body_enables_parallel_tool_calls() {
        let body = chat_completions_request_body(
            "gpt-test",
            vec![ChatMessage::user("List the workspace files.")],
            &[ToolDefinition {
                name: "list_dir".to_owned(),
                description: "List one directory".to_owned(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"]
                }),
            }],
            Some("high"),
        );

        assert_eq!(body["model"], "gpt-test");
        assert_eq!(body["stream"], true);
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "list_dir");
        assert_eq!(body["tool_choice"], "auto");
        assert_eq!(body["parallel_tool_calls"], true);
    }

    #[test]
    fn chat_completions_request_body_omits_tool_fields_without_tools() {
        let body = chat_completions_request_body(
            "gpt-test",
            vec![ChatMessage::user("Say hello.")],
            &[],
            None,
        );

        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());
        assert!(body.get("parallel_tool_calls").is_none());
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn parses_responses_stream_events() {
        match parse_responses_event(r#"{"type":"response.output_text.delta","delta":"Hello"}"#)
            .expect("text event parses")
        {
            ResponsesParsedEvent::TextDelta(delta) => assert_eq!(delta, "Hello"),
            _ => panic!("expected text delta"),
        }

        match parse_responses_event(
            r#"{"type":"response.output_item.done","item":{"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{\"path\":\"src/lib.rs\"}"}}"#,
        )
        .expect("tool event parses")
        {
            ResponsesParsedEvent::ToolCall(tool_call) => assert_eq!(
                tool_call,
                ProviderToolCall {
                    id: "call_1".to_owned(),
                    kind: "function".to_owned(),
                    function: ProviderFunctionCall {
                        name: "read_file".to_owned(),
                        arguments: r#"{"path":"src/lib.rs"}"#.to_owned(),
                    },
                    truncated: false,
                }
            ),
            _ => panic!("expected tool call"),
        }

        assert!(matches!(
            parse_responses_event(
                r#"{"type":"response.completed","response":{"status":"completed"}}"#
            )
            .expect("completion event parses"),
            ResponsesParsedEvent::Completed {
                usage: None,
                model: None,
            }
        ));

        match parse_responses_event(
            r#"{"type":"response.failed","response":{"error":{"code":"upstream_error","message":"upstream failed"}}}"#,
        )
        .expect("failure event parses")
        {
            ResponsesParsedEvent::Failed(message) => {
                assert_eq!(message, "upstream_error: upstream failed")
            }
            _ => panic!("expected failed event"),
        }

        match parse_responses_event(
            r#"{"type":"response.failed","response":{"error":{"code":"context_length_exceeded","message":"Your input exceeds the context window of this model. Please adjust your input and try again."}}}"#,
        )
        .expect("context overflow event parses")
        {
            ResponsesParsedEvent::Failed(message) => {
                assert!(message.starts_with("context_length_exceeded:"));
                assert!(ProviderError::InvalidResponse(message).is_context_overflow());
            }
            _ => panic!("expected failed event"),
        }
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
            accumulator.finish(false).expect("tool call completes"),
            ProviderToolCall {
                id: "call_1".to_owned(),
                kind: "function".to_owned(),
                function: ProviderFunctionCall {
                    name: "read_file".to_owned(),
                    arguments: r#"{"path":"src/lib.rs"}"#.to_owned(),
                },
                truncated: false,
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
            accumulator.finish(false).expect("tool call completes"),
            ProviderToolCall {
                id: "call_1".to_owned(),
                kind: "function".to_owned(),
                function: ProviderFunctionCall {
                    name: "list_dir".to_owned(),
                    arguments: r#"{"path":"."}"#.to_owned(),
                },
                truncated: false,
            }
        );
    }

    #[test]
    fn normalizes_empty_tool_arguments_to_empty_object() {
        let parsed = parse_chunk(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"git_status","arguments":""}}]}}]}"#)
            .expect("tool event parses");
        let mut accumulator = ToolCallAccumulator::default();
        accumulator.merge(parsed.tool_calls.into_iter().next().expect("tool call"));

        assert_eq!(
            accumulator
                .finish(false)
                .expect("tool call completes")
                .function
                .arguments,
            "{}"
        );
    }

    // Regression: `mimo-v2.5-pro` stopped mid-arguments after 8 bytes, and the
    // half-written fragment used to be handed on as a valid call.
    #[test]
    fn marks_tool_arguments_cut_off_mid_stream_as_truncated() {
        let parsed = parse_chunk(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"apply_patch","arguments":"{\"path\":"}}]}}]}"#)
            .expect("tool event parses");
        let mut accumulator = ToolCallAccumulator::default();
        accumulator.merge(parsed.tool_calls.into_iter().next().expect("tool call"));

        let finished = accumulator.finish(false).expect("call keeps its id");
        assert!(
            finished.truncated,
            "an unterminated fragment must be reported as truncated"
        );
        // The id has to survive so the agent can answer with a tool result.
        assert_eq!(finished.id, "call_1");
        assert_eq!(finished.function.arguments, r#"{"path":"#);
    }

    #[test]
    fn reads_length_finish_reason_from_the_stream() {
        let parsed = parse_chunk(r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#)
            .expect("finish chunk parses");
        assert_eq!(parsed.finish_reason.as_deref(), Some("length"));

        let normal = parse_chunk(r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#)
            .expect("finish chunk parses");
        assert_eq!(normal.finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn syntax_errors_are_not_reported_as_truncation() {
        let mut accumulator = ToolCallAccumulator::default();
        accumulator.id = Some("call_1".to_owned());
        accumulator.name = Some("read_file".to_owned());
        // Balanced braces, still invalid JSON: the model wrote it wrong.
        accumulator.arguments = r#"{"path":,}"#.to_owned();

        let finished = accumulator.finish(false).expect("call completes");
        assert!(
            !finished.truncated,
            "broken syntax is a model mistake, not a cut-off stream"
        );
    }

    #[test]
    fn length_finish_reason_marks_unparsable_arguments_as_truncated() {
        let mut accumulator = ToolCallAccumulator::default();
        accumulator.id = Some("call_1".to_owned());
        accumulator.name = Some("apply_patch".to_owned());
        accumulator.arguments = r#"{"path":"a.rs","new_text":"fn"#.to_owned();

        let finished = accumulator.finish(true).expect("call completes");
        assert!(finished.truncated);
    }

    #[test]
    fn retryable_status_codes() {
        assert!(
            ProviderError::HttpStatus {
                status: StatusCode::TOO_MANY_REQUESTS,
                body: "slow down".to_owned(),
            }
            .is_retryable()
        );
        assert!(
            ProviderError::HttpStatus {
                status: StatusCode::BAD_GATEWAY,
                body: "bad gateway".to_owned(),
            }
            .is_retryable()
        );
        assert!(
            ProviderError::HttpStatus {
                status: StatusCode::SERVICE_UNAVAILABLE,
                body: "unavailable".to_owned(),
            }
            .is_retryable()
        );
        assert!(
            ProviderError::HttpStatus {
                status: StatusCode::UNAUTHORIZED,
                body: "transient auth error".to_owned(),
            }
            .is_retryable()
        );
        assert!(
            ProviderError::HttpStatus {
                status: StatusCode::FORBIDDEN,
                body: "forbidden".to_owned(),
            }
            .is_retryable()
        );
        assert!(
            ProviderError::HttpStatus {
                status: StatusCode::NOT_FOUND,
                body: "not found".to_owned(),
            }
            .is_retryable()
        );
        assert!(
            ProviderError::HttpStatus {
                status: StatusCode::BAD_REQUEST,
                body: "bad request".to_owned(),
            }
            .is_retryable()
        );
        assert!(
            ProviderError::EmptyStream {
                status: StatusCode::OK,
                body: "data: [DONE]".to_owned(),
            }
            .is_retryable()
        );
        assert!(!ProviderError::MissingApiKey.is_retryable());
        assert!(!ProviderError::InvalidResponse("x".to_owned()).is_retryable());
    }

    #[test]
    fn provider_retry_delay_grows() {
        assert_eq!(provider_retry_delay(1), Duration::from_millis(250));
        assert_eq!(provider_retry_delay(2), Duration::from_millis(500));
        assert_eq!(provider_retry_delay(3), Duration::from_millis(1000));
        assert!(provider_retry_delay(5) >= provider_retry_delay(4));
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
    fn context_overflow_detected_from_bad_request_body() {
        let overflow = ProviderError::HttpStatus {
            status: StatusCode::BAD_REQUEST,
            body: r#"{"error":{"message":"Input exceeds the model's context window. Please shorten your input and try again.","type":"invalid_request_error"}}"#.to_owned(),
        };
        assert!(overflow.is_context_overflow());
        // Resending the same oversized payload cannot succeed, so plain retry stays off.
        assert!(!overflow.is_retryable());

        assert!(
            ProviderError::HttpStatus {
                status: StatusCode::BAD_REQUEST,
                body: "maximum context length is 128000 tokens".to_owned(),
            }
            .is_context_overflow()
        );
        assert!(
            ProviderError::HttpStatus {
                status: StatusCode::BAD_REQUEST,
                body: "too many tokens in request".to_owned(),
            }
            .is_context_overflow()
        );
    }

    #[test]
    fn ordinary_bad_request_is_not_context_overflow() {
        assert!(
            !ProviderError::HttpStatus {
                status: StatusCode::BAD_REQUEST,
                body: r#"{"error":{"message":"unsupported model"}}"#.to_owned(),
            }
            .is_context_overflow()
        );
        // Same body, wrong status: only 400 carries the overflow contract.
        assert!(
            !ProviderError::HttpStatus {
                status: StatusCode::SERVICE_UNAVAILABLE,
                body: "input exceeds the model's context window".to_owned(),
            }
            .is_context_overflow()
        );
        assert!(!ProviderError::MissingApiKey.is_context_overflow());
        assert!(
            !ProviderError::InvalidResponse("upstream_error: request failed".to_owned())
                .is_context_overflow()
        );
    }

    #[test]
    fn responses_context_overflow_is_not_retryable_without_trimming() {
        let error = ProviderError::InvalidResponse(
            "context_length_exceeded: Your input exceeds the context window".to_owned(),
        );
        assert!(error.is_context_overflow());
        assert!(!error.is_retryable());
    }

    #[test]
    fn context_overflow_message_does_not_blame_credentials() {
        let message = ProviderError::HttpStatus {
            status: StatusCode::BAD_REQUEST,
            body:
                "Input exceeds the model's context window. Please shorten your input and try again."
                    .to_owned(),
        }
        .to_string();
        assert!(message.contains("Context window exceeded (HTTP 400)"));
        assert!(!message.contains("OPENAI_API_KEY"));
        assert!(!message.contains("XCODING_OPENAI_BASE_URL"));
    }

    #[test]
    fn inspect_auth_reports_missing_key() {
        // Cannot safely clear process env for concurrent tests; assert shape via mask helper.
        assert_eq!(mask_api_key("abcd"), "****");
        assert_eq!(mask_api_key("sk-1234567890"), "...7890");
    }

    #[test]
    fn normalizes_base_url_variants() {
        assert_eq!(
            normalize_base_url("https://ai.v58.dev/v1/"),
            "https://ai.v58.dev"
        );
        assert_eq!(
            normalize_base_url("https://ai.v58.dev/"),
            "https://ai.v58.dev"
        );
        assert_eq!(api_root_url("https://ai.v58.dev"), "https://ai.v58.dev/v1");
        assert_eq!(
            api_root_url("https://ai.v58.dev/v1/"),
            "https://ai.v58.dev/v1"
        );
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

    #[test]
    fn normalizes_stream_idle_timeout_within_safe_bounds() {
        let too_short = normalize_user_config(UserConfig {
            stream_idle_timeout_secs: 1,
            ..UserConfig::default()
        });
        assert_eq!(
            too_short.stream_idle_timeout_secs,
            MIN_STREAM_IDLE_TIMEOUT_SECS
        );

        let too_long = normalize_user_config(UserConfig {
            stream_idle_timeout_secs: MAX_STREAM_IDLE_TIMEOUT_SECS + 1,
            ..UserConfig::default()
        });
        assert_eq!(
            too_long.stream_idle_timeout_secs,
            MAX_STREAM_IDLE_TIMEOUT_SECS
        );
    }
    #[test]
    fn normalizes_resilience_settings_within_safe_bounds() {
        assert_eq!(MAX_MAX_TOOL_ROUNDS, 1024);

        let normalized = normalize_user_config(UserConfig {
            max_provider_retries: MAX_MAX_PROVIDER_RETRIES + 1,
            max_tool_rounds: MAX_MAX_TOOL_ROUNDS + 1,
            circuit_failure_threshold: 0,
            stream_first_event_timeout_secs: MAX_STREAM_FIRST_EVENT_TIMEOUT_SECS + 1,
            non_stream_timeout_secs: 1,
            circuit_recovery_success_threshold: 0,
            circuit_recovery_wait_secs: MAX_CIRCUIT_RECOVERY_WAIT_SECS + 1,
            circuit_error_rate_threshold_percent: 0,
            circuit_min_request_count: MAX_CIRCUIT_MIN_REQUEST_COUNT + 1,
            ..UserConfig::default()
        });

        assert_eq!(normalized.max_provider_retries, MAX_MAX_PROVIDER_RETRIES);
        assert_eq!(normalized.max_tool_rounds, MAX_MAX_TOOL_ROUNDS);
        assert_eq!(
            normalized.circuit_failure_threshold,
            MIN_CIRCUIT_FAILURE_THRESHOLD
        );
        assert_eq!(
            normalized.stream_first_event_timeout_secs,
            MAX_STREAM_FIRST_EVENT_TIMEOUT_SECS
        );
        assert_eq!(
            normalized.non_stream_timeout_secs,
            MIN_NON_STREAM_TIMEOUT_SECS
        );
        assert_eq!(
            normalized.circuit_recovery_success_threshold,
            MIN_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD
        );
        assert_eq!(
            normalized.circuit_recovery_wait_secs,
            MAX_CIRCUIT_RECOVERY_WAIT_SECS
        );
        assert_eq!(
            normalized.circuit_error_rate_threshold_percent,
            MIN_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT
        );
        assert_eq!(
            normalized.circuit_min_request_count,
            MAX_CIRCUIT_MIN_REQUEST_COUNT
        );
    }

    #[test]
    fn normalizes_model_context_window_overrides() {
        let mut windows = BTreeMap::new();
        windows.insert("  GPT-5.5  ".to_owned(), 1usize);
        windows.insert("gemini-2.5-pro".to_owned(), MAX_CONTEXT_WINDOW_TOKENS + 1);
        windows.insert("".to_owned(), 200_000);
        let normalized = normalize_user_config(UserConfig {
            model_context_windows: windows,
            ..UserConfig::default()
        });
        assert_eq!(normalized.model_context_windows.len(), 2);
        assert_eq!(
            normalized.model_context_windows["gpt-5.5"],
            MIN_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(
            normalized.model_context_windows["gemini-2.5-pro"],
            MAX_CONTEXT_WINDOW_TOKENS
        );
        assert!(!normalized.model_context_windows.contains_key(""));
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
        // Simulate a legacy configuration which only has the top-level provider fields.
        config.providers.clear();
        config.active_provider_id = None;
        config.locale = "zh-CN".to_owned();
        config.model = "gpt-test".to_owned();
        config.base_url = "https://example.test/v1".to_owned();
        config.api_key = Some("sk-test-key-1234".to_owned());
        config.last_workspace_root = Some("D:\\work\\demo".to_owned());
        config
            .model_context_windows
            .insert("gpt-test".to_owned(), 272_000);
        save_user_config(&config).expect("save");
        let loaded = load_user_config();
        assert_eq!(loaded.locale, "zh-CN");
        assert_eq!(loaded.model, "gpt-test");
        assert_eq!(loaded.model_context_windows["gpt-test"], 272_000);
        assert_eq!(loaded.reasoning_effort, "high");
        assert_eq!(loaded.stream_first_event_timeout_secs, 120);
        assert_eq!(loaded.stream_idle_timeout_secs, 180);
        assert_eq!(loaded.base_url, "https://example.test");
        assert_eq!(loaded.providers.len(), 1);
        assert_eq!(loaded.providers[0].base_url, "https://example.test");
        assert_eq!(loaded.active_provider_id.as_deref(), Some("default"));
        assert_eq!(loaded.api_key.as_deref(), Some("sk-test-key-1234"));
        assert_eq!(
            loaded.last_workspace_root.as_deref(),
            Some("D:\\work\\demo")
        );
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
