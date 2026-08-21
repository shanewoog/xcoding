// Prevent a console window in release Desktop builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod browser;
mod gitnexus;
mod passwords;
mod projects;
mod terminal;
mod workspace_tools;

use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{
    AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use xcoding_agent::AgentService;
use xcoding_core::CoreService;
use xcoding_mcp::{
    McpServerEntry, PluginConfig, is_valid_server_name, load_mcp_config, load_plugin_config,
    save_plugin_config, user_skill_root,
};
use xcoding_protocol::{
    CancelSessionParams, CancelSessionResult, ChatParams, ChatResult, CreateProjectParams,
    CreateProjectResult, ImportProjectParams, ImportProjectResult, ListModelsResult, PingResult,
    ProjectDir, ProviderAuthStatus, ReplaySessionResult, ResolveActionParams, ResolveActionResult,
    RollbackRestorePointParams, RollbackRestorePointResult, Session, SessionDetail,
    SetConfigParams, UserConfig, WorkspaceConfig,
};
use xcoding_providers::{
    apply_user_config_to_env, bootstrap_credentials, inspect_auth, list_models, load_user_config,
    normalize_user_config, save_user_config, user_config_dir,
};

#[derive(Clone, Serialize)]
struct LocalPluginItem {
    id: String,
    kind: String,
    name: String,
    description: String,
    source: String,
    enabled: bool,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    env_keys: Option<Vec<String>>,
}

fn plugin_enabled(
    config: &PluginConfig,
    kind: &str,
    source: &str,
    name: &str,
    default: bool,
) -> bool {
    let map = if kind == "mcp" {
        &config.mcp_enabled
    } else {
        &config.skill_enabled
    };
    map.get(&format!("{source}:{name}"))
        .copied()
        .unwrap_or(default)
}

fn skill_description(raw: &str) -> String {
    let normalized = raw.replace("\r\n", "\n");
    if let Some(frontmatter) = normalized.strip_prefix("---\n") {
        if let Some((header, _)) = frontmatter.split_once("\n---") {
            for line in header.lines() {
                let Some((key, value)) = line.split_once(':') else {
                    continue;
                };
                if key.trim() == "description" && !value.trim().is_empty() {
                    return value.trim().chars().take(240).collect();
                }
            }
        }
    }
    normalized
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or("Local skill")
        .chars()
        .take(240)
        .collect()
}

fn valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().enumerate().all(|(index, ch)| {
            (index == 0 && ch.is_ascii_alphanumeric())
                || (index > 0 && (ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'))
        })
}

fn copy_skill_dir(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|error| error.to_string())?;
    for entry in fs::read_dir(src).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        let target = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_skill_dir(&entry.path(), &target)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), target).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn list_skill_plugins(
    root: &std::path::Path,
    source: &str,
    items: &mut Vec<LocalPluginItem>,
    config: &PluginConfig,
    seen: &mut HashSet<String>,
) -> Result<(), String> {
    let Ok(entries) = fs::read_dir(root) else {
        return Ok(());
    };
    let mut folders = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
        .collect::<Vec<_>>();
    folders.sort_by_key(|entry| entry.file_name());
    for entry in folders {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !valid_skill_name(&name) || !seen.insert(name.clone()) {
            continue;
        }
        let skill_path = entry.path().join("SKILL.md");
        let raw = match fs::read_to_string(&skill_path) {
            Ok(raw) => raw,
            Err(_) => continue,
        };
        let enabled = plugin_enabled(config, "skill", source, &name, true);
        items.push(LocalPluginItem {
            id: format!("{source}:{name}"),
            kind: "skill".to_owned(),
            name,
            description: skill_description(&raw),
            source: source.to_owned(),
            enabled,
            status: if enabled { "enabled" } else { "disabled" }.to_owned(),
            tool_count: None,
            env_keys: None,
        });
    }
    Ok(())
}

