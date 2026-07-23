# Getting Started

XCoding is a local-first AI coding agent with a Rust core and CLI or Desktop clients. Version 1 uses cloud models only and supports the OpenAI-compatible provider named `openai`.

## Prerequisites

- Rust 1.97.0, selected by `rust-toolchain.toml`
- Node.js 22 or later
- pnpm 11 or later
- An API key for your OpenAI-compatible cloud service

## Install And Build

From the repository root:

```powershell
pnpm install
cargo build -p xcoding-server
pnpm build
```

For development, `pnpm cli -- ...` runs the CLI source build and starts `target/debug/xcoding-server` as its local stdio RPC server.


## Local .env And Launcher

You may place a repository-root `.env` file (gitignored; never commit real keys):

```env
OPENAI_API_KEY=...
XCODING_OPENAI_BASE_URL=https://ai.v58.dev/v1
```

The CLI, Desktop shell, and provider load this file when the corresponding variables are missing. **Existing process environment values always win.**

On Windows:

```powershell
.\scripts\xcoding.ps1 chat "Explain this repository"
.\scripts\xcoding.ps1 desktop
.\scripts\xcoding.ps1 acceptance
```
## Configure Cloud Access

Set credentials in the shell that starts XCoding:

```powershell
$env:OPENAI_API_KEY = "..."
$env:XCODING_OPENAI_BASE_URL = "https://ai.v58.dev/v1" # optional
```

`OPENAI_API_KEY` is required for model requests. `XCODING_OPENAI_BASE_URL` is optional and is useful for an OpenAI-compatible endpoint. XCoding never sends credentials through its RPC protocol and does not save them in the workspace, session database, or Desktop settings.

If chat fails with HTTP 401/403 or "OPENAI_API_KEY is not set", verify:

1. The shell that starts CLI/Desktop exports a valid `OPENAI_API_KEY` (or a repo-root `.env` file is present).
2. `XCODING_OPENAI_BASE_URL` points at your OpenAI-compatible `/v1` endpoint when you are not using the default.
3. The key is accepted by that endpoint (provider responses are truncated into the error message for diagnosis).

## Use The CLI

```powershell
pnpm cli -- ping --workspace .
pnpm cli -- config show --workspace .
pnpm cli -- config set --workspace . --mode auto-edit --model gpt-5.5
pnpm cli -- chat "Explain the structure of this repository" --workspace .
```

The CLI database is `<workspace>/.xcoding/xcoding.db`. Configuration stores mode, provider, and model for that workspace. Extra auto-edit command patterns live in `.xcoding/command-allowlist` (editable via `config set --command-allowlist` or Desktop defaults). New chats use those defaults unless a command explicitly supplies a different value.

## Use Desktop

Start the Tauri desktop app from a shell with the same credential variables:

```powershell
pnpm --filter @xcoding/desktop exec tauri dev
```

Desktop stores its database in the operating system application-data directory and keys configuration by workspace path. Its local history and settings are therefore separate from the CLI database.


## Project Rules

XCoding loads workspace rules into the system prompt (when present), in this order:

1. `AGENTS.md`
2. `XCoding.md`
3. `.xcoding/rules.md`

Keep these files short and actionable. Oversized rule files are truncated.

## Provider Auth Status

Check whether cloud credentials are visible to the server without making a model call:

```powershell
pnpm cli -- auth --workspace .
```

Desktop shows the same readiness state (ready / API key missing, base URL, masked key hint) in the left settings panel.

## Environment Doctor

Check workspace, server binary, core RPC, cloud credentials, workspace config, and git in one shot:

`powershell
pnpm cli -- doctor --workspace .
`

Prints JSON. Exit code is 2 when 
eady is false.

## Command Safety Policy

`run_command` is gated by mode, allowlist, and risk labels:

- **ask** — every command needs approval
- **auto-edit** — allowlisted safe developer commands auto-run (builtin plus `.xcoding/command-allowlist`); high-risk and non-allowlisted commands still need approval
- **Hard-denies** commands such as format, shutdown, git clean -fdx, and absolute executables
- **Flags high-risk** shells/network-style helpers such as powershell -Command, cmd /c, git push --force, and pnpm publish

Hard-denied commands never enter the approval queue; they return a tool error to the model.
Ordinary high-risk workspace writes under `.git` / `.xcoding` always need approval, even in auto-edit.

## Next Reading

- [Session Recovery And Safety](./session-safety.md)
- [Desktop](../desktop.md)
- [Protocol](./protocol.md)

## Continue a session

Follow up in an existing finished session (same id, shared history):

```powershell
pnpm cli -- chat "What about the CLI package?" --workspace . --session <session-id>
```

Desktop: select a finished session, then send another message (button shows **Continue**). Use **New chat** to start a fresh session.

