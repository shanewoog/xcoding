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
- feature / bugfix / refactor write loops
- CLI vs Desktop surface parity (static)

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
| 2 | Small feature + tests | deterministic | automated |
| 3 | Bugfix with reproduce-first | deterministic | automated |
| 4 | Behavior-preserving refactor | deterministic | automated |
| 5 | ask mode confirms writes/exec | deterministic | automated |
| 6 | auto-edit writes, still gates commands | deterministic | automated |
| 7 | Rejected patch leaves workspace clean | deterministic | automated |
| 8 | Cancel running task | deterministic | automated |
| 9 | Replay session steps | deterministic | automated |
| 10 | CLI vs Desktop parity | deterministic (static) | automated |

## 中文

`pnpm test:acceptance` 会先跑确定性 e2e（无需云密钥）。加 `--live` 时再跑一次真实网关冒烟。完整 10 条验收矩阵均已接入确定性自动化：任务 2/3/4 由 `write-loop-agent.mjs` 覆盖，任务 10 由 `surface-parity.mjs` 做 CLI/Desktop 表面一致性检查。
