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

## Configure Cloud Access

Set credentials in the shell that starts XCoding:

```powershell
$env:OPENAI_API_KEY = "..."
$env:XCODING_OPENAI_BASE_URL = "https://api.openai.com/v1" # optional
```

`OPENAI_API_KEY` is required for model requests. `XCODING_OPENAI_BASE_URL` is optional and is useful for an OpenAI-compatible endpoint. XCoding never sends credentials through its RPC protocol and does not save them in the workspace, session database, or Desktop settings.

## Use The CLI

```powershell
pnpm cli -- ping --workspace .
pnpm cli -- config show --workspace .
pnpm cli -- config set --workspace . --mode auto-edit --model gpt-4.1
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