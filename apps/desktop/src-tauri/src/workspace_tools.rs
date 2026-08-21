use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[derive(Debug, Serialize)]
pub struct GitEnvironment {
    pub is_repo: bool,
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub insertions: u32,
    pub deletions: u32,
    pub changed_files: u32,
    pub status_lines: Vec<String>,
    pub local_branches: Vec<String>,
    pub root: String,
}

#[derive(Debug, Serialize)]
pub struct DirEntryInfo {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

#[derive(Debug, Serialize)]
pub struct TerminalCommandResult {
    pub command: String,
    pub cwd: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceFileContent {
    pub path: String,
    pub text: String,
    pub byte_size: u64,
    pub binary: bool,
    pub too_large: bool,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceChangedFile {
    pub path: String,
    /// Raw porcelain status code, for example `M`, `MM`, `A`, `D`, `R`, `??`.
    pub status: String,
    pub insertions: u32,
    pub deletions: u32,
    pub untracked: bool,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceChanges {
    pub is_repo: bool,
    pub branch: Option<String>,
    pub insertions: u32,
    pub deletions: u32,
    pub files: Vec<WorkspaceChangedFile>,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceFileDiff {
    pub path: String,
    pub diff: String,
    pub untracked: bool,
    pub binary: bool,
    pub truncated: bool,
}

fn normalize_root(workspace_root: &str) -> Result<PathBuf, String> {
    let root = PathBuf::from(workspace_root.trim());
    if workspace_root.trim().is_empty() {
        return Err("workspace root is empty".to_owned());
    }
    if !root.is_absolute() {
        return Err("workspace root must be an absolute path".to_owned());
    }
    if !root.exists() {
        return Err(format!("workspace root does not exist: {}", root.display()));
    }
    Ok(root)
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn git_command() -> Command {
    let mut command = Command::new("git");
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Never let Git Credential Manager or Git itself show an interactive prompt.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "Never");
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

fn run_git(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = git_command()
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|error| format!("failed to run git: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !output.status.success() {
        if stderr.is_empty() {
            return Err(format!("git {:?} failed", args));
        }
        return Err(stderr);
    }
    Ok(stdout)
}

fn empty_git_environment(root: &Path, is_repo: bool) -> GitEnvironment {
    GitEnvironment {
        is_repo,
        branch: None,
        upstream: None,
        insertions: 0,
        deletions: 0,
        changed_files: 0,
        status_lines: Vec::new(),
        local_branches: Vec::new(),
        root: root.display().to_string(),
    }
}

/// Parse `git status --short --branch` header (`## branch...upstream`).
fn parse_status_branch_header(header: &str) -> (Option<String>, Option<String>) {
    let raw = header.trim().trim_start_matches("##").trim();
    if raw.is_empty() {
        return (None, None);
    }
    // Strip trailing tracking deco like [ahead 1, behind 2]
    let without_deco = raw
        .split_once(" [")
        .map(|(left, _)| left)
        .unwrap_or(raw)
        .trim();
    if without_deco.eq_ignore_ascii_case("HEAD (no branch)") {
        return (Some("HEAD".to_owned()), None);
    }
    if let Some((branch, upstream)) = without_deco.split_once("...") {
        let branch = branch.trim();
        let upstream = upstream.trim();
        return (
            if branch.is_empty() {
                None
            } else {
                Some(branch.to_owned())
            },
            if upstream.is_empty() {
                None
            } else {
                Some(upstream.to_owned())
            },
        );
    }
    let branch = without_deco.trim();
    if branch.is_empty() {
        (None, None)
    } else {
        (Some(branch.to_owned()), None)
    }
}

fn git_environment_sync(
    workspace_root: String,
    include_branches: bool,
) -> Result<GitEnvironment, String> {
    let root = normalize_root(&workspace_root)?;

    // One process for branch + porcelain status (Windows git spawn is expensive).
    let status = match run_git(&root, &["status", "--short", "--branch"]) {
        Ok(value) => value,
        Err(_) => {
            return Ok(empty_git_environment(&root, false));
        }
    };

    let mut branch = None;
    let mut upstream = None;
    let mut status_lines = Vec::new();
    for line in status.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("##") && branch.is_none() && status_lines.is_empty() {
            let (next_branch, next_upstream) = parse_status_branch_header(trimmed);
            branch = next_branch;
            upstream = next_upstream;
            continue;
        }
        status_lines.push(trimmed.to_owned());
    }
    let changed_files = status_lines.len() as u32;

    let mut insertions = 0u32;
    let mut deletions = 0u32;
    if let Ok(numstat) = run_git(&root, &["diff", "--numstat", "HEAD"]) {
        for line in numstat.lines() {
            let mut parts = line.split_whitespace();
            let add = parts.next().unwrap_or("0");
            let del = parts.next().unwrap_or("0");
            if add != "-" {
                insertions = insertions.saturating_add(add.parse().unwrap_or(0));
            }
            if del != "-" {
                deletions = deletions.saturating_add(del.parse().unwrap_or(0));
            }
        }
    }

    let local_branches = if include_branches {
        run_git(&root, &["branch", "--format=%(refname:short)"])
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    Ok(GitEnvironment {
        is_repo: true,
        branch,
        upstream,
        insertions,
        deletions,
        changed_files,
        status_lines,
        local_branches,
        root: root.display().to_string(),
    })
}

#[tauri::command]
pub async fn git_environment(
    workspace_root: String,
    include_branches: Option<bool>,
) -> Result<GitEnvironment, String> {
    let include_branches = include_branches.unwrap_or(false);
    let worker = tauri::async_runtime::spawn_blocking(move || {
        git_environment_sync(workspace_root, include_branches)
    });
    match tokio::time::timeout(Duration::from_secs(4), worker).await {
        Ok(result) => result.map_err(|error| format!("git worker failed: {error}"))?,
        Err(_) => Err("git environment lookup timed out".to_owned()),
    }
}

#[tauri::command]
pub fn list_workspace_entries(
    workspace_root: String,
    relative_path: Option<String>,
) -> Result<Vec<DirEntryInfo>, String> {
    let root = normalize_root(&workspace_root)?;
    let canonical_root = root.canonicalize().map_err(|error| error.to_string())?;
    let rel = relative_path.unwrap_or_default();
    let rel = rel.trim().trim_start_matches(['/', '\\']);
    let dir = if rel.is_empty() {
        canonical_root.clone()
    } else {
        let candidate = canonical_root.join(rel);
        let canonical = candidate
            .canonicalize()
            .map_err(|error| format!("cannot open path: {error}"))?;
        if !canonical.starts_with(&canonical_root) {
            return Err("path escapes workspace root".to_owned());
        }
        if !canonical.is_dir() {
            return Err("path is not a directory".to_owned());
        }
        canonical
    };

    let mut entries = Vec::new();
    let read = fs::read_dir(&dir).map_err(|error| error.to_string())?;
    for item in read {
        let item = item.map_err(|error| error.to_string())?;
        let name = item.file_name().to_string_lossy().to_string();
        if name == ".git" {
            continue;
        }
        let path = item.path();
        let is_dir = path.is_dir();
        let relative = path
            .strip_prefix(&canonical_root)
            .map_err(|_| "path escapes workspace root".to_owned())?
            .to_string_lossy()
            .replace('/', "\\");
        entries.push(DirEntryInfo {
            name,
            path: relative,
            is_dir,
        });
    }
    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    Ok(entries)
}

/// Files larger than this are not sent to the webview: the viewer is meant for
/// source and config files, and anything bigger belongs to an external editor.
const MAX_VIEWABLE_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// A NUL byte in the leading bytes is the same cheap heuristic Git uses to tell
/// binary content from text, and it keeps the viewer from printing garbage.
fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|byte| *byte == 0)
}

#[tauri::command]
pub fn read_workspace_file(
    workspace_root: String,
    relative_path: String,
) -> Result<WorkspaceFileContent, String> {
    let root = normalize_root(&workspace_root)?;
    let canonical_root = root.canonicalize().map_err(|error| error.to_string())?;
    let canonical = resolve_inside_root(&canonical_root, &relative_path)?;
    let metadata = canonical.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("path is not a file".to_owned());
    }
    let relative = canonical
        .strip_prefix(&canonical_root)
        .map_err(|_| "path escapes workspace root".to_owned())?
        .to_string_lossy()
        .replace('/', "\\");
    let byte_size = metadata.len();
    if byte_size > MAX_VIEWABLE_FILE_BYTES {
        return Ok(WorkspaceFileContent {
            path: relative,
            text: String::new(),
            byte_size,
            binary: false,
            too_large: true,
        });
    }
    let bytes = fs::read(&canonical).map_err(|error| error.to_string())?;
    if looks_binary(&bytes) {
        return Ok(WorkspaceFileContent {
            path: relative,
            text: String::new(),
            byte_size,
            binary: true,
            too_large: false,
        });
    }
    Ok(WorkspaceFileContent {
        path: relative,
        text: String::from_utf8_lossy(&bytes).replace("\r\n", "\n"),
        byte_size,
        binary: false,
        too_large: false,
    })
}

/// A diff longer than this is cut off: the review tab is for reading a change,
/// not for streaming a generated file into the webview.
const MAX_DIFF_LINES: usize = 2000;

/// Resolve a workspace-relative path and refuse anything outside the root.
fn resolve_inside_root(canonical_root: &Path, relative_path: &str) -> Result<PathBuf, String> {
    let rel = relative_path.trim().trim_start_matches(['/', '\\']);
    if rel.is_empty() {
        return Err("file path is empty".to_owned());
    }
    let canonical = canonical_root
        .join(rel)
        .canonicalize()
        .map_err(|error| format!("cannot open path: {error}"))?;
    if !canonical.starts_with(canonical_root) {
        return Err("path escapes workspace root".to_owned());
    }
    Ok(canonical)
}

/// Split one `git status --short` line into its status code and path. Renames
/// are reported as `old -> new`; only the new path can be diffed.
fn parse_status_line(line: &str) -> Option<(String, String)> {
    if line.len() < 4 {
        return None;
    }
    let (status, rest) = line.split_at(2);
    let status = status.trim();
    if status.is_empty() {
        return None;
    }
    let path = rest.trim();
    let path = match path.split_once(" -> ") {
        Some((_, new_path)) => new_path,
        None => path,
    };
    let path = path.trim().trim_matches('"');
    if path.is_empty() {
        return None;
    }
    Some((status.to_owned(), path.replace('/', "\\")))
}

fn workspace_changes_sync(workspace_root: String) -> Result<WorkspaceChanges, String> {
    let root = normalize_root(&workspace_root)?;
    let status = match run_git(
        &root,
        &[
            "-c",
            "core.quotepath=false",
            "status",
            "--short",
            "--branch",
        ],
    ) {
        Ok(value) => value,
        Err(_) => {
            return Ok(WorkspaceChanges {
                is_repo: false,
                branch: None,
                insertions: 0,
                deletions: 0,
                files: Vec::new(),
            });
        }
    };

    let mut branch = None;
    let mut files = Vec::new();
    for line in status.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("##") && branch.is_none() && files.is_empty() {
            branch = parse_status_branch_header(trimmed).0;
            continue;
        }
        if let Some((status_code, path)) = parse_status_line(trimmed) {
            let untracked = status_code == "??";
            files.push(WorkspaceChangedFile {
                path,
                status: status_code,
                insertions: 0,
                deletions: 0,
                untracked,
            });
        }
    }

    // Line counts come from one numstat pass; untracked files are absent there
    // and keep their zeroes.
    if let Ok(numstat) = run_git(
        &root,
        &["-c", "core.quotepath=false", "diff", "--numstat", "HEAD"],
    ) {
        for line in numstat.lines() {
            let mut parts = line.split('\t');
            let add = parts.next().unwrap_or("-");
            let del = parts.next().unwrap_or("-");
            let path = parts.next().unwrap_or("").trim().replace('/', "\\");
            if path.is_empty() {
                continue;
            }
            if let Some(entry) = files.iter_mut().find(|entry| entry.path == path) {
                entry.insertions = add.parse().unwrap_or(0);
                entry.deletions = del.parse().unwrap_or(0);
            }
        }
    }

    let insertions = files
        .iter()
        .fold(0u32, |total, file| total.saturating_add(file.insertions));
    let deletions = files
        .iter()
        .fold(0u32, |total, file| total.saturating_add(file.deletions));

    Ok(WorkspaceChanges {
        is_repo: true,
        branch,
        insertions,
        deletions,
        files,
    })
}

#[tauri::command]
pub async fn workspace_changes(workspace_root: String) -> Result<WorkspaceChanges, String> {
    let worker =
        tauri::async_runtime::spawn_blocking(move || workspace_changes_sync(workspace_root));
    match tokio::time::timeout(Duration::from_secs(6), worker).await {
        Ok(result) => result.map_err(|error| format!("git worker failed: {error}"))?,
        Err(_) => Err("workspace changes lookup timed out".to_owned()),
    }
}

fn truncate_diff(diff: String) -> (String, bool) {
    let mut kept = Vec::new();
    let mut truncated = false;
    for (index, line) in diff.lines().enumerate() {
        if index >= MAX_DIFF_LINES {
            truncated = true;
            break;
        }
        kept.push(line);
    }
    (kept.join("\n"), truncated)
}

/// Build an all-added diff for an untracked file: `git diff` has nothing to
/// compare against, but the reviewer still needs to see the new content.
fn untracked_diff(path: &Path, relative: &str) -> Result<WorkspaceFileDiff, String> {
    let metadata = path.metadata().map_err(|error| error.to_string())?;
    if metadata.len() > MAX_VIEWABLE_FILE_BYTES {
        return Ok(WorkspaceFileDiff {
            path: relative.to_owned(),
            diff: String::new(),
            untracked: true,
            binary: false,
            truncated: true,
        });
    }
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    if looks_binary(&bytes) {
        return Ok(WorkspaceFileDiff {
            path: relative.to_owned(),
            diff: String::new(),
            untracked: true,
            binary: true,
            truncated: false,
        });
    }
    let text = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
    let mut lines = vec![format!("+++ {relative}")];
    for line in text.lines() {
        lines.push(format!("+{line}"));
    }
    let (diff, truncated) = truncate_diff(lines.join("\n"));
    Ok(WorkspaceFileDiff {
        path: relative.to_owned(),
        diff,
        untracked: true,
        binary: false,
        truncated,
    })
}

fn workspace_file_diff_sync(
    workspace_root: String,
    relative_path: String,
    untracked: bool,
) -> Result<WorkspaceFileDiff, String> {
    let root = normalize_root(&workspace_root)?;
    let canonical_root = root.canonicalize().map_err(|error| error.to_string())?;
    let canonical = resolve_inside_root(&canonical_root, &relative_path)?;
    let relative = canonical
        .strip_prefix(&canonical_root)
        .map_err(|_| "path escapes workspace root".to_owned())?
        .to_string_lossy()
        .replace('/', "\\");
    if untracked {
        return untracked_diff(&canonical, &relative);
    }
    let git_path = relative.replace('\\', "/");
    let diff = run_git(
        &root,
        &[
            "-c",
            "core.quotepath=false",
            "diff",
            "--unified=3",
            "HEAD",
            "--",
            &git_path,
        ],
    )?;
    let binary = diff.contains("Binary files ");
    let (diff, truncated) = truncate_diff(diff);
    Ok(WorkspaceFileDiff {
        path: relative,
        diff,
        untracked: false,
        binary,
        truncated,
    })
}

#[tauri::command]
pub async fn workspace_file_diff(
    workspace_root: String,
    relative_path: String,
    untracked: Option<bool>,
) -> Result<WorkspaceFileDiff, String> {
    let untracked = untracked.unwrap_or(false);
    let worker = tauri::async_runtime::spawn_blocking(move || {
        workspace_file_diff_sync(workspace_root, relative_path, untracked)
    });
    match tokio::time::timeout(Duration::from_secs(8), worker).await {
        Ok(result) => result.map_err(|error| format!("git worker failed: {error}"))?,
        Err(_) => Err("workspace diff lookup timed out".to_owned()),
    }
}

/// Lets a user point the built-in terminal at a specific shell when automatic
/// discovery cannot find one, for example a portable Git unpacked anywhere.
const TERMINAL_SHELL_ENV: &str = "XCODING_TERMINAL_SHELL";

/// `bash.exe` locations inside one Git for Windows install root. The copy under
/// `bin` is a small launcher that prepares the msys environment, so it is tried
/// before the real binary under `usr\bin`.
fn shell_candidates_in_root(root: &Path) -> [PathBuf; 2] {
    [
        root.join("bin").join("bash.exe"),
        root.join("usr").join("bin").join("bash.exe"),
    ]
}

/// Git install roots implied by PATH. Git for Windows exposes `git.exe` through
/// `<root>\cmd`, `<root>\bin`, or `<root>\mingw64\bin`, and the root itself is
/// not fixed: per-user installs, scoop, winget, and non-system drives all put it
/// somewhere else. Since XCoding already requires Git, the PATH copy of it is the
/// most reliable anchor for finding the bash that ships beside it.
fn git_roots_from_path(path_var: &std::ffi::OsStr) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for entry in std::env::split_paths(path_var) {
        let Some(name) = entry
            .file_name()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase())
        else {
            continue;
        };
        let parent = entry.parent();
        let root = match name.as_str() {
            "cmd" => parent,
            "bin" => {
                let grandparent_name = parent
                    .and_then(|value| value.file_name())
                    .and_then(|value| value.to_str())
                    .map(|value| value.to_ascii_lowercase());
                match grandparent_name.as_deref() {
                    Some("mingw64" | "mingw32" | "usr") => parent.and_then(Path::parent),
                    _ => parent,
                }
            }
            _ => None,
        };
        if let Some(root) = root.filter(|root| {
            !root.as_os_str().is_empty() && !roots.iter().any(|known| known == *root)
        }) {
            roots.push(root.to_path_buf());
        }
    }
    roots
}

