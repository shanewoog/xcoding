import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { runInNewContext } from "node:vm";
import ts from "typescript";

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
assert.equal(component.match(/<RequestLogContent\b/g)?.length, 3, "each detail content area needs a copy control");
const parsedComponent = ts.createSourceFile("RequestLogs.tsx", component, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
const contentComponent = parsedComponent.statements.find(
  (node) => ts.isFunctionDeclaration(node) && node.name?.text === "RequestLogContent",
);
assert.ok(contentComponent, "request log content should share one copy implementation");
let copyStatus = "idle";
let clipboardFails = false;
const copied = [];
const renderContent = runInNewContext(ts.transpileModule(
  `${contentComponent.getText(parsedComponent)}\nRequestLogContent;`,
  { compilerOptions: { target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.React } },
).outputText, {
  React: { createElement: (type, props, ...children) => ({ type, props: props ?? {}, children }) },
  useState: () => [copyStatus, (next) => { copyStatus = next; }],
  t: (_locale, key) => key,
  navigator: { clipboard: { writeText: async (text) => {
    if (clipboardFails) throw new Error("clipboard unavailable");
    copied.push(text);
  } } },
});
function findElement(node, predicate) {
  if (!node || typeof node !== "object") return undefined;
  if (predicate(node)) return node;
  return node.children?.flat(Infinity).map((child) => findElement(child, predicate)).find(Boolean);
}
for (const content of [
  "authorization: [REDACTED]\ncontent-type: application/json",
  '{"input":"中文 <script> & \\n","stream":true}',
  'data: {"delta":"你好"}\r\n\r\ndata: [DONE]\r\n\r\n',
]) {
  copyStatus = "idle";
  const props = { locale: "zh-CN", title: "Content", content };
  const tree = renderContent(props);
  const button = findElement(tree, (node) => node.type === "button");
  assert.equal(button.props.disabled, false);
  assert.deepEqual(findElement(tree, (node) => node.type === "pre").children, [content]);
  await button.props.onClick();
  assert.equal(copied.at(-1), content, "copy must preserve the displayed raw content exactly");
  assert.deepEqual(findElement(renderContent(props), (node) => node.type === "button").children, ["requestLogs.copied"]);
}
copyStatus = "idle";
const empty = renderContent({ locale: "en", title: "Headers", content: "", emptyText: "Unavailable" });
assert.equal(findElement(empty, (node) => node.type === "button").props.disabled, true);
assert.deepEqual(findElement(empty, (node) => node.type === "pre").children, ["Unavailable"]);
clipboardFails = true;
const failureProps = { locale: "en", title: "Body", content: "raw body" };
await findElement(renderContent(failureProps), (node) => node.type === "button").props.onClick();
assert.ok(findElement(renderContent(failureProps), (node) => node.props.role === "alert"), "copy errors must be visible");
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
