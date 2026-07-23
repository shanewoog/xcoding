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

The CLI database is `<workspace>/.xcoding/xcoding.db`. Configuration stores only the selected mode, provider, and model for that workspace. New chats use those defaults unless a command explicitly supplies a different value.

## Use Desktop

Start the Tauri desktop app from a shell with the same credential variables:

```powershell
pnpm --filter @xcoding/desktop exec tauri dev
```

Desktop stores its database in the operating system application-data directory and keys configuration by workspace path. Its local history and settings are therefore separate from the CLI database.

## Next Reading

- [Session Recovery And Safety](./session-safety.md)
- [Desktop](../desktop.md)
- [Protocol](./protocol.md)