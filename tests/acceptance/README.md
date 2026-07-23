# XCoding V1 Acceptance Harness

This suite tracks the V1 acceptance task set from the roadmap.

## Deterministic (no cloud key)

```powershell
pnpm test:e2e
# or
pnpm test:acceptance -- --deterministic
```

Covers:
- read-only grounded answer
- guarded write / reject / auto-edit command gate
- running cancel
- session replay steps

## Live cloud smoke (optional)

Requires `OPENAI_API_KEY` (from env or repo-root `.env`):

```powershell
pnpm test:acceptance -- --live
```

Runs a short monorepo explanation chat against the configured OpenAI-compatible endpoint.

## Full matrix status

| # | Task | Mode | Status |
|---|------|------|--------|
| 1 | Read-only module explanation | deterministic + live | automated |
| 2 | Small feature + tests | live / manual | pending |
| 3 | Bugfix with reproduce-first | live / manual | pending |
| 4 | Behavior-preserving refactor | live / manual | pending |
| 5 | ask mode confirms writes/exec | deterministic | automated |
| 6 | auto-edit writes, still gates commands | deterministic | automated |
| 7 | Rejected patch leaves workspace clean | deterministic | automated |
| 8 | Cancel running task | deterministic | automated |
| 9 | Replay session steps | deterministic | automated |
| 10 | CLI vs Desktop parity | manual | pending |

## 中文

`pnpm test:acceptance` 会先跑确定性 e2e（无需云密钥）。加 `--live` 时再跑一次真实网关冒烟。完整 10 条验收矩阵中，写功能/修 bug/重构/双端等价仍需后续任务补齐；会话回放（task 9）已由 `session.replay` + e2e 覆盖。
