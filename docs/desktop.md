# XCoding Desktop

## Run

Set cloud-model credentials in your shell, then start the Tauri app:

```powershell
$env:OPENAI_API_KEY = "..."
# Optional for OpenAI-compatible cloud providers.
$env:XCODING_OPENAI_BASE_URL = "https://ai.v58.dev/v1"
pnpm --filter @xcoding/desktop exec tauri dev
```

The app stores local session history and workspace defaults in its operating-system application-data directory. Credentials are read from the environment only and are not written to the session database.

## First Workflow

1. Enter the absolute workspace path in the left panel.
2. Choose the mode and model defaults, then save them for that workspace.
3. Send a repository request from the composer.
4. Review the plan, streamed response, tool activity, patch previews, and approval controls.
5. Select saved sessions to review their events, restore points, and task completion summary.

Desktop uses the same guarded agent service as the CLI. The default mode is `ask`; `auto-edit` applies ordinary file patches automatically, while commands still require approval.

## High-risk command review

When the agent proposes a shell-style or force-push command, Desktop shows a **HIGH-RISK** badge, the full command text, and a stronger approve button label. Hard-denied commands never reach this panel; they fail as tool errors instead.

## Task completion summary

When a task finishes, Desktop shows a completion panel with changed files (created/modified/deleted), approximate `+/-` line counts, command success/failure counts, and optional git status/diff snapshots. Use **Copy summary** for the full text report, or **Copy git** for only the git snapshot.

## Multi-turn sessions

Select a finished session in the left list to review history. Sending another message continues that session (shared transcript and restore points). **New chat** clears the selection and starts a new task.

## Three-pane layout

| Pane | Content |
|------|---------|
| Left | Workspace path, auth status, model defaults, and scrollable session history with status badges |
| Center | Conversation transcript (auto-scrolls), empty-state pane map, and composer |
| Right | Sticky approval review when needed, task summary, activity, then collapsible plan / restore / replay |

Session history items show status, mode, model, and a relative updated time. Message roles render as You / Assistant / Tool / System.

### Keyboard

- **Ctrl+Enter** (Windows/Linux) or **Cmd+Enter** (macOS) sends the composer message.

### Trace panel

When a session has no plan, activity, restore points, replay, or summary yet, the right pane shows a short empty Trace hint. Plan, Restore points, and Replay sections collapse when empty and expand when they have content. Pending approvals stay sticky at the top of the trace pane.
