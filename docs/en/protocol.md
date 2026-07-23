# XCoding Protocol Draft

## 1. Purpose

This document defines the V1 local protocol between:

- TypeScript clients (`apps/cli`, `apps/desktop`)
- Rust core (`xcoding-server`)

Goals:

- one core, multiple shells
- streamable agent events
- explicit permission decisions
- stable session replay

Transport options for V1:

- stdio JSON-RPC for CLI
- local WebSocket JSON-RPC for Desktop

All payloads are JSON.

## 2. Conventions

### 2.1 Request/Response

JSON-RPC 2.0 style:

```json
{
  "jsonrpc": "2.0",
  "id": "1",
  "method": "session.create",
  "params": {}
}
```

Success:

```json
{
  "jsonrpc": "2.0",
  "id": "1",
  "result": {}
}
```

Error:

```json
{
  "jsonrpc": "2.0",
  "id": "1",
  "error": {
    "code": 1001,
    "message": "workspace not found",
    "data": {}
  }
}
```

### 2.2 Server Push Events

Use notifications. Phases 1A and 1B emit `session.event` directly on the same transport as the request:

```json
{
  "jsonrpc": "2.0",
  "method": "session.event",
  "params": {
    "type": "text_delta",
    "session_id": "550e8400-e29b-41d4-a716-446655440000",
    "delta": "hello"
  }
}
```

Clients must ignore unknown notification methods and event types so fields and events can be added compatibly.

### 2.3 IDs

- `session_id`: `ses_...`
- `message_id`: `msg_...`
- `tool_call_id`: `tc_...`
- `decision_id`: `dec_...`
- `event_id`: monotonic string or UUID

## 3. Core Types

### 3.1 Mode

```ts
type Mode = "ask" | "auto-edit"
```

### 3.2 SessionStatus

```ts
type SessionStatus =
  | "created"
  | "running"
  | "need_user"
  | "done"
  | "failed"
  | "cancelled"
```

### 3.3 PermissionKind

```ts
type PermissionKind = "read" | "write" | "exec" | "network"
```

### 3.4 Session

```ts
interface Session {
  id: string
  workspace_root: string
  mode: Mode
  provider: string
  model: string
  status: SessionStatus
  created_at: string
  updated_at: string
  title?: string
}
```

### 3.5 Message

```ts
interface Message {
  id: string
  session_id: string
  role: "system" | "user" | "assistant" | "tool"
  content: string
  created_at: string
}
```

## 4. RPC Methods

## 4.1 Health

### `system.ping`

Request params:

```json
{}
```

Result:

```json
{
  "ok": true,
  "version": "0.1.0"
}
```

## 4.2 Auth / Config

### `config.get`

Result example:

```json
{
  "mode": "ask",
  "provider": "openai",
  "model": "gpt-5.5",
  "permissions": {
    "write": "confirm",
    "exec": "confirm",
    "network_tools": "deny"
  }
}
```

### `config.set`

Params example:

```json
{
  "mode": "auto-edit",
  "model": "gpt-5.5"
}
```

### `auth.setProviderKey`

Params:

```json
{
  "provider": "openai",
  "api_key": "sk-..."
}
```

Result:

```json
{
  "ok": true,
  "provider": "openai"
}
```

Note: clients should prefer environment variables in developer workflows. Desktop may store secrets via OS keychain later; V1 can keep this minimal.

## 4.3 Sessions

### `session.create`

Params:

```json
{
  "workspace_root": "D:/work/demo",
  "mode": "ask",
  "provider": "openai",
  "model": "gpt-5.5",
  "title": "Add health check"
}
```

Result:

```json
{
  "session": {
    "id": "ses_123",
    "workspace_root": "D:/work/demo",
    "mode": "ask",
    "provider": "openai",
    "model": "gpt-5.5",
    "status": "created",
    "created_at": "2026-07-22T08:00:00Z",
    "updated_at": "2026-07-22T08:00:00Z",
    "title": "Add health check"
  }
}
```

### `session.list`

Result:

```json
{
  "sessions": []
}
```

### `session.chat` (Phase 1B)

Starts a new cloud-model chat session, persists the user message, streams assistant text, then persists the completed assistant message. The initial implementation supports `provider: "openai"` through the OpenAI-compatible Chat Completions SSE endpoint. Phase 1B exposes the read-only `list_dir`, `read_file`, and `search_code` tools to compatible models, persists each tool result, and streams the execution trace.

Params:

```json
{
  "workspace_root": "D:/work/demo",
  "message": "Summarize this repository",
  "mode": "ask",
  "provider": "openai",
  "model": "gpt-5.5",
  "title": "Repository summary"
}
```

