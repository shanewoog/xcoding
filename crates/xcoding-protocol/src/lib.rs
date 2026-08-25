//! Shared JSON-RPC contracts for XCoding clients and the Rust core.

use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const JSON_RPC_VERSION: &str = "2.0";
pub const DEFAULT_MAX_PROVIDER_RETRIES: u32 = 6;
pub const MIN_MAX_PROVIDER_RETRIES: u32 = 0;
pub const MAX_MAX_PROVIDER_RETRIES: u32 = 10;
pub const DEFAULT_MAX_TOOL_ROUNDS: u32 = 16;
pub const MIN_MAX_TOOL_ROUNDS: u32 = 1;
pub const MAX_MAX_TOOL_ROUNDS: u32 = 1024;
pub const DEFAULT_CIRCUIT_FAILURE_THRESHOLD: u32 = 3;
pub const MIN_CIRCUIT_FAILURE_THRESHOLD: u32 = 1;
pub const MAX_CIRCUIT_FAILURE_THRESHOLD: u32 = 10;
pub const DEFAULT_STREAM_FIRST_EVENT_TIMEOUT_SECS: u64 = 120;
pub const MIN_STREAM_FIRST_EVENT_TIMEOUT_SECS: u64 = 1;
pub const MAX_STREAM_FIRST_EVENT_TIMEOUT_SECS: u64 = 120;
pub const DEFAULT_STREAM_IDLE_TIMEOUT_SECS: u64 = 180;
pub const MIN_STREAM_IDLE_TIMEOUT_SECS: u64 = 60;
pub const MAX_STREAM_IDLE_TIMEOUT_SECS: u64 = 600;
pub const DEFAULT_NON_STREAM_TIMEOUT_SECS: u64 = 600;
pub const MIN_NON_STREAM_TIMEOUT_SECS: u64 = 60;
pub const MAX_NON_STREAM_TIMEOUT_SECS: u64 = 1_200;
pub const DEFAULT_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD: u32 = 2;
pub const MIN_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD: u32 = 1;
pub const MAX_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD: u32 = 20;
pub const DEFAULT_CIRCUIT_RECOVERY_WAIT_SECS: u64 = 60;
pub const MIN_CONTEXT_WINDOW_TOKENS: usize = 1_024;
pub const MAX_CONTEXT_WINDOW_TOKENS: usize = 10_000_000;
pub const DEFAULT_CONTEXT_COMPACTION_THRESHOLD_PERCENT: u32 = 80;
pub const MIN_CONTEXT_COMPACTION_THRESHOLD_PERCENT: u32 = 50;
pub const MAX_CONTEXT_COMPACTION_THRESHOLD_PERCENT: u32 = 95;
pub const MIN_CIRCUIT_RECOVERY_WAIT_SECS: u64 = 30;
pub const MAX_CIRCUIT_RECOVERY_WAIT_SECS: u64 = 120;
pub const DEFAULT_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT: u32 = 60;
pub const MIN_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT: u32 = 1;
pub const MAX_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT: u32 = 100;
pub const DEFAULT_CIRCUIT_MIN_REQUEST_COUNT: u32 = 10;
pub const MIN_CIRCUIT_MIN_REQUEST_COUNT: u32 = 1;
pub const MAX_CIRCUIT_MIN_REQUEST_COUNT: u32 = 100;
pub const MAX_CUSTOM_INSTRUCTIONS_CHARS: usize = 4_000;
pub const MAX_LOCAL_MEMORY_CHARS: usize = 600;
pub const DEFAULT_PERSONALITY: &str = "default";
/// Reply tones accepted by `UserConfig::personality`.
pub const PERSONALITY_OPTIONS: [&str; 5] = [
    "default",
    "pragmatic",
    "friendly",
    "concise",
    "teaching",
];

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

