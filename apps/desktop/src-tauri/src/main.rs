// Prevent a console window in release Desktop builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use xcoding_agent::AgentService;
use xcoding_core::CoreService;
use xcoding_protocol::{
    CancelSessionParams, CancelSessionResult, ChatParams, ChatResult, ListModelsResult, PingResult,
    ProviderAuthStatus, ResolveActionParams, ResolveActionResult, RollbackRestorePointParams,
    RollbackRestorePointResult, ReplaySessionResult, Session, SessionDetail, SetConfigParams,
    UserConfig, WorkspaceConfig,
};
use xcoding_providers::{
    apply_user_config_to_env, bootstrap_credentials, inspect_auth, list_models, load_user_config,
    save_user_config, user_config_dir,
};

fn boot_log(message: &str) {
    let dir = user_config_dir();
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("desktop-boot.log");
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "[{ts}] {message}");
    }
}

fn database_path() -> Result<PathBuf, String> {
    let data_dir = user_config_dir();
    std::fs::create_dir_all(&data_dir).map_err(|error| error.to_string())?;
    Ok(data_dir.join("xcoding.db"))
}

fn open_core(_app: &AppHandle) -> Result<CoreService, String> {
    CoreService::open(database_path()?).map_err(|error| error.to_string())
}

/// CoreService holds a rusqlite Connection (!Send), so agent work cannot live in a
/// Send async future that awaits across DB usage. Run the full agent turn on a
/// blocking worker and block_on there (outside any async poll context).
fn block_on_local<F, T>(future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    tauri::async_runtime::block_on(future)
}

async fn run_agent_blocking<T, F>(work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| format!("agent worker failed: {error}"))?
}

#[tauri::command]
fn provider_status() -> Result<ProviderAuthStatus, String> {
    Ok(inspect_auth())
}

#[tauri::command]
async fn list_provider_models(
    base_url: Option<String>,
    api_key: Option<String>,
) -> Result<ListModelsResult, String> {
    list_models(base_url.as_deref(), api_key.as_deref()).await
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
async fn rollback_restore_point(
    app: AppHandle,
    params: RollbackRestorePointParams,
) -> Result<RollbackRestorePointResult, String> {
    let app_for_events = app.clone();
    run_agent_blocking(move || {
        let core = open_core(&app)?;
        AgentService::new(&core)
            .rollback(params, move |event| {
                let _ = app_for_events.emit("session-event", event);
            })
            .map_err(|error| error.to_string())
    })
    .await
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
async fn resolve_action(
    app: AppHandle,
    params: ResolveActionParams,
) -> Result<ResolveActionResult, String> {
    let app_for_events = app.clone();
    run_agent_blocking(move || {
        let core = open_core(&app)?;
        block_on_local(AgentService::new(&core).resolve(params, move |event| {
            let _ = app_for_events.emit("session-event", event);
        }))
        .map_err(|error| error.to_string())
    })
    .await
}

#[tauri::command]
async fn chat(app: AppHandle, params: ChatParams) -> Result<ChatResult, String> {
    let app_for_events = app.clone();
    run_agent_blocking(move || {
        let core = open_core(&app)?;
        block_on_local(AgentService::new(&core).chat(params, move |event| {
            let _ = app_for_events.emit("session-event", event);
        }))
        .map_err(|error| error.to_string())
    })
    .await
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

fn prepare_webview_profile() {
    // Keep WebView2 profile under ~/.xcoding so portable moves/locks are easier to recover from.
    let profile = user_config_dir().join("webview-profile");
    if let Err(error) = fs::create_dir_all(&profile) {
        boot_log(&format!("webview profile dir failed: {error}"));
        return;
    }
    unsafe {
        std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &profile);
    }
    boot_log(&format!("webview profile={}", profile.display()));
}

fn ensure_main_window(app: &tauri::App) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        boot_log("main window exists from config");
        let _ = window.center();
        let _ = window.show();
        let _ = window.set_focus();
        let _ = window.unminimize();
        return Ok(());
    }

    boot_log("main window missing; creating explicitly");
    let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("XCoding")
        .inner_size(960.0, 720.0)
        .min_inner_size(720.0, 540.0)
        .center()
        .visible(true)
        .focused(true)
        .build()
        .map_err(|error| error.to_string())?;
    let _ = window.show();
    let _ = window.set_focus();
    boot_log("main window created");
    Ok(())
}

fn main() {
    boot_log("main enter");
    std::panic::set_hook(Box::new(|info| {
        boot_log(&format!("panic: {info}"));
    }));

    load_portable_dotenv();
    boot_log("portable dotenv loaded");
    bootstrap_credentials();
    boot_log("credentials bootstrapped");
    prepare_webview_profile();
    boot_log("starting tauri builder");

    let result = tauri::Builder::default()
        .setup(|app| {
            boot_log("setup begin");
            match ensure_main_window(app) {
                Ok(()) => boot_log("ensure_main_window ok"),
                Err(error) => {
                    boot_log(&format!("ensure_main_window failed: {error}"));
                    return Err(error.into());
                }
            }
            boot_log("setup end");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ping,
            provider_status,
            get_user_config,
            list_provider_models,
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
        .run(tauri::generate_context!());

    match result {
        Ok(()) => boot_log("tauri run returned ok"),
        Err(error) => {
            boot_log(&format!("tauri run failed: {error}"));
            panic!("failed to run XCoding Desktop: {error}");
        }
    }
}
