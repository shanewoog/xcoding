use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use chrono::{DateTime, SecondsFormat, Utc};
use futures_util::StreamExt;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{ProviderEventStream, load_user_config, user_config_dir};

const BODY_LIMIT: usize = 8 * 1024 * 1024;
const PAGE_SIZE: usize = 10;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RequestLogHeader {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RequestLogDetail {
    pub id: String,
    pub created_at: String,
    pub model: String,
    pub endpoint: String,
    pub status: String,
    pub http_status: Option<u16>,
    pub duration_ms: u64,
    pub request_body: String,
    #[serde(default)]
    pub request_headers: Option<Vec<RequestLogHeader>>,
    pub response_body: String,
    pub response_content_type: Option<String>,
    pub error: Option<String>,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct RequestLogSummary {
    pub id: String,
    pub created_at: String,
    pub model: String,
    pub endpoint: String,
    pub status: String,
    pub http_status: Option<u16>,
    pub duration_ms: u64,
}

#[derive(Default, Debug, Deserialize)]
#[serde(default)]
pub struct RequestLogQuery {
    pub from: Option<String>,
    pub to: Option<String>,
    pub model: String,
    pub endpoint: String,
    pub status: String,
    pub offset: u32,
}

#[derive(Debug, Serialize)]
pub struct RequestLogPage {
    pub items: Vec<RequestLogSummary>,
    pub has_more: bool,
}

fn database_path() -> PathBuf {
    user_config_dir().join("model-request-logs.db")
}

fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Nanos, true)
}

fn open_database(path: &Path, retention_days: u32) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let connection = Connection::open(path).map_err(|error| error.to_string())?;
    connection
        .busy_timeout(Duration::from_millis(250))
        .map_err(|error| error.to_string())?;
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS request_logs (
            id TEXT PRIMARY KEY, created_at TEXT NOT NULL, model TEXT NOT NULL,
            endpoint TEXT NOT NULL, status TEXT NOT NULL, http_status INTEGER,
            duration_ms INTEGER NOT NULL, detail TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS request_logs_created ON request_logs(created_at DESC, id DESC);",
        )
        .map_err(|error| error.to_string())?;
    let cutoff =
        Utc::now().checked_sub_signed(chrono::Duration::days(i64::from(retention_days.max(1))));
    if let Some(cutoff) = cutoff {
        connection
            .execute(
                "DELETE FROM request_logs WHERE created_at < ?1",
                [timestamp(cutoff)],
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(connection)
}

pub fn query_request_logs(query: &RequestLogQuery) -> Result<RequestLogPage, String> {
    query_at(
        &database_path(),
        load_user_config().model_call_log_retention_days,
        query,
    )
}

fn query_at(
    path: &Path,
    retention_days: u32,
    query: &RequestLogQuery,
) -> Result<RequestLogPage, String> {
    let parse_date = |value: &Option<String>| -> Result<Option<String>, String> {
        value
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(|value| {
                DateTime::parse_from_rfc3339(value)
                    .map(|date| timestamp(date.with_timezone(&Utc)))
                    .map_err(|_| "Invalid date filter".to_owned())
            })
            .transpose()
    };
    let from = parse_date(&query.from)?;
    let to = parse_date(&query.to)?;
    if from
        .as_ref()
        .zip(to.as_ref())
        .is_some_and(|(from, to)| from > to)
    {
        return Err("Start time must not be after end time".into());
    }
    if !["", "pending", "success", "error", "interrupted"].contains(&query.status.as_str()) {
        return Err("Invalid status filter".into());
    }
    if !path.exists() {
        return Ok(RequestLogPage {
            items: Vec::new(),
            has_more: false,
        });
    }
    let connection = open_database(path, retention_days)?;
    let mut statement = connection.prepare(
        "SELECT id, created_at, model, endpoint, status, http_status, duration_ms FROM request_logs
        WHERE (?1 IS NULL OR created_at >= ?1) AND (?2 IS NULL OR created_at <= ?2)
        AND instr(lower(model), lower(?3)) > 0 AND instr(lower(endpoint), lower(?4)) > 0
        AND (?5 = '' OR status = ?5)
        ORDER BY created_at DESC, id DESC LIMIT ?6 OFFSET ?7"
    ).map_err(|error| error.to_string())?;
    let mut items = statement
        .query_map(
            params![
                from,
                to,
                query.model.trim(),
                query.endpoint.trim(),
                query.status,
                PAGE_SIZE + 1,
                query.offset
            ],
            |row| {
                Ok(RequestLogSummary {
                    id: row.get(0)?,
                    created_at: row.get(1)?,
                    model: row.get(2)?,
                    endpoint: row.get(3)?,
                    status: row.get(4)?,
                    http_status: row.get(5)?,
                    duration_ms: row.get(6)?,
                })
            },
        )
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let has_more = items.len() > PAGE_SIZE;
    items.truncate(PAGE_SIZE);
    Ok(RequestLogPage { items, has_more })
}

pub fn request_log_detail(id: &str) -> Result<RequestLogDetail, String> {
    detail_at(
        &database_path(),
        load_user_config().model_call_log_retention_days,
        id,
    )
}

fn detail_at(path: &Path, retention_days: u32, id: &str) -> Result<RequestLogDetail, String> {
    if !path.exists() {
        return Err("Request log not found".into());
    }
    let connection = open_database(path, retention_days)?;
    let detail: String = connection
        .query_row(
            "SELECT detail FROM request_logs WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    serde_json::from_str(&detail).map_err(|error| error.to_string())
}

fn redact(text: &str, api_key: &str) -> String {
    if api_key.is_empty() {
        text.to_owned()
    } else {
        text.replace(api_key, "[REDACTED]")
    }
}

fn redact_fields(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (name, value) in fields {
                if [
                    "authorization",
                    "api_key",
                    "apikey",
                    "api-key",
                    "x-api-key",
                    "access_token",
                    "password",
                    "secret",
                    "cookie",
                    "set-cookie",
                ]
                .contains(&name.to_ascii_lowercase().as_str())
                {
                    *value = Value::String("[REDACTED]".into());
                } else {
                    redact_fields(value);
                }
            }
        }
        Value::Array(values) => values.iter_mut().for_each(redact_fields),
        _ => {}
    }
}

