import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

function formatSessionStatus(status) {
  switch (status) {
    case "need_user":
      return "needs review";
    default:
      return String(status).replaceAll("_", " ");
  }
}

function formatMessageRole(role) {
  switch (role) {
    case "user":
      return "You";
    case "assistant":
      return "Assistant";
    case "tool":
      return "Tool";
    case "system":
      return "System";
    default:
      return role;
  }
}

function formatModeLabel(mode) {
  return mode === "auto-edit" ? "Auto edit" : "Ask";
}

function formatRelativeTime(iso, nowMs = Date.now()) {
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return "";
  const deltaSec = Math.max(0, Math.floor((nowMs - then) / 1000));
  if (deltaSec < 45) return "just now";
  if (deltaSec < 3600) return `${Math.floor(deltaSec / 60)}m ago`;
  if (deltaSec < 86400) return `${Math.floor(deltaSec / 3600)}h ago`;
  if (deltaSec < 86400 * 14) return `${Math.floor(deltaSec / 86400)}d ago`;
  return new Date(then).toLocaleDateString();
}

function sessionMetaLine(session, nowMs = Date.now()) {
  return [formatModeLabel(session.mode), session.model, formatRelativeTime(session.updated_at, nowMs)]
    .filter(Boolean)
    .join(" · ");
}

function hasTraceContent(input) {
  return Boolean(
    input.pendingAction ||
      input.planCount > 0 ||
      input.activityCount > 0 ||
      input.restoreCount > 0 ||
      input.replayCount > 0 ||
      input.taskSummary,
  );
}

