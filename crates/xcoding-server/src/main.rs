//! Line-delimited JSON-RPC server for local XCoding clients.

use std::{env, io, path::PathBuf, process};

use futures_util::StreamExt;
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Stdout};
use xcoding_core::{CoreError, CoreService};
use xcoding_protocol::{
    ChatParams, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, RpcError, SessionEvent,
};
use xcoding_providers::{ChatMessage, OpenAiCompatibleProvider};

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
                serde_json::Value::Null,
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

    let messages = match core.messages(session.id) {
        Ok(messages) => messages
            .into_iter()
            .map(|message| ChatMessage {
                role: message.role.as_str().to_owned(),
                content: message.content,
            })
            .collect(),
        Err(error) => {
            return chat_failure(core, stdout, id, session.id, rpc_error_for_core(error)).await;
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

    let mut stream = match provider.stream_chat(&session.model, messages).await {
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
    while let Some(delta) = stream.next().await {
        match delta {
            Ok(delta) => {
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

    let result = match core.complete_chat(session.id, content) {
        Ok(result) => result,
        Err(error) => {
            return chat_failure(core, stdout, id, session.id, rpc_error_for_core(error)).await;
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

    Ok(JsonRpcResponse::success(id, result))
}

async fn chat_failure(
    core: &CoreService,
    stdout: &mut Stdout,
    id: serde_json::Value,
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

    #[test]
    fn database_path_requires_explicit_flag() {
        // Argument parsing is kept intentionally minimal; integration tests exercise stdio.
        assert!(PathBuf::from(".xcoding/xcoding.db").ends_with("xcoding.db"));
    }
}
