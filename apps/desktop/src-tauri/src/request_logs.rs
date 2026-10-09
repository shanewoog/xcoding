use xcoding_providers::request_logs::{RequestLogDetail, RequestLogPage, RequestLogQuery};

#[tauri::command]
pub async fn query_model_request_logs(query: RequestLogQuery) -> Result<RequestLogPage, String> {
    tauri::async_runtime::spawn_blocking(move || {
        xcoding_providers::request_logs::query_request_logs(&query)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn model_request_log_detail(id: String) -> Result<RequestLogDetail, String> {
    tauri::async_runtime::spawn_blocking(move || {
        xcoding_providers::request_logs::request_log_detail(&id)
    })
    .await
    .map_err(|error| error.to_string())?
}
