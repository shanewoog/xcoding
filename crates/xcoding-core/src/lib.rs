//! XCoding's core request dispatcher and chat session lifecycle.

use std::collections::BTreeSet;
use std::path::Path;

use chrono::Utc;

use serde_json::Value;
use thiserror::Error;
use xcoding_protocol::{
    CancelSessionParams, CancelSessionResult, ChatParams, ChatResult, CreateSessionParams,
    CreateSessionResult, GetConfigParams, GetConfigResult, GetSessionDetailParams,
    GetSessionDetailResult, JsonRpcRequest, JsonRpcResponse, ListSessionsParams,
    ListSessionsResult, Message, MessageRole, PendingAction, PendingActionStatus,
    PersistedSessionEvent, PingResult, RestorePoint, RpcError, Session, SessionDetail,
    SessionEvent, SessionStatus, SetConfigParams, SetConfigResult, TaskSummary, ToolCall, ToolName,
    WorkspaceConfig,
};
use xcoding_store::{SessionStore, StoreError};

#[derive(Debug, Error)]
pub enum CoreError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("invalid chat input: {0}")]
    InvalidInput(String),
    #[error("session not found: {0}")]
    SessionNotFound(String),
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

    pub fn start_chat(&self, params: ChatParams) -> Result<Session, CoreError> {
        if params.workspace_root.trim().is_empty() {
            return Err(CoreError::InvalidInput(
                "workspace_root must not be empty".to_owned(),
            ));
        }
        if params.message.trim().is_empty() {
            return Err(CoreError::InvalidInput(
                "message must not be empty".to_owned(),
            ));
        }

        let config = self.workspace_config(&params.workspace_root)?;
        let session = self.store.create_session(CreateSessionParams {
            workspace_root: params.workspace_root,
            mode: params.mode.unwrap_or(config.mode),
            provider: params.provider.unwrap_or(config.provider),
            model: params.model.unwrap_or(config.model),
            title: params.title,
        })?;
        let session = self.set_status(session.id, SessionStatus::Running)?;
        self.store
            .append_message(session.id, MessageRole::User, params.message)?;
        Ok(session)
    }

    pub fn workspace_config(&self, workspace_root: &str) -> Result<WorkspaceConfig, CoreError> {
        validate_workspace_root(workspace_root)?;
        Ok(self
            .store
            .get_workspace_config(workspace_root)?
            .unwrap_or_else(|| WorkspaceConfig {
                workspace_root: workspace_root.to_owned(),
                mode: xcoding_protocol::Mode::Ask,
                provider: "openai".to_owned(),
                model: "gpt-4.1".to_owned(),
                updated_at: Utc::now(),
            }))
    }

    pub fn set_workspace_config(
        &self,
        params: SetConfigParams,
    ) -> Result<WorkspaceConfig, CoreError> {
        validate_workspace_root(&params.workspace_root)?;
        if params.provider != "openai" {
            return Err(CoreError::InvalidInput(
                "only the openai-compatible cloud provider is supported".to_owned(),
            ));
        }
        if params.model.trim().is_empty() {
            return Err(CoreError::InvalidInput(
                "model must not be empty".to_owned(),
            ));
        }
        Ok(self.store.set_workspace_config(WorkspaceConfig {
            workspace_root: params.workspace_root,
            mode: params.mode,
            provider: params.provider,
            model: params.model.trim().to_owned(),
            updated_at: Utc::now(),
        })?)
    }

    pub fn task_summary(&self, session_id: uuid::Uuid) -> Result<TaskSummary, CoreError> {
        self.session(session_id)?;
        let changed_files = self
            .store
            .list_restore_points(session_id)?
            .into_iter()
            .map(|restore_point| restore_point.path)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut commands_run = 0;
        let mut commands_succeeded = 0;
        let mut commands_failed = 0;
        for event in self.store.list_events(session_id)? {
            if let SessionEvent::ToolEnd {
                tool_call, success, ..
            } = event.event
            {
                if tool_call.name == ToolName::RunCommand {
                    commands_run += 1;
                    if success {
                        commands_succeeded += 1;
                    } else {
                        commands_failed += 1;
                    }
                }
            }
        }
        Ok(TaskSummary {
            changed_files,
            commands_run,
            commands_succeeded,
            commands_failed,
        })
    }

    pub fn list_sessions(&self, workspace_root: Option<&str>) -> Result<Vec<Session>, CoreError> {
        self.store
            .list_sessions(workspace_root)
            .map_err(CoreError::from)
    }

    pub fn messages(&self, session_id: uuid::Uuid) -> Result<Vec<Message>, CoreError> {
        self.store
            .list_messages(session_id)
            .map_err(CoreError::from)
    }

    pub fn session(&self, session_id: uuid::Uuid) -> Result<Session, CoreError> {
        self.store
            .get_session(session_id)?
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))
    }

    pub fn session_detail(&self, session_id: uuid::Uuid) -> Result<SessionDetail, CoreError> {
        Ok(SessionDetail {
            session: self.session(session_id)?,
            messages: self.store.list_messages(session_id)?,
            pending_actions: self.store.list_pending_actions(session_id)?,
            restore_points: self.store.list_restore_points(session_id)?,
            events: self.store.list_events(session_id)?,
        })
    }

    pub fn restore_point(
        &self,
        session_id: uuid::Uuid,
        restore_point_id: uuid::Uuid,
    ) -> Result<RestorePoint, CoreError> {
        let restore_point = self
            .store
            .get_restore_point(restore_point_id)?
            .ok_or_else(|| {
                CoreError::InvalidInput(format!("restore point not found: {restore_point_id}"))
            })?;
        if restore_point.session_id != session_id {
            return Err(CoreError::InvalidInput(
                "restore point does not belong to this session".to_owned(),
            ));
        }
        Ok(restore_point)
    }

    pub fn record_event(&self, event: &SessionEvent) -> Result<PersistedSessionEvent, CoreError> {
        Ok(self.store.record_event(event)?)
    }

    pub fn cancel_session(&self, session_id: uuid::Uuid) -> Result<Session, CoreError> {
        let session = self.session(session_id)?;
        if !matches!(
            session.status,
            SessionStatus::Running | SessionStatus::NeedUser
        ) {
            return Err(CoreError::InvalidInput(
                "only active sessions can be cancelled".to_owned(),
            ));
        }
        self.store.reject_pending_actions(session_id)?;
        self.set_status(session_id, SessionStatus::Cancelled)
    }

    pub fn create_pending_action(
        &self,
        session_id: uuid::Uuid,
        tool_call: ToolCall,
    ) -> Result<PendingAction, CoreError> {
        self.store
            .create_pending_action(session_id, tool_call)
            .map_err(CoreError::from)
    }

    pub fn resolve_pending_action(
        &self,
        session_id: uuid::Uuid,
        action_id: uuid::Uuid,
        approved: bool,
    ) -> Result<PendingAction, CoreError> {
        let session = self.session(session_id)?;
        if session.status != SessionStatus::NeedUser {
            return Err(CoreError::InvalidInput(
                "actions can only be resolved while a session is waiting for approval".to_owned(),
            ));
        }
        let action = self.store.get_pending_action(action_id)?.ok_or_else(|| {
            CoreError::InvalidInput(format!("pending action not found: {action_id}"))
        })?;
        if action.session_id != session_id {
            return Err(CoreError::InvalidInput(
                "pending action does not belong to this session".to_owned(),
            ));
        }
        let status = if approved {
            PendingActionStatus::Approved
        } else {
            PendingActionStatus::Rejected
        };
        self.store
            .resolve_pending_action(action_id, status)?
            .ok_or_else(|| {
                CoreError::InvalidInput("pending action has already been resolved".to_owned())
            })
    }

    pub fn pause_chat(&self, session_id: uuid::Uuid) -> Result<Session, CoreError> {
        self.set_status(session_id, SessionStatus::NeedUser)
    }

    pub fn resume_chat(&self, session_id: uuid::Uuid) -> Result<Session, CoreError> {
        self.set_status(session_id, SessionStatus::Running)
    }

    pub fn create_restore_point(
        &self,
        session_id: uuid::Uuid,
        path: &str,
        original_text: Option<&str>,
        applied_text: &str,
    ) -> Result<RestorePoint, CoreError> {
        self.store
            .create_restore_point(session_id, path, original_text, applied_text)
            .map_err(CoreError::from)
    }

    pub fn record_tool_message(
        &self,
        session_id: uuid::Uuid,
        content: impl Into<String>,
    ) -> Result<Message, CoreError> {
        Ok(self
            .store
            .append_message(session_id, MessageRole::Tool, content)?)
    }
    pub fn complete_chat(
        &self,
        session_id: uuid::Uuid,
        content: impl Into<String>,
    ) -> Result<ChatResult, CoreError> {
        let message = self
            .store
            .append_message(session_id, MessageRole::Assistant, content)?;
        let session = self.set_status(session_id, SessionStatus::Done)?;
        Ok(ChatResult {
            session,
            message: Some(message),
        })
    }

    pub fn fail_chat(&self, session_id: uuid::Uuid) -> Result<Session, CoreError> {
        self.set_status(session_id, SessionStatus::Failed)
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
            "session.list" => self.list_sessions_rpc(request.params),
            "session.detail" => self.session_detail_rpc(request.params),
            "session.cancel" => self.cancel_session_rpc(request.params),
            "config.get" => self.config_get_rpc(request.params),
            "config.set" => self.config_set_rpc(request.params),
            _ => return JsonRpcResponse::failure(id, RpcError::method_not_found(request.method)),
        };

        match result {
            Ok(result) => JsonRpcResponse::success(id, result),
            Err(error) => JsonRpcResponse::failure(id, error),
        }
    }

    fn set_status(
        &self,
        session_id: uuid::Uuid,
        status: SessionStatus,
    ) -> Result<Session, CoreError> {
        self.store
            .set_session_status(session_id, status)?
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))
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

    fn list_sessions_rpc(&self, params: Value) -> Result<Value, RpcError> {
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

    fn session_detail_rpc(&self, params: Value) -> Result<Value, RpcError> {
        let params: GetSessionDetailParams = serde_json::from_value(params).map_err(|error| {
            RpcError::invalid_params(format!("invalid session.detail params: {error}"))
        })?;
        let detail = self
            .session_detail(params.session_id)
            .map_err(|error| RpcError::internal(error.to_string()))?;
        serde_json::to_value(GetSessionDetailResult { detail })
            .map_err(|error| RpcError::internal(error.to_string()))
    }

    fn config_get_rpc(&self, params: Value) -> Result<Value, RpcError> {
        let params: GetConfigParams = serde_json::from_value(params).map_err(|error| {
            RpcError::invalid_params(format!("invalid config.get params: {error}"))
        })?;
        let config =
            self.workspace_config(&params.workspace_root)
                .map_err(|error| match error {
                    CoreError::InvalidInput(message) => RpcError::invalid_params(message),
                    other => RpcError::internal(other.to_string()),
                })?;
        serde_json::to_value(GetConfigResult { config })
            .map_err(|error| RpcError::internal(error.to_string()))
    }

    fn config_set_rpc(&self, params: Value) -> Result<Value, RpcError> {
        let params: SetConfigParams = serde_json::from_value(params).map_err(|error| {
            RpcError::invalid_params(format!("invalid config.set params: {error}"))
        })?;
        let config = self
            .set_workspace_config(params)
            .map_err(|error| match error {
                CoreError::InvalidInput(message) => RpcError::invalid_params(message),
                other => RpcError::internal(other.to_string()),
            })?;
        serde_json::to_value(SetConfigResult { config })
            .map_err(|error| RpcError::internal(error.to_string()))
    }

    fn cancel_session_rpc(&self, params: Value) -> Result<Value, RpcError> {
        let params: CancelSessionParams = serde_json::from_value(params).map_err(|error| {
            RpcError::invalid_params(format!("invalid session.cancel params: {error}"))
        })?;
        let session = self
            .cancel_session(params.session_id)
            .map_err(|error| match error {
                CoreError::InvalidInput(message) => RpcError::invalid_params(message),
                other => RpcError::internal(other.to_string()),
            })?;
        serde_json::to_value(CancelSessionResult { session })
            .map_err(|error| RpcError::internal(error.to_string()))
    }
}