/// Default install locations, used when PATH does not reveal a Git install.
fn well_known_git_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut push_env_root = |variable: &str, suffix: &[&str]| {
        if let Some(base) = std::env::var_os(variable) {
            if base.is_empty() {
                return;
            }
            let mut root = PathBuf::from(base);
            for part in suffix {
                root.push(part);
            }
            roots.push(root);
        }
    };
    push_env_root("LOCALAPPDATA", &["Programs", "Git"]);
    push_env_root("ProgramFiles", &["Git"]);
    push_env_root("ProgramFiles(x86)", &["Git"]);
    push_env_root("USERPROFILE", &["scoop", "apps", "git", "current"]);
    roots.push(PathBuf::from("C:\\Program Files\\Git"));
    roots.push(PathBuf::from("C:\\Program Files (x86)\\Git"));
    roots
}

/// Resolves the Windows shell without touching process state, so the lookup
/// order can be tested directly.
fn resolve_windows_shell(
    explicit: Option<PathBuf>,
    path_var: Option<&std::ffi::OsStr>,
) -> Result<Option<PathBuf>, String> {
    if let Some(explicit) = explicit {
        if explicit.is_file() {
            return Ok(Some(explicit));
        }
        return Err(format!(
            "{TERMINAL_SHELL_ENV} points at `{}`, which is not a file",
            explicit.display()
        ));
    }

    let mut roots = path_var.map(git_roots_from_path).unwrap_or_default();
    for root in well_known_git_roots() {
        if !roots.contains(&root) {
            roots.push(root);
        }
    }

    Ok(roots
        .iter()
        .flat_map(|root| shell_candidates_in_root(root))
        .find(|candidate| candidate.is_file()))
}

