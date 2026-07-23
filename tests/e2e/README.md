# XCoding E2E Fixtures

Run the deterministic agent loops with:

```sh
pnpm test:e2e
```

The tests start the real `xcoding-server` binary, use a local OpenAI-compatible SSE mock, and verify tool calls execute inside fixture workspaces before the assistant returns a final streamed answer. No cloud credentials are used.

## Suites

- `read-only-agent.mjs`: grounded read-only answer from fixture workspace.
- `skills-agent.mjs`: catalog + `load_skill` for `.xcoding/skills/*/SKILL.md`.
- `command-allowlist.mjs`: ask still gates allowlisted commands; auto-edit auto-runs allowlist and still gates high-risk shells.
- `guarded-write-agent.mjs`: ask-mode approvals, reject, auto-edit patch auto-apply, command still gated.
- `running-cancel-agent.mjs`: mid-stream cancel, mid-command cancel, failed command refeed, auto-edit still gates commands.
- `session-replay-agent.mjs`: reconstruct session steps via `session.replay`.
- `write-loop-agent.mjs`: feature (patch + test), bugfix (repro-first), refactor (baseline + rewrite + retest).
- `git-tools-agent.mjs`: read-only `git_status` + `git_diff` + `git_log` + `git_show` against a temporary git fixture.
- `git-write-agent.mjs`: approved `git_add` + `git_commit` (high-risk write) in ask and auto-edit modes.
- `git-push-agent.mjs`: approved `git_push` (high-risk write, no force) in ask and auto-edit modes against a local bare remote.
- `git-fetch-pull-agent.mjs`: approved `git_fetch` + `git_pull` (high-risk write, default ff-only, no force/rebase) in ask and auto-edit modes against a local bare remote.
- `provider-auth-error.mjs`: mock HTTP 401 maps to actionable OPENAI_API_KEY / XCODING_OPENAI_BASE_URL guidance.
- `surface-parity.mjs`: static CLI / Desktop / server method surface parity for shared workflows.

## 中文说明

执行 `pnpm test:e2e` 可运行确定性 Agent 端到端测试。测试启动真实的 `xcoding-server` 二进制，使用本地 OpenAI-compatible SSE mock，并验证模型工具调用、fixture 工作区内执行、工具结果回灌和最终流式回答。测试不需要真实云模型密钥。

完整 V1 验收矩阵包装见 `tests/acceptance/README.md`，运行 `pnpm test:acceptance`。
