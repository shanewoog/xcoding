import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { runInNewContext } from "node:vm";
import ts from "typescript";

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
  const parsedApp = ts.createSourceFile("App.tsx", appSource, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let logSelection;
  function findLogSelection(node) {
    if (ts.isCallExpression(node) && node.expression.getText(parsedApp) === "setModelCallLogs") {
      const argument = node.arguments[0]?.getText(parsedApp);
      if (argument?.startsWith("detail.events")) logSelection = argument;
    }
    ts.forEachChild(node, findLogSelection);
  }
  findLogSelection(parsedApp);
  assert.ok(logSelection, "log loading should select model calls before updating state");
  const selectionScript = ts.transpileModule(`const selected = ${logSelection}; selected;`, {
    compilerOptions: { target: ts.ScriptTarget.ES2022 },
  }).outputText;
  for (const count of [0, 12, 50, 51, 125]) {
    const events = Array.from({ length: count }, (_, index) => [
      { id: `call-${index}`, event: { type: "model_call" } },
      { id: `text-${index}`, event: { type: "text_delta" } },
    ]).flat();
    const expected = events.filter((item) => item.event.type === "model_call").slice(-50);
    const selected = runInNewContext(selectionScript, { detail: { events } });
    assert.deepEqual(Array.from(selected, (item) => item.id), expected.map((item) => item.id));
    assert.ok(selected.length <= 50, "the view must retain only the newest 50 calls");
    assert.equal(events.length, count * 2, "view truncation must not modify session history");
  }
  assert.ok(appSource.includes("modelCallLogs.slice().reverse().map"), "newest calls should render first");
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
  assert.ok(protocolSource.includes("provider_name"), "desktop protocol missing the real provider name");
  assert.ok(protocolSource.includes("provider_id"), "desktop protocol missing the provider id");
  assert.ok(protocolSource.includes("key_hint"), "desktop protocol missing the masked credential hint");
  assert.ok(
    appSource.includes("event.provider_name?.trim() || event.provider"),
    "log rows should prefer the configured provider name over the wire protocol",
  );
  assert.ok(
    appSource.includes('t(locale, "logs.credential")') && appSource.includes("event.key_hint"),
    "log rows should show which credential served the call",
  );
  assert.ok(
    appSource.includes('t(locale, "logs.protocol")'),
    "log rows should keep the wire protocol visible next to the provider name",
  );
  assert.ok(
    appSource.includes('"logs.purpose.memoryExtraction"'),
    "memory extraction calls need their own log label",
  );
  assert.ok(
    appSource.includes("function formatMillisecondsAsDuration"),
    "model log durations should be formatted from milliseconds",
  );
  assert.ok(
    appSource.includes("return minutes > 0 ? `${minutes}m ${remainder}s` : `${wholeSeconds}s`;"),
    "model log durations should show seconds and include minutes after one minute",
  );
  assert.ok(
    appSource.includes("formatMillisecondsAsDuration(event.ttft_ms)"),
    "time to first token should render using the duration formatter",
  );
  assert.ok(
    appSource.includes("formatMillisecondsAsDuration(event.total_ms)"),
    "total duration should render using the duration formatter",
  );
  assert.ok(!appSource.includes("{event.ttft_ms} ms"), "time to first token should not render raw milliseconds");
  assert.ok(!appSource.includes("{event.total_ms} ms"), "total duration should not render raw milliseconds");
  assert.ok(i18nSource.includes('"logs.credential"'), "credential label translation is missing");
  assert.ok(i18nSource.includes('"logs.protocol"'), "wire protocol label translation is missing");
  assert.ok(
    i18nSource.includes('"logs.purpose.memoryExtraction"'),
    "memory extraction label translation is missing",
  );
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
