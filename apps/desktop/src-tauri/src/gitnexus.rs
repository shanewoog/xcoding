use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const MAX_INPUT_LENGTH: usize = 500;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(45);
const ANALYZE_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Deserialize)]
struct GitNexusRegistryEntry {
    name: String,
    path: String,
}

#[derive(Debug, Serialize)]
pub struct GitNexusStatus {
    pub available: bool,
    pub indexed: bool,
    pub up_to_date: bool,
    pub detail: String,
    pub root: String,
}

#[derive(Debug, Serialize)]
pub struct GitNexusCommandResult {
    pub command: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

fn resolve_workspace_root(workspace_root: &str) -> Result<PathBuf, String> {
    let trimmed = workspace_root.trim();
    if trimmed.is_empty() {
        return Err("workspace root is empty".to_owned());
    }
    let root = PathBuf::from(trimmed);
    if !root.is_absolute() {
        return Err("workspace root must be an absolute path".to_owned());
    }
    if !root.is_dir() {
        return Err(format!("workspace root does not exist: {}", root.display()));
    }
    Ok(root)
}

fn validate_input(label: &str, value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{label} is empty"));
    }
    if value.len() > MAX_INPUT_LENGTH {
        return Err(format!("{label} is too long"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{label} contains control characters"));
    }
    Ok(value.to_owned())
}

fn gitnexus_registry_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .ok_or_else(|| "could not locate the local GitNexus registry".to_owned())?;
    Ok(PathBuf::from(home).join(".gitnexus").join("registry.json"))
}

fn normalized_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase()
}

fn repo_name_from_entries(root: &Path, entries: &[GitNexusRegistryEntry]) -> Option<String> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let normalized_root = normalized_path(&root);
    entries.iter().find_map(|entry| {
        let entry_path = PathBuf::from(&entry.path)
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(&entry.path));
        (normalized_path(&entry_path) == normalized_root).then(|| entry.name.clone())
    })
}

fn repo_name_from_registry(root: &Path, registry_path: &Path) -> Result<String, String> {
    let content = std::fs::read_to_string(registry_path)
        .map_err(|_| "GitNexus has no index for this project yet.".to_owned())?;
    let entries: Vec<GitNexusRegistryEntry> = serde_json::from_str(&content)
        .map_err(|error| format!("could not read the local GitNexus registry: {error}"))?;
    repo_name_from_entries(root, &entries)
        .ok_or_else(|| "GitNexus has no index for this project yet.".to_owned())
}

fn repo_args(root: &Path) -> Result<Vec<String>, String> {
    let registry_path = gitnexus_registry_path()?;
    let repo_name = repo_name_from_registry(root, &registry_path)?;
    Ok(vec!["--repo".to_owned(), repo_name])
}

fn gitnexus_command() -> Command {
    #[cfg(windows)]
    let mut command = Command::new("gitnexus.cmd");
    #[cfg(not(windows))]
    let mut command = Command::new("gitnexus");

    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

fn run_gitnexus(root: &Path, args: &[String]) -> Result<Output, String> {
    gitnexus_command()
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|error| format!("failed to run local GitNexus: {error}"))
}

fn result_from_output(args: &[String], output: Output) -> Result<GitNexusCommandResult, String> {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let result = GitNexusCommandResult {
        command: format!("gitnexus {}", args.join(" ")),
        exit_code: output.status.code(),
        stdout,
        stderr,
    };
    if output.status.success() {
        Ok(result)
    } else if result.stderr.is_empty() {
        Err(format!("{} failed", result.command))
    } else {
        Err(result.stderr.clone())
    }
}

async fn run_async(
    workspace_root: String,
    mut args: Vec<String>,
    timeout: Duration,
    require_index: bool,
) -> Result<GitNexusCommandResult, String> {
    let root = resolve_workspace_root(&workspace_root)?;
    if require_index {
        args.extend(repo_args(&root)?);
    }
    let command_args = args.clone();
    let worker = tauri::async_runtime::spawn_blocking(move || run_gitnexus(&root, &command_args));
    let output = match tokio::time::timeout(timeout, worker).await {
        Ok(result) => result.map_err(|error| format!("GitNexus worker failed: {error}"))??,
        Err(_) => return Err("GitNexus command timed out".to_owned()),
    };
    result_from_output(&args, output)
}

fn status_from_output(root: &Path, output: Output) -> GitNexusStatus {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let indexed = output.status.success() && stdout.contains("Repository:");
    let up_to_date = indexed && stdout.contains("Status: ✅ up-to-date");
    let detail = if !output.status.success() {
        if stderr.is_empty() {
            "GitNexus has no index for this project yet.".to_owned()
        } else {
            stderr
        }
    } else if up_to_date {
        "GitNexus index is ready.".to_owned()
    } else if indexed {
        "GitNexus index needs rebuilding.".to_owned()
    } else {
        "GitNexus has no index for this project yet.".to_owned()
    };
    GitNexusStatus {
        available: true,
        indexed,
        up_to_date,
        detail,
        root: root.display().to_string(),
    }
}

