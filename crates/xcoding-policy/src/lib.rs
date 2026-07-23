//! Permission decisions for tool execution and command safety classification.

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

/// Outcome of inspecting a proposed `run_command` invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandAssessment {
    pub decision: PermissionDecision,
    pub high_risk: bool,
    pub allowlisted: bool,
    pub reason: String,
}

/// Backward-compatible evaluation that never treats commands as allowlisted.
pub fn evaluate(mode: &Mode, kind: PermissionKind, high_risk: bool) -> PermissionDecision {
    evaluate_detailed(mode, kind, high_risk, false)
}

/// Mode-aware permission evaluation.
///
/// `command_allowlisted` only affects `PermissionKind::Exec` under `auto-edit`.
pub fn evaluate_detailed(
    mode: &Mode,
    kind: PermissionKind,
    high_risk: bool,
    command_allowlisted: bool,
) -> PermissionDecision {
    match kind {
        PermissionKind::Read => PermissionDecision::Allow,
        PermissionKind::Network => PermissionDecision::Deny,
        PermissionKind::Write if high_risk => PermissionDecision::AskUser,
        PermissionKind::Write if matches!(mode, Mode::AutoEdit) => PermissionDecision::Allow,
        PermissionKind::Write => PermissionDecision::AskUser,
        PermissionKind::Exec if high_risk => PermissionDecision::AskUser,
        PermissionKind::Exec if command_allowlisted && matches!(mode, Mode::AutoEdit) => {
            PermissionDecision::Allow
        }
        PermissionKind::Exec => PermissionDecision::AskUser,
    }
}

