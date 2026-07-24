# XCoding Desktop

## 运行

先在终端设置云模型凭据，再启动 Tauri 应用：

```powershell
$env:OPENAI_API_KEY = "..."
# 可选：使用 OpenAI 兼容的云服务时设置。
$env:XCODING_OPENAI_BASE_URL = "https://ai.v58.dev/v1"
pnpm --filter @xcoding/desktop exec tauri dev
```

应用会将本地会话历史和工作区默认配置保存在操作系统的应用数据目录中。凭据只从环境变量读取，不会写入会话数据库。

## 首次使用

1. 在左侧输入工作区绝对路径。
2. 选择模式和模型默认值，并保存到该工作区。
3. 在输入区发送仓库任务。
4. 查看计划、流式回答、工具活动、补丁预览和审批控件。
5. 选择已保存会话，查看事件、恢复点和任务完成摘要。

Desktop 与 CLI 共用同一套受保护的 Agent 服务。默认模式为 `ask`；`auto-edit` 会自动应用普通文件补丁与白名单安全命令。高风险写入与非白名单命令仍需审批。左侧默认设置面板可编辑工作区 `.xcoding/command-allowlist` 与 `.xcoding/command-denylist` 模式。

## 默认值与诊断

左侧 **Defaults** 面板保存工作区级别的模式与模型设置（v1 供应商固定为 `openai`）：

| 控件 | 行为 |
|------|------|
| Mode | `ask`（默认）或 `auto-edit` |
| Provider | 只读 `openai` |
| Model | 新会话使用的云模型 id |
| Save defaults | 将 mode/provider/model 写入当前工作区路径 |

切换模式时会更新说明文案：

- **ask** — 提出补丁与命令，二者都需审批
- **auto-edit** — 自动应用普通文件补丁与白名单安全命令；**高风险写入与其他命令仍需审批**
- **命令白名单** — 可选工作区模式（`exe` 或 `exe:subcommand`），保存到 `.xcoding/command-allowlist`；Shell/解释器不可加入
- **命令黑名单** — 可选工作区拦截模式，保存到 `.xcoding/command-denylist`；黑名单优先于白名单，且不会自动执行

**Diagnostics** 是客户端检查清单（工作区路径、鉴权、Base URL、默认值）。全部就绪表示可以开始任务；更深入的服务端检查仍请使用 `pnpm cli -- doctor`。


## 高风险命令审批

当 Agent 提出 shell 类或 force-push 命令时，Desktop 会显示 **HIGH-RISK** 标记、完整命令文本，以及更醒目的批准按钮文案。硬拒绝与黑名单拦截不会进入审批面板，而是作为结构化工具错误返回给模型（`command_policy_denied` + `policy_code`）。

## 任务完成摘要

任务结束后，Desktop 会显示完成摘要面板：变更文件（created/modified/deleted）、近似 `+/-` 行数、命令成功/失败计数，以及可选的 git status/diff 快照。可用 **Copy summary** 复制完整文本，或 **Copy git** 仅复制 git 快照。

## 多轮会话

在左侧选中已完成会话可查看历史。再发送消息会**续聊**同一会话（共享对话与恢复点）。**New chat** 清空当前选择并开始新任务。

## 三栏布局

| 区域 | 内容 |
|------|------|
| 左侧 | 工作区路径、鉴权状态、模式/模型默认值、诊断，以及可滚动的会话历史（含状态徽标） |
| 中间 | 对话记录（自动滚动到底部）、空状态三栏说明、输入区 |
| 右侧 | 有待审批时顶部固定审批面板、任务摘要、活动，以及可折叠的计划 / 恢复点 / 回放 |

会话历史会显示状态、模式、模型，以及相对更新时间。消息角色显示为 You / Assistant / Tool / System。

### 快捷键

- **Ctrl+Enter**（Windows/Linux）或 **Cmd+Enter**（macOS）发送输入区消息。

### Trace 面板

当会话尚无计划、活动、恢复点、回放或摘要时，右侧显示简短的 Trace 空状态提示。Plan、Restore points、Replay 在空时折叠，有内容时展开。待审批操作会固定在 Trace 面板顶部。

## 绿色免安装版（Portable）

不需要安装程序。构建并打包：

```powershell
pnpm desktop:portable
# 或
.\scripts\package-desktop-portable.ps1
```

输出目录：`dist/portable/XCoding/`

使用方式：

1. 把整个文件夹拷到任意位置
2. 复制 `.env.example` 为 `.env`，填入 API Key 与 Base URL
3. 双击 `XCoding.exe`

依赖：Windows 10/11 + WebView2 Runtime（系统通常已自带）。会话数据库仍写在系统应用数据目录，不在绿色文件夹内。

