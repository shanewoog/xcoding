# 会话恢复与安全

XCoding 会把每个会话持久化到本地 SQLite 数据库：CLI 使用 `<workspace>/.xcoding/xcoding.db`，Desktop 使用应用数据目录。保存内容包括消息、工具事件、审批请求、恢复点、任务完成摘要与当前会话状态。

## 权限模式

默认模式是 `ask`。XCoding 可以自动读取工作区，但在每次写文件或执行命令前暂停。待审批动作和补丁预览都会保存下来，因此即使 CLI 或 Desktop 重启，也能继续审批。

`auto-edit` 会自动应用普通文件补丁。命令仍然必须审批，`.git`、`.xcoding` 等高风险路径依然受保护。只应在允许 XCoding 修改的工作区中启用该模式。

## 工作区默认配置

每个工作区都有本地的模式、供应商和模型默认配置。V1 仅支持名为 `openai` 的 OpenAI 兼容云供应商，配置中不会包含任何凭据。

```powershell
xcoding config show --workspace <path>
xcoding config set --workspace <path> --mode ask --model gpt-4.1
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

`session show` 会以 JSON 输出已保存的会话详情，其中包含审批/拒绝需要的 action ID、回滚需要的 restore point ID，以及持久化的 `task_completed` 事件。任务完成摘要会列出唯一的已修改文件，并统计成功和失败的命令数量。

## 回滚

每次成功应用补丁都会生成恢复点，记录原始文件内容和 XCoding 应用后的内容。回滚带有冲突保护：只有当前文件内容与 XCoding 当时写入的内容完全一致时，才会恢复。因此，之后由人工或其他工具产生的修改不会被覆盖。

若补丁创建了新文件，回滚会删除该文件。早于“应用后内容”存储机制的旧恢复点会被明确拒绝回滚。

Windows 上替换已有文件时，需要先删除目标文件，再重命名临时文件。XCoding 会在重命名失败时清理临时文件，但这一替换步骤在 Windows 上并非原子操作。

## 取消

`session cancel` 用于正在等待审批的会话。它会将会话标记为已取消、拒绝仍未处理的动作，并阻止之后的审批再执行这些动作。

当前版本还不能中断正在进行的云端模型流式响应或已经运行的命令。要安全支持这两种中断，需要并发请求处理、取消令牌和子进程终止机制，这是后续里程碑的工作。

## 凭据

XCoding 不会把云模型凭据保存到仓库或会话数据库中。请通过环境变量配置 OpenAI 兼容供应商：

```powershell
$env:OPENAI_API_KEY = "..."
$env:XCODING_OPENAI_BASE_URL = "https://api.openai.com/v1" # 可选
```

`OPENAI_API_KEY` 仅保留在启动 CLI 或 Desktop 的进程环境中，RPC 协议不接受任何凭据字段。