# XCoding Desktop

## 运行

先在终端设置云模型凭据，再启动 Tauri 应用：

```powershell
$env:OPENAI_API_KEY = "..."
# 可选：使用 OpenAI 兼容的云服务时设置。
$env:XCODING_OPENAI_BASE_URL = "https://api.openai.com/v1"
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