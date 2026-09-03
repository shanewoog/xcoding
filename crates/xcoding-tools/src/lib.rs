//! Read-only workspace tools used by the Phase 1B agent loop.

use std::{
    collections::VecDeque,
    env, fs,
    io::Read,
    net::{Ipv4Addr, SocketAddr, TcpStream},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::io::Write;
use thiserror::Error;
use xcoding_mcp::{PluginConfig, load_plugin_config, user_skill_root};
use xcoding_policy::{
    COMMAND_ALLOWLIST_RELATIVE_PATH, COMMAND_DENYLIST_RELATIVE_PATH, PermissionDecision,
    PermissionKind, assess_command_with_lists, evaluate_detailed, parse_command_allowlist,
    parse_command_denylist,
};
use xcoding_protocol::{
    MAX_PLAN_STEP_DESCRIPTION_CHARS, MAX_PLAN_STEPS, Mode, PatchPreview, PlanStep, PlanStepStatus,
    ToolCall, ToolName,
};

const DEFAULT_LIST_ENTRIES: usize = 200;
const MAX_LIST_ENTRIES: usize = 1_000;
const DEFAULT_READ_LINES: usize = 200;
const MAX_READ_LINES: usize = 400;
const MAX_READ_BYTES: u64 = 512 * 1024;
const DEFAULT_SEARCH_RESULTS: usize = 50;
const MAX_SEARCH_RESULTS: usize = 100;
const MAX_SEARCH_FILE_BYTES: u64 = 1024 * 1024;
const MAX_SEARCH_CONTEXT_LINES: usize = 3;
const MAX_SKILL_CONTENT_CHARS: usize = 20_000;
const MAX_SKILL_DESCRIPTION_CHARS: usize = 240;
const MAX_SEARCH_CANDIDATES: usize = 500;
const DEFAULT_GIT_LOG_COUNT: usize = 20;
const MAX_GIT_LOG_COUNT: usize = 50;
/// Aggregate byte budget for a structured tool payload (a JSON array of items).
/// Mirrors the cap applied to raw command output by [`truncate_output`].
const MAX_TOOL_JSON_BYTES: usize = 32 * 1024;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Default wall-clock bound for one `run_command` call.
/// Long builds legitimately run for minutes, so this only has to be low enough
/// that a foreground resident service cannot hold a turn open indefinitely.
const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(600);

/// Upper bound a caller may request through `timeout_seconds`.
const MAX_COMMAND_TIMEOUT_SECONDS: u64 = 3_600;

/// Workspace-relative directory holding `run_command` background launch logs.
const BACKGROUND_LOG_DIR: &str = ".xcoding/logs";

/// How long a background launch is observed before reporting success.
/// A service that dies on a bad working directory, a taken port, or a missing
/// config fails within this window, so the launching call can report the real
/// error instead of a pid that no longer exists.
const BACKGROUND_STARTUP_PROBE: Duration = Duration::from_millis(700);

/// Trailing bytes of a background log replayed into the tool result.
const BACKGROUND_LOG_TAIL_BYTES: u64 = 8 * 1024;

/// Default bound for waiting on a background service's port, in seconds.
const DEFAULT_READY_TIMEOUT_SECONDS: u64 = 15;

/// Upper bound a caller may request through `ready_timeout_seconds`.
const MAX_READY_TIMEOUT_SECONDS: u64 = 120;

/// Gap between two port probes while waiting for a background service.
const READY_PROBE_INTERVAL: Duration = Duration::from_millis(250);

/// How long a single connect attempt may block before it counts as not ready.
const READY_CONNECT_TIMEOUT: Duration = Duration::from_millis(500);

/// How long to keep draining a child's pipes after the child itself has exited.
/// A grandchild that inherited the pipe write handle (for example anything
/// launched through `cmd /C start`) keeps the pipe open indefinitely, so reads
/// must not be awaited without a bound or the tool call never returns.
const PIPE_DRAIN_GRACE: Duration = Duration::from_secs(2);

/// Streams a child pipe into a shared buffer so output stays readable even when
/// the reader thread cannot finish. Returns the buffer plus a completion flag.
fn drain_pipe<R>(mut pipe: R) -> (Arc<Mutex<Vec<u8>>>, Arc<AtomicBool>)
where
    R: Read + Send + 'static,
{
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let finished = Arc::new(AtomicBool::new(false));
    let writer = Arc::clone(&buffer);
    let done = Arc::clone(&finished);
    thread::spawn(move || {
        let mut chunk = [0u8; 8 * 1024];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => {
                    if let Ok(mut sink) = writer.lock() {
                        sink.extend_from_slice(&chunk[..count]);
                    } else {
                        break;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        done.store(true, Ordering::SeqCst);
    });
    (buffer, finished)
}

/// Waits up to `grace` for both readers to hit end-of-pipe, then gives up.
fn await_pipe_drain(flags: [&Arc<AtomicBool>; 2], grace: Duration) {
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        if flags.iter().all(|flag| flag.load(Ordering::SeqCst)) {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn snapshot_pipe(buffer: &Arc<Mutex<Vec<u8>>>) -> Vec<u8> {
    buffer.lock().map(|value| value.clone()).unwrap_or_default()
}

/// Windows process-tree cleanup for one `run_command` call.
///
/// `Child::kill` only terminates the direct child, so a grandchild that was
/// detached from it (anything spawned through `cmd /C start`, `subprocess.Popen`,
/// `child_process.spawn`, ...) survives the tool call and keeps holding ports and
/// file locks. A job object with `KILL_ON_JOB_CLOSE` makes the whole tree die
/// with the guard, whether the call finished, timed out, or was cancelled.
#[cfg(windows)]
mod job_object {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    type Handle = *mut std::ffi::c_void;

    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: u32 = 9;

    #[repr(C)]
    #[derive(Default)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    #[derive(Default)]
    struct JobObjectBasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct JobObjectExtendedLimitInformation {
        basic_limit_information: JobObjectBasicLimitInformation,
        io_info: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attributes: *mut std::ffi::c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(
            job: Handle,
            info_class: u32,
            info: *const std::ffi::c_void,
            info_len: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
    }

    /// Owns a kill-on-close job object; dropping it terminates every assigned process.
    pub(super) struct ProcessTreeGuard {
        job: Handle,
    }

    impl ProcessTreeGuard {
        /// Returns `None` when the job object cannot be created or configured, in
        /// which case the caller keeps the previous direct-child-only behaviour.
        pub(super) fn new() -> Option<Self> {
            let job = unsafe { CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()) };
            if job.is_null() {
                return None;
            }
            let mut limits = JobObjectExtendedLimitInformation::default();
            limits.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let assigned = unsafe {
                SetInformationJobObject(
                    job,
                    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
                    (&raw const limits).cast(),
                    size_of::<JobObjectExtendedLimitInformation>() as u32,
                )
            };
            if assigned == 0 {
                unsafe { CloseHandle(job) };
                return None;
            }
            Some(Self { job })
        }

        pub(super) fn adopt(&self, child: &Child) -> bool {
            unsafe { AssignProcessToJobObject(self.job, child.as_raw_handle().cast()) != 0 }
        }
    }

    impl Drop for ProcessTreeGuard {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.job) };
        }
    }
}

/// Creates Git processes without a visible console on Windows.
/// Git is invoked by the task-summary worker as well as user-requested Git tools.
fn git_command() -> Command {
    workspace_command("git")
}

/// Creates workspace child processes without a visible console on Windows.
/// This keeps PowerShell and other command-line tools from flashing a console when
/// an approved tool call is executed from the Desktop app.
fn workspace_command(executable: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(executable);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

fn temporary_sibling(path: &Path) -> PathBuf {
    match path.file_name().and_then(|value| value.to_str()) {
        Some(file_name) => path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(format!(".{file_name}.xcoding.tmp")),
        None => path.with_extension("xcoding.tmp"),
    }
}

/// Write text to `path` as UTF-8 using an atomic rename strategy.
fn write_text_utf8(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = temporary_sibling(path);
    {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_data()?;
    }
    #[cfg(windows)]
    {
        if path.exists() {
            fs::remove_file(path)?;
        }
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

/// Returns true only for a tightly constrained PowerShell HTTP request to a loopback API.
/// This is intentionally conservative because it gates the remembered high-risk approval.
pub fn is_local_api_request(tool_call: &ToolCall) -> bool {
    if tool_call.name != ToolName::RunCommand {
        return false;
    }
    let Ok(args) = parse_arguments::<RunCommandArgs>(&tool_call.arguments) else {
        return false;
    };
    is_local_api_command(&args)
}

fn is_local_api_command(args: &RunCommandArgs) -> bool {
    let executable = args
        .executable
        .rsplit(|character| character == '\\' || character == '/')
        .next()
        .unwrap_or(args.executable.as_str())
        .trim()
        .to_ascii_lowercase();
    if !matches!(
        executable.as_str(),
        "powershell" | "powershell.exe" | "pwsh" | "pwsh.exe"
    ) {
        return false;
    }

    let Some(command_index) = args.args.iter().position(|argument| {
        argument.eq_ignore_ascii_case("-command") || argument.eq_ignore_ascii_case("-c")
    }) else {
        return false;
    };
    if command_index + 2 != args.args.len() {
        return false;
    }

    is_safe_local_api_script(&args.args[command_index + 1])
}

fn is_safe_local_api_script(script: &str) -> bool {
    let normalized = script.trim();
    if normalized.is_empty() {
        return false;
    }
    let lower = normalized.to_ascii_lowercase();
    if lower.contains("$(")
        || lower.contains('|')
        || lower.contains('&')
        || lower.contains(char::from(96))
    {
        return false;
    }

    const FORBIDDEN_COMMANDS: &[&str] = &[
        "remove-item",
        "move-item",
        "copy-item",
        "new-item",
        "set-content",
        "add-content",
        "clear-content",
        "out-file",
        "set-itemproperty",
        "invoke-expression",
        "start-process",
        "stop-process",
        "restart-computer",
        "set-executionpolicy",
    ];
    if FORBIDDEN_COMMANDS
        .iter()
        .any(|command| lower.contains(command))
    {
        return false;
    }

    let urls = extract_http_urls(normalized);
    if urls.is_empty() || urls.iter().any(|url| !is_loopback_http_url(url)) {
        return false;
    }

    let mut invoke_count = 0;
    for statement in split_powershell_statements(normalized) {
        let statement = statement.trim();
        if statement.is_empty() {
            continue;
        }
        let statement_lower = statement.to_ascii_lowercase();
        if statement_lower == "try" || statement_lower == "catch" {
            continue;
        }
        if is_local_api_invoke(&statement_lower) {
            invoke_count += 1;
            continue;
        }
        if let Some((_, expression)) = statement_lower.split_once('=') {
            if is_local_api_invoke(expression.trim()) {
                invoke_count += 1;
                continue;
            }
        }
        if is_local_api_output_reference(&statement_lower) {
            continue;
        }
        return false;
    }

    invoke_count == 1
}

fn is_local_api_invoke(expression: &str) -> bool {
    expression.starts_with("invoke-webrequest") || expression.starts_with("invoke-restmethod")
}

fn is_local_api_output_reference(statement: &str) -> bool {
    statement.starts_with('$')
        && statement.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '$' | '_' | '.')
        })
}

fn split_powershell_statements(script: &str) -> Vec<&str> {
    let mut statements = Vec::new();
    let mut start = 0;
    let mut quote = None;
    for (index, character) in script.char_indices() {
        match quote {
            Some(delimiter) if character == delimiter => quote = None,
            Some(_) => {}
            None if character == char::from(39) || character == '"' => quote = Some(character),
            None if matches!(character, ';' | '{' | '}' | '\n' | '\r') => {
                statements.push(&script[start..index]);
                start = index + character.len_utf8();
            }
            None => {}
        }
    }
    statements.push(&script[start..]);
    statements
}

fn extract_http_urls(script: &str) -> Vec<&str> {
    let lower = script.to_ascii_lowercase();
    let mut urls = Vec::new();
    let mut search_start = 0;
    while search_start < lower.len() {
        let http = lower[search_start..]
            .find("http://")
            .map(|offset| search_start + offset);
        let https = lower[search_start..]
            .find("https://")
            .map(|offset| search_start + offset);
        let Some(start) = [http, https].into_iter().flatten().min() else {
            break;
        };
        let rest = &script[start..];
        let end = rest
            .find(|character: char| {
                character.is_whitespace()
                    || character == char::from(39)
                    || character == '"'
                    || character == char::from(96)
            })
            .unwrap_or(rest.len());
        urls.push(&rest[..end]);
        search_start = start + end.max(1);
    }
    urls
}

fn is_loopback_http_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let Some(authority) = lower
        .strip_prefix("http://")
        .or_else(|| lower.strip_prefix("https://"))
        .map(|rest| rest.split(['/', '?', '#']).next().unwrap_or(rest))
    else {
        return false;
    };
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    if let Some(rest) = authority.strip_prefix('[') {
        let Some((host, suffix)) = rest.split_once(']') else {
            return false;
        };
        return host == "::1" && (suffix.is_empty() || suffix.starts_with(':'));
    }

    let host = authority.split(':').next().unwrap_or_default();
    matches!(host, "127.0.0.1" | "localhost")
}

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("workspace does not exist: {0}")]
    WorkspaceNotFound(String),
    #[error("path is outside the workspace: {0}")]
    PathOutsideWorkspace(String),
    #[error("path is not a directory: {0}")]
    NotDirectory(String),
    #[error("path is not a regular file: {0}")]
    NotFile(String),
    #[error("file is too large to read: {0}")]
    FileTooLarge(String),
    #[error("tool arguments are invalid: {0}")]
    InvalidArguments(String),
    #[error("permission was not granted")]
    PermissionDenied,
    #[error(
        "patch conflict on {0}: file contents changed; re-read the file and retry with updated old_text"
    )]
    PatchConflict(String),
    #[error("command arguments are invalid: {0}")]
    InvalidCommand(String),
    #[error("command blocked by policy ({code}): {reason}")]
    CommandPolicyDenied { code: String, reason: String },
    #[error("command was cancelled")]
    Cancelled,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl ToolError {
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::PatchConflict(_) => Some("patch_conflict"),
            Self::PermissionDenied => Some("permission_denied"),
            Self::Cancelled => Some("cancelled"),
            Self::InvalidArguments(_) => Some("invalid_arguments"),
            Self::InvalidCommand(_) => Some("invalid_command"),
            Self::CommandPolicyDenied { .. } => Some("command_policy_denied"),
            _ => None,
        }
    }

    pub fn path(&self) -> Option<&str> {
        match self {
            Self::PatchConflict(path)
            | Self::NotDirectory(path)
            | Self::NotFile(path)
            | Self::FileTooLarge(path)
            | Self::PathOutsideWorkspace(path)
            | Self::WorkspaceNotFound(path) => Some(path.as_str()),
            _ => None,
        }
    }

    pub fn tool_result_value(&self) -> Value {
        let mut value = json!({ "error": self.to_string() });
        if let Some(code) = self.code() {
            value["code"] = json!(code);
        }
        if let Some(path) = self.path() {
            value["path"] = json!(path);
        }
        if let Self::CommandPolicyDenied { code, reason } = self {
            value["policy_code"] = json!(code);
            value["reason"] = json!(reason);
            value["hint"] = json!(
                "This command is hard-denied by XCoding policy or the workspace denylist. Choose a safer command."
            );
        }
        if matches!(self, Self::PatchConflict(_)) {
            value["hint"] = json!(
                "Re-read the file with read_file, then retry apply_patch using the current contents as old_text."
            );
        }
        value
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolExecution {
    pub output: Value,
    pub summary: String,
}

pub struct ToolRegistry {
    workspace_root: PathBuf,
    command_allowlist: Vec<String>,
    command_denylist: Vec<String>,
    plugin_config: PluginConfig,
}

impl ToolRegistry {
    pub fn new(workspace_root: impl AsRef<Path>) -> Result<Self, ToolError> {
        Self::new_with_plugin_config(workspace_root, load_plugin_config())
    }

    pub fn new_with_plugin_config(
        workspace_root: impl AsRef<Path>,
        plugin_config: PluginConfig,
    ) -> Result<Self, ToolError> {
        let workspace_root = workspace_root.as_ref();
        if !workspace_root.is_dir() {
            return Err(ToolError::WorkspaceNotFound(
                workspace_root.display().to_string(),
            ));
        }

        let workspace_root = workspace_root.canonicalize()?;
        let command_allowlist = load_command_allowlist(&workspace_root);
        let command_denylist = load_command_denylist(&workspace_root);
        Ok(Self {
            workspace_root,
            command_allowlist,
            command_denylist,
            plugin_config,
        })
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn command_allowlist(&self) -> &[String] {
        &self.command_allowlist
    }

    pub fn command_denylist(&self) -> &[String] {
        &self.command_denylist
    }

    pub fn plugin_config(&self) -> &PluginConfig {
        &self.plugin_config
    }

    pub fn execute(&self, mode: &Mode, tool_call: &ToolCall) -> Result<ToolExecution, ToolError> {
        let (kind, high_risk, allowlisted) = self.permission_for(tool_call)?;
        if evaluate_detailed(mode, kind, high_risk, allowlisted) != PermissionDecision::Allow {
            return Err(ToolError::PermissionDenied);
        }
        self.execute_authorized(tool_call)
    }

    /// Returns `(kind, high_risk, command_allowlisted)`.
    pub fn permission_for(
        &self,
        tool_call: &ToolCall,
    ) -> Result<(PermissionKind, bool, bool), ToolError> {
        match tool_call.name {
            ToolName::ListDir
            | ToolName::ReadFile
            | ToolName::SearchCode
            | ToolName::LoadSkill
            | ToolName::GitStatus
            | ToolName::GitDiff
            | ToolName::GitLog
            | ToolName::GitShow
            | ToolName::BrowserState
            | ToolName::UpdatePlan => Ok((PermissionKind::Read, false, false)),
            ToolName::GitAdd
            | ToolName::GitCommit
            | ToolName::GitPush
            | ToolName::GitFetch
            | ToolName::GitPull => {
                // Mutates .git index/refs or talks to a remote; always high-risk write.
                Ok((PermissionKind::Write, true, false))
            }
            ToolName::Mcp => {
                // External MCP tools can perform arbitrary side effects; always ask.
                Ok((PermissionKind::Exec, true, false))
            }
            ToolName::ApplyPatch => {
                let args: ApplyPatchArgs = parse_arguments(&tool_call.arguments)?;
                Ok((PermissionKind::Write, is_high_risk_path(&args.path), false))
            }
            ToolName::RunCommand => {
                let args: RunCommandArgs = parse_arguments(&tool_call.arguments)?;
                let assessment = assess_command_with_lists(
                    &args.executable,
                    &args.args,
                    &self.command_allowlist,
                    &self.command_denylist,
                );
                if assessment.decision == PermissionDecision::Deny {
                    return Err(ToolError::CommandPolicyDenied {
                        code: assessment.code.as_str().to_owned(),
                        reason: assessment.reason,
                    });
                }
                self.validate_git_command_paths(&args.executable, &args.args, args.cwd.as_deref())?;
                Ok((
                    PermissionKind::Exec,
                    assessment.high_risk,
                    assessment.allowlisted,
                ))
            }
        }
    }

    pub fn patch_preview(&self, tool_call: &ToolCall) -> Result<PatchPreview, ToolError> {
        if tool_call.name != ToolName::ApplyPatch {
            return Err(ToolError::InvalidArguments(
                "patch preview requires apply_patch".to_owned(),
            ));
        }
        let args: ApplyPatchArgs = parse_arguments(&tool_call.arguments)?;
        let path = self.resolve_writable(&args.path)?;
        let file_existed = path.exists();
        let current = if file_existed {
            fs::read_to_string(&path)?
        } else {
            String::new()
        };
        if current != args.old_text {
            return Err(ToolError::PatchConflict(self.relative_path(&path)));
        }
        Ok(PatchPreview {
            path: self.relative_path(&path),
            file_existed,
            old_text: args.old_text,
            new_text: args.new_text,
        })
    }

    pub fn execute_authorized(&self, tool_call: &ToolCall) -> Result<ToolExecution, ToolError> {
        self.execute_authorized_cancellable(tool_call, &|| false)
    }

    pub fn execute_authorized_cancellable(
        &self,
        tool_call: &ToolCall,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<ToolExecution, ToolError> {
        match tool_call.name {
            ToolName::ListDir => self.list_dir(parse_arguments(&tool_call.arguments)?),
            ToolName::ReadFile => self.read_file(parse_arguments(&tool_call.arguments)?),
            ToolName::SearchCode => self.search_code(parse_arguments(&tool_call.arguments)?),
            ToolName::LoadSkill => self.load_skill(parse_arguments(&tool_call.arguments)?),
            ToolName::ApplyPatch => self.apply_patch(parse_arguments(&tool_call.arguments)?),
            ToolName::RunCommand => {
                self.run_command(parse_arguments(&tool_call.arguments)?, is_cancelled)
            }
            ToolName::GitStatus => self.git_status(parse_arguments(&tool_call.arguments)?),
            ToolName::GitDiff => self.git_diff(parse_arguments(&tool_call.arguments)?),
            ToolName::GitLog => self.git_log(parse_arguments(&tool_call.arguments)?),
            ToolName::GitShow => self.git_show(parse_arguments(&tool_call.arguments)?),
            ToolName::GitAdd => self.git_add(parse_arguments(&tool_call.arguments)?),
            ToolName::GitCommit => self.git_commit(parse_arguments(&tool_call.arguments)?),
            ToolName::GitPush => self.git_push(parse_arguments(&tool_call.arguments)?),
            ToolName::GitFetch => self.git_fetch(parse_arguments(&tool_call.arguments)?),
            ToolName::GitPull => self.git_pull(parse_arguments(&tool_call.arguments)?),
            ToolName::BrowserState => self.browser_state(),
            ToolName::UpdatePlan => update_plan(parse_arguments(&tool_call.arguments)?),
            ToolName::Mcp => Err(ToolError::InvalidArguments(
                "MCP tools must be executed by the agent MCP runtime".to_owned(),
            )),
        }
    }

    pub fn rollback_patch(
        &self,
        path: &str,
        expected_text: &str,
        original_text: Option<&str>,
    ) -> Result<ToolExecution, ToolError> {
        let path = self.resolve_writable(path)?;
        let current = if path.exists() {
            fs::read_to_string(&path)?
        } else {
            String::new()
        };
        if current != expected_text {
            return Err(ToolError::PatchConflict(self.relative_path(&path)));
        }
        match original_text {
            Some(original_text) => self.write_atomically(&path, original_text)?,
            None if path.exists() => fs::remove_file(&path)?,
            None => {}
        }
        let relative_path = self.relative_path(&path);
        Ok(ToolExecution {
            output: json!({ "path": relative_path, "changed": true, "rolled_back": true }),
            summary: format!("Restored {relative_path}"),
        })
    }

    fn browser_state(&self) -> Result<ToolExecution, ToolError> {
        let path = browser_state_path();
        let missing = json!({ "available": false });
        let Ok(raw) = fs::read_to_string(&path) else {
            return Ok(ToolExecution {
                output: missing,
                summary: "Embedded browser is not available".to_owned(),
            });
        };
        let Ok(value) = serde_json::from_str::<Value>(&raw) else {
            return Ok(ToolExecution {
                output: missing,
                summary: "Embedded browser is not available".to_owned(),
            });
        };
        let available = value
            .get("available")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !available {
            return Ok(ToolExecution {
                output: json!({ "available": false }),
                summary: "Embedded browser is not available".to_owned(),
            });
        }
        let url = value
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let title = value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let visible = value
            .get("visible")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let updated_at = value.get("updated_at").cloned().unwrap_or(Value::Null);
        let summary = if url.trim().is_empty() {
            "Embedded browser is available".to_owned()
        } else if title.trim().is_empty() {
            format!("Embedded browser at {url}")
        } else {
            format!("Embedded browser at {url} ({title})")
        };
        Ok(ToolExecution {
            output: json!({
                "available": true,
                "url": url,
                "title": title,
                "visible": visible,
                "updated_at": updated_at,
            }),
            summary,
        })
    }

    fn list_dir(&self, args: ListDirArgs) -> Result<ToolExecution, ToolError> {
        let path = self.resolve(&args.path)?;
        if !path.is_dir() {
            return Err(ToolError::NotDirectory(self.relative_path(&path)));
        }

        let limit = bounded(args.max_entries, DEFAULT_LIST_ENTRIES, MAX_LIST_ENTRIES);
        let mut entries = fs::read_dir(&path)?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let file_type = entry.file_type().ok()?;
                if file_type.is_symlink() {
                    return None;
                }
                let kind = if file_type.is_dir() {
                    "dir"
                } else if file_type.is_file() {
                    "file"
                } else {
                    "other"
                };
                Some(DirectoryEntry {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    kind: kind.to_owned(),
                })
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        let truncated = entries.len() > limit;
        entries.truncate(limit);

        let path = self.relative_path(&path);
        Ok(ToolExecution {
            output: serde_json::to_value(ListDirOutput {
                path: path.clone(),
                entries,
                truncated,
            })?,
            summary: format!("Listed {path}"),
        })
    }

    fn read_file(&self, args: ReadFileArgs) -> Result<ToolExecution, ToolError> {
        let path = self.resolve(&args.path)?;
        if !path.is_file() {
            return Err(ToolError::NotFile(self.relative_path(&path)));
        }
        let file_size = path.metadata()?.len();
        if file_size > MAX_READ_BYTES {
            return Err(ToolError::FileTooLarge(self.relative_path(&path)));
        }

        let relative_path = self.relative_path(&path);
        if is_high_sensitivity_file(&relative_path) {
            return Ok(ToolExecution {
                output: serde_json::to_value(ReadFileOutput {
                    path: relative_path.clone(),
                    content: String::new(),
                    start_line: 0,
                    end_line: 0,
                    truncated: false,
                    content_redacted: true,
                    redaction_reason: Some("sensitive credential file; content withheld".to_owned()),
                })?,
                summary: format!(
                    "Read metadata for sensitive file {relative_path} ({file_size} bytes)"
                ),
            });
        }

        let mut content = fs::read_to_string(&path)?;
        let is_background_log = is_background_log_path(&relative_path);
        let content_redacted = is_config_file(&relative_path) || is_background_log;
        if content_redacted {
            content = if is_background_log {
                redact_log_text(&content)
            } else {
                redact_config_text(&content)
            };
        }
        let lines = content.lines().collect::<Vec<_>>();
        let start_line = args.start_line.unwrap_or(1).max(1);
        let requested_end = args
            .end_line
            .unwrap_or_else(|| start_line.saturating_add(DEFAULT_READ_LINES - 1));
        let end_line = requested_end
            .min(start_line.saturating_add(MAX_READ_LINES - 1))
            .min(lines.len());
        let content = if start_line <= end_line {
            lines[(start_line - 1)..end_line].join("\n")
        } else {
            String::new()
        };
        Ok(ToolExecution {
            output: serde_json::to_value(ReadFileOutput {
                path: relative_path.clone(),
                content,
                start_line,
                end_line,
                truncated: end_line < lines.len(),
                content_redacted,
                redaction_reason: content_redacted.then_some(if is_background_log {
                    "background log; secret values redacted".to_owned()
                } else {
                    "configuration file; secret values redacted".to_owned()
                }),
            })?,
            summary: format!("Read {relative_path}:{start_line}-{end_line}"),
        })
    }

    fn search_code(&self, args: SearchCodeArgs) -> Result<ToolExecution, ToolError> {
        if args.query.trim().is_empty() {
            return Err(ToolError::InvalidArguments(
                "query must not be empty".to_owned(),
            ));
        }

        let root = self.resolve(&args.path)?;
        if !root.is_dir() {
            return Err(ToolError::NotDirectory(self.relative_path(&root)));
        }

        let limit = bounded(args.max_results, DEFAULT_SEARCH_RESULTS, MAX_SEARCH_RESULTS);
        let context_lines = args
            .context_lines
            .unwrap_or(0)
            .min(MAX_SEARCH_CONTEXT_LINES);
        let glob = args
            .glob
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if let Some(pattern) = glob {
            if pattern.contains('[') || pattern.contains(']') {
                return Err(ToolError::InvalidArguments(
                    "glob character classes are not supported".to_owned(),
                ));
            }
        }

        let query_cmp = if args.case_insensitive {
            args.query.to_lowercase()
        } else {
            args.query.clone()
        };

        let mut pending = VecDeque::from([root]);
        let mut candidates = Vec::new();
        let candidate_cap = (limit.saturating_mul(5)).clamp(limit, MAX_SEARCH_CANDIDATES);

        'walk: while let Some(directory) = pending.pop_front() {
            for entry in fs::read_dir(directory)?.filter_map(Result::ok) {
                let file_type = entry.file_type()?;
                if file_type.is_symlink() {
                    continue;
                }
                if file_type.is_dir() {
                    if !is_ignored_directory(&entry.file_name()) {
                        pending.push_back(entry.path());
                    }
                    continue;
                }
                if !file_type.is_file() || entry.metadata()?.len() > MAX_SEARCH_FILE_BYTES {
                    continue;
                }

                let relative = self.relative_path(&entry.path());
                if let Some(pattern) = glob {
                    if !path_matches_glob(&relative, pattern, args.case_insensitive) {
                        continue;
                    }
                }
                if is_low_value_search_file(&relative) {
                    continue;
                }

                let Ok(content) = fs::read_to_string(entry.path()) else {
                    continue;
                };
                let lines: Vec<&str> = content.lines().collect();
                for (index, line) in lines.iter().enumerate() {
                    let matched = if args.case_insensitive {
                        line.to_lowercase().contains(&query_cmp)
                    } else {
                        line.contains(&query_cmp)
                    };
                    if !matched {
                        continue;
                    }

                    let before = if context_lines == 0 {
                        Vec::new()
                    } else {
                        let start = index.saturating_sub(context_lines);
                        lines[start..index]
                            .iter()
                            .map(|value| (*value).to_owned())
                            .collect()
                    };
                    let after = if context_lines == 0 {
                        Vec::new()
                    } else {
                        let end = (index + 1 + context_lines).min(lines.len());
                        lines[index + 1..end]
                            .iter()
                            .map(|value| (*value).to_owned())
                            .collect()
                    };

                    candidates.push(RankedSearchHit {
                        score: path_rank_score(&relative),
                        result: SearchResult {
                            path: relative.clone(),
                            line: index + 1,
                            text: (*line).to_owned(),
                            before,
                            after,
                        },
                    });
                    if candidates.len() >= candidate_cap {
                        break 'walk;
                    }
                }
            }
        }

        candidates.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.result.path.cmp(&right.result.path))
                .then_with(|| left.result.line.cmp(&right.result.line))
        });
        let over_limit = candidates.len() >= candidate_cap || candidates.len() > limit;
        let results: Vec<SearchResult> = candidates
            .into_iter()
            .take(limit)
            .map(|hit| hit.result)
            .collect();
        let (results, over_budget) = cap_json_items(results);

        Ok(ToolExecution {
            output: json!({ "results": results, "truncated": over_limit || over_budget }),
            summary: format!("Searched for {:?}", args.query),
        })
    }

    fn apply_patch(&self, args: ApplyPatchArgs) -> Result<ToolExecution, ToolError> {
        let path = self.resolve_writable(&args.path)?;
        let file_existed = path.exists();
        let current = if file_existed {
            fs::read_to_string(&path)?
        } else {
            String::new()
        };
        if current != args.old_text {
            return Err(ToolError::PatchConflict(self.relative_path(&path)));
        }

        self.write_atomically(&path, &args.new_text)?;

        let path = self.relative_path(&path);
        Ok(ToolExecution {
            output: json!({ "path": path, "changed": true }),
            summary: format!("Applied patch to {path}"),
        })
    }

    fn run_command(
        &self,
        args: RunCommandArgs,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<ToolExecution, ToolError> {
        let assessment = assess_command_with_lists(
            &args.executable,
            &args.args,
            &self.command_allowlist,
            &self.command_denylist,
        );
        if assessment.decision == PermissionDecision::Deny {
            return Err(ToolError::CommandPolicyDenied {
                code: assessment.code.as_str().to_owned(),
                reason: assessment.reason,
            });
        }
        self.validate_git_command_paths(&args.executable, &args.args, args.cwd.as_deref())?;

        // Never inherit the server RPC stdin pipe: some tools (notably git on
        // Windows) can hang when stdin is an open parent pipe still owned by the
        // JSON-RPC loop.
        // A relative executable path is resolved against the workspace root here
        // because `current_dir` does not affect how the OS looks up the program.
        let program = self.resolve_executable(&args.executable)?;

        // Requested working directory, still confined to the workspace.
        let working_dir = match args.cwd.as_deref() {
            Some(requested) => {
                let resolved = self.resolve(requested)?;
                if !resolved.is_dir() {
                    return Err(ToolError::NotDirectory(self.relative_path(&resolved)));
                }
                resolved
            }
            None => self.workspace_root.clone(),
        };
        let reported_cwd = self.relative_path(&working_dir);

        // Background launch: the caller wants the process to outlive this call, so
        // it is neither awaited nor bound to the job object. Output goes to null
        // because nobody stays behind to drain a pipe, and a full pipe buffer
        // would eventually block the service itself.
        if args.background {
            // A pipe is not an option here: nobody stays behind to drain it and a
            // full buffer would block the service. A file takes the output without
            // a reader, so a launch that fails is still diagnosable afterwards.
            let log_path = self.background_log_path(&args.executable)?;
            let log_file = fs::File::create(&log_path)?;
            let log_errors = log_file.try_clone()?;
            let reported_log = self.relative_path(&log_path);
            let child = workspace_command(&program)
                .args(&args.args)
                .current_dir(&working_dir)
                .stdin(Stdio::null())
                .stdout(Stdio::from(log_file))
                .stderr(Stdio::from(log_errors))
                .spawn()?;
            let mut child = child;
            let pid = child.id();

            // Observe the launch briefly. Reporting a pid for a process that
            // already died is what makes a failed service start unreadable.
            thread::sleep(BACKGROUND_STARTUP_PROBE);
            if let Some(status) = child.try_wait()? {
                let log_tail = read_tail(&log_path, BACKGROUND_LOG_TAIL_BYTES);
                return Ok(ToolExecution {
                    output: json!({
                        "executable": args.executable,
                        "args": args.args,
                        "cwd": reported_cwd,
                        "success": false,
                        "background": true,
                        "exited_immediately": true,
                        "exit_code": status.code(),
                        "log_path": reported_log,
                        "log_tail": log_tail,
                    }),
                    summary: format!(
                        "Exited immediately with code {:?}; output in {reported_log}",
                        status.code()
                    ),
                });
            }

            // With a port to watch, the launch is only done once the service
            // actually accepts connections. Without it, a live pid is all this
            // call can honestly report.
            if let Some(port) = args.ready_port {
                let ready_timeout = args.effective_ready_timeout();
                let outcome = await_port_ready(&mut child, port, ready_timeout, is_cancelled)?;
                let url = format!("http://127.0.0.1:{port}");
                return Ok(match outcome {
                    PortReadyOutcome::Ready { waited } => ToolExecution {
                        output: json!({
                            "executable": args.executable,
                            "args": args.args,
                            "cwd": reported_cwd,
                            "success": true,
                            "background": true,
                            "exited_immediately": false,
                            "pid": pid,
                            "ready": true,
                            "ready_port": port,
                            "url": url,
                            "waited_ms": waited.as_millis(),
                            "log_path": reported_log,
                        }),
                        summary: format!("Service ready on {url} with pid {pid}"),
                    },
                    PortReadyOutcome::Exited { status } => ToolExecution {
                        output: json!({
                            "executable": args.executable,
                            "args": args.args,
                            "cwd": reported_cwd,
                            "success": false,
                            "background": true,
                            "exited_immediately": true,
                            "exit_code": status.code(),
                            "ready": false,
                            "ready_port": port,
                            "log_path": reported_log,
                            "log_tail": read_tail(&log_path, BACKGROUND_LOG_TAIL_BYTES),
                        }),
                        summary: format!(
                            "Died before listening on port {port} with code {:?}; output in {reported_log}",
                            status.code()
                        ),
                    },
                    // Still alive but silent on the port: the pid stays reported so
                    // the caller can stop it instead of leaking a half-started service.
                    PortReadyOutcome::TimedOut => ToolExecution {
                        output: json!({
                            "executable": args.executable,
                            "args": args.args,
                            "cwd": reported_cwd,
                            "success": false,
                            "background": true,
                            "exited_immediately": false,
                            "pid": pid,
                            "ready": false,
                            "ready_port": port,
                            "ready_timed_out": true,
                            "waited_ms": ready_timeout.as_millis(),
                            "log_path": reported_log,
                            "log_tail": read_tail(&log_path, BACKGROUND_LOG_TAIL_BYTES),
                        }),
                        summary: format!(
                            "Still not listening on port {port} after {}s with pid {pid}; output in {reported_log}",
                            ready_timeout.as_secs()
                        ),
                    },
                });
            }

            return Ok(ToolExecution {
                output: json!({
                    "executable": args.executable,
                    "args": args.args,
                    "cwd": reported_cwd,
                    "success": true,
                    "background": true,
                    "exited_immediately": false,
                    "pid": pid,
                    "ready": Value::Null,
                    "log_path": reported_log,
                }),
                summary: format!("Started in background with pid {pid}; output in {reported_log}"),
            });
        }

        let mut child = workspace_command(&program)
            .args(&args.args)
            .current_dir(&working_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_PAGER", "cat")
            .env("PAGER", "cat")
            .spawn()?;

        // Bind the whole spawned tree to this call. Without it, a grandchild that
        // detached from the direct child (`cmd /C start`, `subprocess.Popen`, ...)
        // outlives the tool call and keeps holding ports and file locks.
        #[cfg(windows)]
        let _process_tree = {
            let guard = job_object::ProcessTreeGuard::new();
            if let Some(guard) = &guard {
                // A failed adopt is not fatal: cleanup then degrades to the
                // previous direct-child-only behaviour.
                let _ = guard.adopt(&child);
            }
            guard
        };

        let stdout_pipe = child.stdout.take().expect("stdout pipe");
        let stderr_pipe = child.stderr.take().expect("stderr pipe");
        let (stdout_buffer, stdout_done) = drain_pipe(stdout_pipe);
        let (stderr_buffer, stderr_done) = drain_pipe(stderr_pipe);

        let timeout = args.effective_timeout();
        let deadline = Instant::now() + timeout;
        let mut timed_out = false;
        let status = loop {
            if is_cancelled() {
                let _ = child.kill();
                let _ = child.wait();
                await_pipe_drain([&stdout_done, &stderr_done], PIPE_DRAIN_GRACE);
                return Err(ToolError::Cancelled);
            }
            match child.try_wait()? {
                Some(status) => break status,
                None => {
                    if Instant::now() >= deadline {
                        // A foreground resident service never exits on its own, so
                        // report the output captured so far instead of holding the
                        // turn open until the model or the user gives up.
                        timed_out = true;
                        let _ = child.kill();
                        break child.wait()?;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
            }
        };

        // The child is gone, so anything still holding the pipes is a surviving
        // grandchild. Wait only briefly for trailing output, then report what we
        // have instead of blocking the turn forever.
        await_pipe_drain([&stdout_done, &stderr_done], PIPE_DRAIN_GRACE);
        let stdout = truncate_output(&String::from_utf8_lossy(&snapshot_pipe(&stdout_buffer)));
        let stderr = truncate_output(&String::from_utf8_lossy(&snapshot_pipe(&stderr_buffer)));
        let success = status.success() && !timed_out;
        Ok(ToolExecution {
            output: json!({
                "executable": args.executable,
                "args": args.args,
                "cwd": reported_cwd,
                "success": success,
                "exit_code": status.code(),
                "stdout": stdout,
                "stderr": stderr,
                "timed_out": timed_out,
            }),
            summary: if timed_out {
                format!(
                    "Command timed out after {}s and was terminated",
                    timeout.as_secs()
                )
            } else if success {
                "Command completed".to_owned()
            } else {
                "Command failed".to_owned()
            },
        })
    }

    fn load_skill(&self, args: LoadSkillArgs) -> Result<ToolExecution, ToolError> {
        let name = args.name.trim();
        if !is_valid_skill_name(name) {
            return Err(ToolError::InvalidArguments(
                "skill name must be 1-64 chars of [A-Za-z0-9._-] starting with alphanumeric"
                    .to_owned(),
            ));
        }
        let workspace_relative = format!(".xcoding/skills/{name}/SKILL.md");
        let workspace_candidate = self.workspace_root.join(&workspace_relative);
        let (path, display_path, source) = if workspace_candidate.is_file()
            && self
                .plugin_config
                .skill_enabled
                .get(&format!("workspace:{name}"))
                .copied()
                .unwrap_or(true)
        {
            (
                self.resolve(&workspace_relative)?,
                workspace_relative,
                "workspace",
            )
        } else {
            let user_path = user_skill_root().join(name).join("SKILL.md");
            if !user_path.is_file()
                || !self
                    .plugin_config
                    .skill_enabled
                    .get(&format!("user:{name}"))
                    .copied()
                    .unwrap_or(true)
            {
                return Err(ToolError::NotFile(workspace_relative));
            }
            (
                user_path,
                format!("~/.xcoding/skills/{name}/SKILL.md"),
                "user",
            )
        };
        if !path.is_file() {
            return Err(ToolError::NotFile(display_path));
        }
        if path.metadata()?.len() > MAX_READ_BYTES {
            return Err(ToolError::FileTooLarge(self.relative_path(&path)));
        }
        let raw = fs::read_to_string(&path)?;
        let parsed = parse_skill_file(name, &raw);
        let content = if parsed.body.chars().count() > MAX_SKILL_CONTENT_CHARS {
            let mut truncated = parsed
                .body
                .chars()
                .take(MAX_SKILL_CONTENT_CHARS)
                .collect::<String>();
            truncated.push_str("\n...[truncated skill content]...");
            truncated
        } else {
            parsed.body
        };
        Ok(ToolExecution {
            output: json!({
                "name": name,
                "path": display_path,
                "source": source,
                "description": parsed.description,
                "content": content,
            }),
            summary: format!("Loaded skill {name}"),
        })
    }

    fn git_status(&self, args: GitStatusArgs) -> Result<ToolExecution, ToolError> {
        let pathspec = args
            .path
            .as_deref()
            .filter(|value| !value.trim().is_empty());
        if let Some(path) = pathspec {
            let _ = checked_relative_path(path)?;
        }

        let mut command = git_command();
        command
            .arg("status")
            .arg("--porcelain=v1")
            .arg("--branch")
            // `normal` collapses an untracked directory into a single entry.
            // `all` expands every file inside it, which dominates the payload in
            // workspaces with build output or vendored trees.
            .arg("--untracked-files=normal")
            .current_dir(&self.workspace_root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(path) = pathspec {
            command.arg("--").arg(path);
        }

        let output = command.output()?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if !output.status.success() {
            return Err(ToolError::InvalidCommand(if stderr.trim().is_empty() {
                format!(
                    "git status failed with exit code {:?}",
                    output.status.code()
                )
            } else {
                truncate_output(&stderr)
            }));
        }

        let entries = parse_git_status_lines(&stdout);
        let branch = entries
            .iter()
            .find_map(|entry| entry.get("branch").and_then(Value::as_str))
            .map(str::to_owned);
        let (entries, truncated) = cap_json_items(entries);
        Ok(ToolExecution {
            output: json!({
                "path": pathspec.unwrap_or("."),
                "branch": branch,
                "entries": entries,
                "truncated": truncated,
                "raw": truncate_output(&stdout),
            }),
            summary: format!("Git status for {}", pathspec.unwrap_or(".")),
        })
    }

    fn git_diff(&self, args: GitDiffArgs) -> Result<ToolExecution, ToolError> {
        let pathspec = args
            .path
            .as_deref()
            .filter(|value| !value.trim().is_empty());
        if let Some(path) = pathspec {
            let _ = checked_relative_path(path)?;
        }

        let mut staged = git_command();
        staged
            .arg("diff")
            .arg("--cached")
            .arg("--no-ext-diff")
            .arg("--no-color")
            .current_dir(&self.workspace_root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(path) = pathspec {
            staged.arg("--").arg(path);
        }

        let mut unstaged = git_command();
        unstaged
            .arg("diff")
            .arg("--no-ext-diff")
            .arg("--no-color")
            .current_dir(&self.workspace_root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(path) = pathspec {
            unstaged.arg("--").arg(path);
        }

        let staged_output = staged.output()?;
        let unstaged_output = unstaged.output()?;
        if !staged_output.status.success() || !unstaged_output.status.success() {
            let stderr = format!(
                "{}\n{}",
                String::from_utf8_lossy(&staged_output.stderr),
                String::from_utf8_lossy(&unstaged_output.stderr)
            );
            return Err(ToolError::InvalidCommand(if stderr.trim().is_empty() {
                "git diff failed".to_owned()
            } else {
                truncate_output(stderr.trim())
            }));
        }

        let staged_diff = truncate_output(&String::from_utf8_lossy(&staged_output.stdout));
        let unstaged_diff = truncate_output(&String::from_utf8_lossy(&unstaged_output.stdout));
        Ok(ToolExecution {
            output: json!({
                "path": pathspec.unwrap_or("."),
                "staged": staged_diff,
                "unstaged": unstaged_diff,
            }),
            summary: format!("Git diff for {}", pathspec.unwrap_or(".")),
        })
    }

    fn git_log(&self, args: GitLogArgs) -> Result<ToolExecution, ToolError> {
        let pathspec = args
            .path
            .as_deref()
            .filter(|value| !value.trim().is_empty());
        if let Some(path) = pathspec {
            let _ = checked_relative_path(path)?;
        }
        let max_count = bounded(args.max_count, DEFAULT_GIT_LOG_COUNT, MAX_GIT_LOG_COUNT);

        let mut command = git_command();
        command
            .arg("log")
            .arg(format!("--max-count={max_count}"))
            .arg("--pretty=format:%H%x00%h%x00%an%x00%ae%x00%aI%x00%s%x00%b%x1e")
            .current_dir(&self.workspace_root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_PAGER", "cat")
            .env("PAGER", "cat");
        if let Some(path) = pathspec {
            command.arg("--").arg(path);
        }

        let output = command.output()?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if !output.status.success() {
            return Err(ToolError::InvalidCommand(if stderr.trim().is_empty() {
                format!("git log failed with exit code {:?}", output.status.code())
            } else {
                truncate_output(&stderr)
            }));
        }

        let commits = parse_git_log_records(&stdout);
        Ok(ToolExecution {
            output: json!({
                "path": pathspec.unwrap_or("."),
                "max_count": max_count,
                "commits": commits,
                "raw": truncate_output(&format_git_log_raw(&commits)),
            }),
            summary: format!(
                "Git log ({} commit{}) for {}",
                commits.len(),
                if commits.len() == 1 { "" } else { "s" },
                pathspec.unwrap_or(".")
            ),
        })
    }

    fn git_show(&self, args: GitShowArgs) -> Result<ToolExecution, ToolError> {
        let revision = args.revision.trim();
        if revision.is_empty() {
            return Err(ToolError::InvalidArguments(
                "revision must not be empty".to_owned(),
            ));
        }
        if revision.starts_with('-') {
            return Err(ToolError::InvalidArguments(
                "revision must not start with '-'".to_owned(),
            ));
        }
        let pathspec = args
            .path
            .as_deref()
            .filter(|value| !value.trim().is_empty());
        if let Some(path) = pathspec {
            let _ = checked_relative_path(path)?;
        }

        let mut command = git_command();
        command
            .arg("show")
            .arg("--no-color")
            .arg("--no-ext-diff")
            .arg("--pretty=format:%H%x00%h%x00%an%x00%ae%x00%aI%x00%s%x00%b%x00")
            .arg(revision)
            .current_dir(&self.workspace_root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_PAGER", "cat")
            .env("PAGER", "cat");
        if let Some(path) = pathspec {
            command.arg("--").arg(path);
        }

        let output = command.output()?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if !output.status.success() {
            return Err(ToolError::InvalidCommand(if stderr.trim().is_empty() {
                format!("git show failed with exit code {:?}", output.status.code())
            } else {
                truncate_output(&stderr)
            }));
        }

        let (meta, patch) = parse_git_show_output(&stdout);
        Ok(ToolExecution {
            output: json!({
                "revision": revision,
                "path": pathspec,
                "hash": meta.get("hash").cloned().unwrap_or(Value::Null),
                "short_hash": meta.get("short_hash").cloned().unwrap_or(Value::Null),
                "author": meta.get("author").cloned().unwrap_or(Value::Null),
                "email": meta.get("email").cloned().unwrap_or(Value::Null),
                "date": meta.get("date").cloned().unwrap_or(Value::Null),
                "subject": meta.get("subject").cloned().unwrap_or(Value::Null),
                "body": meta.get("body").cloned().unwrap_or(Value::Null),
                "patch": truncate_output(&patch),
                "raw": truncate_output(&stdout),
            }),
            summary: format!(
                "Git show {}{}",
                revision,
                pathspec
                    .map(|path| format!(" -- {path}"))
                    .unwrap_or_default()
            ),
        })
    }

    fn git_add(&self, args: GitAddArgs) -> Result<ToolExecution, ToolError> {
        if args.paths.is_empty() {
            return Err(ToolError::InvalidArguments(
                "paths must not be empty".to_owned(),
            ));
        }

        let mut normalized = Vec::with_capacity(args.paths.len());
        for path in &args.paths {
            let trimmed = path.trim();
            if trimmed.is_empty() {
                return Err(ToolError::InvalidArguments(
                    "paths must not contain empty entries".to_owned(),
                ));
            }
            let relative = checked_relative_path(trimmed)?;
            if is_high_risk_path(trimmed) {
                return Err(ToolError::InvalidArguments(format!(
                    "refusing to stage high-risk path: {trimmed}"
                )));
            }
            normalized.push(relative.display().to_string());
        }

        let mut command = git_command();
        command
            .arg("add")
            .arg("--")
            .args(&normalized)
            .current_dir(&self.workspace_root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_PAGER", "cat")
            .env("PAGER", "cat");

        let output = command.output()?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if !output.status.success() {
            return Err(ToolError::InvalidCommand(if stderr.trim().is_empty() {
                format!("git add failed with exit code {:?}", output.status.code())
            } else {
                truncate_output(&stderr)
            }));
        }

        Ok(ToolExecution {
            output: json!({
                "paths": normalized,
                "success": true,
                "stdout": truncate_output(&stdout),
                "stderr": truncate_output(&stderr),
            }),
            summary: format!(
                "Staged {} path{}",
                normalized.len(),
                if normalized.len() == 1 { "" } else { "s" }
            ),
        })
    }

    fn git_commit(&self, args: GitCommitArgs) -> Result<ToolExecution, ToolError> {
        let message = args.message.trim();
        if message.is_empty() {
            return Err(ToolError::InvalidArguments(
                "message must not be empty".to_owned(),
            ));
        }
        let allow_empty = args.allow_empty.unwrap_or(false);

        let mut command = git_command();
        command.arg("commit").arg("-m").arg(message);
        if allow_empty {
            command.arg("--allow-empty");
        }
        command
            .current_dir(&self.workspace_root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_PAGER", "cat")
            .env("PAGER", "cat");

        let output = command.output()?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if !output.status.success() {
            return Err(ToolError::InvalidCommand(if stderr.trim().is_empty() {
                format!(
                    "git commit failed with exit code {:?}",
                    output.status.code()
                )
            } else {
                truncate_output(&stderr)
            }));
        }

        let hash = git_rev_parse_head(&self.workspace_root).ok();
        let subject = message.lines().next().unwrap_or(message).to_owned();
        Ok(ToolExecution {
            output: json!({
                "message": message,
                "subject": subject,
                "hash": hash,
                "allow_empty": allow_empty,
                "stdout": truncate_output(&stdout),
                "stderr": truncate_output(&stderr),
            }),
            summary: match hash.as_deref() {
                Some(value) if value.len() >= 7 => {
                    format!("Committed {} ({})", &value[..7], subject)
                }
                Some(value) => format!("Committed {value} ({subject})"),
                None => format!("Committed ({subject})"),
            },
        })
    }

    fn git_push(&self, args: GitPushArgs) -> Result<ToolExecution, ToolError> {
        let remote = args
            .remote
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("origin");
        validate_git_name("remote", remote)?;

        let set_upstream = args.set_upstream.unwrap_or(false);
        let branch = if let Some(branch) = args
            .branch
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            validate_git_name("branch", branch)?;
            branch.to_owned()
        } else {
            current_branch_name(&self.workspace_root)?
        };

        let mut command = git_command();
        command.arg("push");
        if set_upstream {
            command.arg("--set-upstream");
        }
        command
            .arg(remote)
            .arg(&branch)
            .current_dir(&self.workspace_root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_PAGER", "cat")
            .env("PAGER", "cat");

        let output = command.output()?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if !output.status.success() {
            let detail = if !stderr.trim().is_empty() {
                truncate_output(stderr.trim())
            } else if !stdout.trim().is_empty() {
                truncate_output(stdout.trim())
            } else {
                format!("git push failed with exit code {:?}", output.status.code())
            };
            return Err(ToolError::InvalidCommand(detail));
        }

        let head = git_rev_parse_head(&self.workspace_root).ok();
        Ok(ToolExecution {
            output: json!({
                "remote": remote,
                "branch": branch,
                "set_upstream": set_upstream,
                "head": head,
                "success": true,
                "stdout": truncate_output(&stdout),
                "stderr": truncate_output(&stderr),
            }),
            summary: format!(
                "Pushed {} to {}{}",
                branch,
                remote,
                if set_upstream { " (set upstream)" } else { "" }
            ),
        })
    }

    fn git_fetch(&self, args: GitFetchArgs) -> Result<ToolExecution, ToolError> {
        let remote = args
            .remote
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("origin");
        validate_git_name("remote", remote)?;

        let branch = args
            .branch
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                validate_git_name("branch", value)?;
                Ok::<_, ToolError>(value.to_owned())
            })
            .transpose()?;

        let mut command = git_command();
        command.arg("fetch");
        command.arg(remote);
        if let Some(branch) = branch.as_deref() {
            command.arg(branch);
        }
        command
            .current_dir(&self.workspace_root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_PAGER", "cat")
            .env("PAGER", "cat");

        let output = command.output()?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if !output.status.success() {
            let detail = if !stderr.trim().is_empty() {
                truncate_output(stderr.trim())
            } else if !stdout.trim().is_empty() {
                truncate_output(stdout.trim())
            } else {
                format!("git fetch failed with exit code {:?}", output.status.code())
            };
            return Err(ToolError::InvalidCommand(detail));
        }

        Ok(ToolExecution {
            output: json!({
                "remote": remote,
                "branch": branch,
                "success": true,
                "stdout": truncate_output(&stdout),
                "stderr": truncate_output(&stderr),
            }),
            summary: match branch.as_deref() {
                Some(branch) => format!("Fetched {branch} from {remote}"),
                None => format!("Fetched from {remote}"),
            },
        })
    }

    fn git_pull(&self, args: GitPullArgs) -> Result<ToolExecution, ToolError> {
        let remote = args
            .remote
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("origin");
        validate_git_name("remote", remote)?;

        let ff_only = args.ff_only.unwrap_or(true);
        let branch = if let Some(branch) = args
            .branch
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            validate_git_name("branch", branch)?;
            branch.to_owned()
        } else {
            current_branch_name(&self.workspace_root)?
        };

        let mut command = git_command();
        command.arg("pull");
        if ff_only {
            command.arg("--ff-only");
        } else {
            // Explicit non-rebase merge pull only; never --rebase / --force.
            command.arg("--no-rebase");
        }
        command
            .arg(remote)
            .arg(&branch)
            .current_dir(&self.workspace_root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_PAGER", "cat")
            .env("PAGER", "cat");

        let output = command.output()?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if !output.status.success() {
            let detail = if !stderr.trim().is_empty() {
                truncate_output(stderr.trim())
            } else if !stdout.trim().is_empty() {
                truncate_output(stdout.trim())
            } else {
                format!("git pull failed with exit code {:?}", output.status.code())
            };
            return Err(ToolError::InvalidCommand(detail));
        }

        let head = git_rev_parse_head(&self.workspace_root).ok();
        Ok(ToolExecution {
            output: json!({
                "remote": remote,
                "branch": branch,
                "ff_only": ff_only,
                "head": head,
                "success": true,
                "stdout": truncate_output(&stdout),
                "stderr": truncate_output(&stderr),
            }),
            summary: format!(
                "Pulled {} from {}{}",
                branch,
                remote,
                if ff_only {
                    " (ff-only)"
                } else {
                    " (no-rebase)"
                }
            ),
        })
    }

    fn write_atomically(&self, path: &Path, text: &str) -> Result<(), ToolError> {
        write_text_utf8(path, text)?;
        Ok(())
    }

    /// Resolve the `run_command` program for spawning.
    ///
    /// Bare command names are left untouched so the OS resolves them through
    /// `PATH`. Relative paths (already restricted by command policy to `./` and
    /// `target/` prefixes) are anchored to the workspace root, because the OS
    /// resolves a relative program against the parent process directory rather
    /// than the child's `current_dir`.
    fn resolve_executable(&self, executable: &str) -> Result<PathBuf, ToolError> {
        if !executable.contains('/') && !executable.contains('\\') {
            return Ok(PathBuf::from(executable));
        }
        let requested = checked_relative_path(executable)?;
        let target = self.workspace_root.join(requested);
        let canonical = target.canonicalize().map_err(|error| {
            ToolError::InvalidCommand(format!("executable `{executable}` not found: {error}"))
        })?;
        if !canonical.starts_with(&self.workspace_root) {
            return Err(ToolError::PathOutsideWorkspace(executable.to_owned()));
        }
        Ok(canonical)
    }

    fn resolve_writable(&self, requested_path: &str) -> Result<PathBuf, ToolError> {
        let requested = checked_relative_path(requested_path)?;
        let target = self.workspace_root.join(requested);
        // New files may target parents that do not exist yet. Walk up to the
        // nearest existing ancestor and confirm it remains inside the workspace.
        let mut ancestor = target
            .parent()
            .ok_or_else(|| ToolError::PathOutsideWorkspace(requested_path.to_owned()))?
            .to_path_buf();
        while !ancestor.exists() {
            ancestor = ancestor
                .parent()
                .ok_or_else(|| ToolError::PathOutsideWorkspace(requested_path.to_owned()))?
                .to_path_buf();
        }
        let canonical_ancestor = ancestor.canonicalize()?;
        if !canonical_ancestor.starts_with(&self.workspace_root) {
            return Err(ToolError::PathOutsideWorkspace(requested_path.to_owned()));
        }
        if target.exists() && fs::symlink_metadata(&target)?.file_type().is_symlink() {
            return Err(ToolError::PathOutsideWorkspace(requested_path.to_owned()));
        }
        Ok(target)
    }

    fn resolve(&self, requested_path: &str) -> Result<PathBuf, ToolError> {
        let requested_path = checked_relative_path(requested_path)?;
        let resolved = self.workspace_root.join(requested_path).canonicalize()?;
        if !resolved.starts_with(&self.workspace_root) {
            return Err(ToolError::PathOutsideWorkspace(
                requested_path.display().to_string(),
            ));
        }
        Ok(resolved)
    }

    fn validate_git_command_paths(
        &self,
        executable: &str,
        args: &[String],
        cwd: Option<&str>,
    ) -> Result<(), ToolError> {
        if !executable.eq_ignore_ascii_case("git") {
            return Ok(());
        }

        if let Some(cwd) = cwd {
            self.resolve(cwd)?;
        }

        let mut index = 0;
        while index < args.len() {
            let arg = &args[index];
            let option_path: Option<&str> = if arg == "-C"
                || arg == "--git-dir"
                || arg == "--work-tree"
            {
                index += 1;
                args.get(index).map(String::as_str)
            } else if let Some(path) = arg
                .strip_prefix("--git-dir=")
                .or_else(|| arg.strip_prefix("--work-tree="))
            {
                Some(path)
            } else {
                None
            };

            if let Some(path) = option_path {
                self.resolve(path).map(|_| ())?;
            }
            index += 1;
        }
        Ok(())
    }

    fn relative_path(&self, path: &Path) -> String {
        let relative = path.strip_prefix(&self.workspace_root).unwrap_or(path);
        let rendered = relative.to_string_lossy().replace('\\', "/");
        if rendered.is_empty() {
            ".".to_owned()
        } else {
            rendered
        }
    }

    /// Builds the log file a background launch writes to.
    ///
    /// One file per launch, named after the executable plus a timestamp, so a
    /// restart never overwrites the log that explains the previous failure.
    fn background_log_path(&self, executable: &str) -> Result<PathBuf, ToolError> {
        let stem = executable
            .rsplit(|character| character == '/' || character == '\\')
            .next()
            .unwrap_or(executable);
        let stem = stem.split('.').next().unwrap_or(stem);
        let safe: String = stem
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                    character
                } else {
                    '-'
                }
            })
            .collect();
        let safe = if safe.trim_matches('-').is_empty() {
            "command".to_owned()
        } else {
            safe
        };
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis())
            .unwrap_or_default();
        let directory = self.workspace_root.join(BACKGROUND_LOG_DIR);
        fs::create_dir_all(&directory)?;
        Ok(directory.join(format!("{safe}-{stamp}.log")))
    }
}

#[derive(Deserialize)]
struct ListDirArgs {
    #[serde(default)]
    path: String,
    #[serde(default)]
    max_entries: Option<usize>,
}

#[derive(Deserialize)]
struct ReadFileArgs {
    path: String,
    #[serde(default)]
    start_line: Option<usize>,
    #[serde(default)]
    end_line: Option<usize>,
}

#[derive(Deserialize)]
struct ApplyPatchArgs {
    path: String,
    old_text: String,
    new_text: String,
}

#[derive(Deserialize)]
struct RunCommandArgs {
    executable: String,
    #[serde(default)]
    args: Vec<String>,
    /// Optional wall-clock bound in seconds, clamped to
    /// `1..=MAX_COMMAND_TIMEOUT_SECONDS`; defaults to [`DEFAULT_COMMAND_TIMEOUT`].
    #[serde(default)]
    timeout_seconds: Option<u64>,
    /// Launch and return immediately, leaving the process running after the call.
    /// Intended for a project's own background service during local testing.
    #[serde(default)]
    background: bool,
    /// Workspace-relative directory to run in; defaults to the workspace root.
    /// A service that resolves its own data by relative path (migrations,
    /// config, assets) only starts when launched from its own directory.
    #[serde(default)]
    cwd: Option<String>,
    /// Local port a background service must accept connections on before the
    /// launch counts as successful. Only meaningful with `background`.
    #[serde(default)]
    ready_port: Option<u16>,
    /// Bound for waiting on `ready_port`, clamped to
    /// `1..=MAX_READY_TIMEOUT_SECONDS`; defaults to
    /// [`DEFAULT_READY_TIMEOUT_SECONDS`].
    #[serde(default)]
    ready_timeout_seconds: Option<u64>,
}

impl RunCommandArgs {
    fn effective_timeout(&self) -> Duration {
        match self.timeout_seconds {
            Some(seconds) => Duration::from_secs(seconds.clamp(1, MAX_COMMAND_TIMEOUT_SECONDS)),
            None => DEFAULT_COMMAND_TIMEOUT,
        }
    }

    fn effective_ready_timeout(&self) -> Duration {
        let seconds = self
            .ready_timeout_seconds
            .unwrap_or(DEFAULT_READY_TIMEOUT_SECONDS)
            .clamp(1, MAX_READY_TIMEOUT_SECONDS);
        Duration::from_secs(seconds)
    }
}

#[derive(Deserialize)]
struct SearchCodeArgs {
    query: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    max_results: Option<usize>,
    #[serde(default)]
    case_insensitive: bool,
    #[serde(default)]
    glob: Option<String>,
    #[serde(default)]
    context_lines: Option<usize>,
}

#[derive(Deserialize)]
struct LoadSkillArgs {
    name: String,
}

#[derive(Deserialize)]
struct GitStatusArgs {
    #[serde(default)]
    path: Option<String>,
}

#[derive(Deserialize)]
struct GitDiffArgs {
    #[serde(default)]
    path: Option<String>,
}

#[derive(Deserialize)]
struct GitLogArgs {
    #[serde(default)]
    max_count: Option<usize>,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Deserialize)]
struct GitShowArgs {
    revision: String,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Deserialize)]
struct GitAddArgs {
    paths: Vec<String>,
}

#[derive(Deserialize)]
struct GitCommitArgs {
    message: String,
    #[serde(default)]
    allow_empty: Option<bool>,
}

#[derive(Deserialize)]
struct GitPushArgs {
    #[serde(default)]
    remote: Option<String>,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    set_upstream: Option<bool>,
}

#[derive(Deserialize)]
struct GitFetchArgs {
    #[serde(default)]
    remote: Option<String>,
    #[serde(default)]
    branch: Option<String>,
}

#[derive(Deserialize)]
struct GitPullArgs {
    #[serde(default)]
    remote: Option<String>,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    ff_only: Option<bool>,
}

#[derive(Deserialize)]
struct UpdatePlanArgs {
    steps: Vec<UpdatePlanStepArgs>,
}

#[derive(Deserialize)]
struct UpdatePlanStepArgs {
    description: String,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Serialize)]
struct DirectoryEntry {
    name: String,
    kind: String,
}

#[derive(Serialize)]
struct ListDirOutput {
    path: String,
    entries: Vec<DirectoryEntry>,
    truncated: bool,
}

#[derive(Serialize)]
struct ReadFileOutput {
    path: String,
    content: String,
    start_line: usize,
    end_line: usize,
    truncated: bool,
    content_redacted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    redaction_reason: Option<String>,
}

#[derive(Serialize)]
struct SearchResult {
    path: String,
    line: usize,
    text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    before: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    after: Vec<String>,
}

struct RankedSearchHit {
    score: i32,
    result: SearchResult,
}

fn checked_relative_path(requested_path: &str) -> Result<&Path, ToolError> {
    let requested_path = if requested_path.trim().is_empty() {
        Path::new(".")
    } else {
        Path::new(requested_path)
    };
    if requested_path.is_absolute()
        || requested_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(ToolError::PathOutsideWorkspace(
            requested_path.display().to_string(),
        ));
    }
    Ok(requested_path)
}

/// Records the model-authored turn plan. The step count is the model's choice;
/// only obvious garbage (empty list, blank text, absurd length) is rejected.
fn update_plan(args: UpdatePlanArgs) -> Result<ToolExecution, ToolError> {
    if args.steps.is_empty() {
        return Err(ToolError::InvalidArguments(
            "steps must contain at least one step".to_owned(),
        ));
    }
    if args.steps.len() > MAX_PLAN_STEPS {
        return Err(ToolError::InvalidArguments(format!(
            "steps must contain at most {MAX_PLAN_STEPS} steps"
        )));
    }

    let mut steps = Vec::with_capacity(args.steps.len());
    for (index, step) in args.steps.iter().enumerate() {
        let description = step.description.trim();
        if description.is_empty() {
            return Err(ToolError::InvalidArguments(format!(
                "step {} description must not be empty",
                index + 1
            )));
        }
        steps.push(PlanStep {
            id: format!("step_{}", index + 1),
            description: truncate_plan_description(description),
            status: normalize_plan_status(step.status.as_deref()),
        });
    }

    let done = steps
        .iter()
        .filter(|step| step.status == PlanStepStatus::Done)
        .count();
    let output = serde_json::to_value(&steps)
        .map(|steps| json!({ "steps": steps }))
        .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
    Ok(ToolExecution {
        output,
        summary: format!("Updated plan ({done}/{} done)", steps.len()),
    })
}

fn normalize_plan_status(status: Option<&str>) -> PlanStepStatus {
    match status.map(str::trim).unwrap_or_default() {
        "in_progress" | "in-progress" | "current" | "running" => PlanStepStatus::InProgress,
        "done" | "completed" | "complete" => PlanStepStatus::Done,
        _ => PlanStepStatus::Pending,
    }
}

fn truncate_plan_description(description: &str) -> String {
    if description.chars().count() <= MAX_PLAN_STEP_DESCRIPTION_CHARS {
        return description.to_owned();
    }
    description
        .chars()
        .take(MAX_PLAN_STEP_DESCRIPTION_CHARS)
        .collect()
}

fn parse_git_status_lines(stdout: &str) -> Vec<Value> {
    let mut entries = Vec::new();
    for line in stdout.lines() {
        if line.starts_with("## ") {
            let branch = line.trim_start_matches("## ").to_owned();
            entries.push(json!({ "kind": "branch", "branch": branch }));
            continue;
        }
        if line.len() < 3 {
            continue;
        }
        let index_status = line.chars().next().unwrap_or(' ');
        let worktree_status = line.chars().nth(1).unwrap_or(' ');
        let path = line[3..].to_owned();
        entries.push(json!({
            "kind": "entry",
            "index_status": index_status.to_string(),
            "worktree_status": worktree_status.to_string(),
            "path": path,
        }));
    }
    entries
}

fn parse_git_log_records(stdout: &str) -> Vec<Value> {
    let mut commits = Vec::new();
    for record in stdout.split('\u{1e}') {
        let record = record.trim_matches(|c| c == '\0' || c == '\n' || c == '\r');
        if record.is_empty() {
            continue;
        }
        let parts: Vec<&str> = record.split('\0').collect();
        if parts.len() < 6 {
            continue;
        }
        let body = if parts.len() > 6 {
            parts[6..].join("\0").trim().to_owned()
        } else {
            String::new()
        };
        commits.push(json!({
            "hash": parts[0],
            "short_hash": parts[1],
            "author": parts[2],
            "email": parts[3],
            "date": parts[4],
            "subject": parts[5],
            "body": body,
        }));
    }
    commits
}

fn format_git_log_raw(commits: &[Value]) -> String {
    let mut lines = Vec::new();
    for commit in commits {
        let short = commit
            .get("short_hash")
            .and_then(Value::as_str)
            .unwrap_or("");
        let subject = commit.get("subject").and_then(Value::as_str).unwrap_or("");
        let author = commit.get("author").and_then(Value::as_str).unwrap_or("");
        let date = commit.get("date").and_then(Value::as_str).unwrap_or("");
        lines.push(format!("{short} {subject} ({author}, {date})"));
    }
    lines.join("\n")
}

fn parse_git_show_output(stdout: &str) -> (serde_json::Map<String, Value>, String) {
    let mut meta = serde_json::Map::new();
    // pretty=format:%H%x00%h%x00%an%x00%ae%x00%aI%x00%s%x00%b%x00 then patch
    let parts: Vec<&str> = stdout.splitn(8, '\0').collect();
    if parts.len() >= 7 {
        meta.insert("hash".to_owned(), json!(parts[0]));
        meta.insert("short_hash".to_owned(), json!(parts[1]));
        meta.insert("author".to_owned(), json!(parts[2]));
        meta.insert("email".to_owned(), json!(parts[3]));
        meta.insert("date".to_owned(), json!(parts[4]));
        meta.insert("subject".to_owned(), json!(parts[5]));
        meta.insert(
            "body".to_owned(),
            json!(parts[6].trim_end_matches(|c| c == '\n' || c == '\r')),
        );
        let patch = if parts.len() > 7 {
            parts[7]
                .trim_start_matches(|c| c == '\n' || c == '\r')
                .to_owned()
        } else {
            String::new()
        };
        (meta, patch)
    } else {
        (meta, stdout.to_owned())
    }
}

fn load_command_allowlist(workspace_root: &Path) -> Vec<String> {
    let path = workspace_root.join(COMMAND_ALLOWLIST_RELATIVE_PATH);
    match fs::read_to_string(path) {
        Ok(text) => parse_command_allowlist(&text),
        Err(_) => Vec::new(),
    }
}

fn load_command_denylist(workspace_root: &Path) -> Vec<String> {
    let path = workspace_root.join(COMMAND_DENYLIST_RELATIVE_PATH);
    match fs::read_to_string(path) {
        Ok(text) => parse_command_denylist(&text),
        Err(_) => Vec::new(),
    }
}

fn is_valid_skill_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    name.chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
}

