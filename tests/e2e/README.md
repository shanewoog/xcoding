# XCoding E2E Fixtures

Run the deterministic read-only agent loop with:

```sh
pnpm test:e2e
```

The test starts the real `xcoding-server` binary, uses a local OpenAI-compatible SSE mock, and verifies that a tool call is executed inside the fixture workspace before the assistant returns a final streamed answer. No cloud credentials are used.

## 中文说明

执行 `pnpm test:e2e` 可运行确定性的只读 Agent 端到端测试。该测试启动真实的 `xcoding-server` 二进制，使用本地 OpenAI-compatible SSE mock，并验证模型工具调用、fixture 工作区内执行、工具结果回灌和最终流式回答。测试不需要真实云模型密钥。

- `running-cancel-agent.mjs`: mid-stream cancel, mid-command cancel, failed command refeed, auto-edit still gates commands.
