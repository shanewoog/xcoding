//! Read-only workspace tools used by the Phase 1B agent loop.

use std::{
    collections::VecDeque,
    fs,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use thiserror::Error;
use xcoding_policy::{PermissionDecision, PermissionKind, evaluate};
use xcoding_protocol::{Mode, ToolCall, ToolName};

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
    #[error("read permission was not granted")]
    PermissionDenied,
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
        if evaluate(mode, PermissionKind::Read, false) != PermissionDecision::Allow {
            return Err(ToolError::PermissionDenied);
        }

        match tool_call.name {
            ToolName::ListDir => self.list_dir(parse_arguments(&tool_call.arguments)?),
            ToolName::ReadFile => self.read_file(parse_arguments(&tool_call.arguments)?),
            ToolName::SearchCode => self.search_code(parse_arguments(&tool_call.arguments)?),
        }
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

    fn resolve(&self, requested_path: &str) -> Result<PathBuf, ToolError> {
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
struct SearchCodeArgs {
    query: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    max_results: Option<usize>,
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
}
