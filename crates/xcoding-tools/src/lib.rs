//! Read-only workspace tools used by the Phase 1B agent loop.

use std::{
    collections::VecDeque,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;
use xcoding_policy::{PermissionDecision, PermissionKind, evaluate};
use xcoding_protocol::{Mode, PatchPreview, ToolCall, ToolName};

const DEFAULT_LIST_ENTRIES: usize = 200;
const MAX_LIST_ENTRIES: usize = 1_000;
const DEFAULT_READ_LINES: usize = 200;
const MAX_READ_LINES: usize = 400;
const MAX_READ_BYTES: u64 = 512 * 1024;
const DEFAULT_SEARCH_RESULTS: usize = 50;
const MAX_SEARCH_RESULTS: usize = 100;
const MAX_SEARCH_FILE_BYTES: u64 = 1024 * 1024;

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
    #[error("patch did not match the current file contents: {0}")]
    PatchConflict(String),
    #[error("command arguments are invalid: {0}")]
    InvalidCommand(String),
    #[error("command was cancelled")]
    Cancelled,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolExecution {
    pub output: Value,
    pub summary: String,
}

pub struct ToolRegistry {
    workspace_root: PathBuf,
}

impl ToolRegistry {
    pub fn new(workspace_root: impl AsRef<Path>) -> Result<Self, ToolError> {
        let workspace_root = workspace_root.as_ref();
        if !workspace_root.is_dir() {
            return Err(ToolError::WorkspaceNotFound(
                workspace_root.display().to_string(),
            ));
        }

        Ok(Self {
            workspace_root: workspace_root.canonicalize()?,
        })
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn execute(&self, mode: &Mode, tool_call: &ToolCall) -> Result<ToolExecution, ToolError> {
        let (kind, high_risk) = self.permission_for(tool_call)?;
        if evaluate(mode, kind, high_risk) != PermissionDecision::Allow {
            return Err(ToolError::PermissionDenied);
        }
        self.execute_authorized(tool_call)
    }

    pub fn permission_for(
        &self,
        tool_call: &ToolCall,
    ) -> Result<(PermissionKind, bool), ToolError> {
        match tool_call.name {
            ToolName::ListDir
            | ToolName::ReadFile
            | ToolName::SearchCode
            | ToolName::GitStatus
            | ToolName::GitDiff => Ok((PermissionKind::Read, false)),
            ToolName::ApplyPatch => {
                let args: ApplyPatchArgs = parse_arguments(&tool_call.arguments)?;
                Ok((PermissionKind::Write, is_high_risk_path(&args.path)))
            }
            ToolName::RunCommand => Ok((PermissionKind::Exec, false)),
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
            ToolName::ApplyPatch => self.apply_patch(parse_arguments(&tool_call.arguments)?),
            ToolName::RunCommand => {
                self.run_command(parse_arguments(&tool_call.arguments)?, is_cancelled)
            }
            ToolName::GitStatus => self.git_status(parse_arguments(&tool_call.arguments)?),
            ToolName::GitDiff => self.git_diff(parse_arguments(&tool_call.arguments)?),
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
        if path.metadata()?.len() > MAX_READ_BYTES {
            return Err(ToolError::FileTooLarge(self.relative_path(&path)));
        }

        let content = fs::read_to_string(&path)?;
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
        let path = self.relative_path(&path);

        Ok(ToolExecution {
            output: serde_json::to_value(ReadFileOutput {
                path: path.clone(),
                content,
                start_line,
                end_line,
                truncated: end_line < lines.len(),
            })?,
            summary: format!("Read {path}:{start_line}-{end_line}"),
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
        let mut pending = VecDeque::from([root]);
        let mut results = Vec::new();

        while let Some(directory) = pending.pop_front() {
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

                let Ok(content) = fs::read_to_string(entry.path()) else {
                    continue;
                };
                for (index, line) in content.lines().enumerate() {
                    if line.contains(&args.query) {
                        results.push(SearchResult {
                            path: self.relative_path(&entry.path()),
                            line: index + 1,
                            text: line.to_owned(),
                        });
                        if results.len() >= limit {
                            return Ok(ToolExecution {
                                output: json!({ "results": results, "truncated": true }),
                                summary: format!("Searched for {:?}", args.query),
                            });
                        }
                    }
                }
            }
        }

        Ok(ToolExecution {
            output: json!({ "results": results, "truncated": false }),
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
        if args.executable.trim().is_empty() {
            return Err(ToolError::InvalidCommand(
                "executable must not be empty".to_owned(),
            ));
        }
        if Path::new(&args.executable).is_absolute() {
            return Err(ToolError::InvalidCommand(
                "absolute executable paths are not allowed".to_owned(),
            ));
        }

        let mut child = Command::new(&args.executable)
            .args(&args.args)
            .current_dir(&self.workspace_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let mut stdout_pipe = child.stdout.take().expect("stdout pipe");
        let mut stderr_pipe = child.stderr.take().expect("stderr pipe");
        let stdout_handle = thread::spawn(move || {
            let mut buffer = Vec::new();
            let _ = stdout_pipe.read_to_end(&mut buffer);
            buffer
        });
        let stderr_handle = thread::spawn(move || {
            let mut buffer = Vec::new();
            let _ = stderr_pipe.read_to_end(&mut buffer);
            buffer
        });

        let status = loop {
            if is_cancelled() {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_handle.join();
                let _ = stderr_handle.join();
                return Err(ToolError::Cancelled);
            }
            match child.try_wait()? {
                Some(status) => break status,
                None => thread::sleep(Duration::from_millis(50)),
            }
        };

        let stdout = truncate_output(&String::from_utf8_lossy(
            &stdout_handle.join().unwrap_or_default(),
        ));
        let stderr = truncate_output(&String::from_utf8_lossy(
            &stderr_handle.join().unwrap_or_default(),
        ));
        let success = status.success();
        Ok(ToolExecution {
            output: json!({
                "executable": args.executable,
                "args": args.args,
                "success": success,
                "exit_code": status.code(),
                "stdout": stdout,
                "stderr": stderr,
            }),
            summary: if success {
                "Command completed".to_owned()
            } else {
                "Command failed".to_owned()
            },
        })
    }

    fn git_status(&self, args: GitStatusArgs) -> Result<ToolExecution, ToolError> {
        let pathspec = args.path.as_deref().filter(|value| !value.trim().is_empty());
        if let Some(path) = pathspec {
            let _ = checked_relative_path(path)?;
        }

        let mut command = Command::new("git");
        command
            .arg("status")
            .arg("--porcelain=v1")
            .arg("--branch")
            .arg("--untracked-files=all")
            .current_dir(&self.workspace_root)
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
                format!("git status failed with exit code {:?}", output.status.code())
            } else {
                truncate_output(&stderr)
            }));
        }

        let entries = parse_git_status_lines(&stdout);
        let branch = entries
            .iter()
            .find_map(|entry| entry.get("branch").and_then(Value::as_str))
            .map(str::to_owned);
        Ok(ToolExecution {
            output: json!({
                "path": pathspec.unwrap_or("."),
                "branch": branch,
                "entries": entries,
                "raw": truncate_output(&stdout),
            }),
            summary: format!(
                "Git status for {}",
                pathspec.unwrap_or(".")
            ),
        })
    }

    fn git_diff(&self, args: GitDiffArgs) -> Result<ToolExecution, ToolError> {
        let pathspec = args.path.as_deref().filter(|value| !value.trim().is_empty());
        if let Some(path) = pathspec {
            let _ = checked_relative_path(path)?;
        }

        let mut staged = Command::new("git");
        staged
            .arg("diff")
            .arg("--cached")
            .arg("--no-ext-diff")
            .arg("--no-color")
            .current_dir(&self.workspace_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(path) = pathspec {
            staged.arg("--").arg(path);
        }

        let mut unstaged = Command::new("git");
        unstaged
            .arg("diff")
            .arg("--no-ext-diff")
            .arg("--no-color")
            .current_dir(&self.workspace_root)
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

    fn write_atomically(&self, path: &Path, text: &str) -> Result<(), ToolError> {
        let parent = path.parent().expect("workspace file has a parent");
        let temporary = parent.join(format!(
            ".xcoding-{}-{}.tmp",
            path.file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("patch"),
            Uuid::new_v4()
        ));
        fs::write(&temporary, text)?;
        #[cfg(windows)]
        {
            if path.exists() {
                fs::remove_file(path)?;
            }
        }
        if let Err(error) = fs::rename(&temporary, path) {
            let _ = fs::remove_file(&temporary);
            return Err(ToolError::Io(error));
        }
        Ok(())
    }

    fn resolve_writable(&self, requested_path: &str) -> Result<PathBuf, ToolError> {
        let requested = checked_relative_path(requested_path)?;
        let target = self.workspace_root.join(requested);
        let parent = target
            .parent()
            .ok_or_else(|| ToolError::PathOutsideWorkspace(requested_path.to_owned()))?;
        let canonical_parent = parent.canonicalize()?;
        if !canonical_parent.starts_with(&self.workspace_root) {
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

    fn relative_path(&self, path: &Path) -> String {
        let relative = path.strip_prefix(&self.workspace_root).unwrap_or(path);
        let rendered = relative.to_string_lossy().replace('\\', "/");
        if rendered.is_empty() {
            ".".to_owned()
        } else {
            rendered
        }
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
}

#[derive(Deserialize)]
struct SearchCodeArgs {
    query: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    max_results: Option<usize>,
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
}

#[derive(Serialize)]
struct SearchResult {
    path: String,
    line: usize,
    text: String,
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

fn is_high_risk_path(path: &str) -> bool {
    path.split(['/', '\\'])
        .any(|part| part == ".git" || part == ".xcoding")
}

fn truncate_output(value: &str) -> String {
    const MAX_OUTPUT_BYTES: usize = 32 * 1024;
    if value.len() <= MAX_OUTPUT_BYTES {
        value.to_owned()
    } else {
        format!("{}\n[output truncated]", &value[..MAX_OUTPUT_BYTES])
    }
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
        ".git" | ".xcoding" | "node_modules" | "target"
    )
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
    #[test]
    fn reports_git_status_and_diff_for_workspace_changes() {
        let root = workspace();
        let tools = ToolRegistry::new(&root).expect("registry starts");
        let init = Command::new("git")
            .args(["init"])
            .current_dir(&root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git init runs");
        assert!(init.success());
        let _ = Command::new("git")
            .args(["config", "user.email", "xcoding@example.com"])
            .current_dir(&root)
            .status();
        let _ = Command::new("git")
            .args(["config", "user.name", "XCoding"])
            .current_dir(&root)
            .status();
        fs::write(root.join("hello.txt"), "hello\n").expect("file writes");
        let add = Command::new("git")
            .args(["add", "hello.txt"])
            .current_dir(&root)
            .status()
            .expect("git add runs");
        assert!(add.success());
        let commit = Command::new("git")
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
        assert_eq!(
            fs::read_to_string(&existing).expect("external edit remains"),
            "edited elsewhere\n"
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
}