#[tauri::command]
fn list_local_plugins(workspace_root: Option<String>) -> Result<Vec<LocalPluginItem>, String> {
    let config = load_plugin_config();
    let mut items = Vec::new();
    let mut workspace_mcp_names = HashSet::new();
    let workspace = workspace_root
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    if let Some(root) = workspace.as_ref() {
        for server in load_mcp_config(root).map_err(|error| error.to_string())? {
            workspace_mcp_names.insert(server.name.clone());
            let enabled = plugin_enabled(&config, "mcp", "workspace", &server.name, server.enabled);
            let mut env_keys = server.env.keys().cloned().collect::<Vec<_>>();
            env_keys.sort();
            items.push(LocalPluginItem {
                id: format!("workspace:{}", server.name),
                kind: "mcp".to_owned(),
                name: server.name,
                description: format!("MCP server: {}", server.command),
                source: "workspace".to_owned(),
                enabled,
                status: if enabled { "enabled" } else { "disabled" }.to_owned(),
                tool_count: None,
                env_keys: Some(env_keys),
            });
        }
        list_skill_plugins(
            &root.join(".xcoding/skills"),
            "workspace",
            &mut items,
            &config,
            &mut HashSet::new(),
        )?;
    }
    let mut seen_skill_names = items
        .iter()
        .filter(|item| item.kind == "skill")
        .map(|item| item.name.clone())
        .collect::<HashSet<_>>();
    for (name, entry) in &config.mcp_servers {
        if !is_valid_server_name(name) || workspace_mcp_names.contains(name) {
            continue;
        }
        let enabled = plugin_enabled(&config, "mcp", "user", name, entry.enabled);
        let mut env_keys = entry.env.keys().cloned().collect::<Vec<_>>();
        env_keys.sort();
        items.push(LocalPluginItem {
            id: format!("user:{name}"),
            kind: "mcp".to_owned(),
            name: name.clone(),
            description: format!("MCP server: {}", entry.command),
            source: "user".to_owned(),
            enabled,
            status: if enabled { "enabled" } else { "disabled" }.to_owned(),
            tool_count: None,
            env_keys: Some(env_keys),
        });
    }
    list_skill_plugins(
        &user_skill_root(),
        "user",
        &mut items,
        &config,
        &mut seen_skill_names,
    )?;
    items.sort_by(|left, right| left.kind.cmp(&right.kind).then(left.name.cmp(&right.name)));
    Ok(items)
}

#[tauri::command]
fn set_plugin_enabled(
    kind: String,
    source: String,
    name: String,
    enabled: bool,
) -> Result<(), String> {
    if !matches!(kind.as_str(), "mcp" | "skill") || !matches!(source.as_str(), "user" | "workspace")
    {
        return Err("invalid plugin identity".to_owned());
    }
    let valid_name = if kind == "mcp" {
        is_valid_server_name(&name)
    } else {
        valid_skill_name(&name)
    };
    if !valid_name {
        return Err("invalid plugin name".to_owned());
    }
    let mut config = load_plugin_config();
    let map = if kind == "mcp" {
        &mut config.mcp_enabled
    } else {
        &mut config.skill_enabled
    };
    map.insert(format!("{source}:{name}"), enabled);
    save_plugin_config(&config)
}

#[tauri::command]
fn add_local_mcp(
    name: String,
    command: String,
    args: Vec<String>,
    env: HashMap<String, String>,
) -> Result<(), String> {
    let name = name.trim();
    let command = command.trim();
    if !is_valid_server_name(name) {
        return Err("MCP name must use letters, digits, `_`, or `-`".to_owned());
    }
    if command.is_empty() {
        return Err("MCP command is required".to_owned());
    }
    let mut config = load_plugin_config();
    config.mcp_servers.insert(
        name.to_owned(),
        McpServerEntry {
            command: command.to_owned(),
            args,
            env,
            enabled: true,
        },
    );
    save_plugin_config(&config)
}

#[tauri::command]
fn import_local_skill(source_path: String) -> Result<String, String> {
    let source = PathBuf::from(source_path.trim());
    if !source.is_dir() || !source.join("SKILL.md").is_file() {
        return Err("selected folder must contain SKILL.md".to_owned());
    }
    let name = source
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .ok_or_else(|| "invalid Skill folder name".to_owned())?;
    if !valid_skill_name(&name) {
        return Err("Skill folder name must start with a letter or digit and use only letters, digits, `_`, or `-`".to_owned());
    }
    let destination = user_skill_root().join(&name);
    if destination.exists() {
        return Err("a user Skill with this name already exists".to_owned());
    }
    copy_skill_dir(&source, &destination)?;
    Ok(name)
}

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

