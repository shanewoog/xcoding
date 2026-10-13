use std::sync::Mutex;

use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use xcoding_agent::{RelayError, open_active_provider_responses};
use xcoding_protocol::UserConfig;
use xcoding_providers::RawResponsesStream;

const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

pub struct RelayState {
    handle: Mutex<Option<RelayHandle>>,
}

impl Default for RelayState {
    fn default() -> Self {
        Self {
            handle: Mutex::new(None),
        }
    }
}

pub struct RelayHandle {
    task: tauri::async_runtime::JoinHandle<()>,
}

impl RelayHandle {
    pub fn stop(self) {
        self.task.abort();
    }
}

pub fn stop(state: &RelayState) {
    if let Ok(mut slot) = state.handle.lock() {
        if let Some(handle) = slot.take() {
            handle.stop();
        }
    }
}

pub fn restart(state: &RelayState, config: UserConfig) -> Result<(), String> {
    stop(state);
    if !config.local_relay_enabled {
        return Ok(());
    }
    let key = config
        .local_relay_api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .ok_or_else(|| "local relay is enabled but its local API key is empty".to_owned())?;
    let listener = std::net::TcpListener::bind(("127.0.0.1", config.local_relay_port))
        .map_err(|error| format!("failed to bind local relay on 127.0.0.1:{}: {error}", config.local_relay_port))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("failed to configure local relay listener: {error}"))?;
    let bind_key = key.to_owned();
    let task = tauri::async_runtime::spawn(async move {
        match TcpListener::from_std(listener) {
            Ok(listener) => accept_loop(listener, config, bind_key).await,
            Err(error) => eprintln!("failed to start local relay listener: {error}"),
        }
    });
    if let Ok(mut slot) = state.handle.lock() {
        *slot = Some(RelayHandle { task });
    } else {
        task.abort();
        return Err("failed to store local relay state".to_owned());
    }
    Ok(())
}

async fn accept_loop(listener: TcpListener, config: UserConfig, relay_key: String) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            break;
        };
        let config = config.clone();
        let relay_key = relay_key.clone();
        tauri::async_runtime::spawn(async move {
            let _ = handle_connection(stream, &config, &relay_key).await;
        });
    }
}

#[derive(Default)]
struct RequestHead {
    method: String,
    path: String,
    authorization: Option<String>,
    content_type: Option<String>,
    content_length: Option<usize>,
    transfer_encoding: Option<String>,
}

async fn handle_connection(
    stream: TcpStream,
    config: &UserConfig,
    relay_key: &str,
) -> Result<(), std::io::Error> {
    let mut reader = BufReader::new(stream);
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while head.len() <= MAX_HEADER_BYTES {
        if reader.read_exact(&mut byte).await.is_err() {
            return Ok(());
        }
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    if !head.ends_with(b"\r\n\r\n") {
        return write_error(reader.into_inner(), 431, "request headers too large", false).await;
    }
    let request = match parse_request_head(&head[..head.len() - 4]) {
        Ok(request) => request,
        Err((status, message)) => {
            return write_error(reader.into_inner(), status, message, false).await;
        }
    };
    if request.method != "POST" {
        return write_error(reader.into_inner(), 405, "only POST is supported", false).await;
    }
    if request.path != "/v1/responses" && request.path != "/responses" {
        return write_error(reader.into_inner(), 404, "not found", false).await;
    }
    if request
        .transfer_encoding
        .as_deref()
        .is_some_and(|value| !value.eq_ignore_ascii_case("identity"))
    {
        return write_error(reader.into_inner(), 501, "chunked requests are not supported", false).await;
    }
    let Some(content_length) = request.content_length else {
        return write_error(reader.into_inner(), 411, "Content-Length is required", false).await;
    };
    if content_length > MAX_BODY_BYTES {
        return write_error(reader.into_inner(), 413, "request body too large", false).await;
    }
    let Some(authorization) = request.authorization.as_deref() else {
        return write_error(reader.into_inner(), 401, "invalid local relay API key", false).await;
    };
    if !valid_bearer(authorization, relay_key) {
        return write_error(reader.into_inner(), 401, "invalid local relay API key", false).await;
    }

    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).await?;
    let stream = match open_active_provider_responses(
        config,
        &body,
        request.content_type.as_deref(),
    )
    .await
    {
        Ok(stream) => stream,
        Err(RelayError::InvalidRequest(message)) => {
            return write_error(reader.into_inner(), 400, &message, true).await;
        }
        Err(RelayError::Unavailable(message)) => {
            return write_error(reader.into_inner(), 503, &message, true).await;
        }
    };
    write_upstream_response(reader.into_inner(), stream).await
}

