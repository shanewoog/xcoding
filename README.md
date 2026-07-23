# XCoding

XCoding is a local-first AI coding agent platform: a Rust core, a JSON-RPC stdio server, a TypeScript CLI, and a thin Tauri Desktop shell. Version 1 uses cloud models only (OpenAI-compatible), defaults to `ask` mode, and supports optional `auto-edit` for ordinary patches.

XCoding 是本地优先的 AI 编程 Agent 平台：Rust 核心、JSON-RPC stdio 服务、TypeScript CLI 与轻量 Tauri Desktop 壳。V1 仅接云模型（OpenAI-compatible），默认 `ask`，可选 `auto-edit` 自动应用普通补丁。

## Prerequisites

- Rust 1.97.0 (managed by `rust-toolchain.toml`)
- Node.js 22+
- pnpm 11+
- OpenAI-compatible API key

## Build And Verify

```powershell
$env:Path = "D:\WORK\Npm;" + $env:Path
pnpm install
cargo build -p xcoding-server
pnpm build
cargo test -p xcoding-protocol -p xcoding-core -p xcoding-agent -p xcoding-tools
pnpm test:e2e
```

## Quick Start

```powershell
$env:OPENAI_API_KEY = "..."
$env:XCODING_OPENAI_BASE_URL = "https://ai.v58.dev/v1"  # optional
$env:Path = "D:\WORK\Npm;" + $env:Path

pnpm cli -- ping --workspace .
pnpm cli -- chat "Explain this repository" --workspace .
pnpm desktop
```

The CLI launches `target/debug/xcoding-server` as a local stdio JSON-RPC process. On task completion, the summary includes changed files, command counts, and a git branch/status/diff snapshot when the workspace is a git repository.

## Current Capability

- Read tools: `list_dir`, `read_file`, `search_code`
- Write / exec: `apply_patch`, `run_command` (approval-gated in `ask`; patches auto-apply in `auto-edit`)
- Git tools: `git_status`, `git_diff` (read-only)
- Session safety: approval, rollback, cancel, replay, task summary
- Surfaces: CLI + Desktop (same protocol)

## Documentation

- [English](./docs/en/README.md)
- [中文](./docs/zh/README.md)
- [Getting started](./docs/en/getting-started.md)
- [Session safety](./docs/en/session-safety.md)
