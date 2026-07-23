# XCoding 文档

本目录固化 XCoding 的 V1 方案。

## 文档列表

- [architecture.md](./architecture.md)：系统架构与模块边界
- [roadmap.md](./roadmap.md)：分阶段交付计划与验收标准
- [protocol.md](./protocol.md)：CLI/Desktop 与 Rust 核心协议草案

## 已锁定决策

- Rust 核心 + TypeScript 壳
- 首发 CLI + 简易 Desktop
- V1 只接云模型
- V1 不做编辑器插件
- 默认模式：`ask`
- 可选模式：`auto-edit`

## 当前状态

协议、server、CLI、Desktop、写闭环与 git 工具已可用。任务完成摘要会附带 git 状态/diff 快照。请从 [getting-started.md](./getting-started.md) 与 [session-safety.md](./session-safety.md) 开始使用。

## 其他语言

- 英文文档：[../en/README.md](../en/README.md)