#[tauri::command]
pub async fn gitnexus_status(workspace_root: String) -> Result<GitNexusStatus, String> {
    let root = resolve_workspace_root(&workspace_root)?;
    let worker_root = root.clone();
    let worker = tauri::async_runtime::spawn_blocking(move || {
        run_gitnexus(&worker_root, &["status".to_owned()])
    });
    match tokio::time::timeout(COMMAND_TIMEOUT, worker).await {
        Ok(Ok(Ok(output))) => Ok(status_from_output(&root, output)),
        Ok(Ok(Err(error))) if error.contains("failed to run local GitNexus") => {
            Ok(GitNexusStatus {
                available: false,
                indexed: false,
                up_to_date: false,
                detail: "GitNexus was not found on this computer. Install it and try again."
                    .to_owned(),
                root: root.display().to_string(),
            })
        }
        Ok(Ok(Err(error))) => Err(error),
        Ok(Err(error)) => Err(format!("GitNexus worker failed: {error}")),
        Err(_) => Err("GitNexus status lookup timed out".to_owned()),
    }
}

#[tauri::command]
pub async fn gitnexus_analyze(workspace_root: String) -> Result<GitNexusCommandResult, String> {
    run_async(
        workspace_root,
        vec!["analyze".to_owned()],
        ANALYZE_TIMEOUT,
        false,
    )
    .await
}

#[tauri::command]
pub async fn gitnexus_query(
    workspace_root: String,
    search_query: String,
) -> Result<GitNexusCommandResult, String> {
    let query = validate_input("search query", &search_query)?;
    run_async(
        workspace_root,
        vec![
            "query".to_owned(),
            query,
            "--limit".to_owned(),
            "20".to_owned(),
        ],
        COMMAND_TIMEOUT,
        true,
    )
    .await
}

fn symbol_args(
    command: &str,
    symbol: String,
    symbol_uid: Option<String>,
    file_path: Option<String>,
) -> Result<Vec<String>, String> {
    let mut args = vec![command.to_owned()];
    if let Some(uid) = symbol_uid.filter(|value| !value.trim().is_empty()) {
        args.push("--uid".to_owned());
        args.push(validate_input("symbol uid", &uid)?);
        return Ok(args);
    }
    args.push(validate_input("symbol", &symbol)?);
    if let Some(path) = file_path.filter(|value| !value.trim().is_empty()) {
        args.push("--file".to_owned());
        args.push(validate_input("file path", &path)?);
    }
    Ok(args)
}

#[tauri::command]
pub async fn gitnexus_context(
    workspace_root: String,
    symbol: String,
    symbol_uid: Option<String>,
    file_path: Option<String>,
) -> Result<GitNexusCommandResult, String> {
    let args = symbol_args("context", symbol, symbol_uid, file_path)?;
    run_async(workspace_root, args, COMMAND_TIMEOUT, true).await
}

#[tauri::command]
pub async fn gitnexus_impact(
    workspace_root: String,
    symbol: String,
    symbol_uid: Option<String>,
    file_path: Option<String>,
) -> Result<GitNexusCommandResult, String> {
    let mut args = symbol_args("impact", symbol, symbol_uid, file_path)?;
    args.extend([
        "--direction".to_owned(),
        "upstream".to_owned(),
        "--depth".to_owned(),
        "3".to_owned(),
    ]);
    run_async(workspace_root, args, COMMAND_TIMEOUT, true).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_search_input() {
        assert_eq!(
            validate_input("query", "  App panel ").unwrap(),
            "App panel"
        );
        assert!(validate_input("query", "\n").is_err());
        assert!(validate_input("query", "bad\u{0000}query").is_err());
    }

    #[test]
    fn uid_takes_priority_when_building_symbol_command() {
        let args = symbol_args(
            "context",
            "ignored".to_owned(),
            Some("Function:src/app.ts:run".to_owned()),
            Some("src/app.ts".to_owned()),
        )
        .unwrap();
        assert_eq!(args, vec!["context", "--uid", "Function:src/app.ts:run"]);
    }

    #[test]
    fn resolves_repo_name_from_matching_registry_path() {
        let root = PathBuf::from(r"D:\Work\Example");
        let entries = r#"[{"name":"example","path":"d:\\work\\example"}]"#;
        let parsed: Vec<GitNexusRegistryEntry> = serde_json::from_str(entries).unwrap();
        assert_eq!(
            repo_name_from_entries(&root, &parsed).as_deref(),
            Some("example")
        );
    }
}
