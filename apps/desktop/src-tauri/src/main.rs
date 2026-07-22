#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use tauri::{AppHandle, Emitter, Manager};
use xcoding_agent::AgentService;
use xcoding_core::CoreService;
use xcoding_protocol::{CancelSessionParams, CancelSessionResult, ChatParams, ChatResult, PingResult, ResolveActionParams, ResolveActionResult, RollbackRestorePointParams, RollbackRestorePointResult, Session, SessionDetail, SetConfigParams, WorkspaceConfig};

fn database_path(app: &AppHandle) -> Result<PathBuf, String> {
    let data_dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    std::fs::create_dir_all(&data_dir).map_err(|error| error.to_string())?;
    Ok(data_dir.join("xcoding.db"))
}

fn open_core(app: &AppHandle) -> Result<CoreService, String> {
    CoreService::open(database_path(app)?).map_err(|error| error.to_string())
}

#[tauri::command]
fn ping(app: AppHandle) -> Result<PingResult, String> {
    Ok(open_core(&app)?.ping())
}

#[tauri::command]
fn list_sessions(app: AppHandle, workspace_root: Option<String>) -> Result<Vec<Session>, String> {
    open_core(&app)?
        .list_sessions(workspace_root.as_deref())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn workspace_config(app: AppHandle, workspace_root: String) -> Result<WorkspaceConfig, String> {
    open_core(&app)?
        .workspace_config(&workspace_root)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn set_workspace_config(
    app: AppHandle,
    params: SetConfigParams,
) -> Result<WorkspaceConfig, String> {
    open_core(&app)?
        .set_workspace_config(params)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn session_detail(app: AppHandle, session_id: String) -> Result<SessionDetail, String> {
    let session_id = uuid::Uuid::parse_str(&session_id).map_err(|error| error.to_string())?;
    open_core(&app)?.session_detail(session_id).map_err(|error| error.to_string())
}

#[tauri::command]
fn rollback_restore_point(app: AppHandle, params: RollbackRestorePointParams) -> Result<RollbackRestorePointResult, String> {
    let core = open_core(&app)?;
    AgentService::new(&core).rollback(params, move |event| {
        let _ = app.emit("session-event", event);
    }).map_err(|error| error.to_string())
}

#[tauri::command]
fn cancel_session(app: AppHandle, params: CancelSessionParams) -> Result<CancelSessionResult, String> {
    let core = open_core(&app)?;
    let session = core.cancel_session(params.session_id).map_err(|error| error.to_string())?;
    let event = xcoding_protocol::SessionEvent::SessionCancelled {
        session_id: session.id,
        message: "Session cancelled by user".to_owned(),
    };
    let _ = core.record_event(&event);
    let _ = app.emit("session-event", event);
    Ok(CancelSessionResult { session })
}

#[tauri::command]
fn resolve_action(app: AppHandle, params: ResolveActionParams) -> Result<ResolveActionResult, String> {
    let core = open_core(&app)?;
    tauri::async_runtime::block_on(AgentService::new(&core).resolve(params, move |event| {
        let _ = app.emit("session-event", event);
    }))
    .map_err(|error| error.to_string())
}

#[tauri::command]
fn chat(app: AppHandle, params: ChatParams) -> Result<ChatResult, String> {
    let core = open_core(&app)?;
    tauri::async_runtime::block_on(AgentService::new(&core).chat(params, move |event| {
        let _ = app.emit("session-event", event);
    }))
    .map_err(|error| error.to_string())
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![ping, list_sessions, workspace_config, set_workspace_config, session_detail, chat, resolve_action, rollback_restore_point, cancel_session])
        .run(tauri::generate_context!())
        .expect("failed to run XCoding Desktop");
}
