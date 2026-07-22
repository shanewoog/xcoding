# XCoding 协议草案

## 1. 目的

本文定义 V1 本地协议，连接：

- TypeScript 客户端（`apps/cli`、`apps/desktop`）
- Rust 核心（`xcoding-server`）

目标：

- 一套核心，多种壳层
- 可流式推送 Agent 事件
- 显式权限决策
- 稳定的会话回放

V1 传输方式：

- CLI 使用 stdio JSON-RPC
- Desktop 使用本地 WebSocket JSON-RPC

所有载荷均为 JSON。

## 2. 约定

### 2.1 请求/响应

采用 JSON-RPC 2.0 风格：

```json
{
  "jsonrpc": "2.0",
  "id": "1",
  "method": "session.create",
  "params": {}
}
```

成功：

```json
{
  "jsonrpc": "2.0",
  "id": "1",
  "result": {}
}
```

错误：

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

### 2.2 服务端推送事件

使用 notification：

```json
{
  "jsonrpc": "2.0",
  "method": "event",
  "params": {
    "session_id": "ses_123",
    "event": {
      "type": "text_delta",
      "delta": "hello"
    }
  }
}
```

客户端按 session 订阅并渲染事件流。

### 2.3 ID

- `session_id`：`ses_...`
- `message_id`：`msg_...`
- `tool_call_id`：`tc_...`
- `decision_id`：`dec_...`
- `event_id`：单调递增字符串或 UUID

## 3. 核心类型

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

## 4. RPC 方法

## 4.1 Health

### `system.ping`

请求参数：

```json
{}
```

结果：

```json
{
  "ok": true,
  "version": "0.1.0"
}
```

## 4.2 Auth / Config

### `config.get`

结果示例：

```json
{
  "mode": "ask",
  "provider": "openai",
  "model": "gpt-4.1",
  "permissions": {
    "write": "confirm",
    "exec": "confirm",
    "network_tools": "deny"
  }
}
```

### `config.set`

参数示例：

```json
{
  "mode": "auto-edit",
  "model": "gpt-4.1"
}
```

### `auth.setProviderKey`

参数：

```json
{
  "provider": "openai",
  "api_key": "sk-..."
}
```

结果：

```json
{
  "ok": true,
  "provider": "openai"
}
```

说明：开发工作流优先使用环境变量。Desktop 后续可用系统钥匙串保存密钥；V1 可保持最小实现。

## 4.3 Sessions

### `session.create`

参数：

```json
{
  "workspace_root": "D:/work/demo",
  "mode": "ask",
  "provider": "openai",
  "model": "gpt-4.1",
  "title": "Add health check"
}
```

结果：

```json
{
  "session": {
    "id": "ses_123",
    "workspace_root": "D:/work/demo",
    "mode": "ask",
    "provider": "openai",
    "model": "gpt-4.1",
    "status": "created",
    "created_at": "2026-07-22T08:00:00Z",
    "updated_at": "2026-07-22T08:00:00Z",
    "title": "Add health check"
  }
}
```

### `session.list`

结果：

```json
{
  "sessions": []
}
```

### `session.get`

参数：

```json
{
  "session_id": "ses_123"
}
```

### `session.cancel`

参数：

```json
{
  "session_id": "ses_123"
}
```

### `session.replay`

参数：

```json
{
  "session_id": "ses_123"
}
```

结果：

```json
{
  "session": {},
  "events": []
}
```

## 4.4 任务执行

### `session.prompt`

提交用户任务或追问。

参数：

```json
{
  "session_id": "ses_123",
  "message": "Add a /health endpoint and tests",
  "attachments": []
}
```

结果：

```json
{
  "accepted": true,
  "message_id": "msg_1"
}
```

调用后，客户端消费流式 `event` 通知。

### `session.decide`

响应用户权限确认或澄清请求。

参数：

```json
{
  "session_id": "ses_123",
  "decision_id": "dec_1",
  "decision": "allow",
  "note": "looks good"
}
```

`decision` 取值：

- `allow`
- `deny`
- `allow_always_for_session`（可选，V1.x）
- `submit_text`：用于澄清回答

澄清示例：

```json
{
  "session_id": "ses_123",
  "decision_id": "dec_2",
  "decision": "submit_text",
  "text": "Use the existing axum router"
}
```