struct ParsedSkillFile {
    description: String,
    body: String,
}

fn parse_skill_file(folder_name: &str, raw: &str) -> ParsedSkillFile {
    let normalized = raw.replace("\r\n", "\n");
    let (description, body) = if let Some(rest) = normalized.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            let frontmatter = &rest[..end];
            let body = rest[end + "\n---\n".len()..].to_owned();
            let mut description = None;
            for line in frontmatter.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((key, value)) = line.split_once(':') {
                    let key = key.trim();
                    let value = value.trim().trim_matches('"').trim_matches('\'').to_owned();
                    if key == "description" && !value.is_empty() {
                        description = Some(value);
                    }
                }
            }
            (
                description.unwrap_or_else(|| skill_fallback_description(&body)),
                body,
            )
        } else {
            (skill_fallback_description(&normalized), normalized.clone())
        }
    } else {
        (skill_fallback_description(&normalized), normalized.clone())
    };
    let description = if description.chars().count() > MAX_SKILL_DESCRIPTION_CHARS {
        description
            .chars()
            .take(MAX_SKILL_DESCRIPTION_CHARS)
            .collect::<String>()
    } else {
        description
    };
    let _ = folder_name;
    ParsedSkillFile { description, body }
}

fn skill_fallback_description(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .or_else(|| {
            body.lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(|line| line.trim_start_matches('#').trim())
        })
        .unwrap_or("Workspace skill")
        .to_owned()
}

