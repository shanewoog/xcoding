//! Shared guarded coding-agent loop for XCoding clients.

use futures_util::StreamExt;
use serde_json::{Value, json};
use thiserror::Error;
use xcoding_context::ContextSnapshot;
use xcoding_core::{CoreError, CoreService};
use xcoding_policy::{PermissionDecision, evaluate};
use xcoding_protocol::{
    ChatParams, ChatResult, MessageRole, PlanStep, ResolveActionParams, ResolveActionResult,
    RollbackRestorePointParams, RollbackRestorePointResult, Session, SessionEvent, ToolCall,
    ToolName,
};
use xcoding_providers::{
    ChatMessage, OpenAiCompatibleProvider, ProviderError, ProviderEvent, ProviderToolCall,
    ToolDefinition,
};
use xcoding_tools::{ToolError, ToolExecution, ToolRegistry};

const MAX_TOOL_ROUNDS: usize = 8;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error(transparent)]
    Tool(#[from] ToolError),
    #[error("unsupported provider: {0}")]
    UnsupportedProvider(String),
    #[error("invalid tool call from provider: {0}")]
    InvalidProviderToolCall(String),
    #[error("model exceeded the tool-call limit")]
    ToolCallLimit,
}

pub struct AgentService<'a> {
    core: &'a CoreService,
}

impl<'a> AgentService<'a> {
    pub fn new(core: &'a CoreService) -> Self {
        Self { core }
    }

    pub async fn chat<F>(
        &self,
        params: ChatParams,
        mut on_event: F,
    ) -> Result<ChatResult, AgentError>
    where
        F: FnMut(SessionEvent),
    {
        let session = self.core.start_chat(params)?;
        let result = self.run_session(&session, &mut on_event).await;
        if let Err(error) = &result {
            let _ = self.core.fail_chat(session.id);
            self.emit(
                &mut on_event,
                SessionEvent::Error {
                    session_id: session.id,
                    message: error.to_string(),
                },
            );
        }
        result
    }

    pub async fn resolve<F>(
        &self,
        params: ResolveActionParams,
        mut on_event: F,
    ) -> Result<ResolveActionResult, AgentError>
    where
        F: FnMut(SessionEvent),
    {
        let action = self.core.resolve_pending_action(
            params.session_id,
            params.action_id,
            params.approved,
        )?;
        let session = self.core.resume_chat(params.session_id)?;
        let tools = ToolRegistry::new(&session.workspace_root)?;

        let output = if params.approved {
            self.emit(
                &mut on_event,
                SessionEvent::ToolStart {
                    session_id: session.id,
                    tool_call: action.tool_call.clone(),
                    summary: format!("Approved {}", action.tool_call.name.as_str()),
                },
            );
            self.execute_and_record(&session, &tools, &action.tool_call, &mut on_event)?
        } else {
            let output = json!({
                "tool_call_id": action.tool_call.id,
                "rejected": true,
                "reason": "The user rejected this action. Continue without making the change."
            })
            .to_string();
            self.core.record_tool_message(session.id, &output)?;
            self.emit(
                &mut on_event,
                SessionEvent::ToolEnd {
                    session_id: session.id,
                    tool_call: action.tool_call.clone(),
                    success: false,
                    summary: "Action rejected by user".to_owned(),
                },
            );
            output
        };

        let result = self.run_session(&session, &mut on_event).await?;
        let _ = output;
        Ok(ResolveActionResult {
            session: result.session,
            message: result.message,
        })
    }

    pub fn rollback<F>(
        &self,
        params: RollbackRestorePointParams,
        mut on_event: F,
    ) -> Result<RollbackRestorePointResult, AgentError>
    where
        F: FnMut(SessionEvent),
    {
        let session = self.core.session(params.session_id)?;
        let restore_point = self
            .core
            .restore_point(session.id, params.restore_point_id)?;
        let expected_text = restore_point.applied_text.as_deref().ok_or_else(|| {
            AgentError::Core(CoreError::InvalidInput(
                "restore point was created by an older XCoding version and cannot be safely rolled back"
                    .to_owned(),
            ))
        })?;
        let tools = ToolRegistry::new(&session.workspace_root)?;
        let execution = tools.rollback_patch(
            &restore_point.path,
            expected_text,
            restore_point.original_text.as_deref(),
        )?;
        self.core.record_tool_message(
            session.id,
            json!({
                "restore_point_id": restore_point.id,
                "path": restore_point.path,
                "rolled_back": true,
                "output": execution.output,
            })
            .to_string(),
        )?;
        self.emit(
            &mut on_event,
            SessionEvent::RestorePointRolledBack {
                session_id: session.id,
                restore_point: restore_point.clone(),
                summary: execution.summary,
            },
        );
        Ok(RollbackRestorePointResult {
            session: self.core.session(session.id)?,
            restore_point,
        })
    }

