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

    let mut shell = Command::new("powershell");
    shell
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &command,
        ])
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
}