fn is_high_risk_path(path: &str) -> bool {
    let parts: Vec<String> = path
        .split(['/', '\\'])
        .map(|part| part.to_ascii_lowercase())
        .collect();
    let file_name = parts.last().map(String::as_str).unwrap_or("");
    parts.iter().any(|part| part == ".git" || part == ".xcoding")
        || parts.windows(2).any(|window| window[0] == ".git" && window[1] == "hooks")
        || parts.windows(2).any(|window| {
            window[0] == ".github" && window[1] == "workflows"
        })
        || parts.iter().any(|part| part == ".gitlab" || part == ".circleci")
        || matches!(file_name, ".gitlab-ci.yml" | ".gitlab-ci.yaml" | ".circleci.yml")
}

fn validate_git_name(kind: &str, value: &str) -> Result<(), ToolError> {
    if value.is_empty() {
        return Err(ToolError::InvalidArguments(format!(
            "{kind} must not be empty"
        )));
    }
    if value.starts_with('-') {
        return Err(ToolError::InvalidArguments(format!(
            "{kind} must not start with '-'"
        )));
    }
    if value.chars().any(|ch| ch.is_whitespace() || ch == '\0') {
        return Err(ToolError::InvalidArguments(format!(
            "{kind} must not contain whitespace"
        )));
    }
    // Block force-ish tokens and multi-arg smuggling via a single field.
    if value.contains(':') || value.contains("..") {
        return Err(ToolError::InvalidArguments(format!(
            "{kind} must not contain ':' or '..'"
        )));
    }
    Ok(())
}

