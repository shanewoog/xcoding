# 快速开始

XCoding 是一个本地优先的 AI 编程 Agent，提供 Rust 核心、CLI 和 Desktop 客户端。V1 只接入云模型，并使用名为 `openai` 的 OpenAI 兼容供应商。

## 前置条件

- Rust 1.97.0，由 `rust-toolchain.toml` 选择
- Node.js 22 或更高版本
- pnpm 11 或更高版本
- OpenAI 或兼容云服务的 API Key

## 安装与构建

在仓库根目录执行：

```bash
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

Windows 下辅助脚本仍使用 PowerShell：

```powershell
.\scripts\xcoding.ps1 chat "说明这个仓库"
.\scripts\xcoding.ps1 desktop
.\scripts\xcoding.ps1 acceptance
```
## 配置云端访问

在启动 XCoding 的终端中设置凭据：

```bash
export OPENAI_API_KEY="..."
export XCODING_OPENAI_BASE_URL="https://ai.v58.dev/v1" # optional
```

模型请求需要 `OPENAI_API_KEY`。`XCODING_OPENAI_BASE_URL` 可用于设置 OpenAI 兼容服务的地址。XCoding 不会经由 RPC 协议传输密钥，也不会将密钥保存到工作区、会话数据库或 Desktop 设置中。

若聊天返回 HTTP 401/403，或提示 `OPENAI_API_KEY is not set`，请检查：

1. 启动 CLI/Desktop 的终端是否已导出有效的 `OPENAI_API_KEY`（或仓库根目录存在 `.env`）。
2. 未使用默认网关时，`XCODING_OPENAI_BASE_URL` 是否指向正确的 OpenAI 兼容 `/v1` 地址。
3. 该密钥是否被目标服务接受（错误信息会截断展示 provider 返回体，便于排查）。

## 使用 CLI

```bash
pnpm cli -- ping --workspace .
pnpm cli -- config show --workspace .
pnpm cli -- config set --workspace . --mode auto-edit --model gpt-5.5
pnpm cli -- chat "说明这个仓库的结构" --workspace .
```

CLI 数据库位于 `<workspace>/.xcoding/xcoding.db`。配置保存该工作区的模式、供应商和模型偏好；额外 auto-edit 命令白名单保存在 `.xcoding/command-allowlist`，黑名单保存在 `.xcoding/command-denylist`（可用 `config set --command-allowlist` / `--command-denylist` 或 Desktop 默认设置编辑）。除非命令显式传入其他值，新建聊天都会使用这些默认配置。


## 绿色 Desktop（免安装）

```bash
pnpm desktop:portable
```

产出 `dist/portable/XCoding/XCoding.exe`：同目录放 `.env` 后可双击运行。详见 [desktop.md](./desktop.md)。
## 使用 Desktop

在设置了相同凭据变量的终端中启动 Tauri Desktop：

```bash
pnpm --filter @xcoding/desktop exec tauri dev
```

Desktop 将数据库保存在 `~/.xcoding/xcoding.db`，用户偏好保存在 `~/.xcoding/config.json`。工作区策略仍在项目 `.xcoding/` 下。Desktop 历史与 CLI 数据库相互独立。


## 项目规则

若工作区存在规则文件，XCoding 会按以下顺序加载到系统提示中：

1. `AGENTS.md`
2. `XCoding.md`
3. `.xcoding/rules.md`

规则请保持简短可执行；过长内容会被截断。

## 云模型鉴权状态

不发起模型请求，仅检查服务端是否看到云端凭据：

```bash
pnpm cli -- auth --workspace .
```

Desktop 左侧设置区会显示同样的就绪状态（就绪 / 缺少 API key、Base URL、掩码后的 key 提示）。

## 同一供应商配置多个 Key

一个供应商条目可以挂多个来自不同账号的 API Key。在 **设置 → 云端供应商** 里用「添加 Key」建立 Key 池，为每个 Key 填备注和权重。

- 选择算法是平滑加权轮询，每个用户回合只决策一次。权重 `6 / 3 / 1` 大致对应 60% / 30% / 10% 的回合占比。
- 权重填 `0` 或取消「启用」，Key 会保留配置但不参与轮询。
- 被 401/403 拒绝的 Key 在本进程内停用，直到配置里的 Key 值发生变化。429 按 30/60/120 秒冷却，上游返回 `Retry-After` 时优先采用。超时与 5xx 先走 `max_provider_retries`，之后按 10/30/60 秒冷却。
- 若某供应商全部 Key 都在冷却，会提前释放冷却以保证回合仍可执行；被拒绝的 Key 不会因此释放。
- 每个 Key 行会显示轮询状态、成功/失败次数与剩余冷却秒数。日志与事件只记录 Key id 和掩码尾号，不记录完整 Key。

## 同一模型的多供应商均衡路由

同一个逻辑模型可以分摊到多个独立供应商。在 **设置 → 模型路由** 里为模型名添加路由，每条路由绑定一个供应商、一个权重，以及可选的上游模型名。

```json
{
  "model_routes": {
    "claude-opus-5-thinking": [
      { "provider_id": "gorouter", "weight": 3, "enabled": true },
      { "provider_id": "backup-relay", "weight": 1, "enabled": true, "model_override": "claude-opus-5-thinking-20250101" }
    ]
  }
}
```

- 每次上游请求前只决策一次：先按平滑加权轮询选一条路由，再在该供应商内部按 Key 权重选一个 Key，只发一次请求，不并发也不合并多个供应商的回答。
- 轮询粒度是「工具轮次」：一次提问若触发多轮工具调用，第一轮沿用回合开始时的选择，后续每轮重新轮询，因此单次提问也会按权重分摊到多个供应商。中途切换仍受信任级别约束，不会跨级别。
- 无可用 Key 或熔断打开的路由不占轮询名额，权重不会浪费在答不了的供应商上；若所有路由都不可用则保留原顺序，仍给该回合一次尝试机会。
- 失败切换顺序是「同供应商的其他 Key」→「下一条路由的供应商」。冷却与拒绝规则和单供应商 Key 池完全一致。
- `model_override` 只改写发往该供应商的 `model` 字段，用于中转站给同一模型起的别名；会话仍以逻辑模型名显示，日志同时记录逻辑模型名与实际发往上游的模型名。
- 上游模型名输入框带下拉候选：候选来自该行所选供应商已获取的模型列表，聚焦或切换供应商时才按需拉取。留空表示与逻辑模型同名，也可以手填列表里没有的别名。
- 覆盖对该回合的全部上游调用生效，包括上下文压缩与记忆提取这两个辅助调用，不会出现辅助调用仍发逻辑模型名而被上游拒绝的情况。
- 路由不跨信任级别：若某供应商的 `trust_level` 与当前会话所用级别不一致，该路由标记为「信任级别不一致」并跳过。涉密拦截与 Relay 工具确认规则不变。
- 权重填 `0` 或取消启用即保留配置但不参与轮询。未配置路由的模型仍按当前启用供应商与故障切换顺序调用。
- 每条路由会显示状态（可用 / 未参与轮询 / 供应商不存在 / 无可用 Key / Key 全部被拒 / 冷却中 / 信任级别不一致）。日志只记录逻辑模型名、供应商 id、Key id、权重与结果。
- 模型调用日志按「供应商 / 凭据 / 接口协议」三行区分：供应商是配置里的真实名称，凭据只显示掩码尾号，接口协议是 wire 层协议名（如 `openai`）。上下文压缩与记忆提取各自有独立标签。

## 网络代理

在 **设置 → 调用稳定性 → 网络代理** 里选择供应商 API 请求的出网方式：

- **不走代理（直连）** — 忽略系统代理与 `HTTP(S)_PROXY` 环境变量，始终直连。
- **系统代理**（默认，与历史行为一致）— 读取 Windows Internet 设置 / macOS 系统配置，以及 `HTTP_PROXY` / `HTTPS_PROXY` 环境变量。
- **自定义代理** — 填写代理地址，支持 `http://`、`https://`、`socks5://`、`socks5h://`，可带 `user:password@host:port`。