No credential fields are accepted. The server reads `OPENAI_API_KEY` from its environment and optionally reads `XCODING_OPENAI_BASE_URL`; the latter defaults to `https://ai.v58.dev/v1`.

The server emits these `session.event` payloads in order:

```ts
type SessionEvent =
  | { type: "text_delta"; session_id: string; delta: string }
  | { type: "message_completed"; session_id: string; message: Message }
  | { type: "plan"; session_id: string; steps: Array<{ id: string; description: string }> }
  | { type: "tool_start"; session_id: string; tool_call: ToolCall; summary: string }
  | { type: "tool_end"; session_id: string; tool_call: ToolCall; success: boolean; summary: string }
  | { type: "error"; session_id: string; message: string }
```

Result:

```json
{
  "session": { "id": "550e8400-e29b-41d4-a716-446655440000", "status": "done" },
  "message": {
    "id": "4ea8c0cc-79ce-4d4f-94f9-13f8bc77597a",
    "session_id": "550e8400-e29b-41d4-a716-446655440000",
    "role": "assistant",
    "content": "...",
    "created_at": "2026-07-22T08:00:00Z"
  }
}
```
### `session.get`

Params:

```json
{
  "session_id": "ses_123"
}
```

### `session.cancel`

Params:

```json
{
  "session_id": "ses_123"
}
```

### `session.replay`

Params:

```json
{
  "session_id": "ses_123"
}
```

Result:

```json
{
  "session": {},
  "events": []
}
```

## 4.4 Task Execution

### `session.prompt`

Submit a user task or follow-up.

Params:

```json
{
  "session_id": "ses_123",
  "message": "Add a /health endpoint and tests",
  "attachments": []
}
```

Result:

```json
{
  "accepted": true,
  "message_id": "msg_1"
}
```

After this call, clients consume streamed `event` notifications.

### `session.decide`

Respond to a permission or clarification request.

Params:

```json
{
  "session_id": "ses_123",
  "decision_id": "dec_1",
  "decision": "allow",
  "note": "looks good"
}
```

`decision` values:

- `allow`
- `deny`
- `allow_always_for_session` (optional V1.x)
- `submit_text` for clarification answers

Clarification example:

```json
{
  "session_id": "ses_123",
  "decision_id": "dec_2",
  "decision": "submit_text",
  "text": "Use the existing axum router"
}
```

## 5. Event Model

All stream events are wrapped as:

```ts
interface EventEnvelope {
  session_id: string
  event_id: string
  timestamp: string
  event: AgentEvent
}
```

### 5.1 AgentEvent

```ts
type AgentEvent =
  | TextDeltaEvent
  | MessageEvent
  | PlanEvent
  | StatusEvent
  | ToolStartEvent
  | ToolEndEvent
  | DiffEvent
  | PermissionRequestEvent
  | UserInputRequestEvent
  | UsageEvent
  | ErrorEvent
  | FinalEvent
```

### 5.2 Event Payloads

#### `text_delta`

```json
{
  "type": "text_delta",
  "delta": "I will inspect the router next."
}
```

#### `message`

```json
{
  "type": "message",
  "message_id": "msg_2",
  "role": "assistant",
  "content": "I will inspect the router next."
}
```

#### `plan`

```json
{
  "type": "plan",
  "steps": [
    "Locate HTTP router",
    "Add /health endpoint",
    "Add test",
    "Run tests"
  ]
}
```

#### `status`

```json
{
  "type": "status",
  "status": "running"
}
```

#### `tool_start`

```json
{
  "type": "tool_start",
  "tool_call_id": "tc_1",
  "tool": "read_file",
  "permission": "read",
  "input": {
    "path": "src/main.rs"
  }
}
```

#### `tool_end`

```json
{
  "type": "tool_end",
  "tool_call_id": "tc_1",
  "tool": "read_file",
  "ok": true,
  "output": {
    "path": "src/main.rs",
    "content": "..."
  }
}
```

#### `diff`

```json
{
  "type": "diff",
  "tool_call_id": "tc_2",
  "path": "src/routes/health.rs",
  "kind": "create",
  "patch": "@@\n+pub async fn health() -> &'static str {\n+    \"ok\"\n+}\n",
  "applied": false
}
```

`applied`:

- `false` when awaiting approval or only proposed
- `true` after successful apply

#### `permission_request`

```json
{
  "type": "permission_request",
  "decision_id": "dec_1",
  "kind": "write",
  "summary": "Create src/routes/health.rs",
  "tool": "apply_patch",
  "tool_call_id": "tc_2",
  "details": {
    "paths": ["src/routes/health.rs"]
  }
}
```

#### `user_input_request`

```json
{
  "type": "user_input_request",
  "decision_id": "dec_2",
  "prompt": "Which web framework should I modify?",
  "options": ["axum", "actix"]
}
```