/// Classify a workspace command before approval or execution.
///
/// Hard-denied commands never reach the user approval prompt.
/// High-risk commands still require approval but are labeled for review UX.
/// Safe allowlisted commands are marked `decision=Allow` and `allowlisted=true`;
/// mode policy still decides whether they auto-run.
pub fn assess_command(executable: &str, args: &[String]) -> CommandAssessment {
    let executable = executable.trim();
    if executable.is_empty() {
        return denied("executable must not be empty");
    }

    if looks_absolute(executable) {
        return denied("absolute executable paths are not allowed");
    }

    if executable.contains("..") || executable.contains('/') || executable.contains('\\') {
        return denied("executable path separators are not allowed; use a bare command name");
    }

    let exe = strip_windows_extension(&executable.to_ascii_lowercase());
    let joined = args
        .iter()
        .map(|arg| arg.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let lower_joined = joined.to_ascii_lowercase();
    let args_lower: Vec<String> = args.iter().map(|arg| arg.to_ascii_lowercase()).collect();

    // Destructive system operations: hard deny.
    if matches!(
        exe.as_str(),
        "format"
            | "mkfs"
            | "mkfs.ext4"
            | "mkfs.xfs"
            | "diskpart"
            | "shutdown"
            | "reboot"
            | "halt"
            | "poweroff"
            | "bcdedit"
            | "cipher"
    ) {
        return denied(format!("command `{exe}` is blocked by XCoding policy"));
    }

    if exe == "rm" && has_flag(&args_lower, "-rf") && targets_filesystem_root(&args_lower) {
        return denied("recursive delete of filesystem roots is blocked by XCoding policy");
    }

    if exe == "del" || exe == "rmdir" || exe == "rd" {
        if has_flag(&args_lower, "/s") && targets_filesystem_root(&args_lower) {
            return denied("recursive delete of filesystem roots is blocked by XCoding policy");
        }
    }

    if exe == "reg" && args_lower.iter().any(|arg| arg == "delete") {
        if lower_joined.contains("hklm") || lower_joined.contains("hkey_local_machine") {
            return denied("registry deletes under HKLM are blocked by XCoding policy");
        }
    }

    // Network-style helpers are high-risk; still require approval.
    let mut high_risk = matches!(
        exe.as_str(),
        "curl"
            | "wget"
            | "ssh"
            | "scp"
            | "sftp"
            | "ftp"
            | "nc"
            | "ncat"
            | "netcat"
            | "powershell"
            | "pwsh"
            | "cmd"
            | "bash"
            | "sh"
            | "zsh"
            | "python"
            | "python3"
            | "node"
            | "perl"
            | "ruby"
    );

    if exe == "git" {
        if args_lower.iter().any(|arg| arg == "push")
            && args_lower
                .iter()
                .any(|arg| arg == "--force" || arg == "-f" || arg == "--force-with-lease")
        {
            high_risk = true;
        }
        if args_lower.iter().any(|arg| arg == "clean")
            && args_lower.iter().any(|arg| arg == "-fdx" || arg == "-ffdx")
        {
            return denied("git clean -fdx is blocked by XCoding policy");
        }
        if args_lower.iter().any(|arg| arg == "reset")
            && args_lower.iter().any(|arg| arg == "--hard")
            && args_lower
                .iter()
                .any(|arg| arg == "head~" || arg.starts_with("head~"))
        {
            high_risk = true;
        }
    }

    if (exe == "npm" || exe == "pnpm" || exe == "yarn")
        && args_lower.iter().any(|arg| arg == "publish")
    {
        high_risk = true;
    }

    if (exe == "powershell" || exe == "pwsh")
        && args_lower.iter().any(|arg| {
            arg == "-encodedcommand"
                || arg == "-enc"
                || arg == "-command"
                || arg == "-c"
                || arg.starts_with("-command:")
        })
    {
        high_risk = true;
    }

    if exe == "cmd" && args_lower.iter().any(|arg| arg == "/c" || arg == "/k") {
        high_risk = true;
    }

    if high_risk {
        return CommandAssessment {
            decision: PermissionDecision::AskUser,
            high_risk: true,
            allowlisted: false,
            reason: format!("high-risk command `{exe}` requires explicit approval"),
        };
    }

    if is_command_allowlisted(&exe, args) {
        return CommandAssessment {
            decision: PermissionDecision::Allow,
            high_risk: false,
            allowlisted: true,
            reason: format!("allowlisted command `{exe}` may auto-run under auto-edit"),
        };
    }

    CommandAssessment {
        decision: PermissionDecision::AskUser,
        high_risk: false,
        allowlisted: false,
        reason: format!("command `{exe}` requires approval before execution"),
    }
}

/// Strict allowlist for safe, commonly used developer commands.
///
/// Never allowlists high-risk shells/interpreters. Rejects shell metacharacters
/// in arguments so callers cannot smuggle extra execution.
pub fn is_command_allowlisted(executable: &str, args: &[String]) -> bool {
    let exe = strip_windows_extension(&executable.trim().to_ascii_lowercase());
    if exe.is_empty() {
        return false;
    }
    if args.iter().any(|arg| contains_shell_metacharacters(arg)) {
        return false;
    }

    let first = args.first().map(|arg| arg.as_str()).unwrap_or("");
    let first_lower = first.to_ascii_lowercase();

    match exe.as_str() {
        "cargo" => {
            if args.is_empty() {
                return false;
            }
            matches!(
                first_lower.as_str(),
                "check"
                    | "test"
                    | "build"
                    | "clippy"
                    | "fmt"
                    | "tree"
                    | "metadata"
                    | "nextest"
                    | "--version"
                    | "-v"
                    | "--help"
                    | "-h"
            )
        }
        "git" => matches!(
            first_lower.as_str(),
            "status" | "diff" | "log" | "show" | "branch" | "rev-parse" | "describe"
        ),
        "pnpm" | "npm" | "yarn" => {
            if args.iter().any(|arg| arg.eq_ignore_ascii_case("publish")) {
                return false;
            }
            matches!(
                first_lower.as_str(),
                "test" | "run" | "lint" | "build" | "exec" | "typecheck" | "vitest"
            )
        }
        "go" => matches!(
            first_lower.as_str(),
            "test" | "build" | "vet" | "fmt" | "list" | "env" | "version"
        ),
        "tsc" => true,
        "pytest" => true,
        "dotnet" => matches!(first_lower.as_str(), "test" | "build" | "restore"),
        _ => false,
    }
}

fn contains_shell_metacharacters(value: &str) -> bool {
    value.chars().any(|ch| {
        matches!(
            ch,
            '&' | '|' | ';' | '`' | '$' | '\n' | '\r' | '>' | '<' | '(' | ')'
        )
    })
}

fn denied(reason: impl Into<String>) -> CommandAssessment {
    CommandAssessment {
        decision: PermissionDecision::Deny,
        high_risk: true,
        allowlisted: false,
        reason: reason.into(),
    }
}

fn looks_absolute(executable: &str) -> bool {
    let path = std::path::Path::new(executable);
    path.is_absolute()
        || executable.starts_with('/')
        || executable.starts_with('\\')
        || (executable.len() >= 3
            && executable.as_bytes()[1] == b':'
            && (executable.as_bytes()[2] == b'\\' || executable.as_bytes()[2] == b'/'))
}

fn strip_windows_extension(name: &str) -> String {
    for ext in [".exe", ".cmd", ".bat", ".ps1", ".com"] {
        if let Some(stripped) = name.strip_suffix(ext) {
            return stripped.to_owned();
        }
    }
    name.to_owned()
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|arg| {
        if arg == flag {
            return true;
        }
        // Compact Unix style such as -rf for -r -f.
        flag.starts_with('-')
            && !flag.starts_with("--")
            && arg.starts_with('-')
            && !arg.starts_with("--")
            && flag.chars().skip(1).all(|ch| arg.contains(ch))
    })
}

