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

/// Stable machine-readable outcome of command policy evaluation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandPolicyCode {
    EmptyExecutable,
    AbsolutePath,
    PathSeparator,
    DeniedExecutable,
    DeniedRecursiveRootDelete,
    DeniedRegistryHklm,
    DeniedGitClean,
    DeniedGitMirrorPush,
    DeniedDestructiveDisk,
    DeniedWorkspaceDenylist,
    HighRiskShell,
    HighRiskNetwork,
    HighRiskForcePush,
    HighRiskPublish,
    HighRiskInterpreter,
    HighRiskGit,
    HighRiskSudo,
    Allowlisted,
    RequiresApproval,
}

impl CommandPolicyCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EmptyExecutable => "empty_executable",
            Self::AbsolutePath => "absolute_path",
            Self::PathSeparator => "path_separator",
            Self::DeniedExecutable => "denied_executable",
            Self::DeniedRecursiveRootDelete => "denied_recursive_root_delete",
            Self::DeniedRegistryHklm => "denied_registry_hklm",
            Self::DeniedGitClean => "denied_git_clean",
            Self::DeniedGitMirrorPush => "denied_git_mirror_push",
            Self::DeniedDestructiveDisk => "denied_destructive_disk",
            Self::DeniedWorkspaceDenylist => "denied_workspace_denylist",
            Self::HighRiskShell => "high_risk_shell",
            Self::HighRiskNetwork => "high_risk_network",
            Self::HighRiskForcePush => "high_risk_force_push",
            Self::HighRiskPublish => "high_risk_publish",
            Self::HighRiskInterpreter => "high_risk_interpreter",
            Self::HighRiskGit => "high_risk_git",
            Self::HighRiskSudo => "high_risk_sudo",
            Self::Allowlisted => "allowlisted",
            Self::RequiresApproval => "requires_approval",
        }
    }
}

impl std::fmt::Display for CommandPolicyCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Outcome of inspecting a proposed `run_command` invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandAssessment {
    pub decision: PermissionDecision,
    pub high_risk: bool,
    pub allowlisted: bool,
    pub code: CommandPolicyCode,
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
    assess_command_with_lists(executable, args, &[], &[])
}

/// Like [`assess_command`] but also checks workspace-provided extra allowlist patterns.
pub fn assess_command_with_extra(
    executable: &str,
    args: &[String],
    extra_allowlist: &[String],
) -> CommandAssessment {
    assess_command_with_lists(executable, args, extra_allowlist, &[])
}

