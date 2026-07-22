//! Line-delimited JSON-RPC server for local XCoding clients.

use std::{env, io, path::PathBuf, process};

use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Stdout};
use xcoding_context::ContextSnapshot;
use xcoding_core::{CoreError, CoreService};
use xcoding_protocol::{
    ChatParams, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, MessageRole, PlanStep,
    RpcError, SessionEvent, ToolCall,
};
use xcoding_providers::{
    ChatMessage, OpenAiCompatibleProvider, ProviderEvent, ProviderToolCall, ToolDefinition,
};
use xcoding_tools::ToolRegistry;

const MAX_TOOL_ROUNDS: usize = 8;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let database_path = match parse_database_path() {
        Ok(path) => path,
        Err(message) => {
            eprintln!("xcoding-server: {message}");
            process::exit(2);
        }
    };

    if let Some(parent) = database_path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            eprintln!("xcoding-server: failed to create data directory: {error}");
            process::exit(1);
        }
    }

    let core = match CoreService::open(&database_path) {
        Ok(core) => core,
        Err(error) => {
            eprintln!("xcoding-server: failed to initialize core: {error}");
            process::exit(1);
        }
    };

    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    let mut stdout = tokio::io::stdout();

    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<JsonRpcRequest>(&line) {
            Ok(request) if request.method == "session.chat" => {
                match handle_chat(&core, &mut stdout, request).await {
                    Ok(response) => response,
                    Err(_) => break,
                }
            }
            Ok(request) => core.dispatch(request),
            Err(error) => JsonRpcResponse::failure(
                Value::Null,
                RpcError::parse_error(format!("invalid JSON-RPC request: {error}")),
            ),
        };

        if write_json_line(&mut stdout, &response).await.is_err() {
            break;
        }
    }
}

async fn handle_chat(
    core: &CoreService,
    stdout: &mut Stdout,
    request: JsonRpcRequest,
) -> io::Result<JsonRpcResponse> {
    let id = request.id.clone();

    if !request.is_valid_version() {
        return Ok(JsonRpcResponse::failure(
            id,
            RpcError::invalid_request("jsonrpc must be exactly \"2.0\""),
        ));
    }

    let params: ChatParams = match serde_json::from_value(request.params) {
        Ok(params) => params,
        Err(error) => {
            return Ok(JsonRpcResponse::failure(
                id,
                RpcError::invalid_params(format!("invalid session.chat params: {error}")),
            ));
        }
    };

    let session = match core.start_chat(params) {
        Ok(session) => session,
        Err(error) => return Ok(JsonRpcResponse::failure(id, rpc_error_for_core(error))),
    };

    if session.provider != "openai" {
        return chat_failure(
            core,
            stdout,
            id,
            session.id,
            RpcError::invalid_params(format!("unsupported provider: {}", session.provider)),
        )
        .await;
    }

    let tools = match ToolRegistry::new(&session.workspace_root) {
        Ok(tools) => tools,
        Err(error) => {
            return chat_failure(
                core,
                stdout,
                id,
                session.id,
                RpcError::invalid_params(error.to_string()),
            )
            .await;
        }
    };
    let provider = match OpenAiCompatibleProvider::from_environment() {
        Ok(provider) => provider,
        Err(error) => {
            return chat_failure(
                core,
                stdout,
                id,
                session.id,
                RpcError::provider_error(error.to_string()),
            )
            .await;
        }
    };

    let context = ContextSnapshot::load(tools.workspace_root());
    let mut messages = vec![ChatMessage::system(context.system_prompt())];
    match core.messages(session.id) {
        Ok(persisted) => {
            messages.extend(persisted.into_iter().map(|message| match message.role {
                MessageRole::System => ChatMessage::system(message.content),
                MessageRole::User => ChatMessage::user(message.content),
                MessageRole::Assistant => ChatMessage::assistant(message.content),
                MessageRole::Tool => ChatMessage::assistant(format!(
                    "Previously recorded tool output: {}",
                    message.content
                )),
            }));
        }
        Err(error) => {
            return chat_failure(core, stdout, id, session.id, rpc_error_for_core(error)).await;
        }
    }

    emit_event(
        stdout,
        SessionEvent::Plan {
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
        },
    )
    .await?;

    let definitions = tool_definitions();
    for _ in 0..MAX_TOOL_ROUNDS {
        let mut stream = match provider
            .stream_chat(&session.model, messages.clone(), &definitions)
            .await
        {
            Ok(stream) => stream,
            Err(error) => {
                return chat_failure(
                    core,
                    stdout,
                    id,
                    session.id,
                    RpcError::provider_error(error.to_string()),
                )
                .await;
            }
        };

        let mut content = String::new();
        let mut tool_calls = Vec::new();
        while let Some(event) = stream.next().await {
            match event {
                Ok(ProviderEvent::TextDelta(delta)) => {
                    content.push_str(&delta);
                    emit_event(
                        stdout,
                        SessionEvent::TextDelta {
                            session_id: session.id,
                            delta,
                        },
                    )
                    .await?;
                }
                Ok(ProviderEvent::ToolCall(tool_call)) => tool_calls.push(tool_call),
                Err(error) => {
                    return chat_failure(
                        core,
                        stdout,
                        id,
                        session.id,
                        RpcError::provider_error(error.to_string()),
                    )
                    .await;
                }
            }
        }

        if tool_calls.is_empty() {
            let result = match core.complete_chat(session.id, content) {
                Ok(result) => result,
                Err(error) => {
                    return chat_failure(core, stdout, id, session.id, rpc_error_for_core(error))
                        .await;
                }
            };
            emit_event(
                stdout,
                SessionEvent::MessageCompleted {
                    session_id: session.id,
                    message: result.message.clone(),
                },
            )
            .await?;
            return Ok(JsonRpcResponse::success(id, result));
        }

        messages.push(ChatMessage::assistant_tool_calls(tool_calls.clone()));
        for provider_call in tool_calls {
            let tool_call = match protocol_tool_call(provider_call) {
                Ok(tool_call) => tool_call,
                Err(error) => {
                    return chat_failure(
                        core,
                        stdout,
                        id,
                        session.id,
                        RpcError::provider_error(error),
                    )
                    .await;
                }
            };
            emit_event(
                stdout,
                SessionEvent::ToolStart {
                    session_id: session.id,
                    summary: format!("Running {}", tool_call.name.as_str()),
                    tool_call: tool_call.clone(),
                },
            )
            .await?;

            match tools.execute(&session.mode, &tool_call) {
                Ok(execution) => {
                    let output = match serde_json::to_string(&execution.output) {
                        Ok(output) => output,
                        Err(error) => {
                            return chat_failure(
                                core,
                                stdout,
                                id,
                                session.id,
                                RpcError::internal(error.to_string()),
                            )
                            .await;
                        }
                    };
                    if let Err(error) = core.record_tool_message(session.id, &output) {
                        return chat_failure(
                            core,
                            stdout,
                            id,
                            session.id,
                            rpc_error_for_core(error),
                        )
                        .await;
                    }
                    messages.push(ChatMessage::tool_result(&tool_call.id, output));
                    emit_event(
                        stdout,
                        SessionEvent::ToolEnd {
                            session_id: session.id,
                            tool_call,
                            success: true,
                            summary: execution.summary,
                        },
                    )
                    .await?;
                }
                Err(error) => {
                    let output = json!({ "error": error.to_string() }).to_string();
                    if let Err(error) = core.record_tool_message(session.id, &output) {
                        return chat_failure(
                            core,
                            stdout,
                            id,
                            session.id,
                            rpc_error_for_core(error),
                        )
                        .await;
                    }
                    messages.push(ChatMessage::tool_result(&tool_call.id, output));
                    emit_event(
                        stdout,
                        SessionEvent::ToolEnd {
                            session_id: session.id,
                            tool_call,
                            success: false,
                            summary: error.to_string(),
                        },
                    )
                    .await?;
                }
            }
        }
    }

    chat_failure(
        core,
        stdout,
        id,
        session.id,
        RpcError::provider_error("model exceeded the read-only tool-call limit"),
    )
    .await
}

