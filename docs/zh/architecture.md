# XCoding 架构

## 1. 产品定义

XCoding 是一个本地优先的 AI 编程 Agent 平台。

它帮助用户在本地工作区内完成编码任务，流程包括：

1. 理解仓库上下文
2. 规划工作
3. 在权限策略下调用工具
4. 生成并应用代码补丁
5. 在允许时执行命令
6. 记录完整执行轨迹，便于审查与回放

V1 阶段，XCoding 不是完整 IDE，也不是编辑器插件。

### V1 范围

包含：

- CLI 作为完整能力入口
- 简易 Desktop 作为轻量可视化壳
- Rust 核心负责 Agent 运行时、工具、策略、存储与模型访问
- TypeScript 壳负责交互体验
- 仅接入云模型
- 权限模式：`ask`（默认）与 `auto-edit`

不包含：

- VS Code / JetBrains 扩展
- 本地模型运行时
- 复杂多 Agent 编排
- 云端协作 / 多租户 SaaS
- 强制依赖自建向量数据库

## 2. 已锁定决策

| 主题 | 决策 |
|---|---|
| 核心语言 | Rust |
| 壳层语言 | TypeScript |
| 首发形态 | CLI + 简易 Desktop |
| 模型策略 | 仅云模型 |
| 编辑器插件 | V1 不做 |
| 默认自动程度 | `ask` |
| 可选自动程度 | `auto-edit` |
| 业务真相来源 | 仅 Rust 核心 |

## 3. 总体架构

```text
+--------------------------------------------------+
| TypeScript 壳层                                  |
|                                                  |
|  apps/cli         完整能力交互                   |
|  apps/desktop     聊天 / 计划 / diff / 轨迹      |
|  packages/ui      共享展示组件                   |
|  packages/client  RPC 客户端                     |
+--------------------------+-----------------------+
                           | JSON-RPC
                           | stdio | local socket | websocket
+--------------------------v-----------------------+
| Rust 核心                                        |
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
  工作区文件系统       云端 LLM            系统命令
  git / rg             providers           （受策略控制）
```

### 设计原则

CLI 与 Desktop 都只是客户端。

它们不能各自实现第二套 Agent 运行时。
所有规划、工具执行、权限检查、补丁应用与会话持久化，都必须发生在 Rust 中。

## 4. 仓库结构

```text
XCoding/
  apps/
    cli/                 # TypeScript CLI
    desktop/             # TypeScript Desktop 壳
  crates/
    xcoding-core/        # agent loop、编排
    xcoding-tools/       # fs/search/patch/shell/git 工具
    xcoding-mcp/         # stdio MCP 客户端与工作区 mcp.json
    xcoding-policy/      # 权限决策
    xcoding-providers/   # 云模型供应商
    xcoding-context/     # 规则、检索、摘要
    xcoding-store/       # sqlite 会话与事件
    xcoding-protocol/    # 共享协议类型
    xcoding-server/      # 本地 RPC server 二进制
  packages/
    protocol/            # TS 协议类型
    client/              # TS RPC 客户端
    ui/                  # 共享 UI 组件
  configs/
  docs/
  examples/
  tests/
    e2e/
```

## 5. 运行时组件

### 5.1 Session Manager

负责：

- 会话身份
- 工作区绑定
- 消息历史
- 模式与模型设置
- 生命周期状态

会话状态：

- `created`
- `running`
- `need_user`
- `done`
- `failed`
- `cancelled`

### 5.2 Agent Loop

推荐的 V1 循环：

```text
用户目标
  -> 构建上下文
  -> 请求模型下一步
  -> 校验模型输出
  -> 如果是工具调用：
       权限检查
       执行或询问用户
       追加 observation
       继续
  -> 如果是最终答案：
       标记完成
```

该循环必须足够可复现，以便从已存储事件中回放。

### 5.3 Context Engine

注入模型的上下文层级：

1. 系统角色与工具契约
2. 项目规则（`AGENTS.md` 和/或 `.xcoding/rules.md`）
3. 用户目标
4. 相关文件片段
5. 最近轨迹摘要
6. 当前错误 / 测试输出 / 被拒绝的 diff

V1 检索策略：

- 文件树启发
- 用户文本中的路径 / 符号提示
- `rg` 搜索
- 最近改动文件

V1 不强制引入 embedding 索引。

### 5.4 Tool Runtime

V1 工具：