```json
{
  "http_proxy_mode": "custom",
  "http_proxy_url": "http://127.0.0.1:10808"
}
```

- 仅作用于供应商 API 请求，不影响 `run_command` 里的命令、git 操作与 MCP 进程。
- `localhost`、`127.0.0.1`、`::1` 始终不走代理，本地模型服务不会被代理拦住；`NO_PROXY` 会追加到该绕行列表。
- 选了自定义但地址为空会自动降级为系统代理；地址非法时同样退回系统代理，不会让请求直接失败。
- 环境变量 `XCODING_HTTP_PROXY` 优先级高于此设置：`off` / `none` / `direct` 表示直连，`system` 表示跟随系统，其他值当作代理地址，便于 CLI 与无界面环境临时覆盖。
- 保存后在下一次供应商请求生效，不需要重启 Desktop。

## 环境诊断

一键检查工作区、server 二进制、核心 RPC、云模型凭据、工作区配置和 git：

```bash
pnpm cli -- doctor --workspace .
```

返回 JSON。
eady=false 时退出码为 2。

## 命令安全策略

`run_command` 由模式、白名单、黑名单与风险标注共同约束：

- **ask** — 每条命令都需要审批
- **auto-edit** — 白名单内安全开发命令可自动执行（内置 + `.xcoding/command-allowlist`）；高风险与非白名单命令仍需审批
- **full-auto** — 自动执行非网络且未被硬拒绝的工具调用；网络访问与硬拒绝命令仍被拦截，仅用于完全信任的工作区和供应商
- **工作区黑名单**（`.xcoding/command-denylist`）始终拦截匹配项，即使同时在白名单中
- **硬拒绝**：format / shutdown / git clean -fdx / 递归删除根路径 / 绝对路径可执行文件等
- **高风险标注**：`powershell -Command`、`cmd /c`、`git push --force`、`pnpm publish` 等

硬拒绝与黑名单拦截不会进入审批队列，会作为结构化工具错误回传给模型（`code: command_policy_denied`，以及 `policy_code`）。
即使在 auto-edit 下，`.git` / `.xcoding` 等高风险工作区写入也始终需要审批。

## 延伸阅读

- [会话恢复与安全](./session-safety.md)
- [Desktop](./desktop.md)
- [协议](./protocol.md)

## 会话续聊

在已完成的会话上追加提问（同一 session id，共享历史）：

```bash
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