/// Full command assessment with workspace allowlist and denylist patterns.
pub fn assess_command_with_lists(
    executable: &str,
    args: &[String],
    extra_allowlist: &[String],
    extra_denylist: &[String],
) -> CommandAssessment {
    let executable = executable.trim();
    if executable.is_empty() {
        return denied(CommandPolicyCode::EmptyExecutable, "executable must not be empty");
    }

    if looks_absolute(executable) {
        return denied(
            CommandPolicyCode::AbsolutePath,
            "absolute executable paths are not allowed",
        );
    }

    if executable.contains("..") || executable.contains('/') || executable.contains('\\') {
        return denied(
            CommandPolicyCode::PathSeparator,
            "executable path separators are not allowed; use a bare command name",
        );
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
            | "mkfs.btrfs"
            | "mkfs.vfat"
            | "diskpart"
            | "shutdown"
            | "reboot"
            | "halt"
            | "poweroff"
            | "bcdedit"
            | "cipher"
            | "fdisk"
            | "parted"
            | "wipefs"
            | "diskutil"
    ) {
        return denied(
            CommandPolicyCode::DeniedExecutable,
            format!("command `{exe}` is blocked by XCoding policy"),
        );
    }

    if exe == "dd" && args_lower.iter().any(|arg| is_raw_disk_dd_arg(arg)) {
        return denied(
            CommandPolicyCode::DeniedDestructiveDisk,
            "raw disk dd device targets are blocked by XCoding policy",
        );
    }

    if exe == "rm" && has_flag(&args_lower, "-rf") && targets_dangerous_delete_path(&args_lower) {
        return denied(
            CommandPolicyCode::DeniedRecursiveRootDelete,
            "recursive delete of filesystem roots or home directories is blocked by XCoding policy",
        );
    }

    if exe == "del" || exe == "rmdir" || exe == "rd" {
        if has_flag(&args_lower, "/s") && targets_filesystem_root(&args_lower) {
            return denied(
                CommandPolicyCode::DeniedRecursiveRootDelete,
                "recursive delete of filesystem roots is blocked by XCoding policy",
            );
        }
    }

    if (exe == "chmod" || exe == "chown")
        && has_flag(&args_lower, "-r")
        && targets_filesystem_root(&args_lower)
    {
        return denied(
            CommandPolicyCode::DeniedRecursiveRootDelete,
            format!("recursive `{exe}` of filesystem roots is blocked by XCoding policy"),
        );
    }

    if exe == "reg" && args_lower.iter().any(|arg| arg == "delete") {
        if lower_joined.contains("hklm") || lower_joined.contains("hkey_local_machine") {
            return denied(
                CommandPolicyCode::DeniedRegistryHklm,
                "registry deletes under HKLM are blocked by XCoding policy",
            );
        }
    }

    if exe == "git" {
        if args_lower.iter().any(|arg| arg == "clean") && git_clean_is_hard_denied(&args_lower) {
            return denied(
                CommandPolicyCode::DeniedGitClean,
                "git clean that removes ignored files with force is blocked by XCoding policy",
            );
        }
        if args_lower.iter().any(|arg| arg == "push")
            && args_lower.iter().any(|arg| arg == "--mirror")
        {
            return denied(
                CommandPolicyCode::DeniedGitMirrorPush,
                "git push --mirror is blocked by XCoding policy",
            );
        }
    }

    if matches_extra_denylist(&exe, args, extra_denylist) {
        return denied(
            CommandPolicyCode::DeniedWorkspaceDenylist,
            format!("command `{exe}` is blocked by workspace command denylist"),
        );
    }

    // High-risk helpers still require approval.
    if matches!(
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
    ) {
        return high_risk(
            CommandPolicyCode::HighRiskNetwork,
            format!("high-risk network command `{exe}` requires explicit approval"),
        );
    }

    if matches!(
        exe.as_str(),
        "powershell"
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
    ) {
        let code = if matches!(
            exe.as_str(),
            "powershell" | "pwsh" | "cmd" | "bash" | "sh" | "zsh"
        ) {
            CommandPolicyCode::HighRiskShell
        } else {
            CommandPolicyCode::HighRiskInterpreter
        };
        return high_risk(
            code,
            format!("high-risk command `{exe}` requires explicit approval"),
        );
    }

    if matches!(exe.as_str(), "sudo" | "doas" | "runas") {
        return high_risk(
            CommandPolicyCode::HighRiskSudo,
            format!("privileged command `{exe}` requires explicit approval"),
        );
    }

    if exe == "git" {
        if args_lower.iter().any(|arg| arg == "push")
            && args_lower
                .iter()
                .any(|arg| arg == "--force" || arg == "-f" || arg == "--force-with-lease")
        {
            return high_risk(
                CommandPolicyCode::HighRiskForcePush,
                "git force push requires explicit approval",
            );
        }
        if args_lower.iter().any(|arg| arg == "reset")
            && args_lower.iter().any(|arg| arg == "--hard")
        {
            return high_risk(
                CommandPolicyCode::HighRiskGit,
                "git reset --hard requires explicit approval",
            );
        }
        if args_lower.iter().any(|arg| {
            arg == "filter-branch" || arg == "filter-repo" || arg == "rebase" || arg == "--amend"
        }) {
            return high_risk(
                CommandPolicyCode::HighRiskGit,
                "high-risk git operation requires explicit approval",
            );
        }
    }

    if (exe == "npm" || exe == "pnpm" || exe == "yarn")
        && args_lower.iter().any(|arg| arg == "publish")
    {
        return high_risk(
            CommandPolicyCode::HighRiskPublish,
            format!("package publish via `{exe}` requires explicit approval"),
        );
    }

    if is_command_allowlisted_with_extra(&exe, args, extra_allowlist) {
        return CommandAssessment {
            decision: PermissionDecision::Allow,
            high_risk: false,
            allowlisted: true,
            code: CommandPolicyCode::Allowlisted,
            reason: format!("allowlisted command `{exe}` may auto-run under auto-edit"),
        };
    }

    CommandAssessment {
        decision: PermissionDecision::AskUser,
        high_risk: false,
        allowlisted: false,
        code: CommandPolicyCode::RequiresApproval,
        reason: format!("command `{exe}` requires approval before execution"),
    }
}
/// Strict allowlist for safe, commonly used developer commands.
///
/// Never allowlists high-risk shells/interpreters. Rejects shell metacharacters
/// in arguments so callers cannot smuggle extra execution.
pub fn is_command_allowlisted(executable: &str, args: &[String]) -> bool {
    is_command_allowlisted_with_extra(executable, args, &[])
}

