# XCoding Architecture

## 1. Product Definition

XCoding is a local-first AI coding agent platform.

It helps users complete coding tasks inside a local workspace by:

1. Understanding repository context
2. Planning work
3. Calling tools under a permission policy
4. Producing and applying code patches
5. Running commands when allowed
6. Recording a full execution trace for review and replay

XCoding is not a full IDE and not an editor plugin in V1.

### V1 Scope

In scope:

- CLI as the full-capability entrypoint
- Simple Desktop app as a thin visual shell
- Rust core for agent runtime, tools, policy, storage, and model access
- TypeScript shell for UX
- Cloud model providers only
- Permission modes: `ask` (default) and `auto-edit`

Out of scope for V1:

- VS Code / JetBrains extensions
- Local model runtimes
- Complex multi-agent orchestration
- Cloud collaboration / multi-tenant SaaS
- Custom vector database as a hard dependency

## 2. Locked Decisions

| Topic | Decision |
|---|---|
| Core language | Rust |
| Shell language | TypeScript |
| First surfaces | CLI + simple Desktop |
| Model strategy | Cloud providers only |
| Editor plugins | Not in V1 |
| Default autonomy | `ask` |
| Optional autonomy | `auto-edit` |
| Source of truth | Rust core only |

## 3. High-Level Architecture

```text
+--------------------------------------------------+
| TypeScript Shell                                 |
|                                                  |
|  apps/cli         full capability UX             |
|  apps/desktop     chat / plan / diff / trace     |
|  packages/ui      shared presentation            |
|  packages/client  RPC client                     |
+--------------------------+-----------------------+
                           | JSON-RPC
                           | stdio | local socket | websocket
+--------------------------v-----------------------+
| Rust Core                                        |
|                                                  |
|  session manager                                 |
|  agent loop                                      |
|  context engine                                  |
|  tool runtime                                    |
|  policy engine                                   |
|  patch engine                                    |
|  model gateway                                   |
|  trace / store                                   |
+--------------------------+-----------------------+
                           |
         +-----------------+------------------+
         v                 v                  v
  workspace fs        cloud LLMs         OS commands
  git / rg            providers          (policy gated)
```

### Design Principle

CLI and Desktop are clients only.

They must not implement a second agent runtime.
All planning, tool execution, policy checks, patch application, and session persistence happen in Rust.

## 4. Repository Layout

```text
XCoding/
  apps/
    cli/                 # TypeScript CLI
    desktop/             # TypeScript Desktop shell
  crates/
    xcoding-core/        # agent loop, orchestration
    xcoding-tools/       # fs/search/patch/shell/git tools
    xcoding-policy/      # permission decisions
    xcoding-providers/   # cloud model providers
    xcoding-context/     # rules, retrieval, summarization
    xcoding-store/       # sqlite sessions and events
    xcoding-protocol/    # shared protocol types
    xcoding-server/      # local RPC server binary
  packages/
    protocol/            # TS protocol types
    client/              # TS RPC client
    ui/                  # shared UI pieces
  configs/
  docs/
  examples/
  tests/
    e2e/
```

## 5. Runtime Components

### 5.1 Session Manager

Owns:

- session identity
- workspace binding
- message history
- mode and model settings
- lifecycle state

Session states:

- `created`
- `running`
- `need_user`
- `done`
- `failed`
- `cancelled`

### 5.2 Agent Loop

Recommended V1 loop:

```text
user goal
  -> build context
  -> request model step
  -> validate model output
  -> if tool call:
       policy check
       execute or ask user
       append observation
       continue
  -> if final answer:
       mark done
```

The loop must be deterministic enough to replay from stored events.

### 5.3 Context Engine

Context layers injected into the model:

1. System role and tool contracts
2. Project rules (`AGENTS.md` and/or `.xcoding/rules.md`)
3. User goal
4. Relevant file excerpts
5. Recent trace summary
6. Current errors / test output / rejected diffs

V1 retrieval strategy:

- file tree heuristics
- path / symbol hints from user text
- `rg` search
- recent touched files

No mandatory embedding index in V1.

### 5.4 Tool Runtime

V1 tools:

| Tool | Permission | Purpose |
|---|---|---|
| `list_dir` | read | inspect workspace structure |
| `read_file` | read | read file contents |
| `search_code` | read | built-in text search with optional glob/context |
| `apply_patch` | write | apply unified diff / patch |
| `run_command` | exec | run tests or build commands |
| `git_status` | read | inspect workspace git state |
| `git_diff` | read | inspect local changes |
| `git_log` | read | inspect recent commit history |
| `git_show` | read | show one revision metadata and patch |
| `git_add` | write (high-risk) | stage workspace paths (always requires approval) |
| `git_commit` | write (high-risk) | create a commit (always requires approval) |

