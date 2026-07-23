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
use xcoding_policy::{
    PermissionDecision, PermissionKind, assess_command_with_extra, evaluate_detailed,
    parse_command_allowlist, COMMAND_ALLOWLIST_RELATIVE_PATH,
};
use xcoding_protocol::{Mode, PatchPreview, ToolCall, ToolName};

const DEFAULT_LIST_ENTRIES: usize = 200;
const MAX_LIST_ENTRIES: usize = 1_000;
const DEFAULT_READ_LINES: usize = 200;
const MAX_READ_LINES: usize = 400;
const MAX_READ_BYTES: u64 = 512 * 1024;
const DEFAULT_SEARCH_RESULTS: usize = 50;
const MAX_SEARCH_RESULTS: usize = 100;
const MAX_SEARCH_FILE_BYTES: u64 = 1024 * 1024;
const MAX_SEARCH_CONTEXT_LINES: usize = 3;
const MAX_SEARCH_CANDIDATES: usize = 500;
const DEFAULT_GIT_LOG_COUNT: usize = 20;
const MAX_GIT_LOG_COUNT: usize = 50;

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
    command_allowlist: Vec<String>,
}

impl ToolRegistry {
    pub fn new(workspace_root: impl AsRef<Path>) -> Result<Self, ToolError> {
        let workspace_root = workspace_root.as_ref();
        if !workspace_root.is_dir() {
            return Err(ToolError::WorkspaceNotFound(
                workspace_root.display().to_string(),
            ));
        }

        let workspace_root = workspace_root.canonicalize()?;
        let command_allowlist = load_command_allowlist(&workspace_root);
        Ok(Self {
            workspace_root,
            command_allowlist,
        })
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn command_allowlist(&self) -> &[String] {
        &self.command_allowlist
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
            | ToolName::GitStatus
            | ToolName::GitDiff
            | ToolName::GitLog
            | ToolName::GitShow => Ok((PermissionKind::Read, false, false)),
            ToolName::GitAdd | ToolName::GitCommit | ToolName::GitPush => {
                // Mutates .git index/refs or pushes to a remote; always high-risk write.
                Ok((PermissionKind::Write, true, false))
            }
            ToolName::ApplyPatch => {
                let args: ApplyPatchArgs = parse_arguments(&tool_call.arguments)?;
                Ok((PermissionKind::Write, is_high_risk_path(&args.path), false))
            }
            ToolName::RunCommand => {
                let args: RunCommandArgs = parse_arguments(&tool_call.arguments)?;
                let assessment = assess_command_with_extra(
                    &args.executable,
                    &args.args,
                    &self.command_allowlist,
                );
                if assessment.decision == PermissionDecision::Deny {
                    return Err(ToolError::InvalidCommand(assessment.reason));
                }
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
        let context_lines = args.context_lines.unwrap_or(0).min(MAX_SEARCH_CONTEXT_LINES);
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
        let truncated = candidates.len() >= candidate_cap || candidates.len() > limit;
        let results: Vec<SearchResult> = candidates
            .into_iter()
            .take(limit)
            .map(|hit| hit.result)
            .collect();

        Ok(ToolExecution {
            output: json!({ "results": results, "truncated": truncated }),
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
        let assessment = assess_command_with_extra(
            &args.executable,
            &args.args,
            &self.command_allowlist,
        );
        if assessment.decision == PermissionDecision::Deny {
            return Err(ToolError::InvalidCommand(assessment.reason));
        }

        // Never inherit the server RPC stdin pipe: some tools (notably git on
        // Windows) can hang when stdin is an open parent pipe still owned by the
        // JSON-RPC loop.
        let mut child = Command::new(&args.executable)
            .args(&args.args)
            .current_dir(&self.workspace_root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_PAGER", "cat")
            .env("PAGER", "cat")
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
            .stdin(Stdio::null())
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
        let pathspec = args.path.as_deref().filter(|value| !value.trim().is_empty());
        if let Some(path) = pathspec {
            let _ = checked_relative_path(path)?;
        }
        let max_count = bounded(args.max_count, DEFAULT_GIT_LOG_COUNT, MAX_GIT_LOG_COUNT);

        let mut command = Command::new("git");
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
        let pathspec = args.path.as_deref().filter(|value| !value.trim().is_empty());
        if let Some(path) = pathspec {
            let _ = checked_relative_path(path)?;
        }

        let mut command = Command::new("git");
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
                pathspec.map(|path| format!(" -- {path}")).unwrap_or_default()
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

        let mut command = Command::new("git");
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

        let mut command = Command::new("git");
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
                format!("git commit failed with exit code {:?}", output.status.code())
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
        let branch = if let Some(branch) = args.branch.as_deref().map(str::trim).filter(|value| !value.is_empty()) {
            validate_git_name("branch", branch)?;
            branch.to_owned()
        } else {
            current_branch_name(&self.workspace_root)?
        };

        let mut command = Command::new("git");
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

    fn write_atomically(&self, path: &Path, text: &str) -> Result<(), ToolError> {
        let parent = path.parent().expect("workspace file has a parent");
        fs::create_dir_all(parent)?;
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
    #[serde(default)]
    case_insensitive: bool,
    #[serde(default)]
    glob: Option<String>,
    #[serde(default)]
    context_lines: Option<usize>,
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

fn is_high_risk_path(path: &str) -> bool {
    path.split(['/', '\\'])
        .any(|part| part == ".git" || part == ".xcoding")
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
    let output = Command::new("git")
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
            "failed to resolve current branch for git push".to_owned()
        } else {
            truncate_output(stderr.trim())
        }));
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if branch.is_empty() || branch == "HEAD" {
        return Err(ToolError::InvalidArguments(
            "detached HEAD: pass branch explicitly for git_push".to_owned(),
        ));
    }
    validate_git_name("branch", &branch)?;
    Ok(branch)
}

fn git_rev_parse_head(workspace_root: &Path) -> Result<String, ToolError> {
    let output = Command::new("git")
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
        ".rs", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".vue", ".go", ".py",
        ".java", ".kt", ".cs", ".cpp", ".c", ".h", ".hpp", ".md", ".toml", ".json",
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
        assert_eq!(case_search.output["results"][0]["text"], "pub fn find_me() {}");
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
        fs::write(root.join("src/auth.ts"), "export const token = 'secret-marker';\n")
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

    #[test]
    fn reports_git_log_and_show_for_workspace_history() {
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
            .args(["commit", "-m", "init commit"])
            .current_dir(&root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git commit runs");
        assert!(commit.success());
        fs::write(root.join("hello.txt"), "hello world\n").expect("file mutates");
        let add2 = Command::new("git")
            .args(["add", "hello.txt"])
            .current_dir(&root)
            .status()
            .expect("git add runs");
        assert!(add2.success());
        let commit2 = Command::new("git")
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

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn stages_and_commits_with_authorized_git_write_tools() {
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
        fs::write(root.join("hello.txt"), "hello
").expect("file writes");
        let bootstrap_add = Command::new("git")
            .args(["add", "hello.txt"])
            .current_dir(&root)
            .status()
            .expect("bootstrap add");
        assert!(bootstrap_add.success());
        let bootstrap_commit = Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(&root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("bootstrap commit");
        assert!(bootstrap_commit.success());

        fs::write(root.join("hello.txt"), "hello staged
").expect("mutate");
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

        let subject = Command::new("git")
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
        let init = Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(&root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git init runs");
        assert!(init.success());
        let bare_init = Command::new("git")
            .args(["init", "--bare", "-b", "main"])
            .arg(&bare)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("bare init runs");
        assert!(bare_init.success());
        let _ = Command::new("git")
            .args(["config", "user.email", "xcoding@example.com"])
            .current_dir(&root)
            .status();
        let _ = Command::new("git")
            .args(["config", "user.name", "XCoding"])
            .current_dir(&root)
            .status();
        fs::write(root.join("hello.txt"), "hello\\n").expect("file writes");
        assert!(Command::new("git")
            .args(["add", "hello.txt"])
            .current_dir(&root)
            .status()
            .expect("add")
            .success());
        assert!(Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(&root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("commit")
            .success());
        assert!(Command::new("git")
            .args(["remote", "add", "origin"])
            .arg(&bare)
            .current_dir(&root)
            .status()
            .expect("remote add")
            .success());

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

        let remote_head = Command::new("git")
            .args(["--git-dir"])
            .arg(&bare)
            .args(["rev-parse", "main"])
            .output()
            .expect("remote rev-parse");
        assert!(remote_head.status.success());
        let remote_hash = String::from_utf8_lossy(&remote_head.stdout).trim().to_owned();
        let local_hash = git_rev_parse_head(&root).expect("local head");
        assert_eq!(remote_hash, local_hash);

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
        match error {
            ToolError::InvalidCommand(message) => assert!(message.contains("blocked")),
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
        fs::write(
            root.join(".xcoding/command-allowlist"),
            "powershell\ncmd\n",
        )
        .expect("rewrite");
        let tools = ToolRegistry::new(&root).expect("reload");
        assert!(tools.command_allowlist().is_empty());
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

}
