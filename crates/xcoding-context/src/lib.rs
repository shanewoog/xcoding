//! Project-rule loading and prompt context for the coding-agent loop.

use std::{fs, path::Path};

/// Workspace-root rule files, in load order.
const RULE_FILES: [&str; 3] = ["AGENTS.md", "XCoding.md", ".xcoding/rules.md"];
const MAX_RULE_CHARS: usize = 20_000;

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
            relevant_paths: Vec::new(),
        }
    }

    /// Build the system prompt for the active mode (`ask` or `auto-edit`).
    pub fn system_prompt(&self, mode: &str) -> String {
        let mut prompt = format!(
            "You are XCoding, a local coding agent for a software workspace. \
When repository facts are needed, use tools before answering. Never claim a file was inspected unless a tool result contains it. \
Available tools: list_dir, read_file, search_code, apply_patch, run_command, git_status, git_diff. \
Current mode: {mode}. \
In ask mode, propose writes and wait for required approval. In auto-edit mode, ordinary file patches may apply without approval, but shell commands still require user approval. \
Prefer minimal, scoped changes. Do not invent secrets or commit credentials."
        );

        if !self.project_rules.is_empty() {
            prompt.push_str("\n\nProject rules (follow these for this workspace):\n");
            for rule in &self.project_rules {
                prompt.push_str(&format!("\n--- {} ---\n{}\n", rule.path, rule.content));
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
        assert!(prompt.contains("Current mode: ask"));

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
}
