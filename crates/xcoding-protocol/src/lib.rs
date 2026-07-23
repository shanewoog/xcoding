//! Shared JSON-RPC contracts for XCoding clients and the Rust core.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const JSON_RPC_VERSION: &str = "2.0";

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
    ApplyPatch,
    RunCommand,
    GitStatus,
    GitDiff,
}

impl ToolName {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ListDir => "list_dir",
            Self::ReadFile => "read_file",
            Self::SearchCode => "search_code",
            Self::ApplyPatch => "apply_patch",
            Self::RunCommand => "run_command",
            Self::GitStatus => "git_status",
            Self::GitDiff => "git_diff",
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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ProviderAuthStatus {
    pub ready: bool,
    pub has_api_key: bool,
    pub base_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_hint: Option<String>,
    pub message: String,
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
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SetConfigResult {
    pub config: WorkspaceConfig,
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
    fn defaults_session_params() {
        let params: CreateSessionParams = serde_json::from_value(json!({
            "workspace_root": "D:/work/demo"
        }))
        .expect("params parse");

        assert_eq!(params.mode, Mode::Ask);
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
