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
    DeniedAbsoluteDelete,
    DeniedShellDestructiveDelete,
    DeniedRecursiveRootDelete,
    DeniedRegistryHklm,
    DeniedGitClean,
    DeniedGitMirrorPush,
    DeniedGitRemoteDelete,
    DeniedGitHistoryRewrite,
    DeniedGitReferenceDelete,
    DeniedGitForcedWorktreeDelete,
    DeniedDeletePathTraversal,
    DeniedDestructiveDisk,
    DeniedWorkspaceDenylist,
    DeniedDetachedWindow,
    HighRiskShell,
    HighRiskNetwork,
    HighRiskForcePush,
    HighRiskPublish,
    HighRiskPackageInstall,
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
            Self::DeniedAbsoluteDelete => "denied_absolute_delete",
            Self::DeniedShellDestructiveDelete => "denied_shell_destructive_delete",
            Self::DeniedRecursiveRootDelete => "denied_recursive_root_delete",
            Self::DeniedRegistryHklm => "denied_registry_hklm",
            Self::DeniedGitClean => "denied_git_clean",
            Self::DeniedGitMirrorPush => "denied_git_mirror_push",
            Self::DeniedGitRemoteDelete => "denied_git_remote_delete",
            Self::DeniedGitHistoryRewrite => "denied_git_history_rewrite",
            Self::DeniedGitReferenceDelete => "denied_git_reference_delete",
            Self::DeniedGitForcedWorktreeDelete => "denied_git_forced_worktree_delete",
            Self::DeniedDeletePathTraversal => "denied_delete_path_traversal",
            Self::DeniedDestructiveDisk => "denied_destructive_disk",
            Self::DeniedWorkspaceDenylist => "denied_workspace_denylist",
            Self::DeniedDetachedWindow => "denied_detached_window",
            Self::HighRiskShell => "high_risk_shell",
            Self::HighRiskNetwork => "high_risk_network",
            Self::HighRiskForcePush => "high_risk_force_push",
            Self::HighRiskPublish => "high_risk_publish",
            Self::HighRiskPackageInstall => "high_risk_package_install",
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
/// Ordinary workspace file writes (`PermissionKind::Write`, `high_risk=false`) are
/// always allowed in both modes so agents fully operate on files inside the
/// workspace root. Paths outside the workspace remain rejected by the tools layer.
/// High-risk writes (git mutations, `.git` / `.xcoding` paths) still need approval.
/// `command_allowlisted` only affects `PermissionKind::Exec` under `auto-edit`.
///
/// Under `full-auto`, ordinary writes and low-risk commands are allowed without
/// approval. High-risk commands still require approval, while hard-denied commands
/// are rejected earlier by [`assess_command`]. `PermissionKind::Network` stays
/// denied in every mode.
pub fn evaluate_detailed(
    mode: &Mode,
    kind: PermissionKind,
    high_risk: bool,
    command_allowlisted: bool,
) -> PermissionDecision {
    if matches!(mode, Mode::FullAuto)
        && !matches!(kind, PermissionKind::Network)
        && !high_risk
    {
        return PermissionDecision::Allow;
    }
    match kind {
        PermissionKind::Read => PermissionDecision::Allow,
        PermissionKind::Network => PermissionDecision::Deny,
        PermissionKind::Write if high_risk => PermissionDecision::AskUser,
        PermissionKind::Write => PermissionDecision::Allow,
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
        return denied(
            CommandPolicyCode::EmptyExecutable,
            "executable must not be empty",
        );
    }

    if looks_absolute(executable) {
        return denied(
            CommandPolicyCode::AbsolutePath,
            "absolute executable paths are not allowed",
        );
    }

    // Relative build-output paths are a normal local workflow, so allow `./x` and
    // `target/x` while still rejecting absolute paths and upward traversal.
    let normalized_executable = executable.replace('\\', "/");
    let relative_prefix_allowed =
        normalized_executable.starts_with("./") || normalized_executable.starts_with("target/");
    let has_separator = executable.contains('/') || executable.contains('\\');
    if executable.contains("..") || (has_separator && !relative_prefix_allowed) {
        return denied(
            CommandPolicyCode::PathSeparator,
            "executable must be a bare command name or a relative path under ./ or target/",
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

    // A shell receives packed command text, so a destructive operation can be
    // hidden inside one quoted `/c` or `-Command` argument and bypass the
    // direct executable checks below. Refuse that form before approval or
    // execution; destructive file operations must use a direct executable or a
    // structured file API instead of shell parsing.
    if shell_wraps_destructive_delete(&exe, &args_lower) {
        return denied(
            CommandPolicyCode::DeniedShellDestructiveDelete,
            "shell-wrapped delete commands are blocked by XCoding policy; use a direct file operation instead",
        );
    }

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

    if matches!(exe.as_str(), "rm" | "del" | "erase" | "rmdir" | "rd")
        && targets_path_traversal(&args_lower)
    {
        return denied(
            CommandPolicyCode::DeniedDeletePathTraversal,
            "delete targets containing parent traversal are blocked; use a workspace-relative target",
        );
    }

    if exe == "del" || exe == "erase" || exe == "rmdir" || exe == "rd" {
        if has_flag(&args_lower, "/s") && targets_filesystem_root(&args_lower) {
            return denied(
                CommandPolicyCode::DeniedRecursiveRootDelete,
                "recursive delete of filesystem roots is blocked by XCoding policy",
            );
        }
        if targets_absolute_path(&args_lower) {
            return denied(
                CommandPolicyCode::DeniedAbsoluteDelete,
                "absolute-path file deletion is blocked by XCoding policy; use a workspace-relative path",
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
        if git_forced_worktree_delete(&args_lower) {
            return denied(
                CommandPolicyCode::DeniedGitForcedWorktreeDelete,
                "forced git worktree or submodule deletion is blocked by XCoding policy",
            );
        }
        if git_file_delete_target_traverses_parent(&args_lower) {
            return denied(
                CommandPolicyCode::DeniedDeletePathTraversal,
                "destructive git targets containing parent traversal are blocked",
            );
        }
        if args_lower.iter().any(|arg| arg == "clean") {
            return denied(
                CommandPolicyCode::DeniedGitClean,
                "git clean can delete untracked workspace files and is blocked by XCoding policy",
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
        if git_push_deletes_remote_ref(&args_lower) {
            return denied(
                CommandPolicyCode::DeniedGitRemoteDelete,
                "deleting a remote git ref is blocked by XCoding policy",
            );
        }
        if git_history_rewrite_is_irreversible(&args_lower) {
            return denied(
                CommandPolicyCode::DeniedGitHistoryRewrite,
                "irreversible git history cleanup or rewrite is blocked by XCoding policy",
            );
        }
        if git_deletes_reference(&args_lower) {
            return denied(
                CommandPolicyCode::DeniedGitReferenceDelete,
                "deleting git references is blocked by XCoding policy",
            );
        }
    }

    if matches_extra_denylist(&exe, args, extra_denylist) {
        return denied(
            CommandPolicyCode::DeniedWorkspaceDenylist,
            format!("command `{exe}` is blocked by workspace command denylist"),
        );
    }

    // Tool calls must stay silent. `CREATE_NO_WINDOW` only covers the direct
    // child, so a wrapper that hands the launch to the OS (`start`,
    // `Start-Process`) pops a visible terminal or an error dialog that the tools
    // layer cannot suppress, and detaches the payload from process-tree cleanup.
    if let Some(token) = detached_window_request(&exe, &args_lower) {
        return denied(
            CommandPolicyCode::DeniedDetachedWindow,
            format!(
                "`{token}` detaches the program from the tool call and can open a console or an error dialog that cannot be suppressed; run the program directly instead"
            ),
        );
    }

    // High-risk helpers still require approval.
    if matches!(
        exe.as_str(),
        "curl" | "wget" | "ssh" | "scp" | "sftp" | "ftp" | "nc" | "ncat" | "netcat"
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
        if args_lower.iter().any(|arg| arg == "rebase" || arg == "--amend") {
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

    if matches!(exe.as_str(), "npm" | "pnpm" | "yarn" | "npx")
        && package_install_or_script_command(&args_lower)
    {
        return high_risk(
            CommandPolicyCode::HighRiskPackageInstall,
            format!(
                "package installation or lifecycle execution via `{exe}` requires explicit approval"
            ),
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

fn package_install_or_script_command(args: &[String]) -> bool {
    args.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "install"
                | "i"
                | "add"
                | "ci"
                | "update"
                | "upgrade"
                | "exec"
                | "dlx"
                | "postinstall"
                | "preinstall"
                | "prepare"
        )
    }) || (args.iter().any(|arg| arg == "run" || arg == "run-script")
        && args.iter().any(|arg| {
            matches!(arg.as_str(), "postinstall" | "preinstall" | "prepare")
        }))
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
            return Err(format!("{kind} patterns support at most one ':' separator"));
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

/// Detects wrappers that detach the payload from the tool call.
///
/// Returns the offending token so the denial message can name it. `cmd`'s `start`
/// is refused in every form: even `start /B`, which asks for no new window, routes
/// a failed launch through `ShellExecuteEx`, and that shows a modal "cannot find
/// file" dialog inside the child. The tools layer cannot suppress that dialog, so
/// the call would sit on a visible popup until it times out.
///
/// `Start-Process -NoNewWindow` is exempt because that switch is what turns
/// `UseShellExecute` off, so a failed launch surfaces as a PowerShell error
/// instead of a dialog.
fn detached_window_request(exe: &str, args_lower: &[String]) -> Option<&'static str> {
    if !matches!(exe, "cmd" | "powershell" | "pwsh") {
        return None;
    }
    // Arguments arrive either as a vector or as one packed `/C` / `-Command`
    // string, so flatten both shapes into comparable tokens.
    let tokens: Vec<&str> = args_lower
        .iter()
        .flat_map(|arg| {
            arg.split(|c: char| c.is_whitespace() || c == ';')
                .map(|token| token.trim_matches('"'))
                .filter(|token| !token.is_empty())
        })
        .collect();

    if exe == "cmd" {
        // `start` only launches a program from a command position: the first
        // token after /C or /K, or right after a chain operator. This keeps
        // `cmd /C echo start` from tripping the check.
        let mut command_position = true;
        for token in tokens {
            match token {
                "/c" | "/k" | "&&" | "||" | "&" | "|" => command_position = true,
                "start" if command_position => return Some("start"),
                _ if token.starts_with('/') => {}
                _ => command_position = false,
            }
        }
        return None;
    }

    if tokens.iter().any(|token| *token == "-nonewwindow") {
        return None;
    }
    tokens
        .iter()
        .any(|token| matches!(*token, "start-process" | "saps"))
        .then_some("Start-Process")
}

fn shell_wraps_destructive_delete(exe: &str, args_lower: &[String]) -> bool {
    if !matches!(exe, "cmd" | "powershell" | "pwsh") {
        return false;
    }

    // Arguments may be passed as separate argv entries or as one packed script
    // string. Tokenizing both forms catches quoted Windows paths without trying
    // to execute or fully emulate either shell.
    let tokens: Vec<String> = args_lower
        .iter()
        .flat_map(|arg| shell_tokens(arg))
        .collect();

    let mut command_position = true;
    for token in tokens {
        if matches!(token.as_str(), "/c" | "/k" | "-command" | "-c" | "&&" | "||") {
            command_position = true;
            continue;
        }
        if matches!(token.as_str(), "&" | "|" | ";") {
            command_position = true;
            continue;
        }
        if token.starts_with('-') || (exe == "cmd" && token.starts_with('/')) {
            continue;
        }
        if command_position
            && matches!(
                token.trim_start_matches(".\\"),
                "del" | "erase" | "rd" | "rmdir" | "remove-item" | "ri" | "rm"
            )
        {
            return true;
        }
        command_position = false;
    }
    false
}

fn shell_tokens(value: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    let push_current = |tokens: &mut Vec<String>, current: &mut String| {
        if !current.is_empty() {
            tokens.push(std::mem::take(current));
        }
    };

    for ch in value.chars() {
        match ch {
            '"' | '\'' => {}
            ch if ch.is_whitespace() => {
                push_current(&mut tokens, &mut current);
            }
            ch if matches!(ch, '&' | '|' | ';') => {
                push_current(&mut tokens, &mut current);
                tokens.push(ch.to_string());
            }
            ch => current.push(ch),
        }
    }
    push_current(&mut tokens, &mut current);
    tokens
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
        let normalized = arg
            .trim()
            .trim_matches(['"', '\''])
            .replace('/', "\\")
            .to_ascii_lowercase();
        matches!(normalized.as_str(), "/" | "\\" | "/*" | "\\*" | "c:")
            || is_windows_drive_root(&normalized)
    })
}

fn targets_absolute_path(args: &[String]) -> bool {
    args.iter().any(|arg| {
        let path = arg.trim().trim_matches(['"', '\'']);
        if path.starts_with('/') {
            // `/s`, `/q`, and similar forms are rmdir/del switches, not paths.
            // A longer slash-prefixed token is still an absolute POSIX-style
            // path and is blocked for portability.
            return path == "/" || path.len() > 2;
        }
        looks_absolute(path) || is_windows_drive_root(&path.to_ascii_lowercase())
    })
}

fn is_windows_drive_root(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(&path[2..], "" | "\\" | "\\*")
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

fn targets_path_traversal(args: &[String]) -> bool {
    args.iter().any(|arg| {
        let path = arg.trim().trim_matches(['"', '\'']).replace('\\', "/");
        path.split('/').any(|component| component == "..")
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

fn git_push_deletes_remote_ref(args: &[String]) -> bool {
    let Some(push_index) = args.iter().position(|arg| arg == "push") else {
        return false;
    };
    args.iter().skip(push_index + 1).any(|arg| {
        arg == "--delete" || arg == "-d" || (arg.starts_with(':') && arg.len() > 1)
    })
}

fn git_history_rewrite_is_irreversible(args: &[String]) -> bool {
    args.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "filter-branch" | "filter-repo" | "gc" | "prune" | "repack" | "replace"
        )
    }) || (args.iter().any(|arg| arg == "reflog")
        && args.iter().any(|arg| arg == "expire" || arg == "delete"))
}

fn git_deletes_reference(args: &[String]) -> bool {
    args.iter().any(|arg| {
        matches!(arg.as_str(), "update-ref" | "branch" | "tag")
    }) && args.iter().any(|arg| {
        arg == "-d" || arg == "-D" || arg == "--delete" || arg == "--expire=now"
    })
}

fn git_forced_worktree_delete(args: &[String]) -> bool {
    args.iter().any(|arg| {
        matches!(arg.as_str(), "worktree" | "submodule")
    }) && args.iter().any(|arg| arg == "--force" || arg == "-f")
}

fn git_file_delete_target_traverses_parent(args: &[String]) -> bool {
    let Some(command) = args.iter().find(|arg| {
        matches!(
            arg.as_str(),
            "rm" | "checkout" | "restore" | "clean" | "worktree" | "submodule"
        )
    }) else {
        return false;
    };
    let destructive = matches!(command.as_str(), "rm" | "checkout" | "restore" | "clean");
    if !destructive {
        return args.iter().any(|arg| {
            let path = arg.replace('\\', "/");
            path.split('/').any(|component| component == "..")
        });
    }
    args.iter().any(|arg| {
        let path = arg.trim_matches(['"', '\'']).replace('\\', "/");
        !path.starts_with('-') && path.split('/').any(|component| component == "..")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_file_writes_are_allowed_in_both_modes() {
        assert_eq!(
            evaluate(&Mode::AutoEdit, PermissionKind::Write, false),
            PermissionDecision::Allow
        );
        assert_eq!(
            evaluate(&Mode::Ask, PermissionKind::Write, false),
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
        assert!(matches!(
            normalize_allowlist_pattern("git:--version"),
            Ok(_)
        ));
        let assessment = assess_command_with_extra("git", &["--version".to_owned()], &extra);
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
        let assessment = assess_command_with_lists("rg", &["TODO".to_owned()], &allow, &deny);
        assert_eq!(assessment.decision, PermissionDecision::Deny);
        assert_eq!(assessment.code, CommandPolicyCode::DeniedWorkspaceDenylist);
        assert!(!assessment.allowlisted);
    }

    #[test]
    fn parse_command_denylist_accepts_shells() {
        let parsed = parse_command_denylist("# x\npowershell\ngit:push\n");
        assert_eq!(parsed, vec!["powershell".to_owned(), "git:push".to_owned()]);
        assert!(normalize_denylist_pattern("bash").is_ok());
    }

    #[test]
    fn full_auto_allows_low_risk_but_still_confirms_high_risk_operations() {
        assert_eq!(
            evaluate(&Mode::FullAuto, PermissionKind::Write, false),
            PermissionDecision::Allow
        );
        assert_eq!(
            evaluate(&Mode::FullAuto, PermissionKind::Write, true),
            PermissionDecision::AskUser
        );
        assert_eq!(
            evaluate_detailed(&Mode::FullAuto, PermissionKind::Exec, false, false),
            PermissionDecision::Allow
        );
        assert_eq!(
            evaluate_detailed(&Mode::FullAuto, PermissionKind::Exec, true, false),
            PermissionDecision::AskUser
        );
        assert_eq!(
            evaluate(&Mode::FullAuto, PermissionKind::Read, false),
            PermissionDecision::Allow
        );
    }

    #[test]
    fn full_auto_still_denies_network_and_hard_denied_commands() {
        assert_eq!(
            evaluate(&Mode::FullAuto, PermissionKind::Network, false),
            PermissionDecision::Deny
        );
        for (exe, args) in [
            ("format", vec!["C:".to_owned()]),
            ("git", vec!["clean".to_owned(), "-fdx".to_owned()]),
            (
                "git",
                vec!["push".to_owned(), "--mirror".to_owned()],
            ),
        ] {
            let assessment = assess_command(exe, &args);
            assert_eq!(
                assessment.decision,
                PermissionDecision::Deny,
                "{exe} should stay hard-denied"
            );
        }
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
    fn denies_shell_wrapped_recursive_deletes_before_approval() {
        let denied: [(&str, &[&str]); 9] = [
            ("cmd", &["/c", "rmdir", "/s", "/q", r#"E:\target folder"#]),
            ("cmd", &["/c", r#""rmdir /s /q E:\target folder""#]),
            ("cmd", &["/c", "rd /s /q E:\\"]),
            (
                "powershell",
                &[
                    "-Command",
                    "Remove-Item",
                    "E:\\target",
                    "-Recurse",
                    "-Force",
                ],
            ),
            (
                "pwsh",
                &["-Command", r#"Remove-Item 'E:\target folder' -Recurse"#],
            ),
            (
                "powershell",
                &[
                    "-Command",
                    r#"Remove-Item -LiteralPath 'E:\target' -Recurse"#,
                ],
            ),
            ("cmd", &["/c", "echo ok&&rmdir /s /q E:\\target"]),
            ("cmd", &["/c", "echo ok|rmdir /s /q E:\\target"]),
            (
                "powershell",
                &["-Command", "Write-Output ok; Remove-Item E:\\target -Recurse"],
            ),
        ];
        for (exe, args) in denied {
            let args: Vec<String> = args.iter().map(|value| (*value).to_owned()).collect();
            let assessment = assess_command(exe, &args);
            assert_eq!(
                assessment.decision,
                PermissionDecision::Deny,
                "{exe} {args:?}"
            );
            assert_eq!(
                assessment.code,
                CommandPolicyCode::DeniedShellDestructiveDelete,
                "{exe} {args:?}"
            );
        }
    }

    #[test]
    fn recognizes_any_windows_drive_root_for_recursive_delete() {
        for root in [r#"E:\"#, "F:/", "g:", r#"H:\*"#] {
            let assessment = assess_command(
                "rmdir",
                &["/s".to_owned(), "/q".to_owned(), root.to_owned()],
            );
            assert_eq!(assessment.decision, PermissionDecision::Deny, "{root}");
            assert_eq!(
                assessment.code,
                CommandPolicyCode::DeniedRecursiveRootDelete
            );
        }
    }

    #[test]
    fn denies_direct_absolute_file_deletes_but_keeps_relative_paths_available() {
        for (exe, target) in [
            ("del", r#"E:\outside.txt"#),
            ("erase", r#"F:\outside.txt"#),
            ("rmdir", r#"E:\outside-folder"#),
            ("rd", r#"\\server\share\outside-folder"#),
        ] {
            let assessment = assess_command(exe, &[target.to_owned()]);
            assert_eq!(assessment.decision, PermissionDecision::Deny, "{exe} {target}");
            assert_eq!(assessment.code, CommandPolicyCode::DeniedAbsoluteDelete);
        }

        let relative = assess_command("rmdir", &[r#".\cache"#.to_owned()]);
        assert_ne!(relative.code, CommandPolicyCode::DeniedAbsoluteDelete);
    }

    #[test]
    fn shell_without_delete_remains_high_risk_and_askable() {
        let assessment = assess_command("cmd", &["/c".to_owned(), "echo hello".to_owned()]);
        assert_eq!(assessment.decision, PermissionDecision::AskUser);
        assert_eq!(assessment.code, CommandPolicyCode::HighRiskShell);

        let literal = assess_command("cmd", &["/c".to_owned(), "echo rmdir".to_owned()]);
        assert_eq!(literal.decision, PermissionDecision::AskUser);
        assert_eq!(literal.code, CommandPolicyCode::HighRiskShell);
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
            &[
                "push".to_owned(),
                "--mirror".to_owned(),
                "origin".to_owned(),
            ],
        );
        assert_eq!(assessment.decision, PermissionDecision::Deny);
        assert_eq!(assessment.code, CommandPolicyCode::DeniedGitMirrorPush);
    }

    #[test]
    fn denies_git_destructive_operations_before_approval() {
        let denied = [
            ("git", vec!["clean", "-fd"], CommandPolicyCode::DeniedGitClean),
            (
                "git",
                vec!["push", "origin", "--delete", "main"],
                CommandPolicyCode::DeniedGitRemoteDelete,
            ),
            (
                "git",
                vec!["push", "origin", ":main"],
                CommandPolicyCode::DeniedGitRemoteDelete,
            ),
            (
                "git",
                vec!["update-ref", "-d", "refs/heads/main"],
                CommandPolicyCode::DeniedGitReferenceDelete,
            ),
            (
                "git",
                vec!["branch", "-D", "feature"],
                CommandPolicyCode::DeniedGitReferenceDelete,
            ),
            (
                "git",
                vec!["gc", "--prune=now"],
                CommandPolicyCode::DeniedGitHistoryRewrite,
            ),
            (
                "git",
                vec!["worktree", "remove", "--force", "../other"],
                CommandPolicyCode::DeniedGitForcedWorktreeDelete,
            ),
        ];
        for (exe, args, code) in denied {
            let assessment = assess_command(
                exe,
                &args.iter().map(|value| String::from(*value)).collect::<Vec<_>>(),
            );
            assert_eq!(assessment.decision, PermissionDecision::Deny, "{args:?}");
            assert_eq!(assessment.code, code, "{args:?}");
        }
    }

    #[test]
    fn denies_delete_parent_traversal() {
        for (exe, target) in [
            ("del", r#"..\outside.txt"#),
            ("rmdir", "../outside-folder"),
            ("rm", "../outside-folder"),
            ("git", r#"..\outside-file"#),
        ] {
            let args = if exe == "git" {
                vec!["rm".to_owned(), target.to_owned()]
            } else {
                vec![target.to_owned()]
            };
            let assessment = assess_command(exe, &args);
            assert_eq!(assessment.decision, PermissionDecision::Deny, "{exe} {target}");
            assert_eq!(assessment.code, CommandPolicyCode::DeniedDeletePathTraversal);
        }
    }

    #[test]
    fn denies_raw_disk_dd() {
        let assessment =
            assess_command("dd", &["if=/dev/zero".to_owned(), "of=/dev/sda".to_owned()]);
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
    fn package_install_and_lifecycle_commands_require_approval() {
        for args in [
            vec!["npm", "install"],
            vec!["pnpm", "add", "left-pad"],
            vec!["yarn", "upgrade"],
            vec!["npx", "exec", "some-tool"],
        ] {
            let executable = args[0];
            let assessment = assess_command(
                executable,
                &args[1..].iter().map(|value| (*value).to_owned()).collect::<Vec<_>>(),
            );
            assert_eq!(assessment.decision, PermissionDecision::AskUser, "{args:?}");
            assert!(assessment.high_risk, "{args:?}");
            assert_eq!(assessment.code, CommandPolicyCode::HighRiskPackageInstall);
        }
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

    #[test]
    fn allows_relative_build_output_executables() {
        for exe in [
            "./my-app",
            "target/release/etf-sentinel.exe",
            r"target\release\etf-sentinel.exe",
            r".\my-app.exe",
        ] {
            let assessment = assess_command(exe, &[]);
            assert_eq!(
                assessment.decision,
                PermissionDecision::AskUser,
                "{exe} should reach approval instead of a policy denial"
            );
            assert_eq!(assessment.code, CommandPolicyCode::RequiresApproval);
        }

        for exe in ["bin/tool", "../evil", r"..\evil", "target/../evil"] {
            assert_eq!(
                assess_command(exe, &[]).code,
                CommandPolicyCode::PathSeparator,
                "{exe} must stay denied"
            );
        }
    }

    #[test]
    fn denies_wrappers_that_request_a_detached_console() {
        let denied: [(&str, &[&str]); 8] = [
            ("cmd", &["/C", "start", "", "target\\release\\app.exe"]),
            (
                "cmd",
                &["/C", "cd", "/D", "D:\\repo", "&&", "start", "", "app.exe"],
            ),
            ("cmd", &["/C", "start \"\" app.exe"]),
            // `/B` asks for no new window, but a failed launch still lands on a
            // modal ShellExecute dialog that hangs the call.
            ("cmd", &["/C", "start", "/B", "app.exe"]),
            ("cmd", &["/C", "start /B app.exe"]),
            ("powershell", &["-Command", "Start-Process app.exe"]),
            ("powershell", &["-Command", "saps app.exe"]),
            ("pwsh", &["-Command", "Start-Process", "app.exe"]),
        ];
        for (exe, args) in denied {
            let args: Vec<String> = args.iter().map(|value| (*value).to_owned()).collect();
            let assessment = assess_command(exe, &args);
            assert_eq!(
                assessment.decision,
                PermissionDecision::Deny,
                "{exe} {args:?} should be denied"
            );
            assert_eq!(
                assessment.code,
                CommandPolicyCode::DeniedDetachedWindow,
                "{exe} {args:?} should be denied as a detached window request"
            );
        }

        // `-NoNewWindow` turns off `UseShellExecute`, so failures surface as a
        // PowerShell error; a literal `start` in a non-command position is not a
        // launch request.
        let allowed: [(&str, &[&str]); 3] = [
            ("cmd", &["/C", "echo", "start"]),
            ("powershell", &["-Command", "Start-Process -NoNewWindow app"]),
            ("cmd", &["/C", "ver"]),
        ];
        for (exe, args) in allowed {
            let args: Vec<String> = args.iter().map(|value| (*value).to_owned()).collect();
            let assessment = assess_command(exe, &args);
            assert_ne!(
                assessment.code,
                CommandPolicyCode::DeniedDetachedWindow,
                "{exe} {args:?} should not trip the detached window check"
            );
        }
    }
}
