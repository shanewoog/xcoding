# XCoding Roadmap

## 1. Goal

Ship a usable local AI coding agent with:

- Rust core
- TypeScript CLI
- simple Desktop shell
- cloud models only
- default permission mode `ask`

V1 is successful when a user can complete real repository tasks with reviewable plans, diffs, command execution, and replayable traces.

## 2. Product Boundaries

### V1 includes

- workspace-aware agent sessions
- cloud model gateway
- read/search/write/exec tools
- `ask` and `auto-edit`
- CLI complete workflow
- simple Desktop workflow
- SQLite trace storage
- patch preview / apply / reject
- basic rollback

### V1 excludes

- editor extensions
- local model runtime integration
- multi-agent teams
- MCP ecosystem completeness
- hosted multi-user product

## 3. Milestone Plan

## Phase 0 - Skeleton

Duration target: about 1 week

### Deliverables

- Cargo workspace for Rust crates
- JS/TS workspace for apps and packages
- `xcoding-server` boots
- protocol package exists on Rust and TS sides
- CLI can connect to core
- Desktop empty shell can connect to core
- `ping` / health RPC works
- session create/list stubs work

### Exit criteria

- CLI and Desktop both create a session against one local core
- no business logic duplicated in TS shell

### Verification

- manual: start server, run CLI health command, open Desktop and connect
- automated: protocol serialization tests

## Phase 1 - Read-Only Agent

Duration target: about 1 week

### Deliverables

- OpenAI-compatible provider integration
- streaming text events
- tools:
  - `list_dir`
  - `read_file`
  - `search_code`
- project rules loading
- basic context assembly
- session persistence for messages and events

### Exit criteria

- user can ask repository questions and get grounded answers with file references
- stream is visible in both CLI and Desktop

### Verification

- task: "Where is auth handled in this repo?"
- task: "Summarize the module boundary of X"
- assert cited paths exist and content is relevant

## Phase 2 - Write Loop

Duration target: about 1.5 to 2 weeks

### Deliverables

- `apply_patch`
- diff events
- confirmation flow for `ask`
- `run_command`
- command output back into context
- failure recovery loop
- restore point before mutations

### Exit criteria

- agent can implement a small feature or bugfix with tests
- user can approve or reject writes
- rejected writes do not leave partial corruption

### Verification

- task: add a health endpoint and tests
- task: fix a known failing unit test
- task: reject a patch and continue
- task: cancel a running session safely

## Phase 3 - Productization

Duration target: about 1.5 to 2 weeks

### Deliverables

- Desktop 3-pane UX:
  - sessions
  - chat/plan
  - diff/trace
- `auto-edit` mode
- session replay
- config UI / config commands
- change summary at end of task
- e2e task suite
- docs for install, auth, modes, and safety

### Exit criteria

- CLI and Desktop behavior match on the same task
- user can switch `ask` / `auto-edit`
- user can replay a finished session
- core demos are reliable on sample repos

### Verification

- run the V1 acceptance task set end to end
- compare CLI and Desktop traces for the same prompt
- verify rollback after a completed mutation task

## Phase 4 - V1.x Hardening

Duration target: continuous after V1

### Candidates

- better relevant-file retrieval (Wave R: search options + workspace sketch; embeddings later)
- stronger command policy engine
- more cloud providers
- skills system
- MCP support
- better patch confidence and conflict UX
- richer git workflows (Wave S: structured `git_log` + `git_show`; Wave T: approved `git_add` + `git_commit`; push later)
- performance work on large repos

### Still later

- VS Code extension
- JetBrains extension
- local model support
- multi-agent review/implement split

## 4. V1 Acceptance Task Set

These tasks define "done" better than feature checklists.

1. Explain a module in a sample repo using only read tools
2. Add a small feature and complementary tests
3. Fix a bug with reproduce-then-repair behavior
4. Refactor a function without changing behavior
5. In `ask` mode, require confirmation before write and exec
6. In `auto-edit` mode, auto-apply ordinary writes and allowlisted commands; still confirm high-risk / non-allowlisted exec
7. Reject a proposed patch and ensure workspace stays correct
8. Cancel a running task and persist cancelled state
9. Replay a session and reconstruct major steps
10. Run the same task from CLI and Desktop with equivalent results

## 5. Engineering Order

Recommended implementation sequence:

1. protocol and event model
2. server + client connection
3. session store
4. model gateway streaming
5. read tools
6. agent loop for read-only Q&A
7. patch engine
8. confirmation / policy flow
9. command tool
10. Desktop review UX
11. auto-edit
12. replay and polish

## 6. Quality Gates

Every phase should add tests in this order:

1. unit tests for pure logic
2. tool contract tests
3. policy tests
4. session/event persistence tests
5. end-to-end task tests on fixture repos

Definition of a completed feature:

- implementation exists
- verification path exists
- failure behavior is defined
- trace output is useful

## 7. Delivery Recommendation

Near-term execution order:

1. Freeze docs in `docs/`
2. Scaffold monorepo
3. Implement protocol + server skeleton
4. Get CLI chat working read-only
5. Only then invest in Desktop UX depth

Desktop should lag slightly behind core capability, not lead it.

## 8. Exit Definition for V1 Launch

V1 can be called launchable when:

- install and API key setup are documented and work
- one cloud provider is stable
- `ask` is default and reliable
- `auto-edit` is available and clearly explained
- CLI can finish real tasks
- Desktop can finish the same class of tasks
- traces and diffs are reviewable
- rollback path exists
- acceptance task set mostly passes on sample repositories

## Other Language

- Chinese: [../zh/roadmap.md](../zh/roadmap.md)