#### `usage`

```json
{
  "type": "usage",
  "input_tokens": 1200,
  "output_tokens": 350,
  "total_tokens": 1550
}
```

#### `error`

```json
{
  "type": "error",
  "code": "patch_conflict",
  "message": "Failed to apply patch to src/main.rs"
}
```

#### `final`

```json
{
  "type": "final",
  "status": "done",
  "summary": "Added /health endpoint and tests passed"
}
```

## 6. Tool Contracts

Phase 1B implements only the three read-only tools below. Every path is workspace-relative, is canonicalized before use, and must remain inside the workspace. Directory traversal skips `.git`, `.xcoding`, `node_modules`, `target`, and symlinks. `apply_patch` and `run_command` remain Phase 2 contracts.

## 6.1 `list_dir`

Input:

```json
{
  "path": "src",
  "max_entries": 200
}
```

Output:

```json
{
  "path": "src",
  "entries": [
    { "name": "main.rs", "kind": "file" },
    { "name": "routes", "kind": "dir" }
  ]
}
```

## 6.2 `read_file`

Input:

```json
{
  "path": "src/main.rs",
  "start_line": 1,
  "end_line": 200
}
```

Output:

```json
{
  "path": "src/main.rs",
  "content": "...",
  "start_line": 1,
  "end_line": 200,
  "truncated": false
}
```

## 6.3 `search_code`

Search workspace text files for a string. Optional: `case_insensitive`, simple `glob` (`*` / `?`), and `context_lines` (0–3). Source-like paths rank higher; common build directories are skipped.

Input:

```json
{
  "query": "Router::new",
  "path": ".",
  "max_results": 50,
  "case_insensitive": false,
  "glob": "*.rs",
  "context_lines": 1
}
```

Output:

```json
{
  "results": [
    {
      "path": "src/main.rs",
      "line": 42,
      "text": "let app = Router::new()",
      "before": ["// route builder"],
      "after": ["    .route(\"/health\", get(health))"]
    }
  ],
  "truncated": false
}
```

## 6.4 `apply_patch`

Input:

```json
{
  "patch": "*** Begin Patch\n*** Add File: src/routes/health.rs\n+pub async fn health() -> &'static str { \"ok\" }\n*** End Patch"
}
```

Output:

```json
{
  "applied": true,
  "changed_files": ["src/routes/health.rs"]
}
```

## 6.5 `run_command`

Input:

```json
{
  "command": "cargo test",
  "cwd": ".",
  "timeout_ms": 120000
}
```

Output:

```json
{
  "exit_code": 0,
  "stdout": "...",
  "stderr": "...",
  "timed_out": false
}
```

## 6.6 `git_status` / `git_diff` / `git_log` / `git_show` / `git_add` / `git_commit` / `git_push`

Read-only helpers for context, history, and final summaries.

### `git_status`

Input:

```json
{
  "path": "src"
}
```

`path` is optional and workspace-relative.

Output:

```json
{
  "path": "src",
  "branch": "main...origin/main",
  "entries": [
    { "kind": "branch", "branch": "main...origin/main" },
    { "kind": "entry", "index_status": " ", "worktree_status": "M", "path": "src/lib.rs" }
  ],
  "raw": "..."
}
```

### `git_diff`

Input:

```json
{
  "path": "src/lib.rs"
}
```

Output:

```json
{
  "path": "src/lib.rs",
  "staged": "",
  "unstaged": "diff --git a/src/lib.rs b/src/lib.rs\n..."
}
```

### `git_log`

Input:

```json
{
  "max_count": 20,
  "path": "src/lib.rs"
}
```

`max_count` defaults to 20 and is capped at 50. `path` is optional.

Output:

```json
{
  "path": "src/lib.rs",
  "max_count": 20,
  "commits": [
    {
      "hash": "…",
      "short_hash": "abc1234",
      "author": "Name",
      "email": "name@example.com",
      "date": "2026-07-22T12:00:00+08:00",
      "subject": "Improve retrieval",
      "body": ""
    }
  ],
  "raw": "abc1234 Improve retrieval (Name, 2026-07-22T12:00:00+08:00)"
}
```

### `git_show`

Input:

```json
{
  "revision": "HEAD",
  "path": "src/lib.rs"
}
```

`revision` is required (commit-ish). `path` is optional and workspace-relative.

Output:

```json
{
  "revision": "HEAD",
  "path": "src/lib.rs",
  "hash": "…",
  "short_hash": "abc1234",
  "author": "Name",
  "email": "name@example.com",
  "date": "2026-07-22T12:00:00+08:00",
  "subject": "Improve retrieval",
  "body": "",
  "patch": "diff --git a/src/lib.rs b/src/lib.rs\n...",
  "raw": "..."
}
```

