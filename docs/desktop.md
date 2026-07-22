# XCoding Desktop

## Run

Set cloud-model credentials in your shell, then start the Tauri shell:

```powershell
$env:OPENAI_API_KEY = "..."
# Optional for OpenAI-compatible cloud providers.
$env:OPENAI_BASE_URL = "https://api.openai.com/v1"
pnpm --filter @xcoding/desktop exec tauri dev
```

The app stores local session history in its operating-system application-data directory. Credentials are read from the environment only and are not written to the session database.

## First workflow

1. Enter the absolute workspace path in the left panel.
2. Send a repository question from the composer.
3. Watch the plan, streamed response, and read-only tool activity.
4. Select a saved session to review its status.

Desktop currently runs the same read-only agent service as the CLI server. The default mode is `ask`; write and command execution controls arrive in Phase 2.
