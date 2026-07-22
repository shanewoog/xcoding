#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use tauri::{AppHandle, Emitter, Manager};
use xcoding_agent::AgentService;
use xcoding_core::CoreService;
use xcoding_protocol::{ChatParams, ChatResult, PingResult, ResolveActionParams, ResolveActionResult, Session};

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
        .invoke_handler(tauri::generate_handler![ping, list_sessions, chat, resolve_action])
        .run(tauri::generate_context!())
        .expect("failed to run XCoding Desktop");
}
