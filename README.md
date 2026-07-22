# XCoding

XCoding is a local-first AI coding agent platform. The Phase 0 skeleton establishes one Rust core, a JSON-RPC protocol, a CLI client, and a thin Tauri Desktop shell.

XCoding 是一个本地优先的 AI 编程 Agent 平台。当前 Phase 0 骨架已建立 Rust 核心、JSON-RPC 协议、CLI 客户端与轻量 Tauri Desktop 壳。

## Prerequisites

- Rust 1.97.0 (managed by `rust-toolchain.toml`)
- Node.js 22+
- pnpm 11+

## Verify the Skeleton

```powershell
pnpm install
pnpm build
cargo test
pnpm cli -- ping
pnpm cli -- session create --workspace . --title "First XCoding session"
pnpm cli -- session list --workspace .
```

The CLI starts `target/debug/xcoding-server` as a local stdio JSON-RPC process. Build the Rust server first with `cargo build -p xcoding-server` when running the CLI directly.

## Documentation

- [English](./docs/en/README.md)
- [中文](./docs/zh/README.md)