/// Builtin allowlist plus validated workspace extra patterns.
pub fn is_command_allowlisted_with_extra(
    executable: &str,
    args: &[String],
    extra_allowlist: &[String],
) -> bool {
    if is_builtin_command_allowlisted(executable, args) {
        return true;
    }
    matches_extra_allowlist(executable, args, extra_allowlist)
}

fn is_builtin_command_allowlisted(executable: &str, args: &[String]) -> bool {
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

/// Parse a `.xcoding/command-allowlist` file body into normalized patterns.
pub fn parse_command_allowlist(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| normalize_allowlist_pattern(line).ok())
        .collect()
}

/// Parse a `.xcoding/command-denylist` file body into normalized patterns.
pub fn parse_command_denylist(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| normalize_denylist_pattern(line).ok())
        .collect()
}

/// Validate and normalize one allowlist pattern (`exe` or `exe:subcommand`).
pub fn normalize_allowlist_pattern(pattern: &str) -> Result<String, String> {
    let normalized = normalize_command_pattern(pattern, "allowlist")?;
    let exe = normalized
        .split_once(':')
        .map(|(exe, _)| exe)
        .unwrap_or(normalized.as_str());
    if is_never_custom_allowlisted(exe) {
        return Err(format!(
            "command `{exe}` cannot be added to the workspace allowlist"
        ));
    }
    Ok(normalized)
}

/// Validate and normalize one denylist pattern (`exe` or `exe:subcommand`).
///
/// Unlike allowlist patterns, shells and interpreters may be denylisted.
pub fn normalize_denylist_pattern(pattern: &str) -> Result<String, String> {
    normalize_command_pattern(pattern, "denylist")
}

fn normalize_command_pattern(pattern: &str, kind: &str) -> Result<String, String> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return Err(format!("{kind} pattern must not be empty"));
    }
    if pattern.contains("..") || pattern.contains('/') || pattern.contains('\\') {
        return Err(format!(
            "{kind} patterns must be bare command names without path separators"
        ));
    }
    if contains_shell_metacharacters(pattern) {
        return Err(format!(
            "{kind} patterns must not contain shell metacharacters"
        ));
    }

    let (exe_raw, sub) = match pattern.split_once(':') {
        Some((exe, sub)) => (exe.trim(), Some(sub.trim())),
        None => (pattern, None),
    };
    if exe_raw.is_empty() {
        return Err(format!("{kind} executable must not be empty"));
    }
    if let Some(sub) = sub {
        if sub.is_empty() {
            return Err(format!("{kind} subcommand after ':' must not be empty"));
        }
        if sub.contains(':') {
            return Err(format!(
                "{kind} patterns support at most one ':' separator"
            ));
        }
    }

    let exe = strip_windows_extension(&exe_raw.to_ascii_lowercase());
    if exe.is_empty() {
        return Err(format!("{kind} executable must not be empty"));
    }

    Ok(match sub {
        Some(sub) => format!("{exe}:{}", sub.to_ascii_lowercase()),
        None => exe,
    })
}

fn is_never_custom_allowlisted(exe: &str) -> bool {
    matches!(
        exe,
        "format"
            | "mkfs"
            | "mkfs.ext4"
            | "mkfs.xfs"
            | "mkfs.btrfs"
            | "mkfs.vfat"
            | "diskpart"
            | "shutdown"
            | "reboot"
            | "halt"
            | "poweroff"
            | "bcdedit"
            | "cipher"
            | "fdisk"
            | "parted"
            | "wipefs"
            | "diskutil"
            | "dd"
            | "curl"
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
            | "sudo"
            | "doas"
            | "runas"
    )
}

