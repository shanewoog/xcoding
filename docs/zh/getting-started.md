# 快速开始

XCoding 是一个本地优先的 AI 编程 Agent，提供 Rust 核心、CLI 和 Desktop 客户端。V1 只接入云模型，并使用名为 `openai` 的 OpenAI 兼容供应商。

## 前置条件

- Rust 1.97.0，由 `rust-toolchain.toml` 选择
- Node.js 22 或更高版本
- pnpm 11 或更高版本
- OpenAI 或兼容云服务的 API Key

## 安装与构建

在仓库根目录执行：

```powershell
pnpm install
cargo build -p xcoding-server
pnpm build
```

开发时，`pnpm cli -- ...` 会运行 CLI 构建产物，并启动 `target/debug/xcoding-server` 作为本地 stdio RPC 服务。


## 本地 .env 与一键启动

仓库根目录可放置 `.env`（已被 gitignore，勿提交真实密钥）：

```env
OPENAI_API_KEY=...
XCODING_OPENAI_BASE_URL=https://ai.v58.dev/v1
```

CLI、Desktop 与 server provider 会在缺少对应环境变量时自动读取该文件；**已存在的进程环境变量优先**。

Windows 推荐：

```powershell
.\scripts\xcoding.ps1 chat "说明这个仓库"
.\scripts\xcoding.ps1 desktop
.\scripts\xcoding.ps1 acceptance
```
## 配置云端访问

在启动 XCoding 的终端中设置凭据：

```powershell
$env:OPENAI_API_KEY = "..."
$env:XCODING_OPENAI_BASE_URL = "https://ai.v58.dev/v1" # 可选
```

模型请求需要 `OPENAI_API_KEY`。`XCODING_OPENAI_BASE_URL` 可用于设置 OpenAI 兼容服务的地址。XCoding 不会经由 RPC 协议传输密钥，也不会将密钥保存到工作区、会话数据库或 Desktop 设置中。

若聊天返回 HTTP 401/403，或提示 `OPENAI_API_KEY is not set`，请检查：

1. 启动 CLI/Desktop 的终端是否已导出有效的 `OPENAI_API_KEY`（或仓库根目录存在 `.env`）。
2. 未使用默认网关时，`XCODING_OPENAI_BASE_URL` 是否指向正确的 OpenAI 兼容 `/v1` 地址。
3. 该密钥是否被目标服务接受（错误信息会截断展示 provider 返回体，便于排查）。

## 使用 CLI

```powershell
pnpm cli -- ping --workspace .
pnpm cli -- config show --workspace .
pnpm cli -- config set --workspace . --mode auto-edit --model gpt-5.5
pnpm cli -- chat "说明这个仓库的结构" --workspace .
```

CLI 数据库位于 `<workspace>/.xcoding/xcoding.db`。配置保存该工作区的模式、供应商和模型偏好；额外 auto-edit 命令白名单保存在 `.xcoding/command-allowlist`，黑名单保存在 `.xcoding/command-denylist`（可用 `config set --command-allowlist` / `--command-denylist` 或 Desktop 默认设置编辑）。除非命令显式传入其他值，新建聊天都会使用这些默认配置。


## 绿色 Desktop（免安装）

```powershell
pnpm desktop:portable
```

产出 `dist/portable/XCoding/XCoding.exe`：同目录放 `.env` 后可双击运行。详见 [desktop.md](./desktop.md)。
## 使用 Desktop

在设置了相同凭据变量的终端中启动 Tauri Desktop：

```powershell
pnpm --filter @xcoding/desktop exec tauri dev
```

Desktop 将数据库保存在操作系统的应用数据目录中，并按工作区路径保存配置。因此，Desktop 的本地历史和设置与 CLI 数据库相互独立。


## 项目规则

若工作区存在规则文件，XCoding 会按以下顺序加载到系统提示中：

1. `AGENTS.md`
2. `XCoding.md`
3. `.xcoding/rules.md`

规则请保持简短可执行；过长内容会被截断。

## 云模型鉴权状态

不发起模型请求，仅检查服务端是否看到云端凭据：

```powershell
pnpm cli -- auth --workspace .
```

Desktop 左侧设置区会显示同样的就绪状态（就绪 / 缺少 API key、Base URL、掩码后的 key 提示）。

## 环境诊断

一键检查工作区、server 二进制、核心 RPC、云模型凭据、工作区配置和 git：

`powershell
pnpm cli -- doctor --workspace .
`

返回 JSON。
eady=false 时退出码为 2。

## 命令安全策略

`run_command` 由模式、白名单、黑名单与风险标注共同约束：

- **ask** — 每条命令都需要审批
- **auto-edit** — 白名单内安全开发命令可自动执行（内置 + `.xcoding/command-allowlist`）；高风险与非白名单命令仍需审批
- **工作区黑名单**（`.xcoding/command-denylist`）始终拦截匹配项，即使同时在白名单中
- **硬拒绝**：format / shutdown / git clean -fdx / 递归删除根路径 / 绝对路径可执行文件等
- **高风险标注**：powershell -Command、cmd /c、git push --force、pnpm publish 等

硬拒绝与黑名单拦截不会进入审批队列，会作为结构化工具错误回传给模型（`code: command_policy_denied`，以及 `policy_code`）。
即使在 auto-edit 下，`.git` / `.xcoding` 等高风险工作区写入也始终需要审批。

## 延伸阅读

- [会话恢复与安全](./session-safety.md)
- [Desktop](./desktop.md)
- [协议](./protocol.md)

## 会话续聊

在已完成的会话上追加提问（同一 session id，共享历史）：

```powershell
pnpm cli -- chat "CLI 包是做什么的？" --workspace . --session <session-id>
```

Desktop：选中已完成的会话后再发送（按钮显示 **Continue**）。点 **New chat** 开启新会话。

可选：在 `.xcoding/skills/<name>/SKILL.md` 添加工作区 skill，Agent 可通过 `load_skill` 加载。

可选：在 `.xcoding/mcp.json` 配置 stdio MCP 服务器：

```json
{
  "mcpServers": {
    "demo": {
      "command": "node",
      "args": ["mock-mcp-server.mjs"],
      "enabled": true
    }
  }
}
```

已启用服务器会在每次 Agent 回合启动。其工具以命名空间函数名 `mcp__<server>__<tool>` 暴露给模型；协议层工具名为 `mcp`，参数为 `{ server, tool, arguments }`。无论 `ask` 还是 `auto-edit`，MCP 调用都需要用户审批。`xcoding doctor` 会报告 `mcp_config` 状态。
