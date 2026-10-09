use futures_util::StreamExt;
use serde_json::json;
use std::{
    io::{Read, Write},
    net::TcpListener,
    process::Command,
    thread,
    time::Duration,
};
use xcoding_protocol::{HttpProxyMode, ProviderWireApi, UserConfig};
use xcoding_providers::request_logs::{RequestLogQuery, query_request_logs, request_log_detail};
use xcoding_providers::{ChatMessage, OpenAiCompatibleProvider, save_user_config, user_config_dir};

fn mock_response(status: &str, body: &str) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let worker = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let count = socket.read(&mut buffer).unwrap();
            assert!(count > 0);
            request.extend_from_slice(&buffer[..count]);
            if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..end]);
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|length| length.trim().parse().unwrap())
                    })
                    .unwrap();
                if request.len() >= end + 4 + length {
                    break;
                }
            }
        }
        socket.write_all(response.as_bytes()).unwrap();
        String::from_utf8(request).unwrap()
    });
    (endpoint, worker)
}

#[tokio::test]
async fn records_wire_requests_and_responses_without_changing_them() {
    if std::env::var_os("XCODING_REQUEST_LOG_TEST").is_none() {
        let home =
            std::env::temp_dir().join(format!("xcoding-log-http-test-{}", uuid::Uuid::new_v4()));
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "records_wire_requests_and_responses_without_changing_them",
                "--nocapture",
            ])
            .env("XCODING_REQUEST_LOG_TEST", "1")
            .env("USERPROFILE", &home)
            .env("HOME", &home)
            .env("XCODING_HTTP_PROXY", "off")
            .output()
            .unwrap();
        if home.exists() {
            std::fs::remove_dir_all(&home).unwrap();
        }
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let legacy: UserConfig = serde_json::from_value(json!({})).unwrap();
    assert!(!legacy.record_model_requests);
    let mut config = UserConfig {
        http_proxy_mode: HttpProxyMode::Off,
        ..UserConfig::default()
    };
    save_user_config(&config).unwrap();
    let chat = "data: {\"model\":\"actual-model\",\"choices\":[{\"delta\":{\"content\":\"hello 日志\"}}]}\n\ndata: [DONE]\n\n";
    let responses = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello 日志\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{}}\n\n";
    let anthropic = "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hello 日志\"}}\n\ndata: {\"type\":\"message_stop\"}\n\n";
    let (endpoint, worker) = mock_response("200 OK", chat);
    let provider = OpenAiCompatibleProvider::new("test-secret-key", endpoint);
    let mut stream = provider
        .stream_chat("disabled", vec![ChatMessage::user("prompt")], &[], None)
        .await
        .unwrap();
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
    worker.join().unwrap();
    assert!(!user_config_dir().join("model-request-logs.db").exists());
    config.record_model_requests = true;
    save_user_config(&config).unwrap();
    assert!(xcoding_providers::load_user_config().record_model_requests);
    for (wire_api, response, suffix) in [
        (ProviderWireApi::ChatCompletions, chat, "/chat/completions"),
        (ProviderWireApi::Responses, responses, "/responses"),
        (ProviderWireApi::AnthropicMessages, anthropic, "/messages"),
    ] {
        let (endpoint, worker) = mock_response("200 OK", response);
        let provider =
            OpenAiCompatibleProvider::with_wire_api("test-secret-key", endpoint, wire_api);
        let mut stream = provider
            .stream_chat(
                "test-model",
                vec![ChatMessage::user("original prompt")],
                &[],
                None,
            )
            .await
            .unwrap();
        let mut text = String::new();
        while let Some(event) = stream.next().await {
            if let xcoding_providers::ProviderEvent::TextDelta(delta) = event.unwrap() {
                text.push_str(&delta);
            }
        }
        assert_eq!(text, "hello 日志");
        let wire_request = worker.join().unwrap();
        assert!(wire_request.contains("test-secret-key"));
        let page = query_request_logs(&RequestLogQuery::default()).unwrap();
        let detail = request_log_detail(&page.items[0].id).unwrap();
        assert_eq!(detail.status, "success");
        assert_eq!(detail.http_status, Some(200));
        assert!(detail.endpoint.ends_with(suffix));
        assert_eq!(detail.response_body, response);
        let wire_body: serde_json::Value =
            serde_json::from_str(wire_request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&detail.request_body).unwrap(),
            wire_body
        );
        assert!(
            !serde_json::to_string(&detail)
                .unwrap()
                .contains("test-secret-key")
        );
    }
    for (status, response) in [
        ("429 Too Many Requests", "error test-secret-key"),
        ("200 OK", "data: not-json\n\n"),
        (
            "200 OK",
            "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
        ),
    ] {
        let (endpoint, worker) = mock_response(status, response);
        let provider = OpenAiCompatibleProvider::new("test-secret-key", endpoint);
        if let Ok(mut stream) = provider
            .stream_chat("failed-model", vec![], &[], None)
            .await
        {
            let mut failed = false;
            while let Some(event) = stream.next().await {
                if event.is_err() {
                    failed = true;
                    break;
                }
            }
            assert!(failed);
        }
        worker.join().unwrap();
        let page = query_request_logs(&RequestLogQuery::default()).unwrap();
        let detail = request_log_detail(&page.items[0].id).unwrap();
        assert_eq!(detail.status, "error");
        assert_eq!(
            detail.response_body,
            response.replace("test-secret-key", "[REDACTED]")
        );
    }
    let (endpoint, worker) = mock_response("200 OK", chat);
    let provider = OpenAiCompatibleProvider::new("test-secret-key", endpoint);
    let stream = provider
        .stream_chat("cancelled-model", vec![], &[], None)
        .await
        .unwrap();
    drop(stream);
    worker.join().unwrap();
    let page = query_request_logs(&RequestLogQuery::default()).unwrap();
    assert_eq!(page.items[0].status, "interrupted");
    let total = page.items.len();
    config.record_model_requests = false;
    save_user_config(&config).unwrap();
    assert_eq!(
        query_request_logs(&RequestLogQuery::default())
            .unwrap()
            .items
            .len(),
        total
    );
}