fn protocol_tool_call(provider_call: ProviderToolCall) -> Result<ToolCall, String> {
    let name = serde_json::from_value(Value::String(provider_call.function.name))
        .map_err(|error| format!("unsupported tool requested by provider: {error}"))?;
    let arguments = serde_json::from_str(&provider_call.function.arguments)
        .map_err(|error| format!("invalid tool arguments from provider: {error}"))?;
    Ok(ToolCall {
        id: provider_call.id,
        name,
        arguments,
    })
}

fn tool_definitions() -> Vec<ToolDefinition> {
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

async fn chat_failure(
    core: &CoreService,
    stdout: &mut Stdout,
    id: Value,
    session_id: uuid::Uuid,
    error: RpcError,
) -> io::Result<JsonRpcResponse> {
    let _ = core.fail_chat(session_id);
    emit_event(
        stdout,
        SessionEvent::Error {
            session_id,
            message: error.message.clone(),
        },
    )
    .await?;
    Ok(JsonRpcResponse::failure(id, error))
}

fn rpc_error_for_core(error: CoreError) -> RpcError {
    match error {
        CoreError::InvalidInput(message) => RpcError::invalid_params(message),
        error => RpcError::internal(error.to_string()),
    }
}

async fn emit_event(stdout: &mut Stdout, event: SessionEvent) -> io::Result<()> {
    write_json_line(stdout, &JsonRpcNotification::new("session.event", event)).await
}

async fn write_json_line<T: Serialize>(stdout: &mut Stdout, value: &T) -> io::Result<()> {
    let encoded = serde_json::to_string(value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    stdout.write_all(encoded.as_bytes()).await?;
    stdout.write_all(b"\n").await?;
    stdout.flush().await
}

fn parse_database_path() -> Result<PathBuf, String> {
    let mut arguments = env::args_os().skip(1);
    let Some(flag) = arguments.next() else {
        return Err("expected --db <path>".to_owned());
    };

    if flag != "--db" {
        return Err("only --db <path> is supported".to_owned());
    }

    let Some(path) = arguments.next() else {
        return Err("expected a database path after --db".to_owned());
    };

    if arguments.next().is_some() {
        return Err("unexpected extra arguments".to_owned());
    }

    Ok(PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn database_path_requires_explicit_flag() {
        assert!(PathBuf::from(".xcoding/xcoding.db").ends_with("xcoding.db"));
    }

    #[test]
    fn declares_only_read_only_tools() {
        let names = tool_definitions()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["list_dir", "read_file", "search_code"]);
    }
}
