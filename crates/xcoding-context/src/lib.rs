//! Workspace context assembly is introduced with the read-only agent loop.

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ContextSnapshot {
    pub project_rules: Vec<String>,
    pub relevant_paths: Vec<String>,
}
