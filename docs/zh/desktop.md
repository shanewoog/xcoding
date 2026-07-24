# XCoding Desktop

## 运行

启动 Tauri 应用：

```powershell
pnpm --filter @xcoding/desktop exec tauri dev
# 或仓库根目录：
pnpm desktop
```

推荐：在应用内打开 **设置**，填写 Base URL 与 API Key，再点 **保存设置**。
配置写入 `~/.xcoding/config.json`（Windows：`%USERPROFILE%\.xcoding\config.json`）。

仍可用环境变量作为启动时覆盖（进程已有 env 优先，文件只回填缺失项）：

```powershell
$env:OPENAI_API_KEY = "..."
$env:XCODING_OPENAI_BASE_URL = "https://ai.v58.dev/v1"
```

会话数据库位于 `~/.xcoding/xcoding.db`。命令白/黑名单仍保存在各工作区的 `.xcoding/` 下。

## 首次流程

1. 打开 **设置**，选择 **语言**（简体中文或 English）。
2. 配置 **云供应商**：Base URL（默认 `https://ai.v58.dev/v1`）与 API Key，然后 **保存设置**。
3. 设置工作区路径、模式与模型（白/黑名单需要先有工作区路径）。
4. 返回工作台，必要时确认绝对路径，发送任务。
5. 查看计划、流式回答、工具活动、补丁预览与审批控件。
6. 选择已保存会话，查看事件、恢复点与任务完成摘要。

Desktop 与 CLI 共用同一套受保护的 Agent 服务。默认模式为 `ask`；`auto-edit` 会自动应用普通文件补丁与白名单安全命令。高风险写入与非白名单命令仍需审批。

## 设置页

全部配置集中在 **设置** 页（左侧与聊天顶栏按钮）：

| 分区 | 保存位置 |
|------|----------|
| 语言 | UI 语言；写入 `~/.xcoding/config.json`，并镜像到 `localStorage`（`xcoding.locale`） |
| 云供应商 | Provider（`openai` 只读）、Base URL、API Key → `~/.xcoding/config.json` |
| 默认设置 | 工作区路径（上次使用）、模式、模型；白/黑名单 → 工作区 `.xcoding/command-allowlist` / `command-denylist` |
| 诊断 | 客户端清单：工作区、鉴权、Base URL、默认值 |

模式说明：

- **ask** — 提出补丁与命令，二者都需审批
- **auto-edit** — 自动应用普通文件补丁与白名单安全命令；**高风险写入与其他命令仍需审批**
- **命令白名单** — 可选工作区模式（`exe` 或 `exe:subcommand`）；Shell/解释器不可加入
- **命令黑名单** — 可选拦截模式；黑名单优先于白名单，且不会自动执行

**诊断** 仅客户端检查。全部就绪表示可以开始任务；更深入检查请用 `pnpm cli -- doctor`。

### `~/.xcoding/config.json` 示例

```json
{
  "locale": "zh-CN",
  "mode": "ask",
  "provider": "openai",
  "model": "gpt-5.5",
  "base_url": "https://ai.v58.dev/v1",
  "api_key": "sk-...",
  "last_workspace_root": "D:\\WORK\\BittyData\\XCoding"
}
```

v0.1 为便于使用，API Key 以明文保存在用户目录。请勿提交该文件。

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
- **发送** 仅在任务运行中禁用。缺少工作区路径、消息为空或未配置 API Key 时，会在底部提示并显示错误，而不是把按钮永久置灰。

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
2. 首次启动后在 **设置** 中填写 API Key 与 Base URL（或复制 `.env.example` 为旁路 `.env`）
3. 双击 `XCoding.exe`

依赖：Windows 10/11 + WebView2 Runtime（系统通常已自带）。会话数据库与用户配置写在 `%USERPROFILE%\.xcoding\`（`xcoding.db` / `config.json`），不在绿色文件夹内。


### 若出现 “localhost 拒绝连接”

说明打开的是 **开发模式** 二进制（UI 去连 `http://localhost:1420`）。请重新执行：

```powershell
pnpm desktop:portable
```

不要用纯 `cargo build --release` 打包绿色版；必须走 `tauri build`（会启用 `custom-protocol` 内嵌前端）。


### 如果打开后没界面 / 白屏

1. 确认使用 `pnpm desktop:portable` 打出来的包（`dist/portable/XCoding/XCoding.exe`），不要用裸 `cargo build --release`。
2. 安装或修复 [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)。
3. 结束所有 `XCoding` 进程后，删除 WebView 用户目录再重开：
   `%LOCALAPPDATA%\com.shanewoog.xcoding\EBWebView`
4. 生产前端资源必须是相对路径（`./assets/...`）。仓库 Vite 配置已设 `base: './'`。

