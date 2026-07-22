//! Line-delimited JSON-RPC server for local XCoding clients.

use std::{env, io, path::PathBuf, process};

use serde::Serialize;
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines, Stdin, Stdout},
    sync::mpsc,
};
use xcoding_agent::{AgentError, AgentService};
use xcoding_core::{CoreError, CoreService};
use xcoding_protocol::{
    CancelSessionParams, CancelSessionResult, ChatParams, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse, ResolveActionParams, RollbackRestorePointParams, RpcError, SessionEvent,
};

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
                match handle_chat(&core, &mut stdout, &mut lines, request).await {
                    Ok(response) => response,
                    Err(_) => break,
                }
            }
            Ok(request) if request.method == "session.resolve" => {
                match handle_resolve(&core, &mut stdout, &mut lines, request).await {
                    Ok(response) => response,
                    Err(_) => break,
                }
            }
            Ok(request) if request.method == "session.rollback" => {
                match handle_rollback(&core, &mut stdout, request).await {
                    Ok(response) => response,
                    Err(_) => break,
                }
            }
            Ok(request) if request.method == "session.cancel" => {
                match handle_cancel(&core, &mut stdout, request).await {
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
    lines: &mut Lines<BufReader<Stdin>>,
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

    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let agent = AgentService::new(core);
    let chat = agent.chat(params, move |event| {
        let _ = event_tx.send(event);
    });
    tokio::pin!(chat);

    let outcome = drive_long_running(core, stdout, lines, &mut event_rx, &mut chat).await?;

    while let Ok(event) = event_rx.try_recv() {
        emit_event(stdout, event).await?;
    }

    match outcome {
        Ok(result) => Ok(JsonRpcResponse::success(id, result)),
        Err(error) => Ok(JsonRpcResponse::failure(id, rpc_error_for_agent(error))),
    }
}

async fn handle_resolve(
    core: &CoreService,
    stdout: &mut Stdout,
    lines: &mut Lines<BufReader<Stdin>>,
    request: JsonRpcRequest,
) -> io::Result<JsonRpcResponse> {
    let id = request.id.clone();
    if !request.is_valid_version() {
        return Ok(JsonRpcResponse::failure(
            id,
            RpcError::invalid_request("jsonrpc must be exactly \"2.0\""),
        ));
    }
    let params: ResolveActionParams = match serde_json::from_value(request.params) {
        Ok(params) => params,
        Err(error) => {
            return Ok(JsonRpcResponse::failure(
                id,
                RpcError::invalid_params(format!("invalid session.resolve params: {error}")),
            ));
        }
    };

    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let agent = AgentService::new(core);
    let resolve = agent.resolve(params, move |event| {
        let _ = event_tx.send(event);
    });
    tokio::pin!(resolve);

    let outcome = drive_long_running(core, stdout, lines, &mut event_rx, &mut resolve).await?;

    while let Ok(event) = event_rx.try_recv() {
        emit_event(stdout, event).await?;
    }

    match outcome {
        Ok(result) => Ok(JsonRpcResponse::success(id, result)),
        Err(error) => Ok(JsonRpcResponse::failure(id, rpc_error_for_agent(error))),
    }
}

async fn drive_long_running<T, E, F>(
    core: &CoreService,
    stdout: &mut Stdout,
    lines: &mut Lines<BufReader<Stdin>>,
    event_rx: &mut mpsc::UnboundedReceiver<SessionEvent>,
    work: &mut std::pin::Pin<&mut F>,
) -> io::Result<Result<T, E>>
where
    F: std::future::Future<Output = Result<T, E>>,
{
    loop {
        tokio::select! {
            event = event_rx.recv() => {
                if let Some(event) = event {
                    emit_event(stdout, event).await?;
                }
            }
            result = &mut *work => {
                return Ok(result);
            }
            line = lines.next_line() => {
                match line? {
                    Some(line) if !line.trim().is_empty() => {
                        handle_concurrent_request(core, stdout, &line).await?;
                    }
                    Some(_) => {}
                    None => {
                        // stdin closed; keep waiting for the in-flight work to finish.
                    }
                }
            }
        }
    }
}

async fn handle_concurrent_request(
    core: &CoreService,
    stdout: &mut Stdout,
    line: &str,
) -> io::Result<()> {
    let response = match serde_json::from_str::<JsonRpcRequest>(line) {
        Ok(request)
            if matches!(
                request.method.as_str(),
                "session.chat" | "session.resolve" | "session.rollback"
            ) =>
        {
            JsonRpcResponse::failure(
                request.id,
                RpcError::invalid_request(
                    "server is busy with another long-running session request",
                ),
            )
        }
        Ok(request) if request.method == "session.cancel" => {
            handle_cancel(core, stdout, request).await?
        }
        Ok(request) => core.dispatch(request),
        Err(error) => JsonRpcResponse::failure(
            Value::Null,
            RpcError::parse_error(format!("invalid JSON-RPC request: {error}")),
        ),
    };
    write_json_line(stdout, &response).await
}

async fn handle_rollback(
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
    let params: RollbackRestorePointParams = match serde_json::from_value(request.params) {
        Ok(params) => params,
        Err(error) => {
            return Ok(JsonRpcResponse::failure(
                id,
                RpcError::invalid_params(format!("invalid session.rollback params: {error}")),
            ));
        }
    };
    let mut events = Vec::new();
    let outcome = AgentService::new(core).rollback(params, |event| events.push(event));
    for event in events {
        emit_event(stdout, event).await?;
    }
    match outcome {
        Ok(result) => Ok(JsonRpcResponse::success(id, result)),
        Err(error) => Ok(JsonRpcResponse::failure(id, rpc_error_for_agent(error))),
    }
}

async fn handle_cancel(
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
    let params: CancelSessionParams = match serde_json::from_value(request.params) {
        Ok(params) => params,
        Err(error) => {
            return Ok(JsonRpcResponse::failure(
                id,
                RpcError::invalid_params(format!("invalid session.cancel params: {error}")),
            ));
        }
    };
    match core.cancel_session(params.session_id) {
        Ok(session) => {
            let event = SessionEvent::SessionCancelled {
                session_id: session.id,
                message: "Session cancelled by user".to_owned(),
            };
            let _ = core.record_event(&event);
            emit_event(stdout, event).await?;
            Ok(JsonRpcResponse::success(
                id,
                CancelSessionResult { session },
            ))
        }
        Err(error) => Ok(JsonRpcResponse::failure(id, rpc_error_for_core(error))),
    }
}

fn rpc_error_for_agent(error: AgentError) -> RpcError {
    match error {
        AgentError::Core(error) => rpc_error_for_core(error),
        AgentError::UnsupportedProvider(message) => RpcError::invalid_params(message),
        AgentError::Tool(error) => RpcError::invalid_params(error.to_string()),
        AgentError::Provider(error) => RpcError::provider_error(error.to_string()),
        AgentError::InvalidProviderToolCall(message) => RpcError::provider_error(message),
        AgentError::ToolCallLimit => RpcError::provider_error(error.to_string()),
        AgentError::Cancelled => RpcError::invalid_params("session cancelled".to_owned()),
    }
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
    fn database_path_default_is_not_implicit() {
        assert!(PathBuf::from(".xcoding/xcoding.db").ends_with("xcoding.db"));
    }
}