fn matches_extra_allowlist(executable: &str, args: &[String], extra_allowlist: &[String]) -> bool {
    if extra_allowlist.is_empty() {
        return false;
    }
    if args.iter().any(|arg| contains_shell_metacharacters(arg)) {
        return false;
    }
    let exe = strip_windows_extension(&executable.trim().to_ascii_lowercase());
    if exe.is_empty() || is_never_custom_allowlisted(&exe) {
        return false;
    }
    if matches!(exe.as_str(), "pnpm" | "npm" | "yarn")
        && args.iter().any(|arg| arg.eq_ignore_ascii_case("publish"))
    {
        return false;
    }

    let first = args
        .first()
        .map(|arg| arg.to_ascii_lowercase())
        .unwrap_or_default();

    extra_allowlist.iter().any(|pattern| {
        let Ok(normalized) = normalize_allowlist_pattern(pattern) else {
            return false;
        };
        if let Some((pat_exe, pat_sub)) = normalized.split_once(':') {
            exe == pat_exe && first == pat_sub
        } else {
            exe == normalized
        }
    })
}

fn matches_extra_denylist(executable: &str, args: &[String], extra_denylist: &[String]) -> bool {
    if extra_denylist.is_empty() {
        return false;
    }
    let exe = strip_windows_extension(&executable.trim().to_ascii_lowercase());
    if exe.is_empty() {
        return false;
    }
    let first = args
        .first()
        .map(|arg| arg.to_ascii_lowercase())
        .unwrap_or_default();

    extra_denylist.iter().any(|pattern| {
        let Ok(normalized) = normalize_denylist_pattern(pattern) else {
            return false;
        };
        if let Some((pat_exe, pat_sub)) = normalized.split_once(':') {
            exe == pat_exe && first == pat_sub
        } else {
            exe == normalized
        }
    })
}

pub fn render_command_allowlist_file(patterns: &[String]) -> String {
    let mut body = String::from(
        "# XCoding workspace command allowlist\n# One pattern per line: executable or executable:subcommand\n# Example:\n#   rg\n#   make:test\n#   git:--version\n# Shells/interpreters and destructive system commands are rejected.\n",
    );
    for pattern in patterns {
        if let Ok(normalized) = normalize_allowlist_pattern(pattern) {
            body.push_str(&normalized);
            body.push('\n');
        }
    }
    body
}

pub fn render_command_denylist_file(patterns: &[String]) -> String {
    let mut body = String::from(
        "# XCoding workspace command denylist\n# One pattern per line: executable or executable:subcommand\n# Example:\n#   curl\n#   git:push\n#   pnpm:publish\n# Matched commands are hard-denied (no approval prompt).\n",
    );
    for pattern in patterns {
        if let Ok(normalized) = normalize_denylist_pattern(pattern) {
            body.push_str(&normalized);
            body.push('\n');
        }
    }
    body
}

pub const COMMAND_ALLOWLIST_RELATIVE_PATH: &str = ".xcoding/command-allowlist";
pub const COMMAND_DENYLIST_RELATIVE_PATH: &str = ".xcoding/command-denylist";
fn contains_shell_metacharacters(value: &str) -> bool {
    value.chars().any(|ch| {
        matches!(
            ch,
            '&' | '|' | ';' | '`' | '$' | '\n' | '\r' | '>' | '<' | '(' | ')'
        )
    })
}

fn denied(code: CommandPolicyCode, reason: impl Into<String>) -> CommandAssessment {
    CommandAssessment {
        decision: PermissionDecision::Deny,
        high_risk: true,
        allowlisted: false,
        code,
        reason: reason.into(),
    }
}

