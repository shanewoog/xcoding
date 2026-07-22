//! Permission decisions for tool execution. The tool runtime will consume this in Phase 2.

use xcoding_protocol::Mode;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionKind {
    Read,
    Write,
    Exec,
    Network,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionDecision {
    Allow,
    AskUser,
    Deny,
}

pub fn evaluate(mode: &Mode, kind: PermissionKind, high_risk: bool) -> PermissionDecision {
    match kind {
        PermissionKind::Read => PermissionDecision::Allow,
        PermissionKind::Network => PermissionDecision::Deny,
        PermissionKind::Exec => PermissionDecision::AskUser,
        PermissionKind::Write if high_risk => PermissionDecision::AskUser,
        PermissionKind::Write if matches!(mode, Mode::AutoEdit) => PermissionDecision::Allow,
        PermissionKind::Write => PermissionDecision::AskUser,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_edit_allows_normal_writes_but_not_commands() {
        assert_eq!(
            evaluate(&Mode::AutoEdit, PermissionKind::Write, false),
            PermissionDecision::Allow
        );
        assert_eq!(
            evaluate(&Mode::AutoEdit, PermissionKind::Exec, false),
            PermissionDecision::AskUser
        );
    }
}
