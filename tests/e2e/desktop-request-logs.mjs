import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const read = (path) => readFile(new URL("../../" + path, import.meta.url), "utf8");
const [app, component, commands, main, protocol, translations] = await Promise.all([
  "apps/desktop/src/App.tsx", "apps/desktop/src/RequestLogs.tsx",
  "apps/desktop/src-tauri/src/request_logs.rs", "apps/desktop/src-tauri/src/main.rs",
  "packages/protocol/src/index.ts", "apps/desktop/src/i18n.ts",
].map(read));

assert.ok(app.includes('{ id: "requestLogs", labelKey: "settings.tab.requestLogs" }'));
assert.ok(app.includes("setRecordModelRequests(config.record_model_requests ?? false)"));
assert.ok(app.includes("record_model_requests: recordModelRequests"));
assert.ok(app.includes("setRecordModelRequests(savedUser.record_model_requests ?? false)"));
assert.ok(protocol.includes("record_model_requests?: boolean"));
assert.ok(component.includes('role="switch" aria-checked={enabled}'));
assert.ok(component.includes("queryLogs(emptyFilters, 0)"));
assert.ok(component.includes("offset - 10"));
assert.ok(component.includes("offset + 10"));
assert.ok(component.includes("queryLogs(applied, offset"));
assert.ok(component.includes("version !== queryVersion.current"));
assert.ok(component.includes("from?.toISOString() ?? null"));
assert.ok(component.includes("detail.request_body"));
assert.ok(component.includes("detail.request_headers"));
assert.ok(component.includes('"requestLogs.headersUnavailable"'));
assert.ok(component.includes("detail.response_body"));
assert.ok(component.includes("detail.truncated"));
assert.ok(!component.includes("dangerouslySetInnerHTML"));
for (const command of ["query_model_request_logs", "model_request_log_detail"]) {
  assert.ok(component.includes('"' + command + '"'));
  assert.ok(commands.includes("pub async fn " + command));
  assert.ok(main.includes("request_logs::" + command));
}
const keys = new Set([...component.matchAll(/"(requestLogs\.[A-Za-z]+)"/g)].map((match) => match[1]));
for (const key of keys) {
  assert.equal(translations.split('"' + key + '":').length - 1, 2, key + " needs both locales");
}
console.log("desktop request log settings and IPC contracts passed");