### `git_add`

Stage workspace-relative paths. Always classified as **high-risk write** (mutates `.git`), so both `ask` and `auto-edit` require approval.

Input:

```json
{
  "paths": ["src/lib.rs", "README.md"]
}
```

`paths` is required and must be a non-empty array of workspace-relative paths. Absolute paths, `..`, and `.git` / `.xcoding` path segments are rejected.

Output:

```json
{
  "paths": ["src/lib.rs", "README.md"],
  "success": true,
  "stdout": "",
  "stderr": ""
}
```

### `git_commit`

Create a commit with a message. Always classified as **high-risk write**. No amend / `--no-verify` / force flags in this version.

Input:

```json
{
  "message": "Fix workspace retrieval ranking",
  "allow_empty": false
}
```

`message` is required (non-empty after trim). `allow_empty` defaults to false.

Output:

```json
{
  "message": "Fix workspace retrieval ranking",
  "subject": "Fix workspace retrieval ranking",
  "hash": "abc123…",
  "allow_empty": false,
  "stdout": "[main abc123] Fix workspace retrieval ranking\n…",
  "stderr": ""
}
```



### `git_push`

Push a branch to a remote. Always classified as **high-risk write** (updates remote refs). Both `ask` and `auto-edit` require approval. This version never force-pushes.

Input:

```json
{
  "remote": "origin",
  "branch": "main",
  "set_upstream": false
}
```

- `remote` optional; defaults to `origin`. Must be a single remote name (no leading `-`, no whitespace, no `:` / `..`).
- `branch` optional; defaults to the current branch. Detached HEAD requires an explicit branch.
- `set_upstream` optional; defaults to false (`git push --set-upstream` when true).

Output:

```json
{
  "remote": "origin",
  "branch": "main",
  "set_upstream": false,
  "head": "abc123…",
  "success": true,
  "stdout": "…",
  "stderr": "…"
}
```

Auth / network failures surface as tool errors via git stderr (for example missing credentials or rejected non-fast-forward).

## 7. Permission Evaluation Rules

Before executing a tool. In Phase 1B, only read tools are executable, so both `ask` and `auto-edit` auto-allow them:

1. Determine permission kind
2. Check mode
3. Check path confinement / command policy
4. Auto-allow, auto-deny, or emit `permission_request`
5. Wait for `session.decide` when needed
6. Execute or skip
7. Emit `tool_end`

### Mode matrix

| Kind | `ask` | `auto-edit` |
|---|---|---|
| read | auto-allow | auto-allow |
| write | confirm | auto-allow unless high-risk |
| exec | confirm | confirm |
| network tools | deny | deny |

## 8. Error Codes

Suggested ranges:

- `1000-1099` session/config errors
- `1100-1199` auth/provider errors
- `1200-1299` tool errors
- `1300-1399` policy errors
- `1400-1499` internal errors

Examples:

| Code | Meaning |
|---|---|
| 1001 | workspace not found |
| 1002 | session not found |
| 1101 | missing provider api key |
| 1201 | patch conflict |
| 1202 | command timeout |
| 1301 | permission denied |
| 1400 | internal error |

## 9. Client Rendering Expectations

### CLI

- print streamed text
- show plan steps
- prompt for permission decisions
- render compact diffs
- show final summary and status exit code

### Desktop

- append chat messages
- render plan checklist
- open diff viewer on `diff`
- modal/panel for `permission_request`
- timeline from tool/status events
- replay by replaying stored events in order

## 10. Compatibility Rules

V1 rules:

1. Unknown event types must be ignored by clients without crashing
2. Unknown tool fields must be ignored by servers when possible
3. Additive fields are preferred over breaking renames
4. Status and mode enums should stay stable
5. Protocol changes require docs update in this file

## 11. Minimal Happy Path

1. Client calls `system.ping`
2. Client calls `session.create`
3. Client calls `session.prompt`
4. Server emits `status=running`
5. Server emits `plan`
6. Server emits read-only `tool_start` / `tool_end`
7. Server emits `diff` + `permission_request` in `ask` mode
8. Client calls `session.decide allow`
9. Server applies patch and emits `tool_end`
10. Server may request exec permission for tests
11. Server emits `final status=done`

## 12. Open Points for Implementation

These can be decided during scaffolding without blocking docs:

- exact secret storage mechanism for Desktop
- whether Desktop uses WebSocket only or also embedded sidecar stdio
- patch format finalization:
  - custom begin/end patch
  - or strict unified diff only
- whether `allow_always_for_session` ships in first runnable build

## Other Language

- Chinese: [../zh/protocol.md](../zh/protocol.md)
