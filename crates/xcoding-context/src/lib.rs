//! Project-rule loading and prompt context for the coding-agent loop.

use std::{
    collections::VecDeque,
    fs,
    path::Path,
};

/// Workspace-root rule files, in load order.
const RULE_FILES: [&str; 3] = ["AGENTS.md", "XCoding.md", ".xcoding/rules.md"];
const MAX_RULE_CHARS: usize = 20_000;
const MAX_RELEVANT_PATHS: usize = 40;
const SKETCH_MAX_DEPTH: usize = 2;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRule {
    pub path: String,
    pub content: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ContextSnapshot {
    pub project_rules: Vec<ProjectRule>,
    pub relevant_paths: Vec<String>,
}

impl ContextSnapshot {
    pub fn load(workspace_root: &Path) -> Self {
        let project_rules = RULE_FILES
            .into_iter()
            .filter_map(|name| {
                let path = workspace_root.join(name);
                let content = fs::read_to_string(path).ok()?;
                let content = truncate_rule_content(content.trim(), MAX_RULE_CHARS);
                if content.is_empty() {
                    return None;
                }
                Some(ProjectRule {
                    path: name.to_owned(),
                    content,
                })
            })
            .collect();

        Self {
            project_rules,
            relevant_paths: workspace_path_sketch(workspace_root),
        }
    }

    /// Build the system prompt for the active mode (`ask` or `auto-edit`).
    pub fn system_prompt(&self, mode: &str) -> String {
        let mut prompt = format!(
            "You are XCoding, a local coding agent for a software workspace. \
When repository facts are needed, use tools before answering. Never claim a file was inspected unless a tool result contains it. \
Available tools: list_dir, read_file, search_code, apply_patch, run_command, git_status, git_diff, git_log, git_show, git_add, git_commit, git_push, git_fetch, git_pull. \
Current mode: {mode}. \
In ask mode, propose writes and wait for required approval. In auto-edit mode, ordinary file patches and allowlisted safe commands may apply without approval; high-risk writes and non-allowlisted commands still require user approval. \
Prefer minimal, scoped changes. Do not invent secrets or commit credentials. If apply_patch fails with a patch conflict, re-read the file and retry with updated old_text; do not force-write without matching the current contents."
        );

        if !self.project_rules.is_empty() {
            prompt.push_str("\n\nProject rules (follow these for this workspace):\n");
            for rule in &self.project_rules {
                prompt.push_str(&format!("\n--- {} ---\n{}\n", rule.path, rule.content));
            }
        }

        if !self.relevant_paths.is_empty() {
            prompt.push_str(
                "\n\nWorkspace sketch (shallow paths for orientation; still use tools before quoting file contents):\n",
            );
            for path in &self.relevant_paths {
                prompt.push_str("- ");
                prompt.push_str(path);
                prompt.push('\n');
            }
        }

        prompt
    }
}

fn truncate_rule_content(content: &str, max_chars: usize) -> String {
    if content.chars().count() <= max_chars {
        return content.to_owned();
    }
    let mut truncated = content.chars().take(max_chars).collect::<String>();
    truncated.push_str("\n...[truncated project rule]...");
    truncated
}

fn workspace_path_sketch(workspace_root: &Path) -> Vec<String> {
    let mut paths = Vec::new();
    let mut pending = VecDeque::from([(workspace_root.to_path_buf(), 0usize)]);

    while let Some((directory, depth)) = pending.pop_front() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        let mut children: Vec<_> = entries.filter_map(Result::ok).collect();
        children.sort_by_key(|entry| entry.file_name());

        for entry in children {
            if paths.len() >= MAX_RELEVANT_PATHS {
                return paths;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            let name = entry.file_name();
            if file_type.is_dir() && is_ignored_sketch_directory(&name) {
                continue;
            }

            let absolute = entry.path();
            let Some(relative) = relative_path_string(workspace_root, &absolute) else {
                continue;
            };
            if file_type.is_dir() {
                paths.push(format!("{relative}/"));
                if depth + 1 <= SKETCH_MAX_DEPTH {
                    pending.push_back((absolute, depth + 1));
                }
            } else if file_type.is_file() {
                paths.push(relative);
            }
        }
    }

    paths
}

fn relative_path_string(workspace_root: &Path, absolute: &Path) -> Option<String> {
    let relative = absolute.strip_prefix(workspace_root).ok()?;
    let text = relative.to_string_lossy().replace('\\', "/");
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn is_ignored_sketch_directory(name: &std::ffi::OsStr) -> bool {
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

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    fn temp_workspace(label: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock works")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("xcoding-context-{label}-{unique}"));
        fs::create_dir_all(&root).expect("workspace creates");
        root
    }

    #[test]
    fn loads_root_project_rules_into_the_system_prompt() {
        let root = temp_workspace("agents");
        fs::write(root.join("AGENTS.md"), "Run focused tests.").expect("rule writes");

        let context = ContextSnapshot::load(&root);
        assert_eq!(context.project_rules.len(), 1);
        let prompt = context.system_prompt("ask");
        assert!(prompt.contains("Run focused tests."));
        assert!(prompt.contains("apply_patch"));
        assert!(prompt.contains("patch conflict"));
        assert!(prompt.contains("Current mode: ask"));
        assert!(prompt.contains("AGENTS.md"));
        assert!(prompt.contains("Workspace sketch"));

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn loads_dot_xcoding_rules_file() {
        let root = temp_workspace("dot-rules");
        fs::create_dir_all(root.join(".xcoding")).expect("dir creates");
        fs::write(root.join(".xcoding/rules.md"), "Prefer ASCII comments.")
            .expect("rule writes");

        let context = ContextSnapshot::load(&root);
        assert_eq!(context.project_rules.len(), 1);
        assert_eq!(context.project_rules[0].path, ".xcoding/rules.md");
        assert!(
            context
                .system_prompt("auto-edit")
                .contains("Prefer ASCII comments.")
        );

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn loads_all_supported_rule_files_in_order() {
        let root = temp_workspace("all-rules");
        fs::create_dir_all(root.join(".xcoding")).expect("dir creates");
        fs::write(root.join("AGENTS.md"), "agents").expect("write");
        fs::write(root.join("XCoding.md"), "xcoding").expect("write");
        fs::write(root.join(".xcoding/rules.md"), "rules").expect("write");

        let context = ContextSnapshot::load(&root);
        let paths: Vec<_> = context
            .project_rules
            .iter()
            .map(|rule| rule.path.as_str())
            .collect();
        assert_eq!(
            paths,
            vec!["AGENTS.md", "XCoding.md", ".xcoding/rules.md"]
        );

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn truncates_oversized_rule_content() {
        let root = temp_workspace("truncate");
        let oversized = "x".repeat(MAX_RULE_CHARS + 50);
        fs::write(root.join("AGENTS.md"), &oversized).expect("write");

        let context = ContextSnapshot::load(&root);
        assert_eq!(context.project_rules.len(), 1);
        assert!(context.project_rules[0].content.contains("[truncated project rule]"));
        assert!(context.project_rules[0].content.chars().count() < oversized.chars().count());

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn sketches_shallow_workspace_paths_and_skips_ignored_dirs() {
        let root = temp_workspace("sketch");
        fs::create_dir_all(root.join("src/nested")).expect("src creates");
        fs::create_dir_all(root.join("node_modules/pkg")).expect("node_modules creates");
        fs::create_dir_all(root.join("target/debug")).expect("target creates");
        fs::write(root.join("package.json"), "{}\n").expect("package writes");
        fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("main writes");
        fs::write(root.join("src/nested/mod.rs"), "// nested\n").expect("nested writes");
        fs::write(root.join("node_modules/pkg/index.js"), "export {}\n").expect("nm writes");

        let context = ContextSnapshot::load(&root);
        assert!(context.relevant_paths.iter().any(|path| path == "package.json"));
        assert!(context.relevant_paths.iter().any(|path| path == "src/"));
        assert!(context.relevant_paths.iter().any(|path| path == "src/main.rs"));
        assert!(context.relevant_paths.iter().any(|path| path == "src/nested/"));
        assert!(context.relevant_paths.iter().any(|path| path == "src/nested/mod.rs"));
        assert!(!context.relevant_paths.iter().any(|path| path.contains("node_modules")));
        assert!(!context.relevant_paths.iter().any(|path| path.contains("target")));

        let prompt = context.system_prompt("ask");
        assert!(prompt.contains("Workspace sketch"));
        assert!(prompt.contains("src/main.rs"));

        fs::remove_dir_all(root).expect("workspace removes");
    }
}