fn high_risk(code: CommandPolicyCode, reason: impl Into<String>) -> CommandAssessment {
    CommandAssessment {
        decision: PermissionDecision::AskUser,
        high_risk: true,
        allowlisted: false,
        code,
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

fn targets_dangerous_delete_path(args: &[String]) -> bool {
    if targets_filesystem_root(args) {
        return true;
    }
    args.iter().any(|arg| {
        let lower = arg.to_ascii_lowercase();
        matches!(
            lower.as_str(),
            "~" | "~/"
                | "~/*"
                | "$home"
                | "${home}"
                | "%userprofile%"
                | "/home"
                | "/home/*"
                | "/users"
                | "/users/*"
        ) || lower == "c:\\users"
            || lower == "c:/users"
            || lower.starts_with("c:\\users\\")
            || lower.starts_with("c:/users/")
    })
}

fn is_raw_disk_dd_arg(arg: &str) -> bool {
    let lower = arg.to_ascii_lowercase();
    lower.starts_with("if=/dev/")
        || lower.starts_with("of=/dev/")
        || lower.starts_with(r"if=\\.\")
        || lower.starts_with(r"of=\\.\")
        || lower.contains("physicaldrive")
}

fn git_clean_is_hard_denied(args: &[String]) -> bool {
    let force = args.iter().any(|arg| {
        arg == "-f"
            || arg == "--force"
            || (arg.starts_with('-') && !arg.starts_with("--") && arg.contains('f') && arg != "-")
    });
    let ignored = args.iter().any(|arg| {
        arg == "-x"
            || arg == "-X"
            || arg == "--ignored"
            || (arg.starts_with('-')
                && !arg.starts_with("--")
                && (arg.contains('x') || arg.contains('X')))
    });
    force && ignored
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
            evaluate_detailed(&Mode::AutoEdit, PermissionKind::Exec, false, true),
            PermissionDecision::Allow
        );
        assert_eq!(
            evaluate_detailed(&Mode::AutoEdit, PermissionKind::Exec, false, false),
            PermissionDecision::AskUser
        );
        assert_eq!(
            evaluate(&Mode::Ask, PermissionKind::Write, false),
            PermissionDecision::AskUser
        );
        assert_eq!(
            evaluate(&Mode::Ask, PermissionKind::Exec, false),
            PermissionDecision::AskUser
        );
    }

    #[test]
    fn custom_allowlist_extends_builtin() {
        let extra = vec!["git:--version".to_owned(), "rg".to_owned()];
        assert!(is_command_allowlisted_with_extra(
            "git",
            &["--version".to_owned()],
            &extra
        ));
        assert!(is_command_allowlisted_with_extra(
            "rg",
            &["TODO".to_owned(), "src".to_owned()],
            &extra
        ));
        assert!(!is_command_allowlisted_with_extra(
            "git",
            &["--version".to_owned()],
            &[]
        ));
        assert!(matches!(normalize_allowlist_pattern("powershell"), Err(_)));
        assert!(matches!(normalize_allowlist_pattern("git:--version"), Ok(_)));
        let assessment =
            assess_command_with_extra("git", &["--version".to_owned()], &extra);
        assert!(assessment.allowlisted);
        assert_eq!(assessment.decision, PermissionDecision::Allow);
        assert_eq!(assessment.code, CommandPolicyCode::Allowlisted);
    }

    #[test]
    fn parse_command_allowlist_ignores_comments() {
        let parsed = parse_command_allowlist("# comment\nrg\n\nmake:test\n");
        assert_eq!(parsed, vec!["rg".to_owned(), "make:test".to_owned()]);
    }

    #[test]
    fn workspace_denylist_overrides_allowlist() {
        let allow = vec!["rg".to_owned()];
        let deny = vec!["rg".to_owned()];
        let assessment =
            assess_command_with_lists("rg", &["TODO".to_owned()], &allow, &deny);
        assert_eq!(assessment.decision, PermissionDecision::Deny);
        assert_eq!(assessment.code, CommandPolicyCode::DeniedWorkspaceDenylist);
        assert!(!assessment.allowlisted);
    }

    #[test]
    fn parse_command_denylist_accepts_shells() {
        let parsed = parse_command_denylist("# x\npowershell\ngit:push\n");
        assert_eq!(
            parsed,
            vec!["powershell".to_owned(), "git:push".to_owned()]
        );
        assert!(normalize_denylist_pattern("bash").is_ok());
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
        assert_eq!(assessment.code, CommandPolicyCode::DeniedExecutable);
        assert!(!assessment.allowlisted);
        assert!(assessment.reason.contains("blocked"));
    }

    #[test]
    fn denies_absolute_and_path_executables() {
        assert_eq!(
            assess_command(r"C:\Windows\System32\cmd.exe", &[]).code,
            CommandPolicyCode::AbsolutePath
        );
        assert_eq!(
            assess_command("../evil", &[]).code,
            CommandPolicyCode::PathSeparator
        );
        assert_eq!(
            assess_command("tools/run", &[]).code,
            CommandPolicyCode::PathSeparator
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
        assert_eq!(assessment.code, CommandPolicyCode::HighRiskShell);
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
        assert_eq!(assessment.code, CommandPolicyCode::HighRiskForcePush);
        assert!(!assessment.allowlisted);
    }

    #[test]
    fn denies_git_clean_fdx() {
        let assessment = assess_command("git", &["clean".to_owned(), "-fdx".to_owned()]);
        assert_eq!(assessment.decision, PermissionDecision::Deny);
        assert_eq!(assessment.code, CommandPolicyCode::DeniedGitClean);
    }

    #[test]
    fn denies_git_push_mirror() {
        let assessment = assess_command(
            "git",
            &["push".to_owned(), "--mirror".to_owned(), "origin".to_owned()],
        );
        assert_eq!(assessment.decision, PermissionDecision::Deny);
        assert_eq!(assessment.code, CommandPolicyCode::DeniedGitMirrorPush);
    }

    #[test]
    fn denies_raw_disk_dd() {
        let assessment = assess_command(
            "dd",
            &["if=/dev/zero".to_owned(), "of=/dev/sda".to_owned()],
        );
        assert_eq!(assessment.decision, PermissionDecision::Deny);
        assert_eq!(assessment.code, CommandPolicyCode::DeniedDestructiveDisk);
    }

    #[test]
    fn allowlists_common_build_commands() {
        let assessment = assess_command(
            "cargo",
            &[
                "test".to_owned(),
                "-p".to_owned(),
                "xcoding-policy".to_owned(),
            ],
        );
        assert_eq!(assessment.decision, PermissionDecision::Allow);
        assert!(assessment.allowlisted);
        assert!(!assessment.high_risk);
        assert_eq!(assessment.code, CommandPolicyCode::Allowlisted);

        let version = assess_command("cargo", &["--version".to_owned()]);
        assert!(version.allowlisted);

        let git_status = assess_command("git", &["status".to_owned(), "--short".to_owned()]);
        assert!(git_status.allowlisted);
    }

    #[test]
    fn rejects_allowlist_when_args_contain_shell_metacharacters() {
        assert!(!is_command_allowlisted(
            "cargo",
            &[
                "test".to_owned(),
                "&&".to_owned(),
                "rm".to_owned(),
                "-rf".to_owned(),
                "/".to_owned()
            ]
        ));
        let assessment = assess_command(
            "cargo",
            &["test".to_owned(), ";".to_owned(), "evil".to_owned()],
        );
        assert!(!assessment.allowlisted);
        assert_eq!(assessment.decision, PermissionDecision::AskUser);
        assert_eq!(assessment.code, CommandPolicyCode::RequiresApproval);
    }

    #[test]
    fn does_not_allowlist_publish_or_shell_wrappers() {
        assert!(!is_command_allowlisted("pnpm", &["publish".to_owned()]));
        assert!(!is_command_allowlisted(
            "cmd",
            &["/c".to_owned(), "echo".to_owned(), "hi".to_owned()]
        ));
        assert!(!is_command_allowlisted(
            "node",
            &["-e".to_owned(), "1".to_owned()]
        ));
        let publish = assess_command("pnpm", &["publish".to_owned()]);
        assert_eq!(publish.code, CommandPolicyCode::HighRiskPublish);
        assert!(publish.high_risk);
    }

    #[test]
    fn policy_codes_are_stable_snake_case() {
        assert_eq!(
            CommandPolicyCode::DeniedExecutable.as_str(),
            "denied_executable"
        );
        assert_eq!(
            CommandPolicyCode::HighRiskForcePush.as_str(),
            "high_risk_force_push"
        );
        assert_eq!(
            CommandPolicyCode::DeniedWorkspaceDenylist.as_str(),
            "denied_workspace_denylist"
        );
    }
}