/// Reads the process environment and applies the lookup order. Shared with the
/// interactive terminal panel so both shells resolve identically.
pub(crate) fn discover_windows_shell() -> Result<Option<PathBuf>, String> {
    let explicit = std::env::var_os(TERMINAL_SHELL_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let path_var = std::env::var_os("PATH");
    resolve_windows_shell(explicit, path_var.as_deref())
}

fn terminal_shell_command(command: &str) -> Result<Command, String> {
    if cfg!(target_os = "windows") {
        if let Some(shell) = discover_windows_shell()? {
            let mut cmd = Command::new(shell);
            cmd.args(["--noprofile", "--norc", "-lc", command]);
            return Ok(cmd);
        }
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", command]);
        return Ok(cmd);
    }

    let mut cmd = Command::new("bash");
    cmd.args(["-lc", command]);
    Ok(cmd)
}

#[tauri::command]
pub fn run_terminal_command(
    workspace_root: String,
    command: String,
) -> Result<TerminalCommandResult, String> {
    let root = normalize_root(&workspace_root)?;
    let command = command.trim().to_owned();
    if command.is_empty() {
        return Err("command is empty".to_owned());
    }

    let mut shell = terminal_shell_command(&command)?;
    shell
        .current_dir(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    shell.creation_flags(CREATE_NO_WINDOW);
    let output = shell
        .output()
        .map_err(|error| format!("failed to run shell: {error}"))?;

    Ok(TerminalCommandResult {
        command,
        cwd: root.display().to_string(),
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

#[tauri::command]
pub fn open_path(path: String) -> Result<(), String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("path is empty".to_owned());
    }
    open_with_os(trimmed)
}

#[tauri::command]
pub fn open_external_url(url: String) -> Result<(), String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("url is empty".to_owned());
    }
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return Err("only http(s) urls are allowed".to_owned());
    }
    open_with_os(trimmed)
}