    async fn run_session<F>(
        &self,
        session: &Session,
        on_event: &mut F,
    ) -> Result<ChatResult, AgentError>
    where
        F: FnMut(SessionEvent),
    {
        if session.provider != "openai" {
            return Err(AgentError::UnsupportedProvider(session.provider.clone()));
        }

        let tools = ToolRegistry::new(&session.workspace_root)?;
        let provider = OpenAiCompatibleProvider::from_environment()?;
        let context = ContextSnapshot::load(tools.workspace_root());
        let mut messages = vec![ChatMessage::system(context.system_prompt())];
        messages.extend(self.core.messages(session.id)?.into_iter().map(
            |message| match message.role {
                MessageRole::System => ChatMessage::system(message.content),
                MessageRole::User => ChatMessage::user(message.content),
                MessageRole::Assistant => ChatMessage::assistant(message.content),
                MessageRole::Tool => ChatMessage::assistant(format!(
                    "Previously recorded tool output: {}",
                    message.content
                )),
            },
        ));

        self.emit(
            on_event,
            SessionEvent::Plan {
                session_id: session.id,
                steps: vec![
                    PlanStep {
                        id: "inspect".to_owned(),
                        description: "Inspect relevant workspace files before changing anything."
                            .to_owned(),
                    },
                    PlanStep {
                        id: "change".to_owned(),
                        description: "Propose a minimal patch and wait for required approval."
                            .to_owned(),
                    },
                    PlanStep {
                        id: "verify".to_owned(),
                        description: "Run approved verification commands and report the result."
                            .to_owned(),
                    },
                ],
            },
        );

        let definitions = tool_definitions();
        for _ in 0..MAX_TOOL_ROUNDS {
            let mut stream = provider
                .stream_chat(&session.model, messages.clone(), &definitions)
                .await?;
            let mut content = String::new();
            let mut tool_calls = Vec::new();

            while let Some(event) = stream.next().await {
                match event? {
                    ProviderEvent::TextDelta(delta) => {
                        content.push_str(&delta);
                        self.emit(
                            on_event,
                            SessionEvent::TextDelta {
                                session_id: session.id,
                                delta,
                            },
                        );
                    }
                    ProviderEvent::ToolCall(tool_call) => tool_calls.push(tool_call),
                }
            }

            if tool_calls.is_empty() {
                let result = self.core.complete_chat(session.id, content)?;
                self.emit(
                    on_event,
                    SessionEvent::MessageCompleted {
                        session_id: session.id,
                        message: result
                            .message
                            .clone()
                            .expect("completed chat has a message"),
                    },
                );
                return Ok(result);
            }

            messages.push(ChatMessage::assistant_tool_calls(tool_calls.clone()));
            for provider_call in tool_calls {
                let tool_call = protocol_tool_call(provider_call)?;
                self.emit(
                    on_event,
                    SessionEvent::ToolStart {
                        session_id: session.id,
                        summary: format!("Running {}", tool_call.name.as_str()),
                        tool_call: tool_call.clone(),
                    },
                );

                let (kind, high_risk) = tools.permission_for(&tool_call)?;
                match evaluate(&session.mode, kind, high_risk) {
                    PermissionDecision::Allow => {
                        let output =
                            self.execute_and_record(session, &tools, &tool_call, on_event)?;
                        messages.push(ChatMessage::tool_result(&tool_call.id, output));
                    }
                    PermissionDecision::AskUser => {
                        if tool_call.name == ToolName::ApplyPatch {
                            match tools.patch_preview(&tool_call) {
                                Ok(preview) => self.emit(
                                    on_event,
                                    SessionEvent::PatchPreview {
                                        session_id: session.id,
                                        preview,
                                    },
                                ),
                                Err(error) => {
                                    let output = self
                                        .record_tool_error(session, &tool_call, error, on_event)?;
                                    messages.push(ChatMessage::tool_result(&tool_call.id, output));
                                    continue;
                                }
                            }
                        }
                        let action = self
                            .core
                            .create_pending_action(session.id, tool_call.clone())?;
                        let paused = self.core.pause_chat(session.id)?;
                        self.emit(
                            on_event,
                            SessionEvent::ApprovalRequested {
                                session_id: session.id,
                                action,
                                summary: approval_summary(&tool_call),
                            },
                        );
                        return Ok(ChatResult {
                            session: paused,
                            message: None,
                        });
                    }
                    PermissionDecision::Deny => {
                        let output = self.record_tool_error(
                            session,
                            &tool_call,
                            ToolError::PermissionDenied,
                            on_event,
                        )?;
                        messages.push(ChatMessage::tool_result(&tool_call.id, output));
                    }
                }
            }
        }

        Err(AgentError::ToolCallLimit)
    }

    fn emit<F>(&self, on_event: &mut F, event: SessionEvent)
    where
        F: FnMut(SessionEvent),
    {
        let _ = self.core.record_event(&event);
        on_event(event);
    }

