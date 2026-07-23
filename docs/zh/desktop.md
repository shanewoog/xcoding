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

Desktop 与 CLI 共用同一套受保护的 Agent 服务。默认模式为 `ask`；`auto-edit` 会自动应用普通文件补丁，但执行命令仍然需要审批。

## 高风险命令审批

当 Agent 提出 shell 类或 force-push 命令时，Desktop 会显示 **HIGH-RISK** 标记、完整命令文本，以及更醒目的批准按钮文案。硬拒绝命令不会进入审批面板，而是作为工具错误返回给模型。

## 任务完成摘要

任务结束后，Desktop 会显示完成摘要面板：变更文件（created/modified/deleted）、近似 `+/-` 行数、命令成功/失败计数，以及可选的 git status/diff 快照。可用 **Copy summary** 复制完整文本，或 **Copy git** 仅复制 git 快照。

## 多轮会话

在左侧选中已完成会话可查看历史。再发送消息会**续聊**同一会话（共享对话与恢复点）。**New chat** 清空当前选择并开始新任务。