Tool requirements:

- strict JSON schema input
- structured result
- timeout support
- cancellation support
- redaction for secrets when needed

### 5.5 Policy Engine

Permission classes:

- `read`
- `write`
- `exec`
- `network`

Modes:

#### `ask` (default)

- read tools auto-allow
- write requires confirmation
- exec requires confirmation
- non-model network tools deny by default

#### `auto-edit`

- read tools auto-allow
- write auto-allow inside workspace policy
- exec still requires confirmation
- high-risk writes still require confirmation

High-risk examples:

- deleting many files
- modifying `.env` / credential files
- changing git config or hooks
- commands with destructive potential

`full-auto` may exist as an internal enum but is not a supported V1 product mode.

### 5.6 Patch Engine

Rules:

1. Prefer patch application over whole-file overwrite
2. Always emit a reviewable diff event before or when applying
3. Detect apply conflicts
4. Support reject / partial reject where practical
5. Create a restore point before mutating workspace state

Restore strategy for V1:

- prefer git snapshot when repo is clean enough
- otherwise file-level backup snapshot for touched paths

### 5.7 Model Gateway

V1 provider strategy:

- implement a provider-agnostic interface
- ship OpenAI-compatible provider first
- optionally add Anthropic second

Required capabilities:

- chat completions
- streaming tokens
- tool/function calls
- usage accounting

Local models are explicitly out of V1 scope.

### 5.8 Trace and Store

Storage: SQLite

Persist:

- sessions
- messages
- plans
- tool calls and tool results
- diffs / patches
- command logs
- token usage
- final status

Trace is the backbone for:

- Desktop timeline
- debugging agent behavior
- session replay
- auditability

## 6. Client Surfaces

### 6.1 CLI

CLI is a first-class full client.

Suggested commands:

```bash
xcoding init
xcoding auth set
xcoding mode set ask|auto-edit
xcoding chat
xcoding run "<task>"
xcoding session list
xcoding session show <id>
xcoding session replay <id>
```

CLI responsibilities:

- argument parsing
- streaming event rendering
- confirmation prompts
- exit codes and scripting-friendly output

### 6.2 Desktop

Desktop is a thin shell over the same core.

Primary UI regions:

1. Session list
2. Chat + plan stream
3. Diff / files / command trace

Desktop responsibilities:

- workspace picker
- API key / model settings UI
- confirmation dialogs
- diff accept/reject actions
- session replay view

Desktop must call the same RPC methods as CLI.

## 7. Configuration

Project config example: `.xcoding/config.toml`

```toml
model = "gpt-5.5"
provider = "openai"
mode = "ask"
workspace = "."

[permissions]
write = "confirm"
exec = "confirm"
network_tools = "deny"

[providers.openai]
api_key_env = "OPENAI_API_KEY"
base_url = "https://ai.v58.dev/v1"
```

Global user config may live in the user config directory, with project config taking precedence for workspace-scoped values.

## 8. Security Model

Default posture: safe and explicit.

Controls:

- workspace path confinement
- command allow/deny policy
- secret file protection
- confirmation gates by mode
- traceability of every mutation
- no ambient unrestricted shell

Network policy:

- model provider calls allowed through model gateway
- tool-level network access denied by default in V1

## 9. Failure and Recovery

The system should degrade in controlled ways:

| Failure | Behavior |
|---|---|
| model stream interrupted | mark step failed, allow retry |
| patch conflict | do not partially corrupt file; report conflict |
| command timeout | capture partial output, mark tool failed |
| user rejects write | continue with rejection observation |
| user cancels session | stop tools, persist cancelled state |

## 10. Non-Goals for Architecture V1

- replacing git
- replacing the user's editor
- building a general automation OS
- guaranteeing perfect autonomous coding
- hiding actions from the user

XCoding should feel powerful, but inspectable.

## 11. Success Criteria for Architecture

Architecture is successful when:

1. One Rust core serves both CLI and Desktop
2. A coding task can complete with plan, tools, diff, and trace
3. Permission mode materially changes write behavior
4. Session replay can reconstruct what happened
5. Adding a new cloud provider does not require UI rewrites

## Other Language

- Chinese: [../zh/architecture.md](../zh/architecture.md)
