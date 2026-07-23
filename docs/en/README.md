# XCoding Docs

This directory contains the V1 design and operating documentation for XCoding.

## Start Here

- [getting-started.md](./getting-started.md): installation, cloud authentication, CLI, and Desktop startup
- [session-safety.md](./session-safety.md): permission modes, approvals, rollback, cancellation, and task summaries
- [desktop.md](../desktop.md): Desktop workflow

## Design Documents

- [architecture.md](./architecture.md): system architecture and module boundaries
- [roadmap.md](./roadmap.md): phased delivery plan and acceptance criteria
- [protocol.md](./protocol.md): CLI/Desktop <-> Rust core protocol draft

## Locked Decisions

- Rust core + TypeScript shell
- CLI + simple Desktop first
- cloud models only in V1
- no editor extension in V1
- default mode: `ask`
- optional mode: `auto-edit`

## Current Status

Protocol, server, CLI, Desktop, write loop, and git tools are available. Task completion summaries include git status and diff snapshots when the workspace is a repository.

## Other Language

- Chinese docs: [../zh/README.md](../zh/README.md)