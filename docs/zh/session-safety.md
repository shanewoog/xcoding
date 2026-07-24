# 会话恢复与安全

XCoding 会把每个会话持久化到本地 SQLite 数据库：CLI 使用 `<workspace>/.xcoding/xcoding.db`，Desktop 使用应用数据目录。保存内容包括消息、工具事件、审批请求、恢复点、任务完成摘要与当前会话状态。

## 权限模式

默认模式是 `ask`。XCoding 可以自动读取工作区，但在每次写文件或执行命令前暂停。待审批动作和补丁预览都会保存下来，因此即使 CLI 或 Desktop 重启，也能继续审批。

`auto-edit` 会自动应用普通文件补丁。少量安全开发命令（例如 `cargo test`、`git status`、`pnpm test`）也可自动执行。非白名单命令、Shell 解释器，以及 `.git` / `.xcoding` 等高风险路径仍需审批。只应在允许 XCoding 修改的工作区中启用该模式。

## 工作区默认配置

每个工作区都有本地的模式、供应商和模型默认配置。V1 仅支持名为 `openai` 的 OpenAI 兼容云供应商，配置中不会包含任何凭据。

```powershell
xcoding config show --workspace <path>
xcoding config set --workspace <path> --mode ask --model gpt-5.5
xcoding config set --workspace <path> --mode auto-edit
```

CLI 将这些值保存到该工作区的 `.xcoding/xcoding.db`。Desktop 会在应用数据目录的数据库中按工作区路径保存自己的配置，因此当前不会与 CLI 共用数据库。

## 会话命令

```powershell
xcoding session list --workspace <path>
xcoding session show <session-id> --workspace <path>
xcoding session approve <session-id> <action-id> --workspace <path>
xcoding session reject <session-id> <action-id> --workspace <path>
xcoding session rollback <session-id> <restore-point-id> --workspace <path>
xcoding session cancel <session-id> --workspace <path>
```

`session show` 会以 JSON 输出已保存的会话详情，其中包含审批/拒绝需要的 action ID、回滚需要的 restore point ID，以及持久化的 `task_completed` 事件。任务完成摘要会列出唯一已修改文件（含 created/modified/deleted 分类）、基于恢复点估算的增删行数、成功和失败的命令数量；当工作区是 git 仓库时，还会附带完成时的 git_branch、git_status 与 git_diff 快照。`session summary <session-id>` 会以紧凑可读格式输出同一摘要。Desktop 可复制完整摘要或仅复制 git 快照。

## 回滚

每次成功应用补丁都会生成恢复点，记录原始文件内容和 XCoding 应用后的内容。回滚带有冲突保护：只有当前文件内容与 XCoding 当时写入的内容完全一致时，才会恢复。因此，之后由人工或其他工具产生的修改不会被覆盖。

若补丁创建了新文件，回滚会删除该文件。早于“应用后内容”存储机制的旧恢复点会被明确拒绝回滚。

Windows 上替换已有文件时，需要先删除目标文件，再重命名临时文件。XCoding 会在重命名失败时清理临时文件，但这一替换步骤在 Windows 上并非原子操作。

## 取消

`session cancel` 适用于状态为 `running` 或 `need_user` 的活跃会话。

- 等待审批的会话会被标记为已取消，未处理动作会被拒绝，之后也不能再审批执行。
- 进行中的模型流会协作式中断：agent 在读取 SSE 分片时轮询会话状态，并以 `status=cancelled` 结束。
- 正在运行的命令会被终止：`run_command` 在阻塞线程中执行，轮询取消探针，取消后杀掉子进程。
- stdio JSON-RPC server 在 `session.chat` / `session.resolve` 执行期间仍可接受 `session.cancel` 及其他短请求。

## MCP 工具