fn targets_filesystem_root(args: &[String]) -> bool {
    args.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "/" | "\\"
                | "/*"
                | "\\*"
                | "c:\\"
                | "c:/"
                | "c:\\*"
                | "c:/*"
                | "c:"
        ) || arg.eq_ignore_ascii_case("c:\\")
            || arg.eq_ignore_ascii_case("c:/")
            || arg.eq_ignore_ascii_case("c:\\*")
            || arg.eq_ignore_ascii_case("c:/*")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_edit_allows_normal_writes_but_not_non_allowlisted_commands() {
        assert_eq!(
            evaluate(&Mode::AutoEdit, PermissionKind::Write, false),
            PermissionDecision::Allow
        );
        assert_eq!(
            evaluate_detailed(&Mode::AutoEdit, PermissionKind::Exec, false, false),
            PermissionDecision::AskUser
        );
    }

    #[test]
    fn auto_edit_allows_allowlisted_commands() {
        assert_eq!(
            evaluate_detailed(&Mode::AutoEdit, PermissionKind::Exec, false, true),
            PermissionDecision::Allow
        );
        assert_eq!(
            evaluate_detailed(&Mode::Ask, PermissionKind::Exec, false, true),
            PermissionDecision::AskUser
        );
    }

    #[test]
    fn auto_edit_still_asks_for_high_risk_writes_and_commands() {
        assert_eq!(
            evaluate(&Mode::AutoEdit, PermissionKind::Write, true),
            PermissionDecision::AskUser
        );
        assert_eq!(
            evaluate(&Mode::Ask, PermissionKind::Write, true),
            PermissionDecision::AskUser
        );
        assert_eq!(
            evaluate_detailed(&Mode::AutoEdit, PermissionKind::Exec, true, true),
            PermissionDecision::AskUser
        );
    }

    #[test]
    fn denies_destructive_system_commands() {
        let assessment = assess_command("format", &["C:".to_owned()]);
        assert_eq!(assessment.decision, PermissionDecision::Deny);
        assert!(!assessment.allowlisted);
        assert!(assessment.reason.contains("blocked"));
    }

    #[test]
    fn denies_absolute_and_path_executables() {
        assert_eq!(
            assess_command(r"C:\Windows\System32\cmd.exe", &[]).decision,
            PermissionDecision::Deny
        );
        assert_eq!(
            assess_command("../evil", &[]).decision,
            PermissionDecision::Deny
        );
        assert_eq!(
            assess_command("tools/run", &[]).decision,
            PermissionDecision::Deny
        );
    }

    #[test]
    fn marks_shell_interpreters_high_risk_but_askable() {
        let assessment = assess_command(
            "powershell",
            &["-Command".to_owned(), "Get-ChildItem".to_owned()],
        );
        assert_eq!(assessment.decision, PermissionDecision::AskUser);
        assert!(assessment.high_risk);
        assert!(!assessment.allowlisted);
    }

    #[test]
    fn marks_force_push_high_risk() {
        let assessment = assess_command(
            "git",
            &[
                "push".to_owned(),
                "--force".to_owned(),
                "origin".to_owned(),
                "main".to_owned(),
            ],
        );
        assert_eq!(assessment.decision, PermissionDecision::AskUser);
        assert!(assessment.high_risk);
        assert!(!assessment.allowlisted);
    }

    #[test]
    fn denies_git_clean_fdx() {
        let assessment = assess_command("git", &["clean".to_owned(), "-fdx".to_owned()]);
        assert_eq!(assessment.decision, PermissionDecision::Deny);
    }

    #[test]
    fn allowlists_common_build_commands() {
        let assessment = assess_command(
            "cargo",
            &["test".to_owned(), "-p".to_owned(), "xcoding-policy".to_owned()],
        );
        assert_eq!(assessment.decision, PermissionDecision::Allow);
        assert!(assessment.allowlisted);
        assert!(!assessment.high_risk);

        let version = assess_command("cargo", &["--version".to_owned()]);
        assert!(version.allowlisted);

        let git_status = assess_command("git", &["status".to_owned(), "--short".to_owned()]);
        assert!(git_status.allowlisted);
    }

    #[test]
    fn rejects_allowlist_when_args_contain_shell_metacharacters() {
        assert!(!is_command_allowlisted(
            "cargo",
            &["test".to_owned(), "&&".to_owned(), "rm".to_owned(), "-rf".to_owned(), "/".to_owned()]
        ));
        let assessment = assess_command(
            "cargo",
            &["test".to_owned(), ";".to_owned(), "evil".to_owned()],
        );
        assert!(!assessment.allowlisted);
        assert_eq!(assessment.decision, PermissionDecision::AskUser);
    }

    #[test]
    fn does_not_allowlist_publish_or_shell_wrappers() {
        assert!(!is_command_allowlisted("pnpm", &["publish".to_owned()]));
        assert!(!is_command_allowlisted("cmd", &["/c".to_owned(), "echo".to_owned(), "hi".to_owned()]));
        assert!(!is_command_allowlisted("node", &["-e".to_owned(), "1".to_owned()]));
    }
}
