# XCoding 路线图

## 1. 目标

交付一个可用的本地 AI 编程 Agent，具备：

- Rust 核心
- TypeScript CLI
- 简易 Desktop 壳
- 仅云模型
- 默认权限模式 `ask`

当用户能够在真实仓库任务中完成“计划、diff、命令执行、轨迹回放”时，V1 就算成功。

## 2. 产品边界

### V1 包含

- 工作区感知的 Agent 会话
- 云模型网关
- 读/搜/写/执行工具
- `ask` 与 `auto-edit`
- CLI 完整工作流
- 简易 Desktop 工作流
- SQLite 轨迹存储
- 补丁预览 / 应用 / 拒绝
- 基础回滚

### V1 不包含

- 编辑器扩展
- 本地模型运行时集成
- 多 Agent 团队
- 完整 MCP 生态
- 托管式多用户产品

## 3. 里程碑计划

## Phase 0 - 骨架

目标周期：约 1 周

### 交付物

- Rust crates 的 Cargo workspace
- apps/packages 的 JS/TS workspace
- `xcoding-server` 可启动
- Rust 与 TS 两侧都有 protocol 包
- CLI 可连接核心
- Desktop 空壳可连接核心
- `ping` / health RPC 可用
- session create/list 桩实现可用

### 退出标准

- CLI 与 Desktop 都能对同一本地核心创建 session
- TS 壳中不重复实现业务逻辑

### 验证方式

- 手工：启动 server，跑 CLI health，打开 Desktop 并连接
- 自动：协议序列化测试

## Phase 1 - 只读 Agent

目标周期：约 1 周

### 交付物

- OpenAI 兼容供应商接入
- 文本流式事件
- 工具：
  - `list_dir`
  - `read_file`
  - `search_code`
- 项目规则加载
- 基础上下文组装
- 消息与事件的会话持久化

### 退出标准

- 用户可对仓库提问，并得到带文件引用的可靠回答
- CLI 与 Desktop 都能看到流式输出

### 验证方式

- 任务：“这个仓库的鉴权逻辑在哪里？”
- 任务：“总结模块 X 的边界”
- 断言引用路径真实存在且内容相关

## Phase 2 - 可写闭环

目标周期：约 1.5 到 2 周

### 交付物

- `apply_patch`
- diff 事件
- `ask` 的确认流
- `run_command`
- 命令输出回灌上下文
- 失败恢复循环
- 变更前恢复点

### 退出标准

- Agent 能实现小功能或修 bug，并带测试
- 用户可批准或拒绝写入
- 拒绝写入不会造成半写入损坏

### 验证方式

- 任务：增加 health 接口与测试
- 任务：修复一个已知失败单测
- 任务：拒绝补丁后继续
- 任务：安全取消运行中会话

## Phase 3 - 产品化

目标周期：约 1.5 到 2 周

### 交付物

- Desktop 三栏 UX：
  - sessions
  - chat/plan
  - diff/trace
- `auto-edit` 模式
- 会话回放
- 配置 UI / 配置命令
- 任务结束变更汇总
- e2e 任务集
- 安装、鉴权、模式与安全文档

### 退出标准

- 同一任务下 CLI 与 Desktop 行为一致
- 用户可切换 `ask` / `auto-edit`
- 用户可回放已完成会话
- 在样例仓库上核心演示稳定

### 验证方式

- 端到端跑完整 V1 验收任务集
- 对比同一 prompt 下 CLI 与 Desktop 轨迹
- 验证完成变更任务后的回滚路径

## Phase 4 - V1.x 强化

目标周期：V1 之后持续进行

### 候选能力

- 更好的相关文件召回（Wave R：搜索选项 + 工作区 sketch；向量检索后续）
- 更强命令策略引擎
- 更多云供应商
- skills 系统
- MCP 支持
- 更好的补丁置信度与冲突体验
- 更丰富 git 工作流（Wave S：结构化 `git_log` + `git_show`；Wave T：需审批的 `git_add` + `git_commit`；Wave U：需审批且不 force 的 `git_push`；pull/reset/force 稍后）
- 大仓库性能优化

### 更后面再做

- VS Code 扩展
- JetBrains 扩展
- 本地模型支持
- 多 Agent 审查/实现分工

## 4. V1 验收任务集

这些任务比功能清单更能定义“完成”。

1. 仅用只读工具解释样例仓库中的某个模块
2. 增加一个小功能并补测试
3. 以“先复现再修复”的方式修 bug
4. 重构函数且不改变行为
5. 在 `ask` 模式下，写与执行前必须确认
6. 在 `auto-edit` 模式下，自动写普通补丁与白名单命令；高风险/非白名单执行仍需确认
7. 拒绝提议补丁后，工作区保持正确
8. 取消运行中任务并持久化为 cancelled
9. 回放会话并重建主要步骤
10. 同一任务分别从 CLI 与 Desktop 执行，结果等价

## 5. 工程实施顺序

推荐实现顺序：

1. 协议与事件模型
2. server + client 连接
3. session store
4. 模型网关流式输出
5. 只读工具
6. 只读问答 Agent loop
7. patch engine
8. 确认 / 策略流
9. 命令工具
10. Desktop 审查 UX
11. auto-edit
12. 回放与打磨

## 6. 质量门禁

每个阶段按此顺序补测试：

1. 纯逻辑单测
2. 工具契约测试
3. 策略测试
4. 会话/事件持久化测试
5. fixture 仓库上的端到端任务测试

功能完成定义：

- 有实现
- 有验证路径
- 有失败行为定义
- 轨迹输出有用

## 7. 交付建议

近期执行顺序：

1. 冻结 `docs/` 文档
2. 搭 monorepo 骨架
3. 实现 protocol + server skeleton
4. 先让 CLI 只读 chat 跑通
5. 再加深 Desktop UX

Desktop 应略落后于核心能力，而不是反过来带着核心走。

## 8. V1 发布退出定义

满足以下条件时，可称为可发布 V1：

- 安装与 API Key 配置有文档且可用
- 至少一家云供应商稳定
- `ask` 为默认且可靠
- `auto-edit` 可用且说明清楚
- CLI 能完成真实任务
- Desktop 能完成同类任务
- 轨迹与 diff 可审查
- 存在回滚路径
- 验收任务集在样例仓库上大体通过

## 其他语言

- English: [../en/roadmap.md](../en/roadmap.md)
