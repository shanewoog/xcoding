# Getting Started

XCoding is a local-first AI coding agent with a Rust core and CLI or Desktop clients. Version 1 uses cloud models only and supports the OpenAI-compatible provider named `openai`.

## Prerequisites

- Rust 1.97.0, selected by `rust-toolchain.toml`
- Node.js 22 or later
- pnpm 11 or later
- An API key for your OpenAI-compatible cloud service

## Install And Build

From the repository root:

```bash
pnpm install
cargo build -p xcoding-server
pnpm build
```

For development, `pnpm cli -- ...` runs the CLI source build and starts `target/debug/xcoding-server` as its local stdio RPC server.


## Local .env And Launcher

You may place a repository-root `.env` file (gitignored; never commit real keys):

```env
OPENAI_API_KEY=...
XCODING_OPENAI_BASE_URL=https://ai.v58.dev/v1
```

The CLI, Desktop shell, and provider load this file when the corresponding variables are missing. **Existing process environment values always win.**

On Windows, the helper launcher still uses PowerShell:

```powershell
.\scripts\xcoding.ps1 chat "Explain this repository"
.\scripts\xcoding.ps1 desktop
.\scripts\xcoding.ps1 acceptance
```
## Configure Cloud Access

Set credentials in the shell that starts XCoding:

```bash
export OPENAI_API_KEY="..."
export XCODING_OPENAI_BASE_URL="https://ai.v58.dev/v1" # optional
```

`OPENAI_API_KEY` is required for model requests. `XCODING_OPENAI_BASE_URL` is optional and is useful for an OpenAI-compatible endpoint. XCoding never sends credentials through its RPC protocol and does not save them in the workspace, session database, or Desktop settings.

If chat fails with HTTP 401/403 or "OPENAI_API_KEY is not set", verify:

1. The shell that starts CLI/Desktop exports a valid `OPENAI_API_KEY` (or a repo-root `.env` file is present).
2. `XCODING_OPENAI_BASE_URL` points at your OpenAI-compatible `/v1` endpoint when you are not using the default.
3. The key is accepted by that endpoint (provider responses are truncated into the error message for diagnosis).

## Use The CLI

```bash
pnpm cli -- ping --workspace .
pnpm cli -- config show --workspace .
pnpm cli -- config set --workspace . --mode auto-edit --model gpt-5.5
pnpm cli -- chat "Explain the structure of this repository" --workspace .
```

The CLI database is `<workspace>/.xcoding/xcoding.db`. Configuration stores mode, provider, and model for that workspace. Extra auto-edit command patterns live in `.xcoding/command-allowlist`; blocks live in `.xcoding/command-denylist` (editable via `config set --command-allowlist` / `--command-denylist` or Desktop defaults). New chats use those defaults unless a command explicitly supplies a different value.


## Portable Desktop (no install)

```bash
pnpm desktop:portable
```

Produces `dist/portable/XCoding/XCoding.exe`. Place a `.env` beside the exe, then double-click. See [zh desktop docs](../zh/desktop.md) for details.
## Use Desktop

Start the Tauri desktop app from a shell with the same credential variables:

```bash
pnpm --filter @xcoding/desktop exec tauri dev
```

Desktop stores its database at `~/.xcoding/xcoding.db` and user preferences (locale, provider Base URL, API key, last workspace, default mode/model) at `~/.xcoding/config.json`. Workspace mode/model and command policy remain workspace-scoped. Desktop history is therefore separate from the CLI database.


## Project Rules

XCoding loads workspace rules into the system prompt (when present), in this order:

1. `AGENTS.md`
2. `XCoding.md`
3. `.xcoding/rules.md`

Keep these files short and actionable. Oversized rule files are truncated.

## Provider Auth Status

Check whether cloud credentials are visible to the server without making a model call:

```bash
pnpm cli -- auth --workspace .
```

Desktop shows the same readiness state (ready / API key missing, base URL, masked key hint) in Settings and as a compact badge on the left panel. Configure the API key under **Settings → Cloud provider**.

## Multiple Keys For One Provider

One provider entry can hold several API keys from different accounts. Under **Settings → Cloud provider**, use **Add key** to build the pool, then give each key a label and a weight.

- Selection is smooth weighted round-robin, decided once per user turn. Weights `6 / 3 / 1` send roughly 60% / 30% / 10% of turns to each key.
- Weight `0` or an unchecked **Enabled** box keeps a key configured but out of rotation.
- A key refused with 401/403 is dropped for the rest of the process until its value changes in the configuration. A 429 cools down for 30/60/120s, honouring `Retry-After` when the upstream sends it. Timeouts and 5xx first consume `max_provider_retries`, then cool down for 10/30/60s.
- If every key of a provider is cooling down, the cooldowns are released early so the turn can still run. Refused keys are never released this way.
- Each key row shows its rotation state, success/failure counters, and the remaining cooldown. Logs and events carry only the key id and the masked tail, never the secret.

## Balancing One Model Across Providers

One logical model can be spread across independent providers. Under **Settings → Model routing**, add a route per provider with a weight and an optional upstream model name.

```json
{
  "model_routes": {
    "claude-opus-5-thinking": [
      { "provider_id": "gorouter", "weight": 3, "enabled": true },
      { "provider_id": "backup-relay", "weight": 1, "enabled": true, "model_override": "claude-opus-5-thinking-20250101" }
    ]
  }
}
```