fn safe_endpoint(endpoint: &str) -> String {
    let Ok(mut url) = reqwest::Url::parse(endpoint) else {
        return "[invalid URL]".into();
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

fn bounded_text(mut text: String) -> (String, bool) {
    let truncated = text.len() > BODY_LIMIT;
    if truncated {
        let mut end = BODY_LIMIT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    (text, truncated)
}

struct LogState {
    path: PathBuf,
    retention_days: u32,
    detail: RequestLogDetail,
    response: Vec<u8>,
    api_key: String,
    started: Instant,
}

impl LogState {
    fn persist(&self) -> Result<(), String> {
        let connection = open_database(&self.path, self.retention_days)?;
        let detail = serde_json::to_string(&self.detail).map_err(|error| error.to_string())?;
        connection
            .execute(
                "INSERT OR REPLACE INTO request_logs VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    self.detail.id,
                    self.detail.created_at,
                    self.detail.model,
                    self.detail.endpoint,
                    self.detail.status,
                    self.detail.http_status,
                    self.detail.duration_ms,
                    detail
                ],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    fn complete(&mut self, status: &str, error: Option<&str>) {
        self.detail.status = status.into();
        self.detail.duration_ms = self.started.elapsed().as_millis() as u64;
        self.detail.error = error.map(|error| redact(error, &self.api_key));
        let (response_body, truncated) = bounded_text(redact(
            &String::from_utf8_lossy(&self.response),
            &self.api_key,
        ));
        self.detail.response_body = response_body;
        self.detail.truncated |= truncated;
        if self.persist().is_err() {
            eprintln!("Failed to persist model request log");
        }
    }
}

impl Drop for LogState {
    fn drop(&mut self) {
        if self.detail.status == "pending" {
            self.complete("interrupted", Some("Request cancelled or stream dropped"));
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct RequestLog(Option<Arc<Mutex<LogState>>>);

impl RequestLog {
    pub(crate) fn start(endpoint: &str, body: &Value, api_key: &str) -> Self {
        let config = load_user_config();
        Self::start_at(
            &database_path(),
            config.record_model_requests,
            config.model_call_log_retention_days,
            endpoint,
            body,
            api_key,
        )
    }

    fn start_at(
        path: &Path,
        enabled: bool,
        retention_days: u32,
        endpoint: &str,
        body: &Value,
        api_key: &str,
    ) -> Self {
        if !enabled {
            return Self::default();
        }
        let mut safe_body = body.clone();
        redact_fields(&mut safe_body);
        let (request_body, truncated) = bounded_text(redact(
            &serde_json::to_string_pretty(&safe_body).unwrap_or_default(),
            api_key,
        ));
        let state = LogState {
            path: path.into(),
            retention_days,
            response: Vec::new(),
            api_key: api_key.into(),
            started: Instant::now(),
            detail: RequestLogDetail {
                id: Uuid::new_v4().to_string(),
                created_at: timestamp(Utc::now()),
                model: redact(
                    body.get("model")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    api_key,
                ),
                endpoint: redact(&safe_endpoint(endpoint), api_key),
                status: "pending".into(),
                http_status: None,
                duration_ms: 0,
                request_body,
                request_headers: None,
                response_body: String::new(),
                response_content_type: None,
                error: None,
                truncated,
            },
        };
        if state.persist().is_err() {
            eprintln!("Failed to start model request log");
            return Self::default();
        }
        Self(Some(Arc::new(Mutex::new(state))))
    }

    #[cfg(test)]
    fn id(&self) -> Option<String> {
        self.0
            .as_ref()
            .map(|state| state.lock().unwrap().detail.id.clone())
    }

    pub(crate) fn request(&self, request: &reqwest::Request) {
        let Some(state) = &self.0 else {
            return;
        };
        let Ok(mut state) = state.lock() else {
            return;
        };
        let mut headers = request.headers().clone();
        headers
            .entry("accept")
            .or_insert(reqwest::header::HeaderValue::from_static("*/*"));
        if let Some(host) = request.url().host_str() {
            let authority = match request.url().port() {
                Some(port) => format!("{host}:{port}"),
                None => host.to_owned(),
            };
            if let Ok(value) = reqwest::header::HeaderValue::from_str(&authority) {
                headers.entry("host").or_insert(value);
            }
        }
        if let Some(body) = request.body().and_then(reqwest::Body::as_bytes) {
            if let Ok(value) = reqwest::header::HeaderValue::from_str(&body.len().to_string()) {
                headers.entry("content-length").or_insert(value);
            }
        }
        let mut captured: Vec<_> = headers
            .iter()
            .map(|(name, value)| {
                let sensitive = value.is_sensitive()
                    || [
                        "authorization",
                        "proxy-authorization",
                        "cookie",
                        "set-cookie",
                        "x-api-key",
                        "api-key",
                        "api_key",
                        "x-auth-token",
                        "x-access-token",
                    ]
                    .contains(&name.as_str());
                RequestLogHeader {
                    name: name.as_str().to_owned(),
                    value: if sensitive {
                        "[REDACTED]".into()
                    } else {
                        redact(&String::from_utf8_lossy(value.as_bytes()), &state.api_key)
                    },
                }
            })
            .collect();
        captured.sort_by(|left, right| left.name.cmp(&right.name));
        state.detail.request_headers = Some(captured);
        if state.persist().is_err() {
            eprintln!("Failed to persist model request headers");
        }
    }

    pub(crate) fn response(&self, response: &reqwest::Response) {
        if let Some(state) = &self.0 {
            if let Ok(mut state) = state.lock() {
                state.detail.http_status = Some(response.status().as_u16());
                state.detail.response_content_type = response
                    .headers()
                    .get("content-type")
                    .and_then(|value| value.to_str().ok())
                    .map(|value| redact(value, &state.api_key));
            }
        }
    }

    pub(crate) fn append(&self, chunk: &[u8]) {
        if let Some(state) = &self.0 {
            if let Ok(mut state) = state.lock() {
                let remaining = BODY_LIMIT
                    .saturating_add(state.api_key.len())
                    .saturating_sub(state.response.len());
                state
                    .response
                    .extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                state.detail.truncated |= chunk.len() > remaining;
            }
        }
    }

    pub(crate) fn finish(&self, error: Option<&str>) {
        if let Some(state) = &self.0 {
            if let Ok(mut state) = state.lock() {
                state.complete(if error.is_some() { "error" } else { "success" }, error);
            }
        }
    }

    pub(crate) fn wrap(self, mut stream: ProviderEventStream) -> ProviderEventStream {
        if self.0.is_none() {
            return stream;
        }
        Box::pin(async_stream::try_stream! {
            while let Some(event) = stream.next().await {
                match event {
                    Ok(event) => yield event,
                    Err(error) => {
                        self.finish(Some(&error.to_string()));
                        Err(error)?;
                    }
                }
            }
            self.finish(None);
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_path() -> PathBuf {
        std::env::temp_dir().join(format!("xcoding-request-logs-{}.db", Uuid::new_v4()))
    }

    #[test]
    fn disabled_recording_creates_no_database() {
        let path = test_path();
        let log = RequestLog::start_at(
            &path,
            false,
            30,
            "https://example.com/v1",
            &json!({"model":"test"}),
            "secret",
        );
        log.append(b"private response");
        log.finish(None);
        drop(log);
        assert!(!path.exists());
    }

    #[test]
    fn bounds_response_without_leaking_a_key_at_the_cutoff() {
        let path = test_path();
        let log = RequestLog::start_at(
            &path,
            true,
            30,
            "https://example.com",
            &json!({"model":"test"}),
            "boundary-secret",
        );
        let id = log.id().unwrap();
        log.append(&vec![b'a'; BODY_LIMIT - 4]);
        log.append(b"boundary-secret trailing text");
        log.finish(None);
        drop(log);
        let detail = detail_at(&path, 30, &id).unwrap();
        assert!(detail.truncated);
        assert_eq!(detail.response_body.len(), BODY_LIMIT);
        assert!(detail.response_body.ends_with("[RED"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn records_details_redacts_credentials_and_preserves_utf8_chunks() {
        let path = test_path();
        let log = RequestLog::start_at(
            &path,
            true,
            30,
            "https://user:secret@example.com/v1?api_key=secret",
            &json!({"model":"test", "messages":[{"content":"hello"}], "api_key":"secret"}),
            "secret",
        );
        let id = log.id().unwrap();
        let bytes = "data: 你好 secret\n\n".as_bytes();
        log.append(&bytes[..8]);
        log.append(&bytes[8..]);
        log.finish(None);
        drop(log);
        let detail = detail_at(&path, 30, &id).unwrap();
        assert_eq!(detail.status, "success");
        assert!(detail.request_body.contains("hello"));
        assert!(detail.response_body.contains("你好 [REDACTED]"));
        assert!(!serde_json::to_string(&detail).unwrap().contains("secret"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn stores_headers_before_sending_and_redacts_sensitive_values() {
        let path = test_path();
        let body = json!({"model":"test"});
        let log = RequestLog::start_at(
            &path,
            true,
            30,
            "http://[::1]:9080/v1",
            &body,
            "active-secret",
        );
        let id = log.id().unwrap();
        let mut secret_header = reqwest::header::HeaderValue::from_static("separate-secret");
        secret_header.set_sensitive(true);
        let request = reqwest::Client::new()
            .post("http://[::1]:9080/v1")
            .header("authorization", "Bearer active-secret")
            .header("cookie", "session=cookie-secret")
            .header("proxy-authorization", "Basic proxy-secret")
            .header("x-private", secret_header)
            .header("x-custom", "key=active-secret")
            .header("x-tag", "first")
            .header("x-tag", "second")
            .json(&body)
            .build()
            .unwrap();
        log.request(&request);
        let detail = detail_at(&path, 30, &id).unwrap();
        assert_eq!(detail.status, "pending");
        let headers = detail.request_headers.as_ref().unwrap();
        for name in [
            "authorization",
            "cookie",
            "proxy-authorization",
            "x-private",
        ] {
            assert!(
                headers
                    .iter()
                    .any(|header| header.name == name && header.value == "[REDACTED]")
            );
        }
        assert!(
            headers
                .iter()
                .any(|header| header.name == "host" && header.value == "[::1]:9080")
        );
        assert!(
            headers
                .iter()
                .any(|header| header.name == "x-custom" && header.value == "key=[REDACTED]")
        );
        assert_eq!(
            headers
                .iter()
                .filter(|header| header.name == "x-tag")
                .count(),
            2
        );
        let encoded = serde_json::to_string(&detail).unwrap();
        for secret in [
            "active-secret",
            "cookie-secret",
            "proxy-secret",
            "separate-secret",
        ] {
            assert!(!encoded.contains(secret));
        }
        let mut legacy = serde_json::to_value(&detail).unwrap();
        legacy.as_object_mut().unwrap().remove("request_headers");
        assert!(
            serde_json::from_value::<RequestLogDetail>(legacy)
                .unwrap()
                .request_headers
                .is_none()
        );
        drop(log);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn disabled_recording_does_not_store_headers() {
        let path = test_path();
        let log = RequestLog::start_at(
            &path,
            false,
            30,
            "https://example.com",
            &json!({"model":"test"}),
            "secret",
        );
        let request = reqwest::Client::new()
            .post("https://example.com")
            .header("authorization", "Bearer secret")
            .build()
            .unwrap();
        log.request(&request);
        drop(log);
        assert!(!path.exists());
    }

    #[test]
    fn queries_latest_ten_with_filters_pagination_and_retention() {
        let path = test_path();
        for index in 0..13 {
            let log = RequestLog::start_at(
                &path,
                true,
                30,
                "https://example.com/v1",
                &json!({"model":format!("model-{index:02}")}),
                "",
            );
            log.finish(if index == 12 { Some("HTTP 429") } else { None });
        }
        let page = query_at(&path, 30, &RequestLogQuery::default()).unwrap();
        assert_eq!(page.items.len(), 10);
        assert!(page.has_more);
        assert_eq!(page.items[0].model, "model-12");
        let page = query_at(
            &path,
            30,
            &RequestLogQuery {
                offset: 10,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(page.items.len(), 3);
        assert!(!page.has_more);
        let page = query_at(
            &path,
            30,
            &RequestLogQuery {
                status: "error".into(),
                model: "MODEL-12".into(),
                endpoint: "example.com".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(page.items.len(), 1);
        let page = query_at(
            &path,
            30,
            &RequestLogQuery {
                model: "%".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(page.items.is_empty());
        let page = query_at(
            &path,
            30,
            &RequestLogQuery {
                from: Some("2099-01-01T00:00:00Z".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(page.items.is_empty());
        assert!(
            query_at(
                &path,
                30,
                &RequestLogQuery {
                    from: Some("invalid".into()),
                    ..Default::default()
                }
            )
            .is_err()
        );
        let connection = open_database(&path, 30).unwrap();
        connection
            .execute(
                "UPDATE request_logs SET created_at = '2000-01-01T00:00:00.000Z'",
                [],
            )
            .unwrap();
        drop(connection);
        assert!(
            query_at(&path, 30, &RequestLogQuery::default())
                .unwrap()
                .items
                .is_empty()
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn cancellation_and_errors_keep_partial_responses() {
        let path = test_path();
        let log = RequestLog::start_at(
            &path,
            true,
            30,
            "https://example.com",
            &json!({"model":"test"}),
            "",
        );
        let id = log.id().unwrap();
        log.append(b"partial");
        drop(log);
        let detail = detail_at(&path, 30, &id).unwrap();
        assert_eq!(detail.status, "interrupted");
        assert_eq!(detail.response_body, "partial");
        let log = RequestLog::start_at(
            &path,
            true,
            30,
            "https://example.com",
            &json!({"model":"test"}),
            "",
        );
        let id = log.id().unwrap();
        log.append(b"error body");
        log.finish(Some("connection failed"));
        drop(log);
        let detail = detail_at(&path, 30, &id).unwrap();
        assert_eq!(detail.status, "error");
        assert_eq!(detail.error.as_deref(), Some("connection failed"));
        assert_eq!(detail.response_body, "error body");
        std::fs::remove_file(path).unwrap();
    }
}
