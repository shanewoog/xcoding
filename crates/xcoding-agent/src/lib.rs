//! Shared read-only agent loop for XCoding clients.

use futures_util::StreamExt;
use serde_json::{Value, json};
use thiserror::Error;
use xcoding_context::ContextSnapshot;
use xcoding_core::{CoreError, CoreService};
use xcoding_protocol::{
    ChatParams, ChatResult, MessageRole, PlanStep, Session, SessionEvent, ToolCall,
};
use xcoding_providers::{
    ChatMessage, OpenAiCompatibleProvider, ProviderError, ProviderEvent, ProviderToolCall,
    ToolDefinition,
};
use xcoding_tools::{ToolError, ToolRegistry};

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
    #[error("model exceeded the read-only tool-call limit")]
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
            on_event(SessionEvent::Error {
                session_id: session.id,
                message: error.to_string(),
            });
        }
        result
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

        on_event(SessionEvent::Plan {
            session_id: session.id,
            steps: vec![
                PlanStep {
                    id: "inspect".to_owned(),
                    description: "Inspect relevant workspace files before answering.".to_owned(),
                },
                PlanStep {
                    id: "answer".to_owned(),
                    description: "Answer from the gathered repository evidence.".to_owned(),
                },
            ],
        });

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
                        on_event(SessionEvent::TextDelta {
                            session_id: session.id,
                            delta,
                        });
                    }
                    ProviderEvent::ToolCall(tool_call) => tool_calls.push(tool_call),
                }
            }

            if tool_calls.is_empty() {
                let result = self.core.complete_chat(session.id, content)?;
                on_event(SessionEvent::MessageCompleted {
                    session_id: session.id,
                    message: result.message.clone(),
                });
                return Ok(result);
            }

            messages.push(ChatMessage::assistant_tool_calls(tool_calls.clone()));
            for provider_call in tool_calls {
                let tool_call = protocol_tool_call(provider_call)?;
                on_event(SessionEvent::ToolStart {
                    session_id: session.id,
                    summary: format!("Running {}", tool_call.name.as_str()),
                    tool_call: tool_call.clone(),
                });

                match tools.execute(&session.mode, &tool_call) {
                    Ok(execution) => {
                        let output = serde_json::to_string(&execution.output).map_err(|error| {
                            AgentError::InvalidProviderToolCall(error.to_string())
                        })?;
                        self.core.record_tool_message(session.id, &output)?;
                        messages.push(ChatMessage::tool_result(&tool_call.id, output));
                        on_event(SessionEvent::ToolEnd {
                            session_id: session.id,
                            tool_call,
                            success: true,
                            summary: execution.summary,
                        });
                    }
                    Err(error) => {
                        let output = json!({ "error": error.to_string() }).to_string();
                        self.core.record_tool_message(session.id, &output)?;
                        messages.push(ChatMessage::tool_result(&tool_call.id, output));
                        on_event(SessionEvent::ToolEnd {
                            session_id: session.id,
                            tool_call,
                            success: false,
                            summary: error.to_string(),
                        });
                    }
                }
            }
        }

        Err(AgentError::ToolCallLimit)
    }
}

pub fn tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "list_dir".to_owned(),
            description: "List files and directories under a workspace-relative directory."
                .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workspace-relative directory; defaults to ." },
                    "max_entries": { "type": "integer", "minimum": 1, "maximum": 1000 }
                }
            }),
        },
        ToolDefinition {
            name: "read_file".to_owned(),
            description: "Read a bounded line range from a workspace-relative text file."
                .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "start_line": { "type": "integer", "minimum": 1 },
                    "end_line": { "type": "integer", "minimum": 1 }
                },
                "required": ["path"]
            }),
        },
        ToolDefinition {
            name: "search_code".to_owned(),
            description: "Search workspace text files for an exact string.".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "path": { "type": "string", "description": "Workspace-relative directory; defaults to ." },
                    "max_results": { "type": "integer", "minimum": 1, "maximum": 100 }
                },
                "required": ["query"]
            }),
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
    fn declares_only_read_only_tools() {
        let names = tool_definitions()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["list_dir", "read_file", "search_code"]);
    }
}