fn validate_workspace_root(workspace_root: &str) -> Result<(), CoreError> {
    if workspace_root.trim().is_empty() {
        return Err(CoreError::InvalidInput(
            "workspace_root must not be empty".to_owned(),
        ));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use serde_json::json;
    use xcoding_protocol::{
        JsonRpcRequest, JsonRpcResponse, Mode, PlanStep, SessionEvent, ToolCall, ToolName,
    };

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

    #[test]
    fn persists_workspace_defaults_and_uses_them_for_chats() {
        let core = CoreService::in_memory().expect("core starts");
        let workspace_root = "D:/work/configured";
        let defaults = core
            .workspace_config(workspace_root)
            .expect("defaults load");
        assert_eq!(defaults.mode, Mode::Ask);
        assert_eq!(defaults.provider, "openai");
        assert_eq!(defaults.model, "gpt-4.1");

        let saved = core
            .set_workspace_config(SetConfigParams {
                workspace_root: workspace_root.to_owned(),
                mode: Mode::AutoEdit,
                provider: "openai".to_owned(),
                model: "configured-model".to_owned(),
            })
            .expect("config saves");
        assert_eq!(saved.mode, Mode::AutoEdit);
        assert_eq!(saved.model, "configured-model");

        let session = core
            .start_chat(ChatParams {
                workspace_root: workspace_root.to_owned(),
                message: "Use workspace defaults".to_owned(),
                mode: None,
                provider: None,
                model: None,
                title: None,
            })
            .expect("chat starts");
        assert_eq!(session.mode, Mode::AutoEdit);
        assert_eq!(session.provider, "openai");
        assert_eq!(session.model, "configured-model");
    }

    #[test]
    fn summarizes_changed_files_and_command_results() {
        let core = CoreService::in_memory().expect("core starts");
        let session = core
            .start_chat(ChatParams {
                workspace_root: "D:/work/summary".to_owned(),
                message: "Summarize work".to_owned(),
                mode: None,
                provider: None,
                model: None,
                title: None,
            })
            .expect("chat starts");
        core.create_restore_point(session.id, "src/a.rs", Some("old"), "new")
            .expect("first restore point saves");
        core.create_restore_point(session.id, "src/a.rs", Some("new"), "newer")
            .expect("second restore point saves");
        core.create_restore_point(session.id, "src/b.rs", None, "created")
            .expect("third restore point saves");
        for (id, success) in [("command_ok", true), ("command_failed", false)] {
            core.record_event(&SessionEvent::ToolEnd {
                session_id: session.id,
                tool_call: ToolCall {
                    id: id.to_owned(),
                    name: ToolName::RunCommand,
                    arguments: json!({ "command": "echo test" }),
                },
                success,
                summary: "command completed".to_owned(),
            })
            .expect("command event saves");
        }
        core.record_event(&SessionEvent::ToolEnd {
            session_id: session.id,
            tool_call: ToolCall {
                id: "read_file".to_owned(),
                name: ToolName::ReadFile,
                arguments: json!({ "path": "src/a.rs" }),
            },
            success: true,
            summary: "file read".to_owned(),
        })
        .expect("read event saves");

        assert_eq!(
            core.task_summary(session.id).expect("summary loads"),
            TaskSummary {
                changed_files: vec!["src/a.rs".to_owned(), "src/b.rs".to_owned()],
                commands_run: 2,
                commands_succeeded: 1,
                commands_failed: 1,
            }
        );
    }
    #[test]
    fn persists_chat_lifecycle() {
        let core = CoreService::in_memory().expect("core starts");
        let session = core
            .start_chat(ChatParams {
                workspace_root: "D:/work/demo".to_owned(),
                message: "Explain this project".to_owned(),
                mode: Some(Mode::Ask),
                provider: Some("openai".to_owned()),
                model: Some("gpt-4.1".to_owned()),
                title: None,
            })
            .expect("chat starts");

        assert_eq!(session.status, SessionStatus::Running);
        assert_eq!(core.messages(session.id).expect("messages").len(), 1);
        core.record_tool_message(session.id, r#"{"path":"src/lib.rs"}"#)
            .expect("tool message saves");

        let result = core
            .complete_chat(session.id, "XCoding is a local coding agent.")
            .expect("chat completes");
        assert_eq!(result.session.status, SessionStatus::Done);
        assert_eq!(
            result.message.expect("assistant message").role,
            MessageRole::Assistant
        );
        let messages = core.messages(session.id).expect("messages");
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[1].role, MessageRole::Tool);
        assert_eq!(messages[2].role, MessageRole::Assistant);
    }
    #[test]
    fn details_persist_events_restore_points_and_pending_actions() {
        let core = CoreService::in_memory().expect("core starts");
        let session = core
            .start_chat(ChatParams {
                workspace_root: "D:/work/demo".to_owned(),
                message: "Update the configuration".to_owned(),
                mode: Some(Mode::Ask),
                provider: Some("openai".to_owned()),
                model: Some("gpt-4.1".to_owned()),
                title: None,
            })
            .expect("chat starts");
        let action = core
            .create_pending_action(
                session.id,
                ToolCall {
                    id: "patch_1".to_owned(),
                    name: ToolName::ApplyPatch,
                    arguments: json!({ "path": "settings.txt" }),
                },
            )
            .expect("pending action saves");
        let restore_point = core
            .create_restore_point(session.id, "settings.txt", Some("old"), "new")
            .expect("restore point saves");
        core.record_event(&SessionEvent::Plan {
            session_id: session.id,
            steps: vec![PlanStep {
                id: "inspect".to_owned(),
                description: "Inspect settings".to_owned(),
            }],
        })
        .expect("event saves");
        core.pause_chat(session.id).expect("chat pauses");

        let detail = core.session_detail(session.id).expect("detail loads");
        assert_eq!(detail.messages.len(), 1);
        assert_eq!(detail.pending_actions, vec![action.clone()]);
        assert_eq!(detail.restore_points, vec![restore_point]);
        assert_eq!(detail.events.len(), 1);

        let cancelled = core
            .cancel_session(session.id)
            .expect("paused session cancels");
        assert_eq!(cancelled.status, SessionStatus::Cancelled);
        assert_eq!(
            core.session_detail(session.id)
                .expect("cancelled session detail loads")
                .pending_actions[0]
                .status,
            PendingActionStatus::Rejected
        );
        assert!(matches!(
            core.resolve_pending_action(session.id, action.id, true),
            Err(CoreError::InvalidInput(_))
        ));
        assert!(matches!(
            core.cancel_session(session.id),
            Err(CoreError::InvalidInput(_))
        ));
    }
}