| 工具 | 权限 | 用途 |
|---|---|---|
| `list_dir` | read | 查看工作区结构 |
| `read_file` | read | 读取文件内容 |
| `search_code` | read | 内置文本搜索，支持可选 glob/上下文 |
| `load_skill` | read | 加载工作区 skill 全文（`.xcoding/skills/<name>/SKILL.md`） |
| `apply_patch` | write | 应用 unified diff / patch |
| `run_command` | exec | 运行测试或构建命令 |
| `git_status` | read | 查看 git 状态 |
| `git_diff` | read | 查看本地变更 |
| `git_log` | read | 查看近期提交历史 |
| `git_show` | read | 查看单个 revision 元数据与补丁 |
| `git_add` | write (high-risk) | 暂存工作区路径（始终需要审批） |
| `git_commit` | write (high-risk) | 创建提交（始终需要审批） |
| `git_push` | write (high-risk) | 推送分支到远端（始终需要审批；不 force） |
| `git_fetch` | write (high-risk) | 从远端 fetch（始终需要审批；不 force） |
| `git_pull` | write (high-risk) | 从远端 pull（始终需要审批；默认 ff-only，不 force/rebase） |
| `mcp` | exec (high-risk) | 调用已配置 MCP 工具（`server` + `tool` + `arguments`）；始终需审批 |

工具要求：

- 严格 JSON schema 输入
- 结构化结果
- 支持超时
- 支持取消
- 必要时对密钥脱敏

### 5.5 Policy Engine

权限类别：

- `read`
- `write`
- `exec`
- `network`

模式：

#### `ask`（默认）

- 读工具自动允许
- 写操作需要确认
- 执行命令需要确认
- 非模型网络工具默认拒绝

#### `auto-edit`

- 读工具自动允许
- 工作区策略内的写操作自动允许
- 执行命令仍需确认
- 高风险写操作仍需确认

高风险示例：

- 删除大量文件
- 修改 `.env` / 凭证文件
- 修改 git 配置或 hooks
- 具有破坏性的命令

`full-auto` 可作为内部枚举存在，但不是 V1 正式产品模式。

### 5.6 Patch Engine

规则：

1. 优先补丁应用，而不是整文件覆盖
2. 应用前或应用时必须发出可审查的 diff 事件
3. 检测应用冲突
4. 在可行时支持拒绝 / 部分拒绝
5. 修改工作区前创建恢复点

V1 恢复策略：

- 仓库足够干净时优先使用 git snapshot
- 否则对涉及路径做文件级备份

### 5.7 Model Gateway

V1 供应商策略：

- 实现与供应商无关的接口
- 优先交付 OpenAI 兼容供应商
- 可选增加 Anthropic 作为第二家

必需能力：

- chat completions
- token 流式输出
- tool/function calls
- usage 统计

本地模型明确不在 V1 范围。

### 5.8 Trace and Store

存储：SQLite

持久化：

- sessions
- messages
- plans
- tool calls 与 tool results
- diffs / patches
- command logs
- token usage
- final status

轨迹用于：

- Desktop 时间线
- 调试 Agent 行为
- 会话回放
- 审计

## 6. 客户端形态

### 6.1 CLI

CLI 是一等完整客户端。

建议命令：

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

CLI 职责：

- 参数解析
- 流式事件渲染
- 确认提示
- 退出码与便于脚本化的输出

### 6.2 Desktop

Desktop 是同一核心上的轻量壳。

主要 UI 区域：

1. 会话列表
2. 聊天 + 计划流
3. Diff / 文件 / 命令轨迹

Desktop 职责：

- 工作区选择
- API Key / 模型设置 UI
- 确认弹窗（patch / 命令 / git 工具有专用审批展示与 HIGH-RISK 标记）
- diff 接受/拒绝操作
- 会话回放视图

Desktop 必须调用与 CLI 相同的 RPC 方法。

## 7. 配置

项目配置示例：`.xcoding/config.toml`

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

全局用户配置可放在用户配置目录；工作区相关值以项目配置优先。

## 8. 安全模型

默认姿态：安全且显式。

控制项：

- 工作区路径限制
- 命令允许/拒绝策略
- 密钥文件保护
- 按模式设置的确认门槛
- 每次变更都可追踪
- 不提供无限制裸 shell

网络策略：

- 允许通过 model gateway 访问模型供应商
- V1 默认拒绝工具级网络访问

## 9. 失败与恢复

系统应以受控方式降级：

| 失败 | 行为 |
|---|---|
| 模型流中断 | 标记步骤失败，允许重试 |
| 补丁冲突 | 不部分污染文件；报告冲突 |
| 命令超时 | 捕获部分输出，标记工具失败 |
| 用户拒绝写入 | 以拒绝结果作为 observation 继续 |
| 用户取消会话 | 停止工具，持久化为 cancelled |

## 10. 架构 V1 非目标

- 替代 git
- 替代用户编辑器
- 做成通用自动化 OS
- 保证完美自主编码
- 对用户隐藏动作

XCoding 应该强大，但始终可审查。

## 11. 架构成功标准

当以下条件成立时，架构可视为成功：

1. 一套 Rust 核心同时服务 CLI 与 Desktop
2. 编码任务可完成计划、工具、diff 与轨迹闭环
3. 权限模式会实质改变写行为
4. 会话回放可重建发生过什么
5. 增加新的云供应商时不需要改写 UI

## 其他语言

- English: [../en/architecture.md](../en/architecture.md)