fn normalize_workspace_path(value: &str) -> String {
    value
        .trim()
        .replace('\\', "/")
        .trim_start_matches("//?/")
        .trim_start_matches("//./")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

fn direct_project_root_key(workspace_home: &str, candidate: &str) -> Option<String> {
    let home_key = normalize_workspace_path(workspace_home);
    let candidate_key = normalize_workspace_path(candidate);
    if home_key.is_empty() || candidate_key.is_empty() {
        return None;
    }
    let relative = candidate_key.strip_prefix(&(home_key + "/"))?;
    if relative.is_empty() || relative.contains('/') || relative.starts_with('.') {
        return None;
    }
    Some(candidate_key)
}

fn missing_project_session_roots<'a>(
    workspace_home: &str,
    projects: &[ProjectDir],
    session_roots: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let existing = projects
        .iter()
        .map(|project| normalize_workspace_path(&project.path))
        .collect::<HashSet<_>>();
    let mut missing = HashMap::<String, String>::new();
    for root in session_roots {
        let Some(key) = direct_project_root_key(workspace_home, root) else {
            continue;
        };
        if !existing.contains(&key) {
            missing.entry(key).or_insert_with(|| root.to_owned());
        }
    }
    let mut roots = missing.into_values().collect::<Vec<_>>();
    roots.sort_by_key(|root| normalize_workspace_path(root));
    roots
}

/// CoreService holds a rusqlite Connection (!Send), so agent work cannot live in a
/// Send async future that awaits across DB usage. Run the full agent turn on a
/// blocking worker and block_on there (outside any async poll context).
///
/// Use a dedicated current-thread runtime instead of nesting on Tauri's global
/// runtime from `spawn_blocking`, which can stall completion after the model reply.
fn block_on_local<F, T>(future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("local agent runtime")
        .block_on(future)
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
    let next = normalize_user_config(config);
    save_user_config(&next)?;
    apply_user_config_to_env(&next);
    Ok(next)
}

#[tauri::command]
fn list_projects(app: AppHandle, workspace_home: String) -> Result<Vec<ProjectDir>, String> {
    let projects = projects::list_projects(&workspace_home)?;
    let core = open_core(&app)?;
    let sessions = core
        .list_sessions(None)
        .map_err(|error| error.to_string())?;
    for root in missing_project_session_roots(
        &workspace_home,
        &projects,
        sessions
            .iter()
            .map(|session| session.workspace_root.as_str()),
    ) {
        core.delete_workspace_sessions(&root)
            .map_err(|error| error.to_string())?;
    }
    Ok(projects)
}

#[tauri::command]
fn create_project(params: CreateProjectParams) -> Result<CreateProjectResult, String> {
    projects::create_project(params)
}

#[tauri::command]
async fn import_project(params: ImportProjectParams) -> Result<ImportProjectResult, String> {
    tauri::async_runtime::spawn_blocking(move || projects::import_project(params))
        .await
        .map_err(|error| format!("project import task failed: {error}"))?
}

#[tauri::command]
fn pick_directory(title: Option<String>) -> Result<Option<String>, String> {
    projects::pick_directory(title)
}