async function main() {
  const appSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/App.tsx"), "utf8");
  const cssSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/styles.css"), "utf8");
  assert.ok(appSource.includes("groupConversationMessages"), "App.tsx must group consecutive tool messages");
  assert.ok(appSource.includes('className="message message-tool-group"'), "App.tsx missing collapsed tool group");
  assert.ok(cssSource.includes(".message-tool-group"), "styles.css missing tool group styles");
  const layoutSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/layout.ts"), "utf8");
  const panelsSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/panels.tsx"), "utf8");
const apiSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/workspaceApi.ts"), "utf8");

  for (const needle of [
    "export function formatSessionStatus",
    "export function formatMessageRole",
    "export function formatModeLabel",
    "export function formatRelativeTime",
    "export function sessionMetaLine",
    "export function hasTraceContent",
  ]) {
    assert.ok(layoutSource.includes(needle), "layout.ts missing " + needle);
  }

  for (const needle of [
    "sessions-top",
    "empty-chat",
    "empty-hints",
    "onComposerKeyDown",
    "conversationRef",
    "chat-bottom",
    "pendingAction",
    "chat.tip",
    'from "./layout"',
    'from "./i18n"',
    'id="ui-locale"',
    "workspace-missing",
    "loadLocale",
    "saveLocale",
    "field.workspaceHint",
    "settings-page",
    "sidebar-actions",
    "sidebar-settings-button",
    "SettingsIcon",
    "startNewTask",
    "startNewChat",
    "ensure_chat_workspace",
    "field.chats",
    'setView("settings")',
    "EmptyQuickActions",
    "TerminalBottomPanel",
    "RightToolsPanel",
    "HeaderRightPanelToggle",
    "chat-header-actions",
    "EnvironmentPopover",
  ]) {
    assert.ok(appSource.includes(needle), "App.tsx missing " + needle);
  }

  assert.ok(!appSource.includes('className="trace-panel"'), "right trace panel should be removed");
  assert.ok(
    appSource.includes("chat-bottom") && appSource.includes("className={`review-panel"),
    "review panel should live in chat-bottom",
  );
  for (const needle of [
    "empty-quick-actions",
    "bottom-panel",
    "env-popover",
    "ToolPanelTab",
    "TerminalBottomPanel",
    "RightToolsPanel",
    "right-tools-panel",
    "runTerminalCommand",
    "fetchGitEnvironment",
  ]) {
    assert.ok(panelsSource.includes(needle) || appSource.includes(needle), "panel wiring missing " + needle);
  }

  for (const needle of [
    ".status-badge",
    ".sessions-top",
    ".empty-chat",
    ".empty-hints",
    ".chat-bottom",
    ".review-panel",
    "status-need_user",
    "input.workspace-missing",
    "#ui-locale",
    ".settings-page",
    ".sidebar-actions",
    ".sidebar-settings-button",
    ".bottom-panel",
    ".right-tools-panel",
    ".empty-quick-actions",
    ".env-popover",
    ".chat-header-actions",
  ]) {
    assert.ok(cssSource.includes(needle), "styles.css missing " + needle);
  }

  assert.match(
    cssSource,
    /\.workbench\s*\{[\s\S]*?grid-template-columns:\s*minmax\(230px,\s*280px\)\s+minmax\(0,\s*1fr\);/,
    "workbench should be two columns without a permanent right rail",
  );
  assert.ok(cssSource.includes(".workbench.has-right-panel"), "styles should support optional right tools panel");
assert.ok(panelsSource.includes("BuiltInBrowserPanel"), "right browser should be built-in panel");
assert.ok(panelsSource.includes("browserEnsure") || apiSource.includes("browserEnsure"), "built-in browser API wiring missing");
assert.ok(panelsSource.includes("browserEval") && apiSource.includes("browserEval"), "browser scrollbar injection wiring missing");
assert.ok(
  panelsSource.includes("xcoding-browser-scrollbar-style") &&
    panelsSource.includes("scrollbar-color: #3a4350 transparent") &&
    panelsSource.includes("::-webkit-scrollbar"),
  "embedded browser scrollbar should match the dialog scrollbar",
);
assert.match(
  panelsSource,
  /return \(\) => \{\s*void browserHide\(\)\.catch\(\(\) => undefined\);/,
  "native browser should hide when the side panel unmounts",
);
assert.ok(cssSource.includes(".builtin-browser"), "built-in browser styles missing");
assert.ok(panelsSource.includes("browserFind") && apiSource.includes("browserFind"), "browser find wiring missing");
assert.ok(panelsSource.includes("browserScreenshot") && apiSource.includes("browserScreenshot"), "browser screenshot wiring missing");
assert.ok(panelsSource.includes("browser-settings") && panelsSource.includes("xcoding.browserSettings.v1"), "browser settings panel missing");
assert.ok(panelsSource.includes("browser-devicebar") && panelsSource.includes("deviceToolbar"), "browser device toolbar missing");
assert.ok(cssSource.includes(".browser-findbar") && cssSource.includes(".browser-settings"), "browser extra styles missing");
  assert.ok(cssSource.includes("--right-panel-width"), "right panel width CSS variable missing");
  assert.ok(cssSource.includes(".right-panel-resizer"), "right panel resizer styles missing");
  assert.ok(cssSource.includes("flex: 1") && cssSource.includes(".browser-content"), "browser content should flex-fill");
  assert.ok(panelsSource.includes("right-panel-resizer"), "right panel should expose a drag resizer");
  assert.ok(cssSource.includes("left: -18px") && cssSource.includes("width: 28px"), "splitter needs a wide grab area outside the native browser surface");
  assert.ok(panelsSource.includes("const surfaceRef") && panelsSource.includes("const node = surfaceRef.current"), "native browser should measure a dedicated surface host");
  assert.ok(panelsSource.includes('className="browser-surface-host" ref={surfaceRef}') && cssSource.includes(".browser-surface-host") && cssSource.includes("inset: 6px 6px 6px 14px"), "native browser surface must leave the splitter clear for DOM dragging");
  assert.ok(appSource.includes("loadRightPanelWidth"), "App should persist right panel width");
  assert.ok(layoutSource.includes("export function clampRightPanelWidth"), "layout should clamp right panel width");
  assert.ok(!layoutSource.includes("MAX_RIGHT_PANEL_WIDTH"), "right panel should not have a fixed maximum width");
  assert.ok(
    layoutSource.includes("viewportWidth - MIN_SIDEBAR_WIDTH - MIN_CHAT_WIDTH"),
    "right panel maximum should only protect the sidebar and chat minimum widths",
  );
  assert.ok(!cssSource.includes("minmax(270px, 340px)"), "styles should drop the old permanent right-rail column");
  assert.ok(appSource.includes("has-right-panel"), "App should toggle optional right tools panel");
  assert.ok(!appSource.includes("BottomWorkbenchPanel"), "unified bottom multi-tab panel should be split");
  assert.ok(
    appSource.includes("event.ctrlKey || event.metaKey") && appSource.includes('event.key === "Enter"'),
    "composer should submit on Ctrl/Cmd+Enter",
  );

  // Send stays clickable unless the active session is running; missing setup surfaces as an error instead of a dead button.
  assert.ok(appSource.includes("sendBlockReason"), "App.tsx should compute sendBlockReason");
  assert.ok(appSource.includes("error.needProvider"), "App.tsx should surface missing provider on send");
  assert.ok(
    appSource.includes('disabled={isRunning}') &&
      !appSource.includes("disabled={isRunning || !workspaceRoot.trim() || !prompt.trim()}"),
    "Send should not hard-disable on empty workspace/prompt",
  );
  assert.ok(appSource.includes("runningSessionIds") && appSource.includes("anySessionRunning"), "multi-session running state required");
  assert.ok(appSource.includes("startNewTask()") && appSource.includes("startNewChat()"), "new task/chat entry points required");
  // New task/chat must not be gated on the global/active running flag (parallel sessions).
  assert.ok(!/onClick=\{\(\) => startNewTask\(\)\}[^>]*disabled=\{isRunning\}/.test(appSource), "New task must remain available while sessions run");
  assert.ok(!/onClick=\{\(\) => startNewChat\(\)\}[^>]*disabled=\{isRunning\}/.test(appSource), "New chat must remain available while sessions run");
  assert.ok(cssSource.includes(".send-needs-setup"), "styles.css missing .send-needs-setup");

  assert.equal(formatSessionStatus("need_user"), "needs review");
  assert.equal(formatSessionStatus("running"), "running");
  assert.equal(formatMessageRole("user"), "You");
  assert.equal(formatMessageRole("assistant"), "Assistant");
  assert.equal(formatModeLabel("ask"), "Ask");
  assert.equal(formatModeLabel("auto-edit"), "Auto edit");
  assert.equal(formatRelativeTime(new Date().toISOString()), "just now");
  assert.equal(
    sessionMetaLine({ mode: "ask", model: "gpt-5.5", updated_at: new Date().toISOString() }).includes("Ask"),
    true,
  );
  assert.equal(
    hasTraceContent({
      pendingAction: null,
      planCount: 0,
      activityCount: 0,
      restoreCount: 0,
      replayCount: 0,
      taskSummary: null,
    }),
    false,
  );
  assert.equal(
    hasTraceContent({
      pendingAction: null,
      planCount: 1,
      activityCount: 0,
      restoreCount: 0,
      replayCount: 0,
      taskSummary: null,
    }),
    true,
  );

  console.log("Desktop layout UX checks passed.");
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
