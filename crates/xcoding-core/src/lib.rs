//! XCoding's core request dispatcher. Agent orchestration is added in later phases.

use std::path::Path;

use serde_json::Value;
use thiserror::Error;
use xcoding_protocol::{
    CreateSessionParams, CreateSessionResult, JsonRpcRequest, JsonRpcResponse, ListSessionsParams,
    ListSessionsResult, PingResult, RpcError,
};
use xcoding_store::{SessionStore, StoreError};

#[derive(Debug, Error)]
pub enum CoreError {
    #[error(transparent)]
    Store(#[from] StoreError),
}

pub struct CoreService {
    store: SessionStore,
}

impl CoreService {
    pub fn in_memory() -> Result<Self, CoreError> {
        Ok(Self {
            store: SessionStore::in_memory()?,
        })
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, CoreError> {
        Ok(Self {
            store: SessionStore::open(path)?,
        })
    }

    pub fn ping(&self) -> PingResult {
        PingResult {
            ok: true,
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }

    pub fn dispatch(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        let id = request.id.clone();

        if !request.is_valid_version() {
            return JsonRpcResponse::failure(
                id,
                RpcError::invalid_request("jsonrpc must be exactly \"2.0\""),
            );
        }

        let result = match request.method.as_str() {
            "system.ping" => Ok(serde_json::to_value(self.ping()).expect("ping serializes")),
            "session.create" => self.create_session(request.params),
            "session.list" => self.list_sessions(request.params),
            _ => return JsonRpcResponse::failure(id, RpcError::method_not_found(request.method)),
        };

        match result {
            Ok(result) => JsonRpcResponse::success(id, result),
            Err(error) => JsonRpcResponse::failure(id, error),
        }
    }

    fn create_session(&self, params: Value) -> Result<Value, RpcError> {
        let params: CreateSessionParams = serde_json::from_value(params).map_err(|error| {
            RpcError::invalid_params(format!("invalid session.create params: {error}"))
        })?;
        let session = self
            .store
            .create_session(params)
            .map_err(|error| RpcError::internal(error.to_string()))?;
        serde_json::to_value(CreateSessionResult { session })
            .map_err(|error| RpcError::internal(error.to_string()))
    }

    fn list_sessions(&self, params: Value) -> Result<Value, RpcError> {
        let params: ListSessionsParams = serde_json::from_value(params).map_err(|error| {
            RpcError::invalid_params(format!("invalid session.list params: {error}"))
        })?;
        let sessions = self
            .store
            .list_sessions(params.workspace_root.as_deref())
            .map_err(|error| RpcError::internal(error.to_string()))?;
        serde_json::to_value(ListSessionsResult { sessions })
            .map_err(|error| RpcError::internal(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use xcoding_protocol::{JsonRpcRequest, JsonRpcResponse};

    use super::*;

    #[test]
    fn serves_ping() {
        let core = CoreService::in_memory().expect("core starts");
        let response = core.dispatch(JsonRpcRequest::new(json!(1), "system.ping", json!({})));

        match response {
            JsonRpcResponse::Success { result, .. } => assert_eq!(result["ok"], true),
            JsonRpcResponse::Failure { error, .. } => panic!("unexpected error: {error:?}"),
        }
    }

    #[test]
    fn creates_and_lists_sessions() {
        let core = CoreService::in_memory().expect("core starts");
        let create = core.dispatch(JsonRpcRequest::new(
            json!(1),
            "session.create",
            json!({ "workspace_root": "D:/work/demo", "title": "Demo" }),
        ));
        assert!(matches!(create, JsonRpcResponse::Success { .. }));

        let list = core.dispatch(JsonRpcRequest::new(
            json!(2),
            "session.list",
            json!({ "workspace_root": "D:/work/demo" }),
        ));
        match list {
            JsonRpcResponse::Success { result, .. } => {
                assert_eq!(result["sessions"].as_array().expect("array").len(), 1)
            }
            JsonRpcResponse::Failure { error, .. } => panic!("unexpected error: {error:?}"),
        }
    }
}