fn parse_request_head(bytes: &[u8]) -> Result<RequestHead, (u16, &'static str)> {
    let text = std::str::from_utf8(bytes).map_err(|_| (400, "invalid request headers"))?;
    let mut lines = text.split("\r\n");
    let request_line = lines.next().ok_or((400, "invalid request line"))?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or((400, "invalid request line"))?;
    let path = parts.next().ok_or((400, "invalid request line"))?;
    let version = parts.next().ok_or((400, "invalid HTTP version"))?;
    if parts.next().is_some() || version != "HTTP/1.1" {
        return Err((400, "HTTP/1.1 is required"));
    }
    let mut request = RequestHead {
        method: method.to_owned(),
        path: path.to_owned(),
        ..Default::default()
    };
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            return Err((400, "invalid request header"));
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "authorization" => request.authorization = Some(value.to_owned()),
            "content-type" => request.content_type = Some(value.to_owned()),
            "content-length" => {
                let length = value.parse::<usize>().map_err(|_| (400, "invalid Content-Length"))?;
                if request.content_length.replace(length).is_some() {
                    return Err((400, "duplicate Content-Length"));
                }
            }
            "transfer-encoding" => request.transfer_encoding = Some(value.to_owned()),
            _ => {}
        }
    }
    Ok(request)
}

fn valid_bearer(value: &str, expected: &str) -> bool {
    let mut parts = value.split_whitespace();
    matches!((parts.next(), parts.next(), parts.next()), (Some(scheme), Some(key), None) if scheme.eq_ignore_ascii_case("Bearer") && key == expected)
}

async fn write_upstream_response(
    mut stream: TcpStream,
    response: RawResponsesStream,
) -> Result<(), std::io::Error> {
    let status = response.status();
    let reason = status.canonical_reason().unwrap_or("Upstream Response");
    let mut headers = format!(
        "HTTP/1.1 {} {}\r\nConnection: close\r\n",
        status.as_u16(), reason
    );
    if let Some(content_type) = response.content_type() {
        headers.push_str("Content-Type: ");
        headers.push_str(&content_type);
        headers.push_str("\r\n");
    }
    if let Some(retry_after) = response.retry_after_header() {
        headers.push_str("Retry-After: ");
        headers.push_str(&retry_after);
        headers.push_str("\r\n");
    }
    headers.push_str("\r\n");
    stream.write_all(headers.as_bytes()).await?;
    let body = response.into_byte_stream();
    tokio::pin!(body);
    while let Some(chunk) = body.next().await {
        match chunk {
            Ok(chunk) => stream.write_all(&chunk).await?,
            Err(_) => break,
        }
    }
    stream.shutdown().await
}

async fn write_error(
    mut stream: TcpStream,
    status: u16,
    message: &str,
    json_body: bool,
) -> Result<(), std::io::Error> {
    let body = if json_body {
        serde_json::json!({ "error": { "message": message } }).to_string()
    } else {
        message.to_owned()
    };
    let content_type = if json_body { "application/json" } else { "text/plain" };
    let reason = match status {
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        411 => "Length Required",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let headers = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(headers.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    stream.shutdown().await
}

#[cfg(test)]
mod tests {
    use std::{
        net::{SocketAddr, TcpListener as StdTcpListener, TcpStream as StdTcpStream},
        thread,
        time::{Duration, Instant},
    };

    use xcoding_protocol::UserConfig;

    use super::{MAX_BODY_BYTES, RelayState, parse_request_head, restart, stop, valid_bearer};

    #[test]
    fn restart_from_sync_context_starts_the_listener() {
        let probe = StdTcpListener::bind(("127.0.0.1", 0)).expect("probe port binds");
        let port = probe
            .local_addr()
            .expect("probe address is available")
            .port();
        drop(probe);

        let state = RelayState::default();
        let config = UserConfig {
            local_relay_enabled: true,
            local_relay_port: port,
            local_relay_api_key: Some("test-key".to_owned()),
            ..UserConfig::default()
        };
        restart(&state, config).expect("relay starts without a Tokio context on this thread");

        let address = SocketAddr::from(([127, 0, 0, 1], port));
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if StdTcpStream::connect_timeout(&address, Duration::from_millis(100)).is_ok() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "relay did not begin accepting connections"
            );
            thread::sleep(Duration::from_millis(10));
        }

        stop(&state);
    }

    #[test]
    fn accepts_supported_path_and_headers() {
        let request = parse_request_head(
            b"POST /v1/responses HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer local\r\nContent-Type: application/json\r\nContent-Length: 2",
        )
        .expect("request parses");
        assert_eq!(request.path, "/v1/responses");
        assert_eq!(request.content_length, Some(2));
        assert_eq!(request.content_type.as_deref(), Some("application/json"));
        assert!(valid_bearer("bearer local", "local"));
    }

    #[test]
    fn rejects_chunked_and_oversized_bodies_at_the_boundary() {
        let request = parse_request_head(
            b"POST /responses HTTP/1.1\r\nTransfer-Encoding: chunked\r\nContent-Length: 1",
        )
        .expect("request parses");
        assert_eq!(request.transfer_encoding.as_deref(), Some("chunked"));
        assert!(MAX_BODY_BYTES > 0);
        assert!(!valid_bearer("Bearer local extra", "local"));
    }
}
