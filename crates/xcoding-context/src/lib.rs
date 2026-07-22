//! Project-rule loading and prompt context for the read-only agent loop.

use std::{fs, path::Path};

const RULE_FILES: [&str; 2] = ["AGENTS.md", "XCoding.md"];

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

    pub fn system_prompt(&self) -> String {
        let mut prompt = String::from(
            "You are XCoding, a read-only coding assistant. When repository facts are needed, use the available tools before answering. Never claim a file was inspected unless a tool result contains it. This phase only permits list_dir, read_file, and search_code; do not suggest that you edited files or ran commands.",
        );

        if !self.project_rules.is_empty() {
            prompt.push_str("\n\nProject rules:\n");
            for rule in &self.project_rules {
                prompt.push_str(&format!("\n--- {} ---\n{}\n", rule.path, rule.content));
            }
        }

        prompt
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    #[test]
    fn loads_root_project_rules_into_the_system_prompt() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock works")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("xcoding-context-{unique}"));
        fs::create_dir_all(&root).expect("workspace creates");
        fs::write(root.join("AGENTS.md"), "Run focused tests.").expect("rule writes");

        let context = ContextSnapshot::load(&root);
        assert_eq!(context.project_rules.len(), 1);
        assert!(context.system_prompt().contains("Run focused tests."));

        fs::remove_dir_all(root).expect("workspace removes");
    }
}