    fn execute_and_record<F>(
        &self,
        session: &Session,
        tools: &ToolRegistry,
        tool_call: &ToolCall,
        on_event: &mut F,
    ) -> Result<String, AgentError>
    where
        F: FnMut(SessionEvent),
    {
        let execution = (|| -> Result<ToolExecution, ToolError> {
            if tool_call.name == ToolName::ApplyPatch {
                let preview = tools.patch_preview(tool_call)?;
                self.core
                    .create_restore_point(
                        session.id,
                        &preview.path,
                        preview.file_existed.then_some(preview.old_text.as_str()),
                        &preview.new_text,
                    )
                    .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
            }
            tools.execute_authorized(tool_call)
        })();

        match execution {
            Ok(execution) => {
                let output = serde_json::to_string(&execution.output)
                    .map_err(|error| AgentError::InvalidProviderToolCall(error.to_string()))?;
                self.core.record_tool_message(session.id, &output)?;
                self.emit(
                    on_event,
                    SessionEvent::ToolEnd {
                        session_id: session.id,
                        tool_call: tool_call.clone(),
                        success: true,
                        summary: execution.summary,
                    },
                );
                Ok(output)
            }
            Err(error) => self.record_tool_error(session, tool_call, error, on_event),
        }
    }

    fn record_tool_error<F>(
        &self,
        session: &Session,
        tool_call: &ToolCall,
        error: ToolError,
        on_event: &mut F,
    ) -> Result<String, AgentError>
    where
        F: FnMut(SessionEvent),
    {
        let output = json!({ "error": error.to_string() }).to_string();
        self.core.record_tool_message(session.id, &output)?;
        self.emit(
            on_event,
            SessionEvent::ToolEnd {
                session_id: session.id,
                tool_call: tool_call.clone(),
                success: false,
                summary: error.to_string(),
            },
        );
        Ok(output)
    }
}

fn approval_summary(tool_call: &ToolCall) -> String {
    match tool_call.name {
        ToolName::ApplyPatch => "Review and approve the proposed patch".to_owned(),
        ToolName::RunCommand => "Review and approve the requested command".to_owned(),
        _ => format!("Review {}", tool_call.name.as_str()),
    }
}

pub fn tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "list_dir".to_owned(),
            description: "List files and directories under a workspace-relative directory.".to_owned(),
            parameters: json!({ "type": "object", "properties": { "path": { "type": "string", "description": "Workspace-relative directory; defaults to ." }, "max_entries": { "type": "integer", "minimum": 1, "maximum": 1000 } } }),
        },
        ToolDefinition {
            name: "read_file".to_owned(),
            description: "Read a bounded line range from a workspace-relative text file.".to_owned(),
            parameters: json!({ "type": "object", "properties": { "path": { "type": "string" }, "start_line": { "type": "integer", "minimum": 1 }, "end_line": { "type": "integer", "minimum": 1 } }, "required": ["path"] }),
        },
        ToolDefinition {
            name: "search_code".to_owned(),
            description: "Search workspace text files for an exact string.".to_owned(),
            parameters: json!({ "type": "object", "properties": { "query": { "type": "string" }, "path": { "type": "string", "description": "Workspace-relative directory; defaults to ." }, "max_results": { "type": "integer", "minimum": 1, "maximum": 100 } }, "required": ["query"] }),
        },
        ToolDefinition {
            name: "apply_patch".to_owned(),
            description: "Atomically replace a workspace-relative text file only when old_text exactly matches its current content. Use an empty old_text to create a new file.".to_owned(),
            parameters: json!({ "type": "object", "properties": { "path": { "type": "string" }, "old_text": { "type": "string" }, "new_text": { "type": "string" } }, "required": ["path", "old_text", "new_text"] }),
        },
        ToolDefinition {
            name: "run_command".to_owned(),
            description: "Run an approved executable with an argument vector in the workspace root. Never use a shell.".to_owned(),
            parameters: json!({ "type": "object", "properties": { "executable": { "type": "string" }, "args": { "type": "array", "items": { "type": "string" } } }, "required": ["executable"] }),
        },
    ]
}

fn protocol_tool_call(provider_call: ProviderToolCall) -> Result<ToolCall, AgentError> {
    let name =
        serde_json::from_value(Value::String(provider_call.function.name)).map_err(|error| {
            AgentError::InvalidProviderToolCall(format!(
                "unsupported tool requested by provider: {error}"
            ))
        })?;
    let arguments = serde_json::from_str(&provider_call.function.arguments).map_err(|error| {
        AgentError::InvalidProviderToolCall(format!(
            "invalid tool arguments from provider: {error}"
        ))
    })?;
    Ok(ToolCall {
        id: provider_call.id,
        name,
        arguments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declares_guarded_write_tools() {
        let names = tool_definitions()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "list_dir",
                "read_file",
                "search_code",
                "apply_patch",
                "run_command"
            ]
        );
    }
}
