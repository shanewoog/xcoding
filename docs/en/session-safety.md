# Session Recovery And Safety

XCoding persists each session in the local SQLite database at `<workspace>/.xcoding/xcoding.db` for the CLI and in the application data directory for Desktop. A saved session includes messages, tool events, approval requests, restore points, the task completion summary, and the current session status.

## Permission Modes

`ask` is the default. XCoding reads the workspace automatically, but pauses before every patch or command. The pending action and its patch preview are stored, so approval can continue after the CLI or Desktop restarts.

`auto-edit` applies ordinary file patches without a prompt. A small allowlist of safe developer commands (for example `cargo test`, `git status`, `pnpm test`) may also auto-run. Non-allowlisted commands, shell interpreters, and high-risk paths such as `.git` and `.xcoding` still require approval. Use this mode only for a workspace you are ready to let XCoding modify.

## Workspace Defaults

Each workspace has local defaults for mode, provider, and model. Only the `openai` OpenAI-compatible cloud provider is available in V1. The defaults contain no credentials.

```powershell
xcoding config show --workspace <path>
xcoding config set --workspace <path> --mode ask --model gpt-5.5
xcoding config set --workspace <path> --mode auto-edit
```

The CLI stores these values in that workspace's `.xcoding/xcoding.db`. Desktop stores its own workspace-keyed values in its application-data database, so its settings do not currently share a database with the CLI.

## Session Commands

```powershell
xcoding session list --workspace <path>
xcoding session show <session-id> --workspace <path>
xcoding session approve <session-id> <action-id> --workspace <path>
xcoding session reject <session-id> <action-id> --workspace <path>
xcoding session rollback <session-id> <restore-point-id> --workspace <path>
xcoding session cancel <session-id> --workspace <path>
```

`session show` prints stored session detail as JSON. It includes action IDs needed for approval or rejection, restore point IDs needed for rollback, and the persisted `task_completed` event. The completion summary reports unique changed files with created/modified/deleted classification, approximate line add/remove counts from restore points, successful or failed command counts, and when the workspace is a git repository, optional git_branch, git_status, and git_diff snapshots captured at completion. `session summary <session-id>` prints the same completion summary in a compact human-readable form. Desktop can copy the full summary or just the git snapshot.

## Rollback

Every successful patch creates a restore point containing the original and applied file content. Rollback is conflict-protected: XCoding restores a file only when its current content exactly matches the content that XCoding applied. It therefore refuses to overwrite a subsequent human or tool edit.

A restore point for a newly created file removes that file during rollback. Older restore points that predate applied-content storage are intentionally not rollbackable.

On Windows, replacing an existing file requires deleting the destination before renaming the temporary file. XCoding cleans up its temporary file when a rename fails, but that replacement step is not atomic on Windows.

## Cancellation

`session cancel` works for active sessions in `running` or `need_user` state.

- Approval-paused sessions are marked cancelled, outstanding actions are rejected, and later approval is blocked.
- In-flight model streams are interrupted cooperatively: the agent polls session status while reading SSE chunks and exits with `status=cancelled`.
- Running commands are killed: `run_command` executes off the async runtime, polls the cancel probe, and terminates the child process when cancelled.
- The stdio JSON-RPC server accepts `session.cancel` (and other short requests) while `session.chat` / `session.resolve` are in progress.

## Credentials

XCoding does not store cloud credentials in the repository or its session database. Configure the OpenAI-compatible provider through environment variables:

```powershell
$env:OPENAI_API_KEY = "..."
$env:XCODING_OPENAI_BASE_URL = "https://ai.v58.dev/v1" # optional
```

`OPENAI_API_KEY` stays in the environment of the CLI or Desktop process. The RPC protocol accepts no credential fields.

## Command Policy

`run_command` is gated by a strict allowlist plus risk labels:

- Hard deny for clearly destructive system commands (for example `format`, `shutdown`, `git clean -fdx`).
- High-risk shells and force-push style invocations always need approval and are labeled **HIGH-RISK** in the approval summary.
- Under `ask`, every remaining command still needs approval.
- Under `auto-edit`, only allowlisted safe commands auto-run; everything else still needs approval.

Allowlisted families include read-only `git` inspection, `cargo`/`go`/`dotnet` build-test helpers, package-manager `test`/`build`/`lint`/`exec` (not `publish`), plus `tsc` and `pytest`. Arguments containing shell metacharacters are never allowlisted.

Workspace file `.xcoding/command-allowlist` can extend the builtin list with patterns such as `rg` or `git:--version` (one per line; `#` comments allowed). Configure via:

```powershell
xcoding config set --workspace <path> --command-allowlist "rg,git:--version"
```

Shells/interpreters and destructive system commands cannot be allowlisted. `publish` package-manager invocations also stay gated.

Desktop highlights high-risk approvals with a badge, the rendered command, and a stronger confirm action; the CLI prints a HIGH-RISK warning plus the full command line.

## Mode policy signals

During a task, tool activity summaries show how policy decided:

- `Auto-applying apply_patch` — ordinary write auto-ran under `auto-edit`
- `Auto-running run_command` — allowlisted command auto-ran under `auto-edit`
- `Awaiting approval for apply_patch` / `run_command` — paused for user review
- `Running ...` — allowed immediately (reads, or approved execution path)
- `Blocked ...` — hard-denied by policy

Ordinary patches and allowlisted commands under `auto-edit` never emit `approval_requested`. Non-allowlisted commands, high-risk commands, and writes under `.git` / `.xcoding` still require approval.