fn current_branch_name(workspace_root: &Path) -> Result<String, ToolError> {
    let output = git_command()
        .arg("rev-parse")
        .arg("--abbrev-ref")
        .arg("HEAD")
        .current_dir(workspace_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .env("PAGER", "cat")
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ToolError::InvalidCommand(if stderr.trim().is_empty() {
            "failed to resolve current branch".to_owned()
        } else {
            truncate_output(stderr.trim())
        }));
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if branch.is_empty() || branch == "HEAD" {
        return Err(ToolError::InvalidArguments(
            "detached HEAD: pass branch explicitly".to_owned(),
        ));
    }
    validate_git_name("branch", &branch)?;
    Ok(branch)
}

fn git_rev_parse_head(workspace_root: &Path) -> Result<String, ToolError> {
    let output = git_command()
        .arg("rev-parse")
        .arg("HEAD")
        .current_dir(workspace_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .env("PAGER", "cat")
        .output()?;
    if !output.status.success() {
        return Err(ToolError::InvalidCommand(
            "git rev-parse HEAD failed after commit".to_owned(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Reads the last `limit` bytes of a file, or an empty string when it cannot be
/// read. A startup failure prints its reason at the end of the log, so the tail
/// is the part worth reporting.
fn read_tail(path: &Path, limit: u64) -> String {
    let Ok(mut file) = fs::File::open(path) else {
        return String::new();
    };
    let length = file.metadata().map(|data| data.len()).unwrap_or(0);
    if length > limit {
        use std::io::Seek;
        if file.seek(std::io::SeekFrom::Start(length - limit)).is_err() {
            return String::new();
        }
    }
    let mut buffer = Vec::new();
    if file.read_to_end(&mut buffer).is_err() {
        return String::new();
    }
    truncate_output(&redact_log_text(&String::from_utf8_lossy(&buffer)))
}

/// Result of waiting for a freshly launched background service's port.
enum PortReadyOutcome {
    Ready { waited: Duration },
    Exited { status: std::process::ExitStatus },
    TimedOut,
}

/// Waits until a background service accepts a local connection on `port`.
///
/// The child is polled alongside the port so a service that dies while starting
/// is reported as a real exit instead of running the full timeout.
fn await_port_ready(
    child: &mut std::process::Child,
    port: u16,
    timeout: Duration,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<PortReadyOutcome, ToolError> {
    let started = Instant::now();
    let deadline = started + timeout;
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    loop {
        if is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ToolError::Cancelled);
        }
        if TcpStream::connect_timeout(&address, READY_CONNECT_TIMEOUT).is_ok() {
            return Ok(PortReadyOutcome::Ready {
                waited: started.elapsed(),
            });
        }
        if let Some(status) = child.try_wait()? {
            return Ok(PortReadyOutcome::Exited { status });
        }
        if Instant::now() >= deadline {
            return Ok(PortReadyOutcome::TimedOut);
        }
        thread::sleep(READY_PROBE_INTERVAL);
    }
}

fn truncate_output(value: &str) -> String {
    const MAX_OUTPUT_BYTES: usize = 32 * 1024;
    if value.len() <= MAX_OUTPUT_BYTES {
        value.to_owned()
    } else {
        // Cut on a char boundary: the byte cap can land inside a multi-byte
        // character, and slicing a `str` there panics.
        let end = value.floor_char_boundary(MAX_OUTPUT_BYTES);
        format!("{}\n[output truncated]", &value[..end])
    }
}

const CONFIG_SECRET_FIELDS: [&str; 7] = [
    "api_key",
    "apikey",
    "access_token",
    "client_secret",
    "secret_key",
    "password",
    "authorization",
];

fn is_high_sensitivity_file(path: &str) -> bool {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    let name = normalized.rsplit('/').next().unwrap_or(&normalized);
    name == "credentials"
        || name == "credentials.json"
        || name == "id_rsa"
        || name == "id_ed25519"
        || [".pem", ".key", ".p12", ".pfx", ".jks"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
        || normalized.contains("/.aws/credentials")
        || normalized.contains("/.ssh/")
}

fn is_config_file(path: &str) -> bool {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    let name = normalized.rsplit('/').next().unwrap_or(&normalized);
    name == ".env"
        || name.starts_with(".env.")
        || [".json", ".yaml", ".yml", ".toml", ".ini", ".conf"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
}

fn is_background_log_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    normalized.starts_with(".xcoding/logs/")
}

fn redact_log_text(text: &str) -> String {
    let mut in_private_key = false;
    text.lines()
        .map(|line| {
            let lower = line.to_ascii_lowercase();
            if lower.contains("-----begin ") && lower.contains("private key-----") {
                in_private_key = true;
                return "[REDACTED PRIVATE KEY]".to_owned();
            }
            if in_private_key {
                if lower.contains("-----end ") && lower.contains("private key-----") {
                    in_private_key = false;
                }
                return "[REDACTED PRIVATE KEY]".to_owned();
            }

            let line = redact_config_line(line);
            line.split_whitespace()
                .map(|part| {
                    let trimmed = part.trim_matches(|character: char| {
                        !character.is_ascii_alphanumeric() && !matches!(character, '.' | '_' | '-')
                    });
                    let dot_count = trimmed.bytes().filter(|byte| *byte == b'.').count();
                    if (trimmed.starts_with("sk-") && trimmed.len() >= 20)
                        || (trimmed.starts_with("eyJ") && dot_count == 2 && trimmed.len() >= 30)
                    {
                        part.replace(trimmed, "[REDACTED]")
                    } else {
                        part.to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn redact_config_text(text: &str) -> String {
    text.lines()
        .map(redact_config_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn redact_config_line(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let Some(field) = CONFIG_SECRET_FIELDS
        .iter()
        .find(|field| lower.contains(**field))
    else {
        return line.to_owned();
    };
    let Some(field_start) = lower.find(field) else {
        return line.to_owned();
    };
    let Some(separator_offset) = line[field_start + field.len()..]
        .find(['=', ':'])
    else {
        return line.to_owned();
    };
    let separator = field_start + field.len() + separator_offset;
    let value_start = line[separator + 1..]
        .char_indices()
        .find(|(_, character)| !character.is_whitespace())
        .map(|(offset, _)| separator + 1 + offset)
        .unwrap_or(line.len());
    if value_start == line.len() {
        return line.to_owned();
    }
    let quote = line[value_start..].chars().next();
    let value_end = match quote {
        Some('"') | Some('\'') => line[value_start + 1..]
            .find(quote.unwrap())
            .map(|offset| value_start + 1 + offset + 1)
            .unwrap_or(line.len()),
        _ => line[value_start..]
            .find([',', ';', '#'])
            .map(|offset| value_start + offset)
            .unwrap_or(line.len()),
    };
    format!(
        "{}[REDACTED]{}",
        &line[..value_start],
        &line[value_end..]
    )
}

/// Keeps the leading items that fit inside [`MAX_TOOL_JSON_BYTES`] once serialized,
/// so a single tool call cannot flood the conversation with a multi-hundred-kilobyte
/// payload that every later request has to resend. Returns the retained items and
/// whether anything was dropped. The first item is always kept so callers still see
/// a sample when one item alone exceeds the budget.
fn cap_json_items<T: Serialize>(items: Vec<T>) -> (Vec<T>, bool) {
    let mut used = 0usize;
    let mut kept = Vec::with_capacity(items.len());
    let mut dropped = false;
    for item in items {
        let size = serde_json::to_string(&item)
            .map(|text| text.len())
            .unwrap_or(0);
        if !kept.is_empty() && used + size > MAX_TOOL_JSON_BYTES {
            dropped = true;
            break;
        }
        used += size;
        kept.push(item);
    }
    (kept, dropped)
}

fn parse_arguments<T: DeserializeOwned>(arguments: &Value) -> Result<T, ToolError> {
    serde_json::from_value(arguments.clone())
        .map_err(|error| ToolError::InvalidArguments(error.to_string()))
}

fn bounded(value: Option<usize>, default: usize, maximum: usize) -> usize {
    value.unwrap_or(default).clamp(1, maximum)
}

fn is_ignored_directory(name: &std::ffi::OsStr) -> bool {
    matches!(
        name.to_string_lossy().as_ref(),
        ".git"
            | ".xcoding"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | ".next"
            | "coverage"
            | "__pycache__"
            | ".venv"
            | "venv"
            | ".cargo"
    )
}

fn is_low_value_search_file(relative_path: &str) -> bool {
    let lower = relative_path.replace('\\', "/").to_lowercase();
    lower.ends_with(".min.js")
        || lower.ends_with(".map")
        || lower.ends_with(".lock")
        || lower.ends_with("package-lock.json")
        || lower.ends_with("pnpm-lock.yaml")
        || lower.ends_with("yarn.lock")
        || lower.ends_with("cargo.lock")
}

fn path_rank_score(relative_path: &str) -> i32 {
    let lower = relative_path.replace('\\', "/").to_lowercase();
    let mut score = 0;
    if lower.starts_with("src/") || lower.contains("/src/") {
        score += 30;
    }
    if lower.starts_with("crates/")
        || lower.starts_with("apps/")
        || lower.starts_with("packages/")
        || lower.starts_with("lib/")
    {
        score += 25;
    }
    if lower.starts_with("tests/") || lower.contains("/tests/") {
        score += 10;
    }
    const SOURCE_EXTS: &[&str] = &[
        ".rs", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".vue", ".go", ".py", ".java", ".kt",
        ".cs", ".cpp", ".c", ".h", ".hpp", ".md", ".toml", ".json",
    ];
    if SOURCE_EXTS.iter().any(|ext| lower.ends_with(ext)) {
        score += 10;
    }
    if lower.starts_with("dist/") || lower.contains("/dist/") {
        score -= 40;
    }
    if lower.ends_with(".min.js") || lower.ends_with(".map") {
        score -= 50;
    }
    score
}

fn user_config_dir() -> PathBuf {
    if let Ok(home) = env::var("USERPROFILE") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join(".xcoding");
        }
    }
    if let Ok(home) = env::var("HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join(".xcoding");
        }
    }
    env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".xcoding")
}

fn browser_state_path() -> PathBuf {
    if let Ok(path) = env::var("XCODING_BROWSER_STATE_PATH") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    user_config_dir().join("browser-state.json")
}

fn path_matches_glob(relative_path: &str, pattern: &str, case_insensitive: bool) -> bool {
    let path = relative_path.replace('\\', "/");
    let pattern = pattern.replace('\\', "/");
    let (path, pattern) = if case_insensitive {
        (path.to_lowercase(), pattern.to_lowercase())
    } else {
        (path, pattern)
    };
    if pattern.contains('/') {
        return glob_match(&pattern, &path);
    }
    let file_name = path.rsplit('/').next().unwrap_or(&path);
    glob_match(&pattern, file_name)
}

fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    fn matches(pattern: &[char], text: &[char]) -> bool {
        match (pattern.first(), text.first()) {
            (None, None) => true,
            (Some('*'), _) => {
                for index in 0..=text.len() {
                    if matches(&pattern[1..], &text[index..]) {
                        return true;
                    }
                }
                false
            }
            (Some('?'), Some(_)) => matches(&pattern[1..], &text[1..]),
            (Some(expected), Some(actual)) if expected == actual => {
                matches(&pattern[1..], &text[1..])
            }
            _ => false,
        }
    }
    matches(&pattern, &text)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use serde_json::json;
    use xcoding_protocol::{Mode, ToolCall, ToolName};

    use super::*;

    fn workspace() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock works")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("xcoding-tools-{unique}"));
        fs::create_dir_all(&root).expect("workspace creates");
        root
    }

    fn local_api_tool_call(script: &str) -> ToolCall {
        ToolCall {
            id: "local-api".to_owned(),
            name: ToolName::RunCommand,
            arguments: json!({
                "executable": "powershell",
                "args": ["-Command", script]
            }),
        }
    }

    #[test]
    fn identifies_only_tightly_scoped_loopback_powershell_api_requests() {
        let sample = r#"try { $r = Invoke-WebRequest -Uri 'http://127.0.0.1:8787/api/analyze' -Method POST -Body '{"code":"513310","skip_llm":true}' -ContentType 'application/json' -UseBasicParsing -TimeoutSec 30; $r.Content } catch { $_.Exception.Message }"#;
        assert!(is_local_api_request(&local_api_tool_call(sample)));
        assert!(is_local_api_request(&local_api_tool_call(
            "Invoke-RestMethod -Uri 'https://localhost:8787/api/analyze' -Method GET"
        )));
        assert!(is_local_api_request(&local_api_tool_call(
            "Invoke-WebRequest -Uri 'http://[::1]:8787/api/analyze' -Method GET"
        )));

        assert!(!is_local_api_request(&local_api_tool_call(
            "Invoke-WebRequest -Uri 'https://example.test/api/analyze' -Method POST"
        )));
        assert!(!is_local_api_request(&local_api_tool_call(
            "Invoke-WebRequest -Uri 'http://127.0.0.1:8787/api/analyze' -Method POST; Remove-Item .\\important.txt"
        )));
        assert!(!is_local_api_request(&local_api_tool_call("Get-ChildItem")));
        assert!(!is_local_api_request(&ToolCall {
            id: "extra-argument".to_owned(),
            name: ToolName::RunCommand,
            arguments: json!({
                "executable": "powershell",
                "args": ["-Command", "Invoke-WebRequest -Uri 'http://127.0.0.1:8787/api/analyze'", "-NoProfile"]
            }),
        }));
        assert!(!is_local_api_request(&local_api_tool_call(
            "Invoke-WebRequest -Uri 'http://127.0.0.1:8787/api/analyze'; $x = Get-ChildItem"
        )));
    }

    #[test]
    fn loads_workspace_skill_as_read_only_tool() {
        let root = workspace();
        let skill_dir = root.join(".xcoding/skills/demo-skill");
        fs::create_dir_all(&skill_dir).expect("skill dir");
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: demo-skill\ndescription: Demo skill for tests\n---\n# Demo\nUse this skill.\n",
        )
        .expect("skill writes");
        let tools = ToolRegistry::new(&root).expect("registry starts");

        let loaded = tools
            .execute(
                &Mode::Ask,
                &ToolCall {
                    id: "skill_1".to_owned(),
                    name: ToolName::LoadSkill,
                    arguments: json!({ "name": "demo-skill" }),
                },
            )
            .expect("skill loads");
        assert_eq!(loaded.output["name"], "demo-skill");
        assert_eq!(loaded.output["description"], "Demo skill for tests");
        assert!(
            loaded.output["content"]
                .as_str()
                .unwrap()
                .contains("Use this skill.")
        );
        assert_eq!(loaded.summary, "Loaded skill demo-skill");

        let missing = tools.execute(
            &Mode::Ask,
            &ToolCall {
                id: "skill_missing".to_owned(),
                name: ToolName::LoadSkill,
                arguments: json!({ "name": "nope" }),
            },
        );
        assert!(missing.is_err());

        let bad = tools.execute(
            &Mode::Ask,
            &ToolCall {
                id: "skill_bad".to_owned(),
                name: ToolName::LoadSkill,
                arguments: json!({ "name": "../escape" }),
            },
        );
        assert!(bad.is_err());

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn loads_user_skill_when_workspace_does_not_have_one() {
        let root = workspace();
        let name = format!(
            "user-only-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock works")
                .as_nanos()
        );
        let skill_dir = user_skill_root().join(&name);
        fs::create_dir_all(&skill_dir).expect("user skill dir");
        fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: User skill for tests\n---\n# User\nUse this user skill.\n"),
        )
        .expect("user skill writes");

        let tools = ToolRegistry::new_with_plugin_config(&root, PluginConfig::default())
            .expect("registry starts");
        let loaded = tools
            .execute(
                &Mode::Ask,
                &ToolCall {
                    id: "user_skill_1".to_owned(),
                    name: ToolName::LoadSkill,
                    arguments: json!({ "name": name }),
                },
            )
            .expect("user skill loads");

        assert_eq!(loaded.output["source"], "user");
        assert_eq!(loaded.output["description"], "User skill for tests");
        assert!(
            loaded.output["content"]
                .as_str()
                .unwrap()
                .contains("Use this user skill.")
        );

        fs::remove_dir_all(skill_dir).expect("user skill removes");
        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn reads_and_searches_files_inside_the_workspace() {
        let root = workspace();
        fs::create_dir_all(root.join("src")).expect("source directory creates");
        fs::write(
            root.join("src/lib.rs"),
            "pub fn hello() {}\n// TODO: test\n",
        )
        .expect("source file writes");
        let tools = ToolRegistry::new(&root).expect("registry starts");

        let read = tools
            .execute(
                &Mode::Ask,
                &ToolCall {
                    id: "read_1".to_owned(),
                    name: ToolName::ReadFile,
                    arguments: json!({ "path": "src/lib.rs", "end_line": 1 }),
                },
            )
            .expect("file reads");
        assert_eq!(read.output["content"], "pub fn hello() {}");

        let search = tools
            .execute(
                &Mode::Ask,
                &ToolCall {
                    id: "search_1".to_owned(),
                    name: ToolName::SearchCode,
                    arguments: json!({ "query": "TODO" }),
                },
            )
            .expect("code searches");
        assert_eq!(search.output["results"][0]["path"], "src/lib.rs");

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn redacts_sensitive_files_but_preserves_safe_source_reads() {
        let root = workspace();
        fs::write(root.join(".env"), "API_KEY=plain-secret\nMODE=dev\n")
            .expect("env file writes");
        fs::write(
            root.join("config.json"),
            r#"{"api_key":"plain-secret","name":"demo"}"#,
        )
        .expect("config file writes");
        fs::write(root.join("id_rsa"), "PRIVATE KEY plain-secret\n").expect("key writes");
        fs::create_dir_all(root.join("src")).expect("source directory creates");
        fs::write(root.join("src/lib.rs"), "const VALUE: &str = \"plain-secret\";\n")
            .expect("source file writes");
        let tools = ToolRegistry::new(&root).expect("registry starts");

        let read = |id: &str, path: &str| {
            tools
                .execute(
                    &Mode::Ask,
                    &ToolCall {
                        id: id.to_owned(),
                        name: ToolName::ReadFile,
                        arguments: json!({ "path": path }),
                    },
                )
                .expect("file reads")
        };

        let env = read("read_env", ".env");
        assert_eq!(env.output["content_redacted"], true);
        assert!(env.output["content"].as_str().unwrap().contains("API_KEY=[REDACTED]"));
        assert!(!env.output["content"].as_str().unwrap().contains("plain-secret"));

        let config = read("read_config", "config.json");
        assert_eq!(config.output["content_redacted"], true);
        assert!(config.output["content"].as_str().unwrap().contains("api_key"));
        assert!(!config.output["content"].as_str().unwrap().contains("plain-secret"));

        let key = read("read_key", "id_rsa");
        assert_eq!(key.output["content"], "");
        assert_eq!(key.output["content_redacted"], true);
        assert!(key.output["redaction_reason"].as_str().unwrap().contains("sensitive"));
        assert!(key.summary.contains("sensitive"));

        let source = read("read_source", "src/lib.rs");
        assert_eq!(source.output["content_redacted"], false);
        assert_eq!(source.output["content"], "const VALUE: &str = \"plain-secret\";");

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn redacts_background_logs_when_read_back() {
        let root = workspace();
        fs::create_dir_all(root.join(".xcoding/logs")).expect("log directory creates");
        fs::write(
            root.join(".xcoding/logs/service.log"),
            "api_key=plain-secret\nstarted normally\nsk-test-token-1234567890\neyJaaaaaaaaaaaaaaaaaaaaaaaaaaaa.eyJbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.sig\n",
        )
        .expect("log writes");
        let tools = ToolRegistry::new(&root).expect("registry starts");
        let execution = tools
            .execute(
                &Mode::Ask,
                &ToolCall {
                    id: "read-log".to_owned(),
                    name: ToolName::ReadFile,
                    arguments: json!({ "path": ".xcoding/logs/service.log" }),
                },
            )
            .expect("log reads");

        assert_eq!(execution.output["content_redacted"], true);
        let content = execution.output["content"].as_str().unwrap();
        assert!(content.contains("api_key=[REDACTED]"));
        assert!(content.contains("started normally"));
        assert!(!content.contains("plain-secret"));
        assert!(!content.contains("sk-test-token-1234567890"));
        assert!(!content.contains("eyJaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn search_code_caps_total_result_bytes() {
        let root = workspace();
        let wide_line = format!("match_token {}\n", "x".repeat(1500));
        let mut wide = String::new();
        for _ in 0..DEFAULT_SEARCH_RESULTS {
            wide.push_str(&wide_line);
        }
        fs::write(root.join("wide.rs"), &wide).expect("wide file writes");
        let tools = ToolRegistry::new(&root).expect("registry starts");

        let search = tools
            .execute(
                &Mode::Ask,
                &ToolCall {
                    id: "search_bytes".to_owned(),
                    name: ToolName::SearchCode,
                    arguments: json!({ "query": "match_token" }),
                },
            )
            .expect("search runs");

        let results = search.output["results"].as_array().expect("results array");
        assert!(!results.is_empty(), "expected at least one retained result");
        assert!(
            results.len() < DEFAULT_SEARCH_RESULTS,
            "expected byte cap to drop results, kept {}",
            results.len()
        );
        assert_eq!(search.output["truncated"], json!(true));
        let encoded = serde_json::to_string(&search.output["results"]).expect("results serialize");
        assert!(
            encoded.len() <= MAX_TOOL_JSON_BYTES + 4096,
            "results payload {} exceeded budget",
            encoded.len()
        );

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn truncate_output_cuts_on_char_boundary_for_multibyte_text() {
        // 32 KiB lands mid-character for 3-byte text: boundaries sit at
        // multiples of 3, so byte 32_768 is the last byte of a character.
        let value = "\u{4e2d}".repeat(10_923);
        assert_eq!(value.len(), 32_769);
        assert!(!value.is_char_boundary(32 * 1024));

        let truncated = truncate_output(&value);

        let kept = truncated
            .strip_suffix("\n[output truncated]")
            .expect("truncation marker appended");
        assert_eq!(kept, "\u{4e2d}".repeat(10_922));
    }

    #[test]
    fn truncate_output_keeps_output_within_budget_verbatim() {
        let value = "\u{4e2d}".repeat(10_922);
        assert_eq!(value.len(), 32_766);
        assert_eq!(truncate_output(&value), value);
    }

    #[test]
    fn git_status_collapses_untracked_directories() {
        let root = workspace();
        let tools = ToolRegistry::new(&root).expect("registry starts");
        let init = git_command()
            .args(["init"])
            .current_dir(&root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git init runs");
        assert!(init.success());
        let nested = root.join("build/artifacts");
        fs::create_dir_all(&nested).expect("nested dirs create");
        for index in 0..25 {
            fs::write(nested.join(format!("out{index}.bin")), "x\n").expect("artifact writes");
        }

        let status = tools
            .execute(
                &Mode::Ask,
                &ToolCall {
                    id: "status_collapse".to_owned(),
                    name: ToolName::GitStatus,
                    arguments: json!({}),
                },
            )
            .expect("git status runs");

        let entries = status.output["entries"].as_array().expect("entries array");
        let untracked: Vec<&str> = entries
            .iter()
            .filter(|entry| entry.get("worktree_status").and_then(Value::as_str) == Some("?"))
            .filter_map(|entry| entry.get("path").and_then(Value::as_str))
            .collect();
        assert_eq!(
            untracked,
            vec!["build/"],
            "expected the untracked tree to collapse into one entry"
        );
        assert!(
            !status.output["raw"]
                .as_str()
                .expect("raw text")
                .contains("out0.bin")
        );
        assert_eq!(status.output["truncated"], json!(false));

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn git_status_caps_total_entry_bytes() {
        let root = workspace();
        let tools = ToolRegistry::new(&root).expect("registry starts");
        let init = git_command()
            .args(["init"])
            .current_dir(&root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git init runs");
        assert!(init.success());
        let file_count = 300usize;
        for index in 0..file_count {
            let name = format!("{index:04}_{}.txt", "n".repeat(100));
            fs::write(root.join(&name), "x\n").expect("untracked file writes");
        }

        let status = tools
            .execute(
                &Mode::Ask,
                &ToolCall {
                    id: "status_bytes".to_owned(),
                    name: ToolName::GitStatus,
                    arguments: json!({}),
                },
            )
            .expect("git status runs");

        let entries = status.output["entries"].as_array().expect("entries array");
        assert!(!entries.is_empty(), "expected at least one retained entry");
        assert!(
            entries.len() < file_count,
            "expected byte cap to drop entries, kept {}",
            entries.len()
        );
        assert_eq!(status.output["truncated"], json!(true));
        let encoded = serde_json::to_string(&status.output["entries"]).expect("entries serialize");
        assert!(
            encoded.len() <= MAX_TOOL_JSON_BYTES + 4096,
            "entries payload {} exceeded budget",
            encoded.len()
        );

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn search_code_supports_case_glob_and_context() {
        let root = workspace();
        fs::create_dir_all(root.join("src")).expect("src creates");
        fs::create_dir_all(root.join("notes")).expect("notes creates");
        fs::write(
            root.join("src/lib.rs"),
            "// preamble\npub fn find_me() {}\n// trailer\n",
        )
        .expect("source writes");
        fs::write(root.join("notes/readme.md"), "find_me in docs\n").expect("doc writes");
        let tools = ToolRegistry::new(&root).expect("registry starts");

        let case_search = tools
            .execute(
                &Mode::Ask,
                &ToolCall {
                    id: "search_case".to_owned(),
                    name: ToolName::SearchCode,
                    arguments: json!({
                        "query": "FIND_ME",
                        "case_insensitive": true,
                        "glob": "*.rs",
                        "context_lines": 1,
                    }),
                },
            )
            .expect("case search");
        assert_eq!(case_search.output["results"].as_array().unwrap().len(), 1);
        assert_eq!(case_search.output["results"][0]["path"], "src/lib.rs");
        assert_eq!(
            case_search.output["results"][0]["text"],
            "pub fn find_me() {}"
        );
        assert_eq!(case_search.output["results"][0]["before"][0], "// preamble");
        assert_eq!(case_search.output["results"][0]["after"][0], "// trailer");

        let exact = tools
            .execute(
                &Mode::Ask,
                &ToolCall {
                    id: "search_exact".to_owned(),
                    name: ToolName::SearchCode,
                    arguments: json!({ "query": "FIND_ME" }),
                },
            )
            .expect("exact search");
        assert_eq!(exact.output["results"].as_array().unwrap().len(), 0);

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn search_code_prefers_source_paths_over_generated() {
        let root = workspace();
        fs::create_dir_all(root.join("src")).expect("src creates");
        // dist is ignored as a directory now; use a non-ignored generated-looking path.
        fs::create_dir_all(root.join("generated")).expect("generated creates");
        fs::write(
            root.join("src/auth.ts"),
            "export const token = 'secret-marker';\n",
        )
        .expect("src writes");
        fs::write(
            root.join("generated/bundle.min.js"),
            "var token='secret-marker';\n",
        )
        .expect("generated writes");
        let tools = ToolRegistry::new(&root).expect("registry starts");

        let search = tools
            .execute(
                &Mode::Ask,
                &ToolCall {
                    id: "search_rank".to_owned(),
                    name: ToolName::SearchCode,
                    arguments: json!({ "query": "secret-marker", "max_results": 1 }),
                },
            )
            .expect("ranked search");
        assert_eq!(search.output["results"][0]["path"], "src/auth.ts");

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn rejects_paths_outside_the_workspace() {
        let root = workspace();
        let tools = ToolRegistry::new(&root).expect("registry starts");

        let error = tools
            .execute(
                &Mode::Ask,
                &ToolCall {
                    id: "read_1".to_owned(),
                    name: ToolName::ReadFile,
                    arguments: json!({ "path": "../outside.txt" }),
                },
            )
            .expect_err("outside path is rejected");
        assert!(matches!(error, ToolError::PathOutsideWorkspace(_)));

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn reports_git_status_and_diff_for_workspace_changes() {
        let root = workspace();
        let tools = ToolRegistry::new(&root).expect("registry starts");
        let init = git_command()
            .args(["init"])
            .current_dir(&root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git init runs");
        assert!(init.success());
        let _ = git_command()
            .args(["config", "user.email", "xcoding@example.com"])
            .current_dir(&root)
            .status();
        let _ = git_command()
            .args(["config", "user.name", "XCoding"])
            .current_dir(&root)
            .status();
        fs::write(root.join("hello.txt"), "hello\n").expect("file writes");
        let add = git_command()
            .args(["add", "hello.txt"])
            .current_dir(&root)
            .status()
            .expect("git add runs");
        assert!(add.success());
        let commit = git_command()
            .args(["commit", "-m", "init"])
            .current_dir(&root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git commit runs");
        assert!(commit.success());
        fs::write(root.join("hello.txt"), "hello world\n").expect("file mutates");
        fs::write(root.join("new.txt"), "new\n").expect("new file writes");

        let status = tools
            .execute(
                &Mode::Ask,
                &ToolCall {
                    id: "status_1".to_owned(),
                    name: ToolName::GitStatus,
                    arguments: json!({}),
                },
            )
            .expect("git status runs");
        let raw = status.output["raw"].as_str().expect("raw status");
        assert!(raw.contains("hello.txt"), "{raw}");
        assert!(raw.contains("new.txt"), "{raw}");
        assert_eq!(status.output["truncated"], json!(false));

        let diff = tools
            .execute(
                &Mode::Ask,
                &ToolCall {
                    id: "diff_1".to_owned(),
                    name: ToolName::GitDiff,
                    arguments: json!({ "path": "hello.txt" }),
                },
            )
            .expect("git diff runs");
        let unstaged = diff.output["unstaged"].as_str().expect("unstaged diff");
        assert!(unstaged.contains("hello world"), "{unstaged}");

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn reports_git_log_and_show_for_workspace_history() {
        let root = workspace();
        let tools = ToolRegistry::new(&root).expect("registry starts");
        let init = git_command()
            .args(["init"])
            .current_dir(&root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git init runs");
        assert!(init.success());
        let _ = git_command()
            .args(["config", "user.email", "xcoding@example.com"])
            .current_dir(&root)
            .status();
        let _ = git_command()
            .args(["config", "user.name", "XCoding"])
            .current_dir(&root)
            .status();
        fs::write(root.join("hello.txt"), "hello\n").expect("file writes");
        let add = git_command()
            .args(["add", "hello.txt"])
            .current_dir(&root)
            .status()
            .expect("git add runs");
        assert!(add.success());
        let commit = git_command()
            .args(["commit", "-m", "init commit"])
            .current_dir(&root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git commit runs");
        assert!(commit.success());
        fs::write(root.join("hello.txt"), "hello world\n").expect("file mutates");
        let add2 = git_command()
            .args(["add", "hello.txt"])
            .current_dir(&root)
            .status()
            .expect("git add runs");
        assert!(add2.success());
        let commit2 = git_command()
            .args(["commit", "-m", "second commit"])
            .current_dir(&root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git commit runs");
        assert!(commit2.success());

        let log = tools
            .execute(
                &Mode::Ask,
                &ToolCall {
                    id: "log_1".to_owned(),
                    name: ToolName::GitLog,
                    arguments: json!({ "max_count": 5 }),
                },
            )
            .expect("git log runs");
        let commits = log.output["commits"].as_array().expect("commits array");
        assert!(commits.len() >= 2, "{:?}", log.output);
        let subjects: Vec<&str> = commits
            .iter()
            .filter_map(|c| c.get("subject").and_then(Value::as_str))
            .collect();
        assert!(
            subjects.iter().any(|s| s.contains("second commit")),
            "{subjects:?}"
        );

        let show = tools
            .execute(
                &Mode::Ask,
                &ToolCall {
                    id: "show_1".to_owned(),
                    name: ToolName::GitShow,
                    arguments: json!({ "revision": "HEAD", "path": "hello.txt" }),
                },
            )
            .expect("git show runs");
        let subject = show.output["subject"].as_str().expect("subject");
        assert!(subject.contains("second commit"), "{subject}");
        let patch = show.output["patch"].as_str().unwrap_or("");
        let raw = show.output["raw"].as_str().unwrap_or("");
        assert!(
            patch.contains("hello world") || raw.contains("hello world"),
            "patch={patch} raw={raw}"
        );

        let missing = tools
            .execute(
                &Mode::Ask,
                &ToolCall {
                    id: "show_missing".to_owned(),
                    name: ToolName::GitShow,
                    arguments: json!({ "revision": "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef" }),
                },
            )
            .expect_err("bad revision fails");
        assert!(matches!(missing, ToolError::InvalidCommand(_)));

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn ask_mode_auto_applies_ordinary_workspace_patches() {
        let root = workspace();
        let tools = ToolRegistry::new(&root).expect("registry starts");
        let applied = tools
            .execute(
                &Mode::Ask,
                &ToolCall {
                    id: "ask_write".to_owned(),
                    name: ToolName::ApplyPatch,
                    arguments: json!({
                        "path": "notes/hello.txt",
                        "old_text": "",
                        "new_text": "hello workspace\n"
                    }),
                },
            )
            .expect("ask mode allows ordinary workspace writes");
        assert_eq!(applied.output["path"], "notes/hello.txt");
        assert_eq!(
            fs::read_to_string(root.join("notes/hello.txt")).expect("file written"),
            "hello workspace\n"
        );

        let denied = tools
            .execute(
                &Mode::Ask,
                &ToolCall {
                    id: "ask_high_risk".to_owned(),
                    name: ToolName::ApplyPatch,
                    arguments: json!({
                        "path": ".xcoding/secret.txt",
                        "old_text": "",
                        "new_text": "nope\n"
                    }),
                },
            )
            .expect_err("high-risk workspace paths still need approval");
        assert!(matches!(denied, ToolError::PermissionDenied));

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn marks_git_write_tools_as_high_risk() {
        let root = workspace();
        let tools = ToolRegistry::new(&root).expect("registry starts");

        let (kind, high_risk, allowlisted) = tools
            .permission_for(&ToolCall {
                id: "add_perm".to_owned(),
                name: ToolName::GitAdd,
                arguments: json!({ "paths": ["hello.txt"] }),
            })
            .expect("git_add permission");
        assert_eq!(kind, PermissionKind::Write);
        assert!(high_risk);
        assert!(!allowlisted);

        let (kind, high_risk, allowlisted) = tools
            .permission_for(&ToolCall {
                id: "commit_perm".to_owned(),
                name: ToolName::GitCommit,
                arguments: json!({ "message": "msg" }),
            })
            .expect("git_commit permission");
        assert_eq!(kind, PermissionKind::Write);
        assert!(high_risk);
        assert!(!allowlisted);

        let (kind, high_risk, allowlisted) = tools
            .permission_for(&ToolCall {
                id: "push_perm".to_owned(),
                name: ToolName::GitPush,
                arguments: json!({}),
            })
            .expect("git_push permission");
        assert_eq!(kind, PermissionKind::Write);
        assert!(high_risk);
        assert!(!allowlisted);

        for (id, name, args) in [
            (
                "fetch_perm",
                ToolName::GitFetch,
                json!({ "remote": "origin" }),
            ),
            (
                "pull_perm",
                ToolName::GitPull,
                json!({ "remote": "origin", "branch": "main" }),
            ),
        ] {
            let (kind, high_risk, allowlisted) = tools
                .permission_for(&ToolCall {
                    id: id.to_owned(),
                    name,
                    arguments: args,
                })
                .expect("git fetch/pull permission");
            assert_eq!(kind, PermissionKind::Write);
            assert!(high_risk);
            assert!(!allowlisted);
        }

        // Even auto-edit must not auto-run high-risk git writes through execute().
        let denied = tools
            .execute(
                &Mode::AutoEdit,
                &ToolCall {
                    id: "add_denied".to_owned(),
                    name: ToolName::GitAdd,
                    arguments: json!({ "paths": ["hello.txt"] }),
                },
            )
            .expect_err("auto-edit still denies unauthorized high-risk write");
        assert!(matches!(denied, ToolError::PermissionDenied));

        let denied_push = tools
            .execute(
                &Mode::AutoEdit,
                &ToolCall {
                    id: "push_denied".to_owned(),
                    name: ToolName::GitPush,
                    arguments: json!({ "remote": "origin", "branch": "main" }),
                },
            )
            .expect_err("auto-edit still denies unauthorized git push");
        assert!(matches!(denied_push, ToolError::PermissionDenied));

        let denied_fetch = tools
            .execute(
                &Mode::AutoEdit,
                &ToolCall {
                    id: "fetch_denied".to_owned(),
                    name: ToolName::GitFetch,
                    arguments: json!({ "remote": "origin" }),
                },
            )
            .expect_err("auto-edit still denies unauthorized git fetch");
        assert!(matches!(denied_fetch, ToolError::PermissionDenied));

        let denied_pull = tools
            .execute(
                &Mode::AutoEdit,
                &ToolCall {
                    id: "pull_denied".to_owned(),
                    name: ToolName::GitPull,
                    arguments: json!({ "remote": "origin", "branch": "main" }),
                },
            )
            .expect_err("auto-edit still denies unauthorized git pull");
        assert!(matches!(denied_pull, ToolError::PermissionDenied));

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn stages_and_commits_with_authorized_git_write_tools() {
        let root = workspace();
        let tools = ToolRegistry::new(&root).expect("registry starts");
        let init = git_command()
            .args(["init"])
            .current_dir(&root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git init runs");
        assert!(init.success());
        let _ = git_command()
            .args(["config", "user.email", "xcoding@example.com"])
            .current_dir(&root)
            .status();
        let _ = git_command()
            .args(["config", "user.name", "XCoding"])
            .current_dir(&root)
            .status();
        fs::write(
            root.join("hello.txt"),
            "hello
",
        )
        .expect("file writes");
        let bootstrap_add = git_command()
            .args(["add", "hello.txt"])
            .current_dir(&root)
            .status()
            .expect("bootstrap add");
        assert!(bootstrap_add.success());
        let bootstrap_commit = git_command()
            .args(["commit", "-m", "init"])
            .current_dir(&root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("bootstrap commit");
        assert!(bootstrap_commit.success());

        fs::write(
            root.join("hello.txt"),
            "hello staged
",
        )
        .expect("mutate");
        let empty_paths = tools
            .execute_authorized(&ToolCall {
                id: "add_empty".to_owned(),
                name: ToolName::GitAdd,
                arguments: json!({ "paths": [] }),
            })
            .expect_err("empty paths rejected");
        assert!(matches!(empty_paths, ToolError::InvalidArguments(_)));

        let high_risk_path = tools
            .execute_authorized(&ToolCall {
                id: "add_dotgit".to_owned(),
                name: ToolName::GitAdd,
                arguments: json!({ "paths": [".git/config"] }),
            })
            .expect_err("dotgit rejected");
        assert!(matches!(high_risk_path, ToolError::InvalidArguments(_)));

        let staged = tools
            .execute_authorized(&ToolCall {
                id: "add_ok".to_owned(),
                name: ToolName::GitAdd,
                arguments: json!({ "paths": ["hello.txt"] }),
            })
            .expect("git add authorized");
        assert_eq!(staged.output["success"], true);
        assert!(
            staged.summary.contains("Staged"),
            "summary={}",
            staged.summary
        );

        let empty_message = tools
            .execute_authorized(&ToolCall {
                id: "commit_empty".to_owned(),
                name: ToolName::GitCommit,
                arguments: json!({ "message": "   " }),
            })
            .expect_err("empty message rejected");
        assert!(matches!(empty_message, ToolError::InvalidArguments(_)));

        let committed = tools
            .execute_authorized(&ToolCall {
                id: "commit_ok".to_owned(),
                name: ToolName::GitCommit,
                arguments: json!({ "message": "stage and commit via tools" }),
            })
            .expect("git commit authorized");
        let hash = committed.output["hash"].as_str().expect("hash");
        assert!(!hash.is_empty(), "{:?}", committed.output);
        assert_eq!(
            committed.output["subject"].as_str().unwrap(),
            "stage and commit via tools"
        );

        let subject = git_command()
            .args(["log", "-1", "--pretty=%s"])
            .current_dir(&root)
            .output()
            .expect("git log");
        assert!(subject.status.success());
        assert_eq!(
            String::from_utf8_lossy(&subject.stdout).trim(),
            "stage and commit via tools"
        );

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn pushes_with_authorized_git_push_tool() {
        let root = workspace();
        let tools = ToolRegistry::new(&root).expect("registry starts");
        let bare = root.parent().unwrap().join(format!(
            "{}_remote.git",
            root.file_name().unwrap().to_string_lossy()
        ));
        let init = git_command()
            .args(["init", "-b", "main"])
            .current_dir(&root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git init runs");
        assert!(init.success());
        let bare_init = git_command()
            .args(["init", "--bare", "-b", "main"])
            .arg(&bare)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("bare init runs");
        assert!(bare_init.success());
        let _ = git_command()
            .args(["config", "user.email", "xcoding@example.com"])
            .current_dir(&root)
            .status();
        let _ = git_command()
            .args(["config", "user.name", "XCoding"])
            .current_dir(&root)
            .status();
        fs::write(root.join("hello.txt"), "hello\\n").expect("file writes");
        assert!(
            git_command()
                .args(["add", "hello.txt"])
                .current_dir(&root)
                .status()
                .expect("add")
                .success()
        );
        assert!(
            git_command()
                .args(["commit", "-m", "init"])
                .current_dir(&root)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("commit")
                .success()
        );
        assert!(
            git_command()
                .args(["remote", "add", "origin"])
                .arg(&bare)
                .current_dir(&root)
                .status()
                .expect("remote add")
                .success()
        );

        let bad_remote = tools
            .execute_authorized(&ToolCall {
                id: "push_bad".to_owned(),
                name: ToolName::GitPush,
                arguments: json!({ "remote": "--force" }),
            })
            .expect_err("flag remote rejected");
        assert!(matches!(bad_remote, ToolError::InvalidArguments(_)));

        let bad_branch = tools
            .execute_authorized(&ToolCall {
                id: "push_branch".to_owned(),
                name: ToolName::GitPush,
                arguments: json!({ "branch": "main:refs/heads/evil" }),
            })
            .expect_err("refspec smuggling rejected");
        assert!(matches!(bad_branch, ToolError::InvalidArguments(_)));

        let pushed = tools
            .execute_authorized(&ToolCall {
                id: "push_ok".to_owned(),
                name: ToolName::GitPush,
                arguments: json!({
                    "remote": "origin",
                    "branch": "main",
                    "set_upstream": true
                }),
            })
            .expect("git push authorized");
        assert_eq!(pushed.output["success"], true);
        assert_eq!(pushed.output["remote"], "origin");
        assert_eq!(pushed.output["branch"], "main");
        assert_eq!(pushed.output["set_upstream"], true);
        assert!(
            pushed.summary.contains("Pushed"),
            "summary={}",
            pushed.summary
        );

        let remote_head = git_command()
            .args(["--git-dir"])
            .arg(&bare)
            .args(["rev-parse", "main"])
            .output()
            .expect("remote rev-parse");
        assert!(remote_head.status.success());
        let remote_hash = String::from_utf8_lossy(&remote_head.stdout)
            .trim()
            .to_owned();
        let local_hash = git_rev_parse_head(&root).expect("local head");
        assert_eq!(remote_hash, local_hash);

        let _ = fs::remove_dir_all(&bare);
        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn fetches_and_pulls_with_authorized_git_tools() {
        let root = workspace();
        let tools = ToolRegistry::new(&root).expect("registry starts");
        let bare = root.parent().unwrap().join(format!(
            "{}_remote.git",
            root.file_name().unwrap().to_string_lossy()
        ));
        let peer = root.parent().unwrap().join(format!(
            "{}_peer",
            root.file_name().unwrap().to_string_lossy()
        ));
        let init = git_command()
            .args(["init", "-b", "main"])
            .current_dir(&root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git init runs");
        assert!(init.success());
        let bare_init = git_command()
            .args(["init", "--bare", "-b", "main"])
            .arg(&bare)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("bare init runs");
        assert!(bare_init.success());
        for (key, value) in [
            ("user.email", "xcoding@example.com"),
            ("user.name", "XCoding"),
        ] {
            let _ = git_command()
                .args(["config", key, value])
                .current_dir(&root)
                .status();
        }
        fs::write(root.join("hello.txt"), "hello\n").expect("file writes");
        assert!(
            git_command()
                .args(["add", "hello.txt"])
                .current_dir(&root)
                .status()
                .expect("add")
                .success()
        );
        assert!(
            git_command()
                .args(["commit", "-m", "init"])
                .current_dir(&root)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("commit")
                .success()
        );
        assert!(
            git_command()
                .args(["remote", "add", "origin"])
                .arg(&bare)
                .current_dir(&root)
                .status()
                .expect("remote add")
                .success()
        );
        assert!(
            git_command()
                .args(["push", "--set-upstream", "origin", "main"])
                .current_dir(&root)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("initial push")
                .success()
        );

        // Peer clone advances remote.
        assert!(
            git_command()
                .args(["clone"])
                .arg(&bare)
                .arg(&peer)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("clone peer")
                .success()
        );
        for (key, value) in [
            ("user.email", "xcoding@example.com"),
            ("user.name", "XCoding"),
        ] {
            let _ = git_command()
                .args(["config", key, value])
                .current_dir(&peer)
                .status();
        }
        fs::write(peer.join("hello.txt"), "hello from peer\n").expect("peer writes");
        assert!(
            git_command()
                .args(["add", "hello.txt"])
                .current_dir(&peer)
                .status()
                .expect("peer add")
                .success()
        );
        assert!(
            git_command()
                .args(["commit", "-m", "peer advance"])
                .current_dir(&peer)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("peer commit")
                .success()
        );
        assert!(
            git_command()
                .args(["push", "origin", "main"])
                .current_dir(&peer)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("peer push")
                .success()
        );
        let peer_head = git_rev_parse_head(&peer).expect("peer head");

        let bad_remote = tools
            .execute_authorized(&ToolCall {
                id: "fetch_bad".to_owned(),
                name: ToolName::GitFetch,
                arguments: json!({ "remote": "--force" }),
            })
            .expect_err("flag remote rejected");
        assert!(matches!(bad_remote, ToolError::InvalidArguments(_)));

        let bad_branch = tools
            .execute_authorized(&ToolCall {
                id: "pull_branch".to_owned(),
                name: ToolName::GitPull,
                arguments: json!({ "branch": "main:refs/heads/evil" }),
            })
            .expect_err("refspec smuggling rejected");
        assert!(matches!(bad_branch, ToolError::InvalidArguments(_)));

        let fetched = tools
            .execute_authorized(&ToolCall {
                id: "fetch_ok".to_owned(),
                name: ToolName::GitFetch,
                arguments: json!({ "remote": "origin", "branch": "main" }),
            })
            .expect("git fetch authorized");
        assert_eq!(fetched.output["success"], true);
        assert_eq!(fetched.output["remote"], "origin");
        assert_eq!(fetched.output["branch"], "main");
        assert!(
            fetched.summary.contains("Fetched"),
            "summary={}",
            fetched.summary
        );

        let pulled = tools
            .execute_authorized(&ToolCall {
                id: "pull_ok".to_owned(),
                name: ToolName::GitPull,
                arguments: json!({
                    "remote": "origin",
                    "branch": "main",
                    "ff_only": true
                }),
            })
            .expect("git pull authorized");
        assert_eq!(pulled.output["success"], true);
        assert_eq!(pulled.output["ff_only"], true);
        assert_eq!(pulled.output["head"].as_str().unwrap(), peer_head);
        assert!(
            pulled.summary.contains("Pulled"),
            "summary={}",
            pulled.summary
        );
        let local_content = fs::read_to_string(root.join("hello.txt")).expect("read local");
        assert_eq!(local_content.replace("\r\n", "\n"), "hello from peer\n");

        let _ = fs::remove_dir_all(&peer);
        let _ = fs::remove_dir_all(&bare);
        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn rolls_back_patches_only_when_the_applied_text_is_unchanged() {
        let root = workspace();
        let tools = ToolRegistry::new(&root).expect("registry starts");
        let existing = root.join("settings.txt");
        fs::write(&existing, "new\n").expect("patched file writes");

        tools
            .rollback_patch("settings.txt", "new\n", Some("old\n"))
            .expect("existing file rolls back");
        assert_eq!(
            fs::read_to_string(&existing).expect("restored file reads"),
            "old\n"
        );

        let created = root.join("created.txt");
        fs::write(&created, "created\n").expect("created patch writes");
        tools
            .rollback_patch("created.txt", "created\n", None)
            .expect("created file rolls back");
        assert!(!created.exists());

        fs::write(&existing, "edited elsewhere\n").expect("external edit writes");
        let error = tools
            .rollback_patch("settings.txt", "old\n", Some("before\n"))
            .expect_err("rollback refuses to overwrite an external edit");
        assert!(matches!(error, ToolError::PatchConflict(_)));
        assert_eq!(error.code(), Some("patch_conflict"));
        assert!(
            error.to_string().contains("re-read the file"),
            "error={}",
            error
        );
        assert_eq!(
            fs::read_to_string(&existing).expect("external edit remains"),
            "edited elsewhere\n"
        );

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn apply_patch_reports_structured_conflict() {
        let root = workspace();
        let tools = ToolRegistry::new(&root).expect("registry starts");
        let path = root.join("notes.txt");
        fs::write(&path, "current\n").expect("seed file");

        let error = tools
            .execute_authorized(&ToolCall {
                id: "conflict".to_owned(),
                name: ToolName::ApplyPatch,
                arguments: json!({
                    "path": "notes.txt",
                    "old_text": "stale\n",
                    "new_text": "next\n"
                }),
            })
            .expect_err("stale old_text conflicts");
        assert!(matches!(error, ToolError::PatchConflict(_)));
        assert_eq!(error.code(), Some("patch_conflict"));
        assert_eq!(error.path(), Some("notes.txt"));
        assert!(
            error.to_string().contains("patch conflict on notes.txt"),
            "error={}",
            error
        );
        let value = error.tool_result_value();
        assert_eq!(value["code"], "patch_conflict");
        assert_eq!(value["path"], "notes.txt");
        assert!(
            value["hint"]
                .as_str()
                .unwrap_or_default()
                .contains("read_file"),
            "hint={value}"
        );
        assert_eq!(
            fs::read_to_string(&path).expect("file remains"),
            "current\n"
        );

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn cancels_long_running_command() {
        let root = workspace();
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = std::sync::Arc::clone(&cancelled);
        let tools_root = root.clone();
        let handle = std::thread::spawn(move || {
            let tools = ToolRegistry::new(&tools_root).expect("tools open");
            tools.execute_authorized_cancellable(
                &ToolCall {
                    id: "cmd".to_owned(),
                    name: ToolName::RunCommand,
                    arguments: if cfg!(windows) {
                        json!({
                            "executable": "ping",
                            "args": ["127.0.0.1", "-n", "30"]
                        })
                    } else {
                        json!({
                            "executable": "sleep",
                            "args": ["30"]
                        })
                    },
                },
                &|| flag.load(std::sync::atomic::Ordering::SeqCst),
            )
        });
        std::thread::sleep(Duration::from_millis(300));
        cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
        let result = handle.join().expect("command thread joins");
        assert!(matches!(result, Err(ToolError::Cancelled)));
        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn rejects_blocked_commands_before_spawn() {
        let root = workspace();
        let tools = ToolRegistry::new(&root).expect("tools");
        let error = tools
            .permission_for(&ToolCall {
                id: "1".to_owned(),
                name: ToolName::RunCommand,
                arguments: json!({ "executable": "format", "args": ["C:"] }),
            })
            .expect_err("blocked");
        match &error {
            ToolError::CommandPolicyDenied { code, reason } => {
                assert_eq!(code, "denied_executable");
                assert!(reason.contains("blocked"));
            }
            other => panic!("unexpected error: {other}"),
        }
        let value = error.tool_result_value();
        assert_eq!(value["code"], "command_policy_denied");
        assert_eq!(value["policy_code"], "denied_executable");
    }

    #[test]
    fn rejects_shell_wrapped_deletes_before_spawn_even_in_background() {
        let root = workspace();
        let tools = ToolRegistry::new(&root).expect("tools");
        let error = tools
            .permission_for(&ToolCall {
                id: "delete-background".to_owned(),
                name: ToolName::RunCommand,
                arguments: json!({
                    "executable": "cmd",
                    "args": ["/c", "rmdir /s /q E:\\outside"],
                    "background": true
                }),
            })
            .expect_err("shell-wrapped delete must be hard denied");
        match &error {
            ToolError::CommandPolicyDenied { code, .. } => {
                assert_eq!(code, "denied_shell_destructive_delete");
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn marks_high_risk_shell_commands() {
        let root = workspace();
        let tools = ToolRegistry::new(&root).expect("tools");
        let (kind, high_risk, _allowlisted) = tools
            .permission_for(&ToolCall {
                id: "1".to_owned(),
                name: ToolName::RunCommand,
                arguments: json!({
                    "executable": "powershell",
                    "args": ["-Command", "Get-ChildItem"]
                }),
            })
            .expect("askable");
        assert_eq!(kind, xcoding_policy::PermissionKind::Exec);
        assert!(high_risk);
    }

    #[test]
    fn marks_dot_xcoding_paths_high_risk() {
        let root = workspace();
        let tools = ToolRegistry::new(&root).expect("tools");
        let (kind, high_risk, _allowlisted) = tools
            .permission_for(&ToolCall {
                id: "1".to_owned(),
                name: ToolName::ApplyPatch,
                arguments: json!({
                    "path": ".xcoding/secret.txt",
                    "old_text": "",
                    "new_text": "secret\n"
                }),
            })
            .expect("patch");
        assert_eq!(kind, xcoding_policy::PermissionKind::Write);
        assert!(high_risk);
    }

    #[test]
    fn marks_hooks_and_ci_paths_high_risk() {
        for path in [
            ".git/hooks/pre-commit",
            ".github/workflows/ci.yml",
            ".gitlab-ci.yml",
            ".circleci/config.yml",
        ] {
            let call = ToolCall {
                id: format!("path-{path}"),
                name: ToolName::ApplyPatch,
                arguments: serde_json::json!({
                    "path": path,
                    "old_text": "",
                    "new_text": "changed"
                }),
            };
            let root = workspace();
            let tools = ToolRegistry::new(&root).expect("tools");
            let (_, high_risk, _) = tools.permission_for(&call).expect("patch");
            assert!(high_risk, "{path}");
        }
    }

    #[test]
    fn honors_workspace_command_allowlist_file() {
        let root = workspace();
        fs::create_dir_all(root.join(".xcoding")).expect("dir");
        fs::write(
            root.join(".xcoding/command-allowlist"),
            "git:--version\n# comment\n",
        )
        .expect("allowlist writes");
        let tools = ToolRegistry::new(&root).expect("tools");
        assert_eq!(tools.command_allowlist(), &["git:--version".to_owned()]);
        let (kind, high_risk, allowlisted) = tools
            .permission_for(&ToolCall {
                id: "t-custom".to_owned(),
                name: ToolName::RunCommand,
                arguments: json!({
                    "executable": "git",
                    "args": ["--version"]
                }),
            })
            .expect("custom allowlisted");
        assert_eq!(kind, PermissionKind::Exec);
        assert!(!high_risk);
        assert!(allowlisted);
        fs::write(root.join(".xcoding/command-allowlist"), "powershell\ncmd\n").expect("rewrite");
        let tools = ToolRegistry::new(&root).expect("reload");
        assert!(tools.command_allowlist().is_empty());
    }

    #[test]
    fn honors_workspace_command_denylist_file() {
        let root = workspace();
        fs::create_dir_all(root.join(".xcoding")).expect("dir");
        fs::write(
            root.join(".xcoding/command-denylist"),
            "cargo:--version\n# comment\n",
        )
        .expect("denylist writes");
        let tools = ToolRegistry::new(&root).expect("tools");
        assert_eq!(tools.command_denylist(), &["cargo:--version".to_owned()]);
        let error = tools
            .permission_for(&ToolCall {
                id: "t-deny".to_owned(),
                name: ToolName::RunCommand,
                arguments: json!({
                    "executable": "cargo",
                    "args": ["--version"]
                }),
            })
            .expect_err("denylisted cargo --version");
        let value = error.tool_result_value();
        match &error {
            ToolError::CommandPolicyDenied { code, reason } => {
                assert_eq!(code, "denied_workspace_denylist");
                assert!(!reason.is_empty());
            }
            other => panic!("expected CommandPolicyDenied, got {other:?}"),
        }
        assert_eq!(value["code"], "command_policy_denied");
        assert_eq!(value["policy_code"], "denied_workspace_denylist");
        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn marks_allowlisted_build_commands() {
        let root = workspace();
        let tools = ToolRegistry::new(&root).expect("tools");
        let (kind, high_risk, allowlisted) = tools
            .permission_for(&ToolCall {
                id: "1".to_owned(),
                name: ToolName::RunCommand,
                arguments: json!({
                    "executable": "cargo",
                    "args": ["--version"]
                }),
            })
            .expect("allowlisted");
        assert_eq!(kind, xcoding_policy::PermissionKind::Exec);
        assert!(!high_risk);
        assert!(allowlisted);
        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn previews_patch_when_parent_directory_is_missing() {
        let root = workspace();
        let tools = ToolRegistry::new(&root).expect("tools");
        let preview = tools
            .patch_preview(&ToolCall {
                id: "1".to_owned(),
                name: ToolName::ApplyPatch,
                arguments: json!({
                    "path": "nested/missing/new.txt",
                    "old_text": "",
                    "new_text": "created\n"
                }),
            })
            .expect("preview");
        assert_eq!(preview.path.replace('\\', "/"), "nested/missing/new.txt");
        assert!(!preview.file_existed);
        assert_eq!(preview.new_text, "created\n");
    }

    #[test]
    fn reads_browser_state_snapshot_as_read_only_tool() {
        let root = workspace();
        let tools = ToolRegistry::new(&root).expect("registry starts");
        let state_path = root.join("browser-state.json");
        fs::write(
            &state_path,
            r#"{"available":true,"url":"https://example.test/docs","title":"Docs","visible":true,"updated_at":123}"#,
        )
        .expect("state write");
        // SAFETY: test-only path override for deterministic browser_state reads.
        unsafe {
            env::set_var("XCODING_BROWSER_STATE_PATH", &state_path);
        }
        let loaded = tools
            .execute(
                &Mode::Ask,
                &ToolCall {
                    id: "browser_1".to_owned(),
                    name: ToolName::BrowserState,
                    arguments: json!({}),
                },
            )
            .expect("browser state loads");
        unsafe {
            env::remove_var("XCODING_BROWSER_STATE_PATH");
        }
        assert_eq!(loaded.output["available"], true);
        assert_eq!(loaded.output["url"], "https://example.test/docs");
        assert_eq!(loaded.output["title"], "Docs");
        assert_eq!(loaded.output["visible"], true);
        assert!(loaded.summary.contains("example.test/docs"));
    }

    #[test]
    fn records_a_model_authored_plan_of_any_length() {
        let root = workspace();
        let tools = ToolRegistry::new(&root).expect("registry starts");
        let call = ToolCall {
            id: "plan_1".to_owned(),
            name: ToolName::UpdatePlan,
            arguments: json!({
                "steps": [
                    { "description": "  Read the popover render path  ", "status": "done" },
                    { "description": "Add the update_plan tool", "status": "in_progress" },
                    { "description": "Emit the plan event" },
                    { "description": "Update desktop step highlighting", "status": "unknown" },
                    { "description": "Run cargo tests" }
                ]
            }),
        };
        // Read permission, so a plan refresh never asks for approval.
        assert_eq!(
            tools.permission_for(&call).expect("permission resolves"),
            (PermissionKind::Read, false, false)
        );

        let execution = tools.execute(&Mode::Ask, &call).expect("plan records");
        let steps = execution.output["steps"]
            .as_array()
            .expect("steps array")
            .clone();
        assert_eq!(steps.len(), 5);
        assert_eq!(steps[0]["id"], "step_1");
        assert_eq!(steps[0]["description"], "Read the popover render path");
        assert_eq!(steps[0]["status"], "done");
        assert_eq!(steps[1]["status"], "in_progress");
        assert_eq!(steps[2]["status"], "pending");
        assert_eq!(steps[3]["status"], "pending");
        assert_eq!(steps[4]["id"], "step_5");
        assert_eq!(execution.summary, "Updated plan (1/5 done)");
    }

    #[test]
    fn rejects_empty_or_oversized_plans() {
        let root = workspace();
        let tools = ToolRegistry::new(&root).expect("registry starts");
        let call = |steps: Value| ToolCall {
            id: "plan_1".to_owned(),
            name: ToolName::UpdatePlan,
            arguments: json!({ "steps": steps }),
        };

        assert!(matches!(
            tools.execute(&Mode::Ask, &call(json!([]))),
            Err(ToolError::InvalidArguments(_))
        ));
        assert!(matches!(
            tools.execute(&Mode::Ask, &call(json!([{ "description": "   " }]))),
            Err(ToolError::InvalidArguments(_))
        ));
        let too_many = (0..MAX_PLAN_STEPS + 1)
            .map(|index| json!({ "description": format!("step {index}") }))
            .collect::<Vec<_>>();
        assert!(matches!(
            tools.execute(&Mode::Ask, &call(json!(too_many))),
            Err(ToolError::InvalidArguments(_))
        ));
    }

    #[test]
    fn resolves_relative_executables_against_the_workspace_root() {
        let root = workspace();
        let build_dir = root.join("target/release");
        fs::create_dir_all(&build_dir).expect("build dir creates");
        let binary = build_dir.join(if cfg!(windows) {
            "demo-tool.exe"
        } else {
            "demo-tool"
        });
        fs::write(&binary, b"binary").expect("binary writes");
        let tools = ToolRegistry::new(&root).expect("registry starts");
        let expected = binary.canonicalize().expect("binary canonicalizes");

        // Bare command names stay untouched so PATH lookup still applies.
        assert_eq!(
            tools.resolve_executable("cargo").expect("bare command"),
            PathBuf::from("cargo")
        );

        let relative = if cfg!(windows) {
            "target/release/demo-tool.exe"
        } else {
            "target/release/demo-tool"
        };
        assert_eq!(
            tools.resolve_executable(relative).expect("target path"),
            expected
        );
        assert_eq!(
            tools
                .resolve_executable(&format!("./{relative}"))
                .expect("dot-slash path"),
            expected
        );
        #[cfg(windows)]
        assert_eq!(
            tools
                .resolve_executable("target\\release\\demo-tool.exe")
                .expect("backslash path"),
            expected
        );

        let missing = tools
            .resolve_executable("target/release/absent-tool")
            .expect_err("missing executable");
        assert!(matches!(missing, ToolError::InvalidCommand(_)));

        let escaping = tools
            .resolve_executable("../evil")
            .expect_err("parent traversal");
        assert!(matches!(escaping, ToolError::PathOutsideWorkspace(_)));

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    #[cfg(windows)]
    fn cmd_start_does_not_deadlock_on_inherited_pipes() {
        use std::time::Instant;

        let root = workspace();
        // `start` is refused by policy at the tool-call boundary, so the leak is
        // reproduced the way it actually reaches us in practice: inside a script.
        // `ping` inherits the stdout pipe and outlives the `cmd` that launched it.
        fs::write(
            root.join("leak-pipe.cmd"),
            "@echo off\r\nstart /B ping 127.0.0.1 -n 60\r\n",
        )
        .expect("script writes");
        let tools = ToolRegistry::new(&root).expect("tools");
        let start = Instant::now();
        let result = tools.execute_authorized_cancellable(
            &ToolCall {
                id: "deadlock-check".to_owned(),
                name: ToolName::RunCommand,
                arguments: json!({
                    "executable": "cmd",
                    "args": ["/C", "leak-pipe.cmd"]
                }),
            },
            &|| false,
        );
        let elapsed = start.elapsed();

        // Before the bounded drain this call never returned: `start` hands the
        // inherited pipe write handle to a process that outlives `cmd`, so reading
        // to end blocked forever. The bound is deliberately loose because the
        // regression being guarded is an unbounded hang, not slowness.
        assert!(
            elapsed.as_secs() < 15,
            "cmd /C start deadlock: took {elapsed:?}"
        );
        assert!(result.is_ok(), "expected ok, got error: {result:?}");
        // The call must finish on the child's own exit, not on the timeout path.
        assert_eq!(result.unwrap().output["timed_out"], false);
        // The job object kills `ping` as the call returns, but Windows releases
        // its handle on the working directory a moment later, so the cleanup is
        // retried instead of asserted on the first attempt.
        for attempt in 0..25 {
            if fs::remove_dir_all(&root).is_ok() {
                break;
            }
            assert!(attempt < 24, "workspace removes");
            thread::sleep(Duration::from_millis(100));
        }
    }

    #[test]
    fn terminates_a_command_that_never_exits() {
        let root = workspace();
        let tools = ToolRegistry::new(&root).expect("tools");
        let start = Instant::now();
        let execution = tools
            .execute_authorized_cancellable(
                &ToolCall {
                    id: "timeout-check".to_owned(),
                    name: ToolName::RunCommand,
                    arguments: if cfg!(windows) {
                        json!({
                            "executable": "ping",
                            "args": ["127.0.0.1", "-n", "120"],
                            "timeout_seconds": 2
                        })
                    } else {
                        json!({
                            "executable": "sleep",
                            "args": ["120"],
                            "timeout_seconds": 2
                        })
                    },
                },
                &|| false,
            )
            .expect("timeout reports output instead of failing the call");
        let elapsed = start.elapsed();

        // A foreground resident process used to hold the turn open forever.
        assert!(
            elapsed.as_secs() < 20,
            "timeout did not fire: took {elapsed:?}"
        );
        assert_eq!(execution.output["timed_out"], true);
        assert_eq!(execution.output["success"], false);
        assert!(execution.summary.contains("timed out"));
        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    #[cfg(windows)]
    fn runs_a_command_in_the_requested_workspace_subdirectory() {
        let root = workspace();
        // Stands in for a service that only finds its own migrations, config, or
        // assets when it is started from its own directory instead of the
        // workspace root.
        let service_dir = root.join("server");
        fs::create_dir_all(&service_dir).expect("service dir creates");
        let tools = ToolRegistry::new(&root).expect("tools");

        let execution = tools
            .execute_authorized_cancellable(
                &ToolCall {
                    id: "cwd-check".to_owned(),
                    name: ToolName::RunCommand,
                    arguments: json!({
                        "executable": "cmd",
                        "args": ["/C", "cd"],
                        "cwd": "server"
                    }),
                },
                &|| false,
            )
            .expect("command runs in the requested directory");

        assert_eq!(execution.output["success"], true);
        assert_eq!(execution.output["cwd"], "server");
        let stdout = execution.output["stdout"].as_str().expect("stdout text");
        assert!(
            stdout.trim().to_ascii_lowercase().ends_with("\\server"),
            "command did not run in the subdirectory: {stdout:?}"
        );

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn rejects_a_working_directory_that_is_not_a_workspace_directory() {
        let root = workspace();
        fs::write(root.join("notes.txt"), b"text").expect("file writes");
        let tools = ToolRegistry::new(&root).expect("tools");

        let outside = tools
            .run_command(
                parse_arguments(&json!({
                    "executable": "cargo",
                    "args": ["--version"],
                    "cwd": "../"
                }))
                .expect("arguments parse"),
                &|| false,
            )
            .expect_err("parent traversal is refused");
        assert!(matches!(outside, ToolError::PathOutsideWorkspace(_)));

        let not_a_dir = tools
            .run_command(
                parse_arguments(&json!({
                    "executable": "cargo",
                    "args": ["--version"],
                    "cwd": "notes.txt"
                }))
                .expect("arguments parse"),
                &|| false,
            )
            .expect_err("a file is not a working directory");
        assert!(matches!(not_a_dir, ToolError::NotDirectory(_)));

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn run_command_executes_with_original_arguments() {
        let root = workspace();
        let tools = ToolRegistry::new(&root).expect("registry starts");
        let execution = tools
            .run_command(
                parse_arguments(&json!({
                    "executable": "cmd",
                    "args": ["/C", "echo", "api_key=plain-secret"]
                }))
                .expect("arguments parse"),
                &|| false,
            )
            .expect("command runs");

        assert_eq!(execution.output["success"], true);
        assert_eq!(execution.output["args"][2], "api_key=plain-secret");
        assert!(execution.output["stdout"].as_str().unwrap().contains("plain-secret"));

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn rejects_git_repository_paths_outside_workspace_before_spawn() {
        let root = workspace();
        let tools = ToolRegistry::new(&root).expect("tools");
        for args in [
            vec!["-C", "..", "status"],
            vec!["--git-dir=..\\other\\.git", "status"],
            vec!["--work-tree", "..\\other", "status"],
        ] {
            let error = tools
                .run_command(
                    parse_arguments(&json!({
                        "executable": "git",
                        "args": args
                    }))
                    .expect("arguments parse"),
                    &|| false,
                )
                .expect_err("git path must stay in workspace");
            assert!(matches!(error, ToolError::PathOutsideWorkspace(_)), "{error:?}");
        }
        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    #[cfg(windows)]
    fn reports_why_a_background_service_died_on_startup() {
        let root = workspace();
        // Stands in for a service that fails on startup: it prints the reason and
        // exits, which used to be reported as a successful launch with a pid.
        fs::write(
            root.join("broken-service.cmd"),
            "@echo off\r\necho migrations path not found 1>&2\r\nexit /b 3\r\n",
        )
        .expect("script writes");
        let tools = ToolRegistry::new(&root).expect("tools");

        let execution = tools
            .execute_authorized_cancellable(
                &ToolCall {
                    id: "bg-failure".to_owned(),
                    name: ToolName::RunCommand,
                    arguments: json!({
                        "executable": "cmd",
                        "args": ["/C", "broken-service.cmd"],
                        "background": true
                    }),
                },
                &|| false,
            )
            .expect("failed launch is reported, not an error");

        assert_eq!(execution.output["success"], false);
        assert_eq!(execution.output["exited_immediately"], true);
        assert_eq!(execution.output["exit_code"], 3);
        assert!(
            execution.output["log_tail"]
                .as_str()
                .expect("log tail text")
                .contains("migrations path not found"),
            "startup failure was not reported: {:?}",
            execution.output["log_tail"]
        );

        // The log stays on disk so it can be read again after the call.
        let log_path = execution.output["log_path"]
            .as_str()
            .expect("log path reported");
        assert!(log_path.starts_with(".xcoding/logs/"));
        assert!(root.join(log_path).is_file(), "log file kept at {log_path}");

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn clamps_requested_timeouts_into_the_supported_range() {
        let parse = |value: Value| {
            parse_arguments::<RunCommandArgs>(&value)
                .expect("args parse")
                .effective_timeout()
        };
        assert_eq!(
            parse(json!({ "executable": "cargo" })),
            DEFAULT_COMMAND_TIMEOUT
        );
        assert_eq!(
            parse(json!({ "executable": "cargo", "timeout_seconds": 30 })),
            Duration::from_secs(30)
        );
        assert_eq!(
            parse(json!({ "executable": "cargo", "timeout_seconds": 0 })),
            Duration::from_secs(1)
        );
        assert_eq!(
            parse(json!({ "executable": "cargo", "timeout_seconds": 99_999 })),
            Duration::from_secs(MAX_COMMAND_TIMEOUT_SECONDS)
        );
    }

    /// A grandchild that detached from the direct child used to survive the call
    /// and keep holding its port; the job object must reap it with the guard.
    #[test]
    #[cfg(windows)]
    fn reclaims_detached_grandchildren_when_the_call_returns() {
        let root = workspace();
        // The redirection and quoting live in a script so the tool still receives
        // a plain argument vector.
        fs::write(
            root.join("spawn-grandchild.cmd"),
            "@echo off\r\nstart /B cmd /C ping 127.0.0.1 -n 120 > marker.txt\r\n",
        )
        .expect("script writes");
        let tools = ToolRegistry::new(&root).expect("tools");
        tools
            .execute_authorized_cancellable(
                &ToolCall {
                    id: "tree-check".to_owned(),
                    name: ToolName::RunCommand,
                    arguments: json!({
                        "executable": "cmd",
                        "args": ["/C", "spawn-grandchild.cmd"],
                        "timeout_seconds": 5
                    }),
                },
                &|| false,
            )
            .expect("spawn script runs");

        // `ping` appends a line per second, so a surviving grandchild keeps growing
        // the marker file after the tool call has already returned.
        let marker = root.join("marker.txt");
        let first = marker.metadata().map(|value| value.len()).unwrap_or(0);
        thread::sleep(Duration::from_secs(4));
        let second = marker.metadata().map(|value| value.len()).unwrap_or(0);
        assert_eq!(
            first, second,
            "detached grandchild survived the call and kept writing ({first} -> {second} bytes)"
        );

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    #[cfg(windows)]
    fn background_launch_survives_the_call_that_started_it() {
        let root = workspace();
        // Stands in for a project's own background service: keeps running and
        // keeps writing after the launching tool call has returned.
        fs::write(
            root.join("bg-service.cmd"),
            "@echo off\r\nping 127.0.0.1 -n 120 > marker.txt\r\n",
        )
        .expect("script writes");
        let tools = ToolRegistry::new(&root).expect("tools");
        let start = Instant::now();
        let execution = tools
            .execute_authorized_cancellable(
                &ToolCall {
                    id: "bg-check".to_owned(),
                    name: ToolName::RunCommand,
                    arguments: json!({
                        "executable": "cmd",
                        "args": ["/C", "bg-service.cmd"],
                        "background": true
                    }),
                },
                &|| false,
            )
            .expect("background launch runs");

        // The call must not wait for the service to exit.
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "background launch blocked for {:?}",
            start.elapsed()
        );
        assert_eq!(execution.output["background"], true);
        assert_eq!(execution.output["exited_immediately"], false);
        let pid = execution.output["pid"].as_u64().expect("pid reported");

        // `ping` appends a line per second, so a surviving service keeps growing
        // the marker file after the call returned.
        let marker = root.join("marker.txt");
        thread::sleep(Duration::from_secs(2));
        let first = marker.metadata().map(|value| value.len()).unwrap_or(0);
        thread::sleep(Duration::from_secs(3));
        let second = marker.metadata().map(|value| value.len()).unwrap_or(0);
        assert!(
            second > first,
            "background service died with the call ({first} -> {second} bytes)"
        );

        // Nothing reclaims a background launch, so the test cleans up after itself.
        let _ = workspace_command("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        for attempt in 0..25 {
            if fs::remove_dir_all(&root).is_ok() {
                break;
            }
            assert!(attempt < 24, "workspace removes");
            thread::sleep(Duration::from_millis(100));
        }
    }

    /// Picks a port that is free right now by binding and releasing it.
    #[cfg(windows)]
    fn free_port() -> u16 {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("port binds");
        listener.local_addr().expect("bound address").port()
    }

    #[cfg(windows)]
    #[test]
    fn waits_until_a_background_service_accepts_connections() {
        let root = workspace();
        let port = free_port();
        // Stands in for a service with a slow start: the port only opens after a
        // delay, so a launch reported before that would be reported too early.
        fs::write(
            root.join("listen-service.ps1"),
            format!(
                "Start-Sleep -Seconds 2\r\n$listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, {port})\r\n$listener.Start()\r\nStart-Sleep -Seconds 60\r\n"
            ),
        )
        .expect("script writes");
        let tools = ToolRegistry::new(&root).expect("tools");

        let execution = tools
            .execute_authorized_cancellable(
                &ToolCall {
                    id: "bg-ready".to_owned(),
                    name: ToolName::RunCommand,
                    arguments: json!({
                        "executable": "powershell",
                        "args": ["-NoProfile", "-File", "listen-service.ps1"],
                        "background": true,
                        "ready_port": port,
                        "ready_timeout_seconds": 30
                    }),
                },
                &|| false,
            )
            .expect("background launch runs");

        assert_eq!(execution.output["success"], true);
        assert_eq!(execution.output["ready"], true);
        assert_eq!(execution.output["ready_port"], port);
        assert_eq!(
            execution.output["url"],
            format!("http://127.0.0.1:{port}").as_str()
        );
        // The wait is real: the service only listens after its startup delay.
        assert!(
            execution.output["waited_ms"]
                .as_u64()
                .expect("waited reported")
                >= 1_000,
            "ready reported without waiting: {:?}",
            execution.output["waited_ms"]
        );
        let pid = execution.output["pid"].as_u64().expect("pid reported");

        let _ = workspace_command("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        for attempt in 0..25 {
            if fs::remove_dir_all(&root).is_ok() {
                break;
            }
            assert!(attempt < 24, "workspace removes");
            thread::sleep(Duration::from_millis(100));
        }
    }

    #[cfg(windows)]
    #[test]
    fn reports_a_service_that_dies_before_its_port_opens() {
        let root = workspace();
        let port = free_port();
        // A service that fails its own startup checks never reaches listening, so
        // the launch must fail on the real exit instead of waiting out the timeout.
        fs::write(
            root.join("dying-service.cmd"),
            "@echo off\r\nping 127.0.0.1 -n 2 > nul\r\necho port already in use 1>&2\r\nexit /b 7\r\n",
        )
        .expect("script writes");
        let tools = ToolRegistry::new(&root).expect("tools");

        let started = Instant::now();
        let execution = tools
            .execute_authorized_cancellable(
                &ToolCall {
                    id: "bg-dies".to_owned(),
                    name: ToolName::RunCommand,
                    arguments: json!({
                        "executable": "cmd",
                        "args": ["/C", "dying-service.cmd"],
                        "background": true,
                        "ready_port": port,
                        "ready_timeout_seconds": 30
                    }),
                },
                &|| false,
            )
            .expect("failed launch is reported, not an error");

        assert_eq!(execution.output["success"], false);
        assert_eq!(execution.output["ready"], false);
        assert_eq!(execution.output["exit_code"], 7);
        assert!(
            execution.output["log_tail"]
                .as_str()
                .expect("log tail text")
                .contains("port already in use"),
            "startup failure was not reported: {:?}",
            execution.output["log_tail"]
        );
        // The exit ends the wait early rather than burning the full timeout.
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "waited for the full timeout after the service died: {:?}",
            started.elapsed()
        );

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[cfg(windows)]
    #[test]
    fn reports_a_live_service_that_never_opens_its_port() {
        let root = workspace();
        let port = free_port();
        // Alive but never listening: the launch must come back at the timeout with
        // the pid, so the caller can stop it instead of leaking a half-started service.
        fs::write(
            root.join("silent-service.cmd"),
            "@echo off\r\nping 127.0.0.1 -n 120 > nul\r\n",
        )
        .expect("script writes");
        let tools = ToolRegistry::new(&root).expect("tools");

        let execution = tools
            .execute_authorized_cancellable(
                &ToolCall {
                    id: "bg-silent".to_owned(),
                    name: ToolName::RunCommand,
                    arguments: json!({
                        "executable": "cmd",
                        "args": ["/C", "silent-service.cmd"],
                        "background": true,
                        "ready_port": port,
                        "ready_timeout_seconds": 2
                    }),
                },
                &|| false,
            )
            .expect("timed-out launch is reported, not an error");

        assert_eq!(execution.output["success"], false);
        assert_eq!(execution.output["ready"], false);
        assert_eq!(execution.output["ready_timed_out"], true);
        assert_eq!(execution.output["exited_immediately"], false);
        let pid = execution.output["pid"].as_u64().expect("pid reported");

        let _ = workspace_command("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        for attempt in 0..25 {
            if fs::remove_dir_all(&root).is_ok() {
                break;
            }
            assert!(attempt < 24, "workspace removes");
            thread::sleep(Duration::from_millis(100));
        }
    }
}
