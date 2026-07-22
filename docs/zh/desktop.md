# XCoding Desktop

## 运行

先在终端设置云模型凭据，再启动 Tauri 桌面壳：

```powershell
$env:OPENAI_API_KEY = "..."
# 可选：使用 OpenAI 兼容的云服务时设置。
$env:OPENAI_BASE_URL = "https://api.openai.com/v1"
pnpm --filter @xcoding/desktop exec tauri dev
```

应用会将本地会话历史保存在操作系统的应用数据目录中。凭据只从环境变量读取，不会写入会话数据库。

## 首次使用

1. 在左侧输入工作区绝对路径。
2. 在输入区发送代码仓库问题。
3. 查看计划、流式回答和只读工具活动。
4. 选择已保存的会话以查看状态。

Desktop 当前与 CLI server 复用同一套只读 Agent 服务。默认模式为 `ask`；写入与命令执行控制会在 Phase 2 提供。
