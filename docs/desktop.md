# XCoding Desktop

## Run

Start the Tauri app:

```powershell
pnpm --filter @xcoding/desktop exec tauri dev
# or from repo root after PATH is set:
pnpm desktop
```

Preferred: open **Settings** in the app and set Base URL + API key, then **Save settings**.
Values are written to `~/.xcoding/config.json` (Windows: `%USERPROFILE%\.xcoding\config.json`).

Optional overrides still work at process start (existing env wins; the file only fills missing values):

```powershell
$env:OPENAI_API_KEY = "..."
$env:XCODING_OPENAI_BASE_URL = "https://ai.v58.dev/v1"
```

Session database lives at `~/.xcoding/xcoding.db`. Workspace command policy files stay under each project's `.xcoding/`.

## First Workflow

1. Open **Settings** and choose **Language** (English or 简体中文).
2. Configure **Cloud provider**: Base URL (default `https://ai.v58.dev/v1`) and API key, then **Save settings**.
3. Set workspace path, mode, and model (allow/deny lists need a workspace path).
4. Return to the workbench, enter a real absolute workspace path if needed, and send a task.
5. Review the plan, streamed response, tool activity, patch previews, and approval controls.
6. Select saved sessions to review events, restore points, and task completion summary.

Desktop uses the same guarded agent service as the CLI. The default mode is `ask`; `auto-edit` applies ordinary file patches and allowlisted safe commands automatically. High-risk writes and non-allowlisted commands still require approval.

## Settings page

All configuration lives on the **Settings** page (button in the left panel and chat header):

| Section | What it stores |
|---------|----------------|
| Language | UI locale; also written to `~/.xcoding/config.json` and mirrored in `localStorage` (`xcoding.locale`) |
| Cloud provider | Provider (`openai`, read-only), Base URL, API key → `~/.xcoding/config.json` |
| Defaults | Workspace path (last-used), mode, model; allowlist/denylist → workspace `.xcoding/command-allowlist` / `command-denylist` |
| Diagnostics | Client checklist: workspace, provider auth, base URL, defaults |

Mode help:

- **ask** — propose patches and commands; both need approval
- **auto-edit** — apply ordinary file patches and allowlisted safe commands automatically; **high-risk writes and other commands still need approval**
- **Command allowlist** — optional workspace patterns (`exe` or `exe:subcommand`); shells/interpreters cannot be allowlisted
- **Command denylist** — optional workspace block patterns; denylist overrides allowlist and never auto-runs

**Diagnostics** is client-side only. Green means ready enough to start a task; use `pnpm cli -- doctor` for deeper server checks.

### Example `~/.xcoding/config.json`

```json
{
  "locale": "zh-CN",
  "mode": "ask",
  "provider": "openai",
  "model": "gpt-5.5",
  "base_url": "https://ai.v58.dev/v1",
  "api_key": "sk-...",
  "last_workspace_root": "D:\\WORK\\BittyData\\XCoding"
}
```

API keys are stored in plain text in the user home directory for v0.1 convenience. Do not commit this file.

## High-risk command review

When the agent proposes a shell-style or force-push command, Desktop shows a **HIGH-RISK** badge, the full command text, and a stronger approve button label. Hard-denied and denylisted commands never reach this panel; they fail as structured tool errors (`command_policy_denied` + `policy_code`) instead.

## Task completion summary

When a task finishes, Desktop shows a completion panel with changed files (created/modified/deleted), approximate `+/-` line counts, command success/failure counts, and optional git status/diff snapshots. Use **Copy summary** for the full text report, or **Copy git** for only the git snapshot.

## Multi-turn sessions

Select a finished session in the left list to review history. Sending another message continues that session (shared transcript and restore points). **New chat** clears the selection and starts a new task.

## Three-pane layout

| Pane | Content |
|------|---------|
| Left | Workspace path, compact provider status, Settings/Refresh, session history |
| Center | Conversation transcript (auto-scrolls), empty-state tips, composer; Settings also from the header |
| Right | Sticky approval review when needed, task summary, activity, then collapsible plan / restore / replay |

Session history items show status, mode, model, and a relative updated time. Message roles render as You / Assistant / Tool / System.

### Keyboard

- **Ctrl+Enter** (Windows/Linux) or **Cmd+Enter** (macOS) sends the composer message.
- **Send** stays enabled whenever a task is not running. Missing workspace path, empty message, or missing API key show an error (and footer hint) instead of greying the button out forever.

### Trace panel

When a session has no plan, activity, restore points, replay, or summary yet, the right pane shows a short empty Trace hint. Plan, Restore points, and Replay sections collapse when empty and expand when they have content. Pending approvals stay sticky at the top of the trace pane.


## Portable package (no installer)

No installer required. Build and package:

```powershell
pnpm desktop:portable
# or
.\scripts\package-desktop-portable.ps1
```

Output: `dist/portable/XCoding/`

1. Copy the whole folder anywhere
2. Open **Settings** on first launch and set API key + Base URL (or place a `.env` next to the exe)
3. Double-click `XCoding.exe`

Requires Windows 10/11 + WebView2 Runtime. Session database and user config live under `%USERPROFILE%\.xcoding\` (`xcoding.db` / `config.json`), not inside the portable folder.

### If you see "localhost refused to connect"

That binary is a **dev-mode** build (UI tries `http://localhost:1420`). Rebuild with:

```powershell
pnpm desktop:portable
```

Do not package a raw `cargo build --release` for portable use; use `tauri build` so `custom-protocol` embeds the UI.


### If the window is blank / no UI

1. Use the package from `pnpm desktop:portable` (`dist/portable/XCoding/XCoding.exe`), not a raw `cargo build --release` binary.
2. Install or repair [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/).
3. Kill all `XCoding` processes, then delete the WebView profile and reopen:
   `%LOCALAPPDATA%\com.shanewoog.xcoding\EBWebView`
4. Production frontend assets must be relative (`./assets/...`). The repo Vite config sets `base: './'`.

