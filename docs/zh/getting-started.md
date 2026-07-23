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

## 配置云端访问

在启动 XCoding 的终端中设置凭据：

```powershell
$env:OPENAI_API_KEY = "..."
$env:XCODING_OPENAI_BASE_URL = "https://ai.v58.dev/v1" # 可选
```

模型请求需要 `OPENAI_API_KEY`。`XCODING_OPENAI_BASE_URL` 可用于设置 OpenAI 兼容服务的地址。XCoding 不会经由 RPC 协议传输密钥，也不会将密钥保存到工作区、会话数据库或 Desktop 设置中。

## 使用 CLI

```powershell
pnpm cli -- ping --workspace .
pnpm cli -- config show --workspace .
pnpm cli -- config set --workspace . --mode auto-edit --model gpt-5.5
pnpm cli -- chat "说明这个仓库的结构" --workspace .
```

CLI 数据库位于 `<workspace>/.xcoding/xcoding.db`。配置只保存该工作区的模式、供应商和模型偏好。除非命令显式传入其他值，新建聊天都会使用这些默认配置。

## 使用 Desktop

在设置了相同凭据变量的终端中启动 Tauri Desktop：

```powershell
pnpm --filter @xcoding/desktop exec tauri dev
```

Desktop 将数据库保存在操作系统的应用数据目录中，并按工作区路径保存配置。因此，Desktop 的本地历史和设置与 CLI 数据库相互独立。

## 延伸阅读

- [会话恢复与安全](./session-safety.md)
- [Desktop](./desktop.md)
- [协议](./protocol.md)