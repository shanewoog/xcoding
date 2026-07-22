# Phase 0 - Repository and Protocol Skeleton

## Included

- Cargo workspace for the Rust core modules
- pnpm workspace for CLI, Desktop, protocol, and client packages
- Shared JSON-RPC 2.0 request/response contracts in Rust and TypeScript
- SQLite-backed `session.create` and `session.list`
- `xcoding-server` line-delimited JSON-RPC process over stdio
- CLI commands for `ping`, creating sessions, and listing sessions
- Tauri Desktop shell that verifies access to the Rust core

## Validation

```powershell
cargo test
cargo build -p xcoding-server
pnpm install
pnpm build
pnpm cli -- ping
pnpm cli -- session create --workspace . --title "First XCoding session"
pnpm cli -- session list --workspace .
```

## Deliberately Deferred

- model gateway and streaming events
- repository tools
- agent loop
- permission prompts
- Desktop JSON-RPC sidecar lifecycle
