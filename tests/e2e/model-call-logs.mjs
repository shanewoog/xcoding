import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

async function main() {
  const [appSource, cssSource, i18nSource, protocolSource] = await Promise.all([
    readFile(resolve(repositoryRoot, "apps/desktop/src/App.tsx"), "utf8"),
    readFile(resolve(repositoryRoot, "apps/desktop/src/styles.css"), "utf8"),
    readFile(resolve(repositoryRoot, "apps/desktop/src/i18n.ts"), "utf8"),
    readFile(resolve(repositoryRoot, "packages/protocol/src/index.ts"), "utf8"),
  ]);

  assert.ok(appSource.includes('"model-logs"'), "App should expose the model-call log view");
  assert.ok(appSource.includes("composer-model-log-button"), "composer should expose a model-call log entry");
  assert.ok(appSource.includes('item.event.type === "model_call"'), "log view should filter model_call events");
  assert.ok(appSource.includes("session_detail"), "log view should read persisted session details");
  assert.ok(
    appSource.includes("const pendingConversationScrollToBottomRef = useRef(false)"),
    "model log navigation should track a pending jump to the latest conversation content",
  );
  assert.ok(
    appSource.includes("if (pendingConversationScrollToBottomRef.current)"),
    "returning from model logs should consume the pending jump after the conversation remounts",
  );
  assert.ok(
    appSource.includes("onClick={returnToConversationFromModelLogs}"),
    "the model log back button should return through the scroll-to-bottom handler",
  );
  assert.ok(
    appSource.includes("const observer = new ResizeObserver"),
    "conversation bottom-follow mode should survive workbench layout resizing",
  );
  assert.ok(
    appSource.includes("observer.observe(node)"),
    "conversation resizing should be observed after the workbench mounts",
  );
  assert.ok(protocolSource.includes('type: "model_call"'), "desktop protocol missing model_call event");
  assert.ok(protocolSource.includes("output_chars"), "desktop protocol missing sanitized output count");
  assert.ok(protocolSource.includes("tool_calls"), "desktop protocol missing tool-call count");
  assert.ok(cssSource.includes(".model-logs-page"), "model logs page styles are missing");
  assert.ok(cssSource.includes(".model-log-error"), "model log error styles are missing");
  assert.ok(i18nSource.includes('"logs.title"'), "model logs title translation is missing");
  assert.ok(i18nSource.includes('"logs.subtitle"'), "model logs safety copy is missing");
  console.log("Model call log desktop checks passed.");
}

main().catch((error) => {
  console.error(error instanceof Error ? error.stack : String(error));
  process.exitCode = 1;
});