impl JsonRpcRequest {
    pub fn new(id: Value, method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: JSON_RPC_VERSION.to_owned(),
            id,
            method: method.into(),
            params,
        }
    }

    pub fn is_valid_version(&self) -> bool {
        self.jsonrpc == JSON_RPC_VERSION
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum JsonRpcResponse {
    Success {
        jsonrpc: String,
        id: Value,
        result: Value,
    },
    Failure {
        jsonrpc: String,
        id: Value,
        error: RpcError,
    },
}

impl JsonRpcResponse {
    pub fn success<T: Serialize>(id: Value, result: T) -> Self {
        Self::Success {
            jsonrpc: JSON_RPC_VERSION.to_owned(),
            id,
            result: serde_json::to_value(result).expect("protocol result must serialize"),
        }
    }

    pub fn failure(id: Value, error: RpcError) -> Self {
        Self::Failure {
            jsonrpc: JSON_RPC_VERSION.to_owned(),
            id,
            error,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct JsonRpcNotification<TParams = Value> {
    pub jsonrpc: String,
    pub method: String,
    pub params: TParams,
}

impl<TParams> JsonRpcNotification<TParams> {
    pub fn new(method: impl Into<String>, params: TParams) -> Self {
        Self {
            jsonrpc: JSON_RPC_VERSION.to_owned(),
            method: method.into(),
            params,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    pub fn parse_error(message: impl Into<String>) -> Self {
        Self {
            code: -32700,
            message: message.into(),
            data: None,
        }
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self {
            code: -32600,
            message: message.into(),
            data: None,
        }
    }

    pub fn method_not_found(method: impl Into<String>) -> Self {
        Self {
            code: -32601,
            message: format!("method not found: {}", method.into()),
            data: None,
        }
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
            data: None,
        }
    }

    pub fn provider_error(message: impl Into<String>) -> Self {
        Self {
            code: 1101,
            message: message.into(),
            data: None,
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: -32000,
            message: message.into(),
            data: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    Ask,
    AutoEdit,
    FullAuto,
}

impl Default for Mode {
    fn default() -> Self {
        Self::Ask
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Created,
    Running,
    NeedUser,
    Done,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

impl MessageRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ToolName {
    ListDir,
    ReadFile,
    SearchCode,
    LoadSkill,
    ApplyPatch,
    RunCommand,
    GitStatus,
    GitDiff,
    GitLog,
    GitShow,
    GitAdd,
    GitCommit,
    GitPush,
    GitFetch,
    GitPull,
    /// Read the desktop side-browser URL/title/visibility snapshot.
    BrowserState,
    /// External MCP tool (`mcp__server__tool` at the provider layer).
    Mcp,
}

impl ToolName {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ListDir => "list_dir",
            Self::ReadFile => "read_file",
            Self::SearchCode => "search_code",
            Self::LoadSkill => "load_skill",
            Self::ApplyPatch => "apply_patch",
            Self::RunCommand => "run_command",
            Self::GitStatus => "git_status",
            Self::GitDiff => "git_diff",
            Self::GitLog => "git_log",
            Self::GitShow => "git_show",
            Self::GitAdd => "git_add",
            Self::GitCommit => "git_commit",
            Self::GitPush => "git_push",
            Self::GitFetch => "git_fetch",
            Self::GitPull => "git_pull",
            Self::BrowserState => "browser_state",
            Self::Mcp => "mcp",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: ToolName,
    pub arguments: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PendingActionStatus {
    Pending,
    Approved,
    Rejected,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PendingAction {
    pub id: Uuid,
    pub session_id: Uuid,
    pub tool_call: ToolCall,
    pub status: PendingActionStatus,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PatchPreview {
    pub path: String,
    pub file_existed: bool,
    pub old_text: String,
    pub new_text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct RestorePoint {
    pub id: Uuid,
    pub session_id: Uuid,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applied_text: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct WorkspaceConfig {
    pub workspace_root: String,
    pub mode: Mode,
    pub provider: String,
    pub model: String,
    /// Extra auto-edit command allowlist patterns from `.xcoding/command-allowlist`.
    #[serde(default)]
    pub command_allowlist: Vec<String>,
    /// Workspace command denylist patterns from `.xcoding/command-denylist`.
    #[serde(default)]
    pub command_denylist: Vec<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    Created,
    Modified,
    Deleted,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct FileChangeSummary {
    pub path: String,
    pub kind: FileChangeKind,
    pub lines_added: u32,
    pub lines_removed: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct TaskSummary {
    pub changed_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_changes: Vec<FileChangeSummary>,
    pub commands_run: u32,
    pub commands_succeeded: u32,
    pub commands_failed: u32,
    #[serde(default)]
    pub lines_added: u32,
    #[serde(default)]
    pub lines_removed: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_diff: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PersistedSessionEvent {
    pub id: Uuid,
    pub session_id: Uuid,
    pub event: SessionEvent,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SessionDetail {
    pub session: Session,
    pub messages: Vec<Message>,
    pub pending_actions: Vec<PendingAction>,
    pub restore_points: Vec<RestorePoint>,
    pub events: Vec<PersistedSessionEvent>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PlanStep {
    pub id: String,
    pub description: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Session {
    pub id: Uuid,
    pub workspace_root: String,
    pub mode: Mode,
    pub provider: String,
    pub model: String,
    pub status: SessionStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Message {
    pub id: Uuid,
    pub session_id: Uuid,
    pub role: MessageRole,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

/// Persisted compact summary of an early session-message prefix.
///
/// The original messages remain in the `messages` table for replay. This record
/// only controls which history is sent to the provider on later turns.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ContextCompaction {
    pub session_id: Uuid,
    pub summary: String,
    pub compacted_message_count: usize,
    pub updated_at: DateTime<Utc>,
}

/// A durable fact distilled from a finished turn, scoped to one workspace.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct LocalMemory {
    pub id: Uuid,
    pub workspace_root: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ProviderAuthStatus {
    pub ready: bool,
    pub has_api_key: bool,
    pub base_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_hint: Option<String>,
    pub message: String,
}

/// One model entry from an OpenAI-compatible `/models` response.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ProviderModel {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owned_by: Option<String>,
}

/// Result of listing models from the configured cloud provider.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ListModelsResult {
    pub models: Vec<ProviderModel>,
    pub base_url: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderWireApi {
    #[default]
    ChatCompletions,
    Responses,
}

/// Vision delegate configuration for models without native vision support.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct VisionDelegateConfig {
    /// Whether vision delegation is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Provider id to use for vision (e.g., "openai").
    #[serde(default)]
    pub provider_id: String,
    /// Vision model id (e.g., "gpt-4o").
    #[serde(default)]
    pub model: String,
    /// Timeout for vision model calls in seconds.
    #[serde(default = "default_vision_timeout")]
    pub timeout_seconds: u64,
}

fn default_vision_timeout() -> u64 {
    30
}

/// Model capability flags for vision support detection.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ModelCapabilities {
    /// Whether the model natively supports image inputs.
    #[serde(default)]
    pub supports_vision: bool,
}

/// One OpenAI-compatible cloud provider endpoint in Desktop settings.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CloudProviderConfig {
    pub id: String,
    /// Display name shown in the Desktop provider manager.
    pub name: String,
    /// OpenAI-compatible API host without a trailing `/v1` suffix.
    pub base_url: String,
    /// HTTP request/stream protocol used by this provider.
    #[serde(default)]
    pub wire_api: ProviderWireApi,
    /// Full API key when configured. Never log this value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

/// User-level Desktop/CLI preferences stored under `~/.xcoding/config.json`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct UserConfig {
    /// UI locale: `en` or `zh-CN`.
    #[serde(default = "default_locale")]
    pub locale: String,
    #[serde(default)]
    pub mode: Mode,
    /// Extra instructions appended to the system prompt for every session on this machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_instructions: Option<String>,
    /// Default reply tone: `default` | `pragmatic` | `friendly` | `concise` | `teaching`.
    #[serde(default = "default_personality")]
    pub personality: String,
    /// Whether finished turns may distill durable facts into local memory.
    #[serde(default, skip_serializing_if = "is_false")]
    pub local_memory_enabled: bool,
    /// Whether turns that called MCP tools may also produce local memory.
    #[serde(default = "default_tool_memory_enabled")]
    pub tool_memory_enabled: bool,
    /// Technical provider id used by sessions (currently always openai-compatible).
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default = "default_model")]
    pub model: String,
    /// Reasoning effort for compatible models: none | low | medium | high.
    #[serde(default = "default_reasoning_effort")]
    pub reasoning_effort: String,
    /// Retries after the initial failed request for one provider before trying a backup provider.
    #[serde(default = "default_max_provider_retries")]
    pub max_provider_retries: u32,
    /// Whether XCoding may switch to another configured provider after the active provider fails.
    #[serde(default = "default_provider_fallback_enabled")]
    pub provider_fallback_enabled: bool,
    /// Maximum model↔tool interaction rounds for one user turn before the agent stops.
    #[serde(default = "default_max_tool_rounds")]
    pub max_tool_rounds: u32,
    /// Consecutive failed turns that open a provider circuit.
    #[serde(default = "default_circuit_failure_threshold")]
    pub circuit_failure_threshold: u32,
    /// Maximum wait for the first stream event, including the initial response setup.
    #[serde(default = "default_stream_first_event_timeout_secs")]
    pub stream_first_event_timeout_secs: u64,
    /// Maximum gap between stream events after the first event is received.
    #[serde(default = "default_stream_idle_timeout_secs")]
    pub stream_idle_timeout_secs: u64,
    /// Reserved for future non-streaming provider calls; streaming chat does not use this yet.
    #[serde(default = "default_non_stream_timeout_secs")]
    pub non_stream_timeout_secs: u64,
    /// Successful half-open turns required before a provider circuit closes.
    #[serde(default = "default_circuit_recovery_success_threshold")]
    pub circuit_recovery_success_threshold: u32,
    /// Seconds a tripped provider circuit remains open before a half-open probe.
    #[serde(default = "default_circuit_recovery_wait_secs")]
    pub circuit_recovery_wait_secs: u64,
    /// Failure rate percentage that can open a circuit once enough requests were sampled.
    #[serde(default = "default_circuit_error_rate_threshold_percent")]
    pub circuit_error_rate_threshold_percent: u32,
    /// Minimum number of completed provider turns used for failure-rate circuit evaluation.
    #[serde(default = "default_circuit_min_request_count")]
    pub circuit_min_request_count: u32,
    /// Active OpenAI-compatible API host without a trailing `/v1` suffix.
    #[serde(default = "default_base_url")]
    pub base_url: String,
    /// Active API key when configured. Never log this value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Managed cloud providers. At most one is active via `active_provider_id`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<CloudProviderConfig>,
    /// Id of the currently enabled cloud provider in `providers`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_provider_id: Option<String>,
    /// Last project path opened in Desktop (agent workspace root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_workspace_root: Option<String>,
    /// Parent directory where Desktop creates new projects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_home: Option<String>,
    /// Project paths removed from the Desktop project area (folders are kept on disk).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden_project_paths: Vec<String>,
    /// Whether tightly constrained PowerShell requests to loopback HTTP APIs may run without a high-risk prompt.
    #[serde(default, skip_serializing_if = "is_false")]
    pub skip_local_api_confirmation: bool,
    /// Per-model context window overrides keyed by normalized (trimmed, lowercased) model id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_context_windows: BTreeMap<String, usize>,
    /// Percentage of the configured model context window at which pre-compaction starts.
    #[serde(default = "default_context_compaction_threshold_percent")]
    pub context_compaction_threshold_percent: u32,
    /// Vision delegate configuration for models without native vision support.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision_delegate: Option<VisionDelegateConfig>,
    /// Model capability flags keyed by normalized model id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_capabilities: BTreeMap<String, ModelCapabilities>,
}

impl Default for UserConfig {
    fn default() -> Self {
        let provider = CloudProviderConfig {
            id: "default".to_owned(),
            name: "openai".to_owned(),
            base_url: default_base_url(),
            wire_api: ProviderWireApi::default(),
            api_key: None,
        };
        Self {
            locale: default_locale(),
            mode: Mode::default(),
            custom_instructions: None,
            personality: default_personality(),
            local_memory_enabled: false,
            tool_memory_enabled: default_tool_memory_enabled(),
            provider: default_provider(),
            model: default_model(),
            reasoning_effort: default_reasoning_effort(),
            max_provider_retries: default_max_provider_retries(),
            provider_fallback_enabled: default_provider_fallback_enabled(),
            max_tool_rounds: default_max_tool_rounds(),
            circuit_failure_threshold: default_circuit_failure_threshold(),
            stream_first_event_timeout_secs: default_stream_first_event_timeout_secs(),
            stream_idle_timeout_secs: default_stream_idle_timeout_secs(),
            non_stream_timeout_secs: default_non_stream_timeout_secs(),
            circuit_recovery_success_threshold: default_circuit_recovery_success_threshold(),
            circuit_recovery_wait_secs: default_circuit_recovery_wait_secs(),
            circuit_error_rate_threshold_percent: default_circuit_error_rate_threshold_percent(),
            circuit_min_request_count: default_circuit_min_request_count(),
            base_url: provider.base_url.clone(),
            api_key: None,
            providers: vec![provider.clone()],
            active_provider_id: Some(provider.id),
            last_workspace_root: None,
            workspace_home: None,
            hidden_project_paths: Vec::new(),
            skip_local_api_confirmation: false,
            model_context_windows: BTreeMap::new(),
            context_compaction_threshold_percent: DEFAULT_CONTEXT_COMPACTION_THRESHOLD_PERCENT,
            vision_delegate: None,
            model_capabilities: BTreeMap::new(),
        }
    }
}

/// On-disk project under the workspace home.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ProjectDir {
    pub path: String,
    pub dir_name: String,
    pub title: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CreateProjectParams {
    pub workspace_home: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CreateProjectResult {
    pub project: ProjectDir,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ImportProjectParams {
    pub workspace_home: String,
    pub source_path: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ImportProjectResult {
    pub project: ProjectDir,
    pub already_existed: bool,
    pub copied: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PingResult {
    pub ok: bool,
    pub version: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CreateSessionParams {
    pub workspace_root: String,
    #[serde(default)]
    pub mode: Mode,
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CreateSessionResult {
    pub session: Session,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct ListSessionsParams {
    #[serde(default)]
    pub workspace_root: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ListSessionsResult {
    pub sessions: Vec<Session>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct GetSessionDetailParams {
    pub session_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct GetSessionDetailResult {
    pub detail: SessionDetail,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ReplaySessionParams {
    pub session_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ReplayStep {
    pub kind: String,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<ToolName>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ReplaySessionResult {
    pub session: Session,
    pub events: Vec<PersistedSessionEvent>,
    pub steps: Vec<ReplayStep>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct GetConfigParams {
    pub workspace_root: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct GetConfigResult {
    pub config: WorkspaceConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SetConfigParams {
    pub workspace_root: String,
    pub mode: Mode,
    pub provider: String,
    pub model: String,
    /// When `Some`, rewrite `.xcoding/command-allowlist`. `None` leaves the file unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_allowlist: Option<Vec<String>>,
    /// When `Some`, rewrite `.xcoding/command-denylist`. `None` leaves the file unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_denylist: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SetConfigResult {
    pub config: WorkspaceConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ChatImageAttachment {
    /// MIME type, e.g. `image/png`.
    pub mime_type: String,
    /// Raw base64 payload without a `data:` prefix.
    pub data_base64: String,
    /// Optional original file name for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ChatParams {
    pub workspace_root: String,
    pub message: String,
    #[serde(default)]
    pub mode: Option<Mode>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    /// When set, continue an existing finished session instead of creating a new one.
    #[serde(default)]
    pub session_id: Option<Uuid>,
    /// Optional image attachments for vision-capable models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<ChatImageAttachment>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ChatResult {
    pub session: Session,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<Message>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct RollbackRestorePointParams {
    pub session_id: Uuid,
    pub restore_point_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct RollbackRestorePointResult {
    pub session: Session,
    pub restore_point: RestorePoint,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CancelSessionParams {
    pub session_id: Uuid,
    /// Optional assistant text already shown in the UI; persisted on cancel/steer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial_assistant: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CancelSessionResult {
    pub session: Session,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ResolveActionParams {
    pub session_id: Uuid,
    pub action_id: Uuid,
    pub approved: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ResolveActionResult {
    pub session: Session,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<Message>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEvent {
    TextDelta {
        session_id: Uuid,
        delta: String,
    },
    MessageCompleted {
        session_id: Uuid,
        message: Message,
    },
    Plan {
        session_id: Uuid,
        steps: Vec<PlanStep>,
    },
    ToolStart {
        session_id: Uuid,
        tool_call: ToolCall,
        summary: String,
    },
    ToolEnd {
        session_id: Uuid,
        tool_call: ToolCall,
        success: bool,
        summary: String,
    },
    PatchPreview {
        session_id: Uuid,
        preview: PatchPreview,
    },
    ApprovalRequested {
        session_id: Uuid,
        action: PendingAction,
        summary: String,
    },
    RestorePointRolledBack {
        session_id: Uuid,
        restore_point: RestorePoint,
        summary: String,
    },
    SessionCancelled {
        session_id: Uuid,
        message: String,
    },
    TaskCompleted {
        session_id: Uuid,
        summary: TaskSummary,
    },
    /// A retryable cloud-provider interruption occurred before output was received.
    Retrying {
        session_id: Uuid,
        attempt: u32,
        max_attempts: u32,
        message: String,
    },
    /// Sanitized audit record for one model HTTP request. It deliberately excludes
    /// credentials, request messages, and generated text.
    ModelCall {
        session_id: Uuid,
        provider: String,
        model: String,
        endpoint: String,
        purpose: String,
        round: u32,
        attempt: u32,
        max_attempts: u32,
        success: bool,
        output_chars: usize,
        tool_calls: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Vision delegate started processing images.
    VisionDelegateStart {
        session_id: Uuid,
        image_count: usize,
        delegate_model: String,
        /// True when the images come from an earlier turn rather than the
        /// message the user just sent, so the UI can say so instead of
        /// implying the current message has attachments. Defaults to false so
        /// events persisted before this field stay readable.
        #[serde(default)]
        historical: bool,
    },
    /// Vision delegate successfully returned image descriptions.
    VisionDelegateSuccess {
        session_id: Uuid,
        image_count: usize,
        description_length: usize,
    },
    /// Vision delegate failed to process images.
    VisionDelegateFailed {
        session_id: Uuid,
        image_count: usize,
        error: String,
    },
    Error {
        session_id: Uuid,
        message: String,
    },
}

fn default_provider() -> String {
    "openai".to_owned()
}

fn default_model() -> String {
    "gpt-5.5".to_owned()
}

fn default_reasoning_effort() -> String {
    "high".to_owned()
}

fn default_max_provider_retries() -> u32 {
    DEFAULT_MAX_PROVIDER_RETRIES
}

fn default_provider_fallback_enabled() -> bool {
    true
}

fn default_max_tool_rounds() -> u32 {
    DEFAULT_MAX_TOOL_ROUNDS
}

fn default_circuit_failure_threshold() -> u32 {
    DEFAULT_CIRCUIT_FAILURE_THRESHOLD
}

fn default_stream_first_event_timeout_secs() -> u64 {
    DEFAULT_STREAM_FIRST_EVENT_TIMEOUT_SECS
}

fn default_stream_idle_timeout_secs() -> u64 {
    DEFAULT_STREAM_IDLE_TIMEOUT_SECS
}

fn default_non_stream_timeout_secs() -> u64 {
    DEFAULT_NON_STREAM_TIMEOUT_SECS
}

fn default_circuit_recovery_success_threshold() -> u32 {
    DEFAULT_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD
}

fn default_circuit_recovery_wait_secs() -> u64 {
    DEFAULT_CIRCUIT_RECOVERY_WAIT_SECS
}

fn default_circuit_error_rate_threshold_percent() -> u32 {
    DEFAULT_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT
}

fn default_circuit_min_request_count() -> u32 {
    DEFAULT_CIRCUIT_MIN_REQUEST_COUNT
}

fn default_base_url() -> String {
    "https://ai.v58.dev".to_owned()
}

fn default_locale() -> String {
    "en".to_owned()
}

fn default_personality() -> String {
    DEFAULT_PERSONALITY.to_owned()
}

fn default_tool_memory_enabled() -> bool {
    true
}

fn default_context_compaction_threshold_percent() -> u32 {
    DEFAULT_CONTEXT_COMPACTION_THRESHOLD_PERCENT
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trips_json_rpc_request() {
        let request = JsonRpcRequest::new(json!(1), "system.ping", json!({}));
        let encoded = serde_json::to_string(&request).expect("request serializes");
        let decoded: JsonRpcRequest = serde_json::from_str(&encoded).expect("request parses");

        assert_eq!(decoded, request);
        assert!(decoded.is_valid_version());
    }

    #[test]
    fn defaults_user_config_reasoning_effort() {
        let config: UserConfig = serde_json::from_value(json!({
            "locale": "en",
            "mode": "ask",
            "provider": "openai",
            "model": "gpt-5.5",
            "base_url": "https://example.test/v1"
        }))
        .expect("user config parses");
        assert_eq!(config.reasoning_effort, "high");
        assert_eq!(
            config.stream_idle_timeout_secs,
            DEFAULT_STREAM_IDLE_TIMEOUT_SECS
        );
        assert_eq!(config.max_provider_retries, DEFAULT_MAX_PROVIDER_RETRIES);
        assert!(config.provider_fallback_enabled);
        assert_eq!(config.max_tool_rounds, DEFAULT_MAX_TOOL_ROUNDS);
        assert_eq!(
            config.circuit_failure_threshold,
            DEFAULT_CIRCUIT_FAILURE_THRESHOLD
        );
        assert_eq!(
            config.stream_first_event_timeout_secs,
            DEFAULT_STREAM_FIRST_EVENT_TIMEOUT_SECS
        );
        assert_eq!(
            config.non_stream_timeout_secs,
            DEFAULT_NON_STREAM_TIMEOUT_SECS
        );
        assert_eq!(
            config.circuit_recovery_success_threshold,
            DEFAULT_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD
        );
        assert_eq!(
            config.circuit_recovery_wait_secs,
            DEFAULT_CIRCUIT_RECOVERY_WAIT_SECS
        );
        assert_eq!(
            config.circuit_error_rate_threshold_percent,
            DEFAULT_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT
        );
        assert_eq!(
            config.circuit_min_request_count,
            DEFAULT_CIRCUIT_MIN_REQUEST_COUNT
        );
        assert!(config.providers.is_empty());
        assert_eq!(config.active_provider_id, None);
        assert!(!config.skip_local_api_confirmation);
    }

    #[test]
    fn serializes_local_api_confirmation_only_when_enabled() {
        let default_config = UserConfig::default();
        let default_json =
            serde_json::to_value(&default_config).expect("default config serializes");
        assert!(default_json.get("skip_local_api_confirmation").is_none());

        let enabled = UserConfig {
            skip_local_api_confirmation: true,
            ..default_config
        };
        let enabled_json = serde_json::to_value(&enabled).expect("enabled config serializes");
        assert_eq!(enabled_json["skip_local_api_confirmation"], true);
    }

    #[test]
    fn defaults_and_round_trips_model_context_windows() {
        let legacy: UserConfig = serde_json::from_value(json!({
            "locale": "en",
            "mode": "ask",
            "provider": "openai",
            "model": "gpt-5.5",
            "base_url": "https://example.test/v1"
        }))
        .expect("legacy user config parses");
        assert!(legacy.model_context_windows.is_empty());
        assert!(
            serde_json::to_value(&legacy)
                .expect("legacy config serializes")
                .get("model_context_windows")
                .is_none()
        );

        let mut windows = BTreeMap::new();
        windows.insert("gpt-5.5".to_owned(), 272_000usize);
        let configured = UserConfig {
            model_context_windows: windows,
            ..UserConfig::default()
        };
        let json = serde_json::to_value(&configured).expect("configured config serializes");
        assert_eq!(json["model_context_windows"]["gpt-5.5"], 272_000);
        let decoded: UserConfig =
            serde_json::from_value(json).expect("configured config round-trips");
        assert_eq!(decoded.model_context_windows["gpt-5.5"], 272_000);
    }

    #[test]
    fn defaults_user_config_includes_provider_slot() {
        let config = UserConfig::default();
        assert_eq!(config.base_url, "https://ai.v58.dev");
        assert_eq!(config.providers.len(), 1);
        assert_eq!(config.providers[0].base_url, "https://ai.v58.dev");
        assert_eq!(
            config.providers[0].wire_api,
            ProviderWireApi::ChatCompletions
        );
        assert_eq!(config.active_provider_id.as_deref(), Some("default"));
    }

    #[test]
    fn provider_wire_api_is_backward_compatible() {
        let legacy: CloudProviderConfig = serde_json::from_value(json!({
            "id": "legacy",
            "name": "Legacy provider",
            "base_url": "https://example.test"
        }))
        .expect("legacy provider config parses");
        assert_eq!(legacy.wire_api, ProviderWireApi::ChatCompletions);

        let responses = CloudProviderConfig {
            wire_api: ProviderWireApi::Responses,
            ..legacy
        };
        let encoded = serde_json::to_value(responses).expect("provider config serializes");
        assert_eq!(encoded["wire_api"], "responses");
    }

    #[test]
    fn defaults_session_params() {
        let params: CreateSessionParams = serde_json::from_value(json!({
            "workspace_root": "D:/work/demo"
        }))
        .expect("params parse");

        assert_eq!(params.mode, Mode::Ask);
        assert_eq!(
            serde_json::to_value(Mode::FullAuto).unwrap(),
            serde_json::json!("full-auto")
        );
        assert_eq!(
            serde_json::from_value::<Mode>(serde_json::json!("full-auto")).unwrap(),
            Mode::FullAuto
        );
        assert_eq!(params.provider, "openai");
        assert_eq!(params.model, "gpt-5.5");
    }

    #[test]
    fn serializes_session_event_notification() {
        let session_id = Uuid::nil();
        let notification = JsonRpcNotification::new(
            "session.event",
            SessionEvent::TextDelta {
                session_id,
                delta: "Hello".to_owned(),
            },
        );

        assert_eq!(
            serde_json::to_value(notification).expect("notification serializes"),
            json!({
                "jsonrpc": "2.0",
                "method": "session.event",
                "params": {
                    "type": "text_delta",
                    "session_id": session_id,
                    "delta": "Hello"
                }
            })
        );
    }

    #[test]
    fn serializes_read_only_tool_events() {
        let event = SessionEvent::ToolStart {
            session_id: Uuid::nil(),
            tool_call: ToolCall {
                id: "call_1".to_owned(),
                name: ToolName::ReadFile,
                arguments: json!({ "path": "src/main.rs" }),
            },
            summary: "Read src/main.rs".to_owned(),
        };

        assert_eq!(
            serde_json::to_value(event).expect("event serializes"),
            json!({
                "type": "tool_start",
                "session_id": "00000000-0000-0000-0000-000000000000",
                "tool_call": {
                    "id": "call_1",
                    "name": "read_file",
                    "arguments": { "path": "src/main.rs" }
                },
                "summary": "Read src/main.rs"
            })
        );
    }
}