可选 MCP 服务器配置在 `.xcoding/mcp.json` 的 `mcpServers` 下。V1 仅支持 stdio JSON-RPC：每个 Agent 回合会启动已启用服务器，完成 `initialize` / `notifications/initialized` / `tools/list`，并以 `mcp__server__tool` 命名空间暴露给模型。

协议层工具名为 `mcp`，参数为 `{ "server", "tool", "arguments" }`。MCP 一律按高风险 `exec` 处理，在 `ask` 与 `auto-edit` 下都需要审批。服务器启动失败会写入系统提示警告；仅当 `mcp.json` 非法 JSON 时才会直接失败。

## 凭据

XCoding 不会把云模型凭据保存到仓库或会话数据库中。请通过环境变量配置 OpenAI 兼容供应商：

```powershell
$env:OPENAI_API_KEY = "..."
$env:XCODING_OPENAI_BASE_URL = "https://ai.v58.dev/v1" # 可选
```

`OPENAI_API_KEY` 仅保留在启动 CLI 或 Desktop 的进程环境中，RPC 协议不接受任何凭据字段。

## 命令策略

`run_command` 由硬拒绝名单、白名单、可选工作区名单与风险标注共同约束：

- **硬拒绝**：明显危险的系统命令（例如 `format`、`shutdown`、递归删除根路径、注册表机器范围修改、`git clean -fdx`、mirror push）。硬拒绝命令不会进入审批队列。
- **工作区黑名单**（`.xcoding/command-denylist`）：匹配模式一律拦截，即使同时出现在白名单中。
- Shell / force-push 等高风险调用始终需要审批，并在审批摘要中标注 **HIGH-RISK**（含结构化策略码）。
- 在 `ask` 模式下，其余命令仍全部需要审批。
- 在 `auto-edit` 模式下，仅白名单内的安全命令可自动执行，其他命令仍需审批。

白名单覆盖只读 `git` 查询、`cargo`/`go`/`dotnet` 构建测试类命令、包管理器的 `test`/`build`/`lint`/`exec`（不含 `publish`），以及 `tsc`、`pytest`。参数中含有 shell 元字符的调用不会进入白名单。

工作区文件：

- `.xcoding/command-allowlist` — 扩展内置白名单（`rg` 或 `git:--version`；每行一条；支持 `#` 注释）
- `.xcoding/command-denylist` — 拦截模式，即使在 `auto-edit` 下（可包含 shell；黑名单优先于白名单）

配置方式：

```powershell
xcoding config set --workspace <path> --command-allowlist "rg,git:--version"
xcoding config set --workspace <path> --command-denylist "cargo:--version,powershell"
```

Shell/解释器与破坏性系统命令不可加入白名单；包管理器的 `publish` 也始终保持受控。

策略拦截命令时，工具结果结构为：

- `code`：`command_policy_denied`
- `policy_code`：机器可读原因（例如 `denied_executable`、`denied_workspace_denylist`、`denied_git_clean`）
- `reason`：人类可读说明

Desktop 会用徽章、完整命令文本和更醒目的确认按钮突出高风险审批；CLI 会打印 HIGH-RISK 警告和完整命令行。

## 模式策略信号

任务进行中，工具活动摘要会标明策略判定：

- `Auto-applying apply_patch` — 在 `auto-edit` 下自动应用了普通写操作
- `Auto-running run_command` — 在 `auto-edit` 下自动执行了白名单命令
- `Awaiting approval for apply_patch` / `run_command` — 已暂停等待用户审批
- `Running ...` — 立即允许（只读，或已获准执行路径）
- `Blocked ...` — 被策略硬拒绝

`auto-edit` 下的普通补丁与白名单命令不会发出 `approval_requested`。非白名单命令、高风险命令，以及 `.git` / `.xcoding` 路径的写入仍需审批。

## 工作区 Skills

可选 skill 放在 `.xcoding/skills/<name>/SKILL.md`。XCoding 只在系统提示中列出名称与描述；完整说明在 Agent 调用只读工具 `load_skill` 时加载。
