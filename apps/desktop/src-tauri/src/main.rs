use std::path::PathBuf;

use tauri::{AppHandle, Emitter, Manager};
use xcoding_agent::AgentService;
use xcoding_core::CoreService;
use xcoding_protocol::{
    CancelSessionParams, CancelSessionResult, ChatParams, ChatResult, PingResult,
    ProviderAuthStatus, ResolveActionParams, ResolveActionResult, RollbackRestorePointParams,
    RollbackRestorePointResult, ReplaySessionResult, Session, SessionDetail, SetConfigParams,
    UserConfig, WorkspaceConfig,
};
use xcoding_providers::{
    apply_user_config_to_env, bootstrap_credentials, inspect_auth, load_user_config,
    save_user_config, user_config_dir,
};

fn database_path() -> Result<PathBuf, String> {
    let data_dir = user_config_dir();
    std::fs::create_dir_all(&data_dir).map_err(|error| error.to_string())?;
    Ok(data_dir.join("xcoding.db"))
}

fn open_core(_app: &AppHandle) -> Result<CoreService, String> {
    CoreService::open(database_path()?).map_err(|error| error.to_string())
}

#[tauri::command]
fn provider_status() -> Result<ProviderAuthStatus, String> {
    Ok(inspect_auth())
}

#[tauri::command]
fn get_user_config() -> Result<UserConfig, String> {
    Ok(load_user_config())
}

#[tauri::command]
fn set_user_config(config: UserConfig) -> Result<UserConfig, String> {
    let mut next = config;
    next.provider = if next.provider.trim().is_empty() {
        "openai".to_owned()
    } else {
        next.provider.trim().to_owned()
    };
    next.model = next.model.trim().to_owned();
    next.base_url = next.base_url.trim().trim_end_matches('/').to_owned();
    if next.base_url.is_empty() {
        next.base_url = "https://ai.v58.dev/v1".to_owned();
    }
    next.locale = next.locale.trim().to_owned();
    if next.locale.is_empty() {
        next.locale = "en".to_owned();
    }
    if let Some(key) = next.api_key.as_mut() {
        let trimmed = key.trim().to_owned();
        if trimmed.is_empty() {
            next.api_key = None;
        } else {
            *key = trimmed;
        }
    }
    if let Some(root) = next.last_workspace_root.as_mut() {
        let trimmed = root.trim().to_owned();
        if trimmed.is_empty() {
            next.last_workspace_root = None;
        } else {
            *root = trimmed;
        }
    }
    save_user_config(&next)?;
    apply_user_config_to_env(&next);
    Ok(next)
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
    open_core(&app)
        ?.session_detail(session_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn session_replay(app: AppHandle, session_id: String) -> Result<ReplaySessionResult, String> {
    let session_id = uuid::Uuid::parse_str(&session_id).map_err(|error| error.to_string())?;
    open_core(&app)
        ?.session_replay(session_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn rollback_restore_point(
    app: AppHandle,
    params: RollbackRestorePointParams,
) -> Result<RollbackRestorePointResult, String> {
    let core = open_core(&app)?;
    AgentService::new(&core)
        .rollback(params, move |event| {
            let _ = app.emit("session-event", event);
        })
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn cancel_session(
    app: AppHandle,
    params: CancelSessionParams,
) -> Result<CancelSessionResult, String> {
    let core = open_core(&app)?;
    let session = core
        .cancel_session(params.session_id)
        .map_err(|error| error.to_string())?;
    let event = xcoding_protocol::SessionEvent::SessionCancelled {
        session_id: session.id,
        message: "Session cancelled by user".to_owned(),
    };
    let _ = core.record_event(&event);
    let _ = app.emit("session-event", event);
    Ok(CancelSessionResult { session })
}

#[tauri::command]
fn resolve_action(
    app: AppHandle,
    params: ResolveActionParams,
) -> Result<ResolveActionResult, String> {
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

fn load_portable_dotenv() {
    // Portable / green build: prefer `.env` next to the executable first.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(".env");
            if candidate.is_file() {
                let _ = dotenvy::from_path(&candidate);
            }
        }
    }
}

fn main() {
    load_portable_dotenv();
    bootstrap_credentials();
    tauri::Builder::default()
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.center();
                let _ = window.show();
                let _ = window.set_focus();
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ping,
            provider_status,
            get_user_config,
            set_user_config,
            list_sessions,
            workspace_config,
            set_workspace_config,
            session_detail,
            session_replay,
            chat,
            resolve_action,
            rollback_restore_point,
            cancel_session
        ])
        .run(tauri::generate_context!())
        .expect("failed to run XCoding Desktop");
}

