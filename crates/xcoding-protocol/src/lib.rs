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

fn default_provider() -> String {
    "openai".to_owned()
}

fn default_model() -> String {
    "gpt-4.1".to_owned()
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
        assert_eq!(params.model, "gpt-4.1");
    }
}