#[tauri::command]
fn ensure_chat_workspace(workspace_home: Option<String>) -> Result<String, String> {
    projects::ensure_chat_workspace(workspace_home.as_deref())
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
fn delete_session(app: AppHandle, session_id: String) -> Result<(), String> {
    let session_id = uuid::Uuid::parse_str(&session_id).map_err(|error| error.to_string())?;
    open_core(&app)?
        .delete_session(session_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn delete_workspace_sessions(app: AppHandle, workspace_root: String) -> Result<usize, String> {
    open_core(&app)?
        .delete_workspace_sessions(&workspace_root)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn rename_session(app: AppHandle, session_id: String, title: String) -> Result<Session, String> {
    let session_id = uuid::Uuid::parse_str(&session_id).map_err(|error| error.to_string())?;
    open_core(&app)?
        .rename_session(session_id, title)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn workspace_config(app: AppHandle, workspace_root: String) -> Result<WorkspaceConfig, String> {
    open_core(&app)?
        .workspace_config(&workspace_root)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn count_local_memories(app: AppHandle, workspace_root: String) -> Result<usize, String> {
    open_core(&app)?
        .count_local_memories(&workspace_root)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn clear_local_memories(app: AppHandle, workspace_root: String) -> Result<usize, String> {
    open_core(&app)?
        .clear_local_memories(&workspace_root)
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
    open_core(&app)?
        .session_detail(session_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn session_replay(app: AppHandle, session_id: String) -> Result<ReplaySessionResult, String> {
    let session_id = uuid::Uuid::parse_str(&session_id).map_err(|error| error.to_string())?;
    open_core(&app)?
        .session_replay(session_id)
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
        .cancel_session(params.session_id, params.partial_assistant.as_deref())
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

fn window_title() -> String {
    format!("XCoding v{}", env!("CARGO_PKG_VERSION"))
}

fn ensure_main_window(app: &tauri::App) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        window
            .set_title(&window_title())
            .map_err(|error| error.to_string())?;
        boot_log("main window exists from config and remains hidden until the UI is ready");
        return Ok(());
    }

    boot_log("main window missing; creating hidden");
    WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title(window_title())
        .inner_size(1510.0, 720.0)
        .min_inner_size(720.0, 540.0)
        .center()
        .visible(false)
        .focused(false)
        .build()
        .map_err(|error| error.to_string())?;
    boot_log("main window created hidden");
    Ok(())
}

/// Reveal the main window only after React has rendered its first frame.
/// This prevents Windows' empty WebView background from flashing at startup.
#[tauri::command]
fn show_main_window(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window is unavailable".to_owned())?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())?;
    boot_log("main window shown after frontend ready");
    Ok(())
}

fn restore_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[cfg(windows)]
struct SingleInstanceGuard {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[derive(Debug)]
enum SingleInstanceError {
    AlreadyRunning,
    OsError(u32),
}

#[cfg(windows)]
impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(windows)]
fn acquire_single_instance() -> Result<SingleInstanceGuard, SingleInstanceError> {
    use std::ptr::null;
    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows_sys::Win32::System::Threading::CreateMutexW;

    let name: Vec<u16> = "Local\\XCoding.SingleInstance"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe { CreateMutexW(null(), 0, name.as_ptr()) };
    if handle.is_null() {
        return Err(SingleInstanceError::OsError(unsafe { GetLastError() }));
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            let _ = windows_sys::Win32::Foundation::CloseHandle(handle);
        }
        return Err(SingleInstanceError::AlreadyRunning);
    }
    Ok(SingleInstanceGuard { handle })
}

#[cfg(not(windows))]
struct SingleInstanceGuard;

#[cfg(not(windows))]
fn acquire_single_instance() -> Result<SingleInstanceGuard, SingleInstanceError> {
    Ok(SingleInstanceGuard)
}

fn main() {
    boot_log("main enter");
    let _single_instance = match acquire_single_instance() {
        Ok(guard) => guard,
        Err(SingleInstanceError::AlreadyRunning) => {
            boot_log("another XCoding instance is already running; exiting");
            return;
        }
        Err(SingleInstanceError::OsError(error)) => {
            boot_log(&format!("single-instance mutex failed: Win32 error {error}"));
            panic!("failed to acquire XCoding single-instance mutex: Win32 error {error}");
        }
    };
    boot_log("single-instance mutex acquired");
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
            let show_item = MenuItem::with_id(app, "show", "显示 XCoding", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let tray_menu = Menu::with_items(app, &[&show_item, &quit_item])?;
            let tray_icon = app
                .default_window_icon()
                .cloned()
                .ok_or_else(|| "default window icon is unavailable".to_owned())?;
            TrayIconBuilder::with_id("xcoding-tray")
                .menu(&tray_menu)
                .icon(tray_icon)
                .tooltip("XCoding")
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => restore_main_window(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        restore_main_window(tray.app_handle());
                    }
                })
                .build(app)?;
            // Flip any sessions left in Running state by a previous process to Cancelled.
            // A blocking chat invoke cannot survive a process restart, so Running rows at
            // startup are always zombie rows that would otherwise lock the composer in queue mode.
            match database_path()
                .and_then(|path| CoreService::open(path).map_err(|e| e.to_string()))
            {
                Ok(core) => match core.reconcile_interrupted_sessions() {
                    Ok(n) if n > 0 => boot_log(&format!(
                        "reconciled {n} interrupted session(s) to cancelled"
                    )),
                    Ok(_) => {}
                    Err(error) => boot_log(&format!(
                        "reconcile_interrupted_sessions failed (non-fatal): {error}"
                    )),
                },
                Err(error) => boot_log(&format!(
                    "open for reconciliation failed (non-fatal): {error}"
                )),
            }
            boot_log("setup end");
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .manage(terminal::TerminalState {
            session: std::sync::Mutex::new(None),
        })
        .manage(browser::BrowserRegistry::default())
        .invoke_handler(tauri::generate_handler![
            ping,
            show_main_window,
            provider_status,
            get_user_config,
            list_provider_models,
            set_user_config,
            list_local_plugins,
            set_plugin_enabled,
            add_local_mcp,
            import_local_skill,
            list_projects,
            create_project,
            import_project,
            pick_directory,
            ensure_chat_workspace,
            list_sessions,
            delete_session,
            delete_workspace_sessions,
            rename_session,
            workspace_config,
            set_workspace_config,
            count_local_memories,
            clear_local_memories,
            session_detail,
            session_replay,
            chat,
            resolve_action,
            rollback_restore_point,
            cancel_session,
            workspace_tools::git_environment,
            workspace_tools::list_workspace_entries,
            workspace_tools::read_workspace_file,
            workspace_tools::workspace_changes,
            workspace_tools::workspace_file_diff,
            workspace_tools::run_terminal_command,
            workspace_tools::open_path,
            workspace_tools::open_external_url,
            terminal::terminal_start,
            terminal::terminal_input,
            terminal::terminal_resize,
            terminal::terminal_stop,
            gitnexus::gitnexus_status,
            gitnexus::gitnexus_analyze,
            gitnexus::gitnexus_query,
            gitnexus::gitnexus_context,
            gitnexus::gitnexus_impact,
            browser::browser_ensure,
            browser::browser_set_bounds,
            browser::browser_navigate,
            browser::browser_reload,
            browser::browser_force_reload,
            browser::browser_back,
            browser::browser_forward,
            browser::browser_show,
            browser::browser_hide,
            browser::browser_close,
            browser::browser_adopt_session,
            browser::browser_set_user_agent,
            browser::browser_set_zoom,
            browser::browser_print,
            browser::browser_clear_data,
            browser::browser_current_url,
            browser::browser_eval,
            browser::browser_find,
            browser::browser_download_dir,
            browser::browser_save_snapshot,
            browser::browser_passwords_list,
            browser::browser_password_save,
            browser::browser_password_delete,
            browser::browser_password_reveal,
            browser::browser_password_capture,
            browser::browser_password_fill
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

#[cfg(test)]
mod tests {
    use super::{direct_project_root_key, missing_project_session_roots};
    use xcoding_protocol::ProjectDir;

    #[cfg(windows)]
    #[test]
    fn single_instance_mutex_is_exclusive_and_released_by_guard() {
        let first = super::acquire_single_instance().expect("first instance should acquire mutex");
        assert!(matches!(
            super::acquire_single_instance(),
            Err(super::SingleInstanceError::AlreadyRunning)
        ));
        drop(first);
        assert!(super::acquire_single_instance().is_ok());
    }

    #[test]
    fn recognizes_only_visible_direct_project_roots() {
        assert_eq!(
            direct_project_root_key("D:/WORK/Code", "d:\\work\\code\\Demo\\"),
            Some("d:/work/code/demo".to_owned())
        );
        assert!(direct_project_root_key("D:/WORK/Code", "D:/WORK/Code/.xcoding-chat").is_none());
        assert!(direct_project_root_key("D:/WORK/Code", "D:/WORK/Code/Demo/src").is_none());
        assert!(direct_project_root_key("D:/WORK/Code", "D:/WORK/CodeElse/Demo").is_none());
    }

    #[test]
    fn finds_deleted_project_sessions_without_touching_existing_or_chat_roots() {
        let projects = vec![ProjectDir {
            path: "D:/WORK/Code/Keep".to_owned(),
            dir_name: "Keep".to_owned(),
            title: "Keep".to_owned(),
        }];
        let roots = [
            "D:/WORK/Code/Missing",
            "d:\\work\\code\\missing\\",
            "D:/WORK/Code/Keep",
            "D:/WORK/Code/.xcoding-chat",
            "D:/WORK/Code/Nested/src",
            "D:/Other/Outside",
        ];

        assert_eq!(
            missing_project_session_roots("D:/WORK/Code", &projects, roots),
            vec!["D:/WORK/Code/Missing".to_owned()]
        );
    }
}
