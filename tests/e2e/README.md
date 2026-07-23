# XCoding E2E Fixtures

Run the deterministic agent loops with:

```sh
pnpm test:e2e
```

The tests start the real `xcoding-server` binary, use a local OpenAI-compatible SSE mock, and verify tool calls execute inside fixture workspaces before the assistant returns a final streamed answer. No cloud credentials are used.

## Suites

- `read-only-agent.mjs`: grounded read-only answer from fixture workspace.
- `guarded-write-agent.mjs`: ask-mode approvals, reject, auto-edit patch auto-apply, command still gated.
- `running-cancel-agent.mjs`: mid-stream cancel, mid-command cancel, failed command refeed, auto-edit still gates commands.
- `session-replay-agent.mjs`: reconstruct session steps via `session.replay`.
- `write-loop-agent.mjs`: feature (patch + test), bugfix (repro-first), refactor (baseline + rewrite + retest).
- `surface-parity.mjs`: static CLI / Desktop / server method surface parity for shared workflows.

## 中文说明

执行 `pnpm test:e2e` 可运行确定性 Agent 端到端测试。测试启动真实的 `xcoding-server` 二进制，使用本地 OpenAI-compatible SSE mock，并验证模型工具调用、fixture 工作区内执行、工具结果回灌和最终流式回答。测试不需要真实云模型密钥。

完整 V1 验收矩阵包装见 `tests/acceptance/README.md`，运行 `pnpm test:acceptance`。