fn open_with_os(target: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let mut command = Command::new("cmd");
        command
            .args(["/C", "start", "", target])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|error| format!("failed to open: {error}"))?;
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(target)
            .spawn()
            .map_err(|error| format!("failed to open: {error}"))?;
        return Ok(());
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        Command::new("xdg-open")
            .arg(target)
            .spawn()
            .map_err(|error| format!("failed to open: {error}"))?;
        Ok(())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn lists_nested_workspace_entries_as_relative_paths() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "xcoding-workspace-entries-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("src").join("engine").join("nested"))
            .expect("create nested workspace");

        let root_text = root.to_string_lossy().to_string();
        let src_entries = list_workspace_entries(root_text.clone(), Some("src".to_owned()))
            .expect("list src entries");
        let engine = src_entries
            .iter()
            .find(|entry| entry.name == "engine")
            .expect("engine entry");
        assert_eq!(engine.path, "src\\engine");

        let engine_entries =
            list_workspace_entries(root_text, Some(engine.path.clone())).expect("list engine");
        assert_eq!(engine_entries.len(), 1);
        assert_eq!(engine_entries[0].path, "src\\engine\\nested");

        fs::remove_dir_all(root).expect("remove temporary workspace");
    }

    #[test]
    fn reads_workspace_text_file_with_normalized_newlines() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("xcoding-read-file-{}-{unique}", std::process::id()));
        fs::create_dir_all(root.join("src")).expect("create workspace");
        fs::write(
            root.join("src").join("lib.rs"),
            b"fn main() {}\r\nlet x = 1;\r\n",
        )
        .expect("write text file");

        let root_text = root.to_string_lossy().to_string();
        let content = read_workspace_file(root_text.clone(), "src\\lib.rs".to_owned())
            .expect("read text file");
        assert_eq!(content.path, "src\\lib.rs");
        assert_eq!(content.text, "fn main() {}\nlet x = 1;\n");
        assert!(!content.binary);
        assert!(!content.too_large);

        fs::remove_dir_all(root).expect("remove temporary workspace");
    }

    #[test]
    fn reports_binary_files_without_returning_content() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "xcoding-read-binary-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create workspace");
        fs::write(root.join("blob.bin"), [0x4du8, 0x5a, 0x00, 0x01]).expect("write binary file");

        let content =
            read_workspace_file(root.to_string_lossy().to_string(), "blob.bin".to_owned())
                .expect("read binary file");
        assert!(content.binary);
        assert!(content.text.is_empty());

        fs::remove_dir_all(root).expect("remove temporary workspace");
    }

    #[test]
    fn rejects_reads_outside_workspace_root() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "xcoding-read-escape-{}-{unique}",
            std::process::id()
        ));
        let root = base.join("workspace");
        fs::create_dir_all(&root).expect("create workspace");
        fs::write(base.join("secret.txt"), b"secret").expect("write outside file");

        let error = read_workspace_file(
            root.to_string_lossy().to_string(),
            "..\\secret.txt".to_owned(),
        )
        .expect_err("must refuse to leave the workspace root");
        assert!(
            error.contains("escapes workspace root"),
            "unexpected error: {error}"
        );

        fs::remove_dir_all(base).expect("remove temporary workspace");
    }

    #[test]
    fn parses_status_lines_including_renames_and_untracked() {
        assert_eq!(
            parse_status_line(" M src/lib.rs"),
            Some(("M".to_owned(), "src\\lib.rs".to_owned()))
        );
        assert_eq!(
            parse_status_line("?? notes/new.md"),
            Some(("??".to_owned(), "notes\\new.md".to_owned()))
        );
        assert_eq!(
            parse_status_line("R  old/name.rs -> new/name.rs"),
            Some(("R".to_owned(), "new\\name.rs".to_owned()))
        );
        assert_eq!(parse_status_line("  "), None);
    }

    #[test]
    fn builds_an_all_added_diff_for_untracked_files() {
        let root = temporary_root("untracked-diff");
        fs::create_dir_all(&root).expect("create workspace");
        fs::write(root.join("new.txt"), b"first\r\nsecond\r\n").expect("write file");

        let diff = untracked_diff(&root.join("new.txt"), "new.txt").expect("build diff");
        assert!(diff.untracked);
        assert!(!diff.binary);
        assert_eq!(diff.diff, "+++ new.txt\n+first\n+second");

        fs::remove_dir_all(root).expect("remove temporary workspace");
    }

    #[test]
    fn rejects_diffs_outside_workspace_root() {
        let base = temporary_root("diff-escape");
        let root = base.join("workspace");
        fs::create_dir_all(&root).expect("create workspace");
        fs::write(base.join("secret.txt"), b"secret").expect("write outside file");

        let canonical_root = root.canonicalize().expect("canonicalize root");
        let error = resolve_inside_root(&canonical_root, "..\\secret.txt")
            .expect_err("must refuse to leave the workspace root");
        assert!(
            error.contains("escapes workspace root"),
            "unexpected error: {error}"
        );

        fs::remove_dir_all(base).expect("remove temporary workspace");
    }

    fn temporary_root(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("xcoding-{label}-{}-{unique}", std::process::id()))
    }

    #[test]
    fn derives_git_roots_from_every_path_layout() {
        let path = std::env::join_paths([
            Path::new("D:\\Tools\\Git\\cmd"),
            Path::new("D:\\Tools\\Git\\mingw64\\bin"),
            Path::new("E:\\Portable\\Git\\bin"),
            Path::new("C:\\Windows\\System32"),
        ])
        .expect("join paths");

        let roots = git_roots_from_path(&path);

        assert!(roots.contains(&PathBuf::from("D:\\Tools\\Git")));
        assert!(roots.contains(&PathBuf::from("E:\\Portable\\Git")));
        // `D:\Tools\Git` must appear once even though two PATH entries imply it.
        assert_eq!(
            roots
                .iter()
                .filter(|root| *root == &PathBuf::from("D:\\Tools\\Git"))
                .count(),
            1
        );
        // System32 has no recognized parent layout, so it must not become a root.
        assert!(!roots.contains(&PathBuf::from("C:\\Windows")));
    }

    #[test]
    fn prefers_bash_from_a_path_derived_git_root() {
        let root = temporary_root("shell-path-root");
        let bin = root.join("Git").join("bin");
        fs::create_dir_all(&bin).expect("create git bin");
        let bash = bin.join("bash.exe");
        fs::write(&bash, b"stub").expect("write bash stub");
        let path = std::env::join_paths([root.join("Git").join("cmd").as_path()]).expect("join");

        let resolved = resolve_windows_shell(None, Some(&path)).expect("resolve shell");

        assert_eq!(resolved.as_deref(), Some(bash.as_path()));
        fs::remove_dir_all(root).expect("remove temporary root");
    }

    #[test]
    fn falls_back_to_usr_bin_bash_when_bin_launcher_is_absent() {
        let root = temporary_root("shell-usr-bin");
        let usr_bin = root.join("Git").join("usr").join("bin");
        fs::create_dir_all(&usr_bin).expect("create usr bin");
        let bash = usr_bin.join("bash.exe");
        fs::write(&bash, b"stub").expect("write bash stub");
        let path = std::env::join_paths([root.join("Git").join("cmd").as_path()]).expect("join");

        let resolved = resolve_windows_shell(None, Some(&path)).expect("resolve shell");

        assert_eq!(resolved.as_deref(), Some(bash.as_path()));
        fs::remove_dir_all(root).expect("remove temporary root");
    }

    #[test]
    fn explicit_shell_override_wins_over_discovery() {
        let root = temporary_root("shell-override");
        fs::create_dir_all(&root).expect("create root");
        let shell = root.join("custom-bash.exe");
        fs::write(&shell, b"stub").expect("write shell stub");

        let resolved =
            resolve_windows_shell(Some(shell.clone()), None).expect("resolve explicit shell");

        assert_eq!(resolved.as_deref(), Some(shell.as_path()));
        fs::remove_dir_all(root).expect("remove temporary root");
    }

    #[test]
    fn rejects_an_explicit_shell_that_does_not_exist() {
        let missing = temporary_root("shell-missing").join("bash.exe");

        let error = resolve_windows_shell(Some(missing), None).expect_err("must reject");

        assert!(
            error.contains(TERMINAL_SHELL_ENV),
            "unexpected error: {error}"
        );
    }
}