## 5. 事件模型

所有流式事件包装为：

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

### 5.2 事件载荷

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

`applied`：

- `false`：等待批准，或仅提议
- `true`：成功应用后

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

## 6. 工具契约

## 6.1 `list_dir`

输入：

```json
{
  "path": "src",
  "max_entries": 200
}
```

输出：

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

输入：

```json
{
  "path": "src/main.rs",
  "start_line": 1,
  "end_line": 200
}
```

输出：

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

输入：

```json
{
  "query": "Router::new",
  "path": ".",
  "max_results": 50
}
```

输出：

```json
{
  "results": [
    {
      "path": "src/main.rs",
      "line": 42,
      "text": "let app = Router::new()"
    }
  ]
}
```

## 6.4 `apply_patch`

输入：

```json
{
  "patch": "*** Begin Patch\n*** Add File: src/routes/health.rs\n+pub async fn health() -> &'static str { \"ok\" }\n*** End Patch"
}
```

输出：

```json
{
  "applied": true,
  "changed_files": ["src/routes/health.rs"]
}
```

## 6.5 `run_command`

输入：

```json
{
  "command": "cargo test",
  "cwd": ".",
  "timeout_ms": 120000
}
```

输出：

```json
{
  "exit_code": 0,
  "stdout": "...",
  "stderr": "...",
  "timed_out": false
}
```

## 6.6 `git_status` / `git_diff`

只读辅助工具，用于上下文与最终汇总。

## 7. 权限评估规则

执行工具前：

1. 判断权限类别
2. 检查模式
3. 检查路径限制 / 命令策略
4. 自动允许、自动拒绝，或发出 `permission_request`
5. 需要时等待 `session.decide`
6. 执行或跳过
7. 发出 `tool_end`

### 模式矩阵

| 类别 | `ask` | `auto-edit` |
|---|---|---|
| read | 自动允许 | 自动允许 |
| write | 需确认 | 非高风险时自动允许 |
| exec | 需确认 | 需确认 |
| network tools | 拒绝 | 拒绝 |

## 8. 错误码

建议区间：

- `1000-1099` session/config 错误
- `1100-1199` auth/provider 错误
- `1200-1299` tool 错误
- `1300-1399` policy 错误
- `1400-1499` internal 错误

示例：

| 代码 | 含义 |
|---|---|
| 1001 | workspace not found |
| 1002 | session not found |
| 1101 | missing provider api key |
| 1201 | patch conflict |
| 1202 | command timeout |
| 1301 | permission denied |
| 1400 | internal error |

## 9. 客户端渲染预期

### CLI

- 打印流式文本
- 展示计划步骤
- 对权限决策发起确认
- 紧凑渲染 diff
- 展示最终汇总与退出码

### Desktop

- 追加聊天气泡
- 渲染计划清单
- 收到 `diff` 时打开 diff 视图
- 对 `permission_request` 弹出确认面板
- 用 tool/status 事件构建时间线
- 按事件顺序回放会话

## 10. 兼容性规则

V1 规则：

1. 客户端遇到未知事件类型时必须忽略，不能崩溃
2. 服务端在可能时应忽略未知工具字段
3. 优先做字段增量，不轻易做破坏性重命名
4. status 与 mode 枚举应保持稳定
5. 协议变更必须同步更新本文档

## 11. 最小成功路径

1. 客户端调用 `system.ping`
2. 客户端调用 `session.create`
3. 客户端调用 `session.prompt`
4. 服务端发出 `status=running`
5. 服务端发出 `plan`
6. 服务端发出只读 `tool_start` / `tool_end`
7. 在 `ask` 模式下发出 `diff` + `permission_request`
8. 客户端调用 `session.decide allow`
9. 服务端应用补丁并发出 `tool_end`
10. 服务端可能请求执行测试命令
11. 服务端发出 `final status=done`

## 12. 实现阶段可再定事项

以下事项不阻塞当前文档冻结，可在脚手架阶段决定：

- Desktop 密钥的具体存储机制
- Desktop 仅用 WebSocket，还是同时支持 sidecar stdio
- 补丁格式最终选择：
  - 自定义 begin/end patch
  - 或严格 unified diff
- 首个可运行版本是否包含 `allow_always_for_session`

## 其他语言

- English: [../en/protocol.md](../en/protocol.md)