- One decision per upstream request: smooth weighted round-robin picks a route, then the provider's own key rotation picks a key. A single request goes out; answers are never fanned out or merged across providers.
- Rotation happens per tool round: the first round of a turn reuses the selection made when the turn opened, and every later round rotates again, so even a single question is shared across providers by weight. A mid-turn switch still stays inside the active trust level.
- Routes with no usable key or an open circuit do not consume a rotation slot, so their weight is never spent on a provider that cannot answer. When no route is usable the original order is kept so the turn still gets one attempt.
- Failover order is other keys of the same provider first, then the next route. Cooldown and refusal rules match the single-provider key pool.
- `model_override` rewrites only the `model` field sent to that provider, for relays that alias the same model. Sessions keep the logical model name, and logs record both the logical name and the model actually sent upstream.
- The upstream-model field offers a dropdown of models already fetched from that row's provider; the fetch happens on focus or provider change, not on first paint. Leave it empty to reuse the logical model name, or type an alias that is not in the list.
- The override applies to every upstream call of the turn, including the context-compaction and memory-extraction helpers, so an auxiliary call can never leak the logical model name to a relay that rejects it.
- Routes never cross trust levels: a provider whose `trust_level` differs from the level in use is marked **Trust level mismatch** and skipped. Confidential-content blocking and relay tool confirmation are unchanged.
- Weight `0` or an unchecked route stays configured but out of rotation. Models without routes keep using the active provider and its fallbacks.
- Each route shows its state (ready / out of rotation / provider missing / no usable key / all keys rejected / cooling down / trust level mismatch). Logs carry the logical model name, provider id, key id, weight, and result.
- Model call logs separate **Provider** (the configured display name), **Credential** (masked tail only), and **Wire protocol** (the protocol name, for example `openai`). Context compaction and memory extraction each get their own label.

## Network Proxy

Under **Settings → Call resilience → Network proxy**, choose how provider API requests reach the network:

- **No proxy (direct)** — ignore OS proxy settings and `HTTP(S)_PROXY`, always connect directly.
- **System proxy** (default, matching the historical behaviour) — follow Windows Internet Settings / macOS system config plus the `HTTP_PROXY` / `HTTPS_PROXY` variables.
- **Custom proxy** — enter an address; `http://`, `https://`, `socks5://` and `socks5h://` are accepted, optionally with `user:password@host:port`.

```json
{
  "http_proxy_mode": "custom",
  "http_proxy_url": "http://127.0.0.1:10808"
}
```

- The setting only covers provider API traffic; `run_command` processes, git, and MCP servers are unaffected.
- `localhost`, `127.0.0.1` and `::1` always bypass the proxy so local model servers stay reachable, and `NO_PROXY` is appended to that bypass list.
- Custom mode with an empty address degrades to system proxy, and a malformed address falls back to system settings instead of failing the request.
- `XCODING_HTTP_PROXY` overrides the saved setting: `off` / `none` / `direct` forces a direct connection, `system` follows the OS, and anything else is treated as a proxy URL. Useful for CLI and headless runs.
- Saving takes effect on the next provider request; no Desktop restart is required.

## Environment Doctor

Check workspace, server binary, core RPC, cloud credentials, workspace config, and git in one shot:

```bash
pnpm cli -- doctor --workspace .
```

Prints JSON. Exit code is 2 when 
eady is false.

## Command Safety Policy

`run_command` is gated by mode, allowlist, denylist, and risk labels:

- **ask** — every command needs approval
- **auto-edit** — allowlisted safe developer commands auto-run (builtin plus `.xcoding/command-allowlist`); high-risk and non-allowlisted commands still need approval
- **full-auto** — auto-runs non-network tool calls that are not hard-denied; network access and hard-denied commands remain blocked, for fully trusted workspaces and providers only
- **Workspace denylist** (`.xcoding/command-denylist`) always blocks matches, even when also allowlisted
- **Hard-denies** commands such as format, shutdown, git clean -fdx, recursive root deletes, and absolute executables
- **Flags high-risk** shells/network-style helpers such as `powershell -Command`, `cmd /c`, `git push --force`, and `pnpm publish`

Hard-denied and denylisted commands never enter the approval queue; they return a structured tool error (`code: command_policy_denied`, plus `policy_code`) to the model.
Ordinary high-risk workspace writes under `.git` / `.xcoding` always need approval, even in auto-edit.


Optional: add workspace skills under `.xcoding/skills/<name>/SKILL.md` so the agent can call `load_skill`.

Optional: configure stdio MCP servers in `.xcoding/mcp.json`:

```json
{
  "mcpServers": {
    "demo": {
      "command": "node",
      "args": ["mock-mcp-server.mjs"],
      "enabled": true
    }
  }
}
```

Enabled servers are started for each agent turn. Their tools appear to the model as namespaced functions `mcp__<server>__<tool>`. Protocol tool name is `mcp` with arguments `{ server, tool, arguments }`. Every MCP call requires user approval in both `ask` and `auto-edit` modes. `xcoding doctor` reports `mcp_config` status.

## Next Reading

- [Session Recovery And Safety](./session-safety.md)
- [Desktop](../desktop.md)
- [Protocol](./protocol.md)

## Continue a session

Follow up in an existing finished session (same id, shared history):

```bash
pnpm cli -- chat "What about the CLI package?" --workspace . --session <session-id>
```

Desktop: select a finished session, then send another message (button shows **Continue**). Use **New chat** to start a fresh session.
