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
  switch (mode) {
    case "full-auto":
      return "Full auto";
    case "auto-edit":
      return "Auto edit";
    default:
      return "Ask";
  }
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

const DRAFT_SESSION_KEY = "__draft__";

function sessionStateKey(sessionId) {
  const trimmed = sessionId?.trim();
  return trimmed ? trimmed : DRAFT_SESSION_KEY;
}

function rightPanelStateFor(sessionId, openBySession, tabBySession, defaultTab) {
  const key = sessionStateKey(sessionId);
  return { open: openBySession[key] === true, tab: tabBySession[key] ?? defaultTab };
}

function dropSessionKey(map, sessionId) {
  const key = sessionStateKey(sessionId);
  if (!(key in map)) return map;
  const { [key]: _removed, ...rest } = map;
  return rest;
}

function adoptDraftSessionKey(map, sessionId) {
  const key = sessionStateKey(sessionId);
  if (key === DRAFT_SESSION_KEY || !(DRAFT_SESSION_KEY in map)) return map;
  const { [DRAFT_SESSION_KEY]: draft, ...rest } = map;
  if (key in rest) return rest;
  return { ...rest, [key]: draft };
}

function markSessionCompletedUnseen(current, sessionId) {
  const id = sessionId.trim();
  if (!id || current.includes(id)) return current;
  return [...current, id];
}

function clearSessionCompletedUnseen(current, sessionId) {
  const id = sessionId.trim();
  if (!id || !current.includes(id)) return current;
  return current.filter((entry) => entry !== id);
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
  const i18nSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/i18n.ts"), "utf8");

  for (const needle of [
    "export function formatSessionStatus",
    "export function formatMessageRole",
    "export function formatModeLabel",
    "export function formatRelativeTime",
    "export function sessionMetaLine",
    "export function hasTraceContent",
    "export function sessionStateKey",
    "export function rightPanelStateFor",
    "export function dropSessionKey",
    "export function adoptDraftSessionKey",
    "export function markSessionCompletedUnseen",
    "export function clearSessionCompletedUnseen",
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
    'invoke("terminal_start"',
    "fetchGitEnvironment",
  ]) {
    assert.ok(panelsSource.includes(needle) || appSource.includes(needle), "panel wiring missing " + needle);
  }

  for (const needle of [
    "terminal-context-menu",
    "navigator.clipboard.writeText",
    "navigator.clipboard.readText",
    "getSelection()",
    "term.paste(text)",
    "term.selectAll()",
    "term.clear()",
  ]) {
    assert.ok(panelsSource.includes(needle), "terminal context menu missing " + needle);
  }
  for (const key of ["action.paste", "action.selectAll", "action.clearTerminal", "terminal.contextMenu"]) {
    assert.ok(i18nSource.includes(`"${key}"`), "terminal context menu translation missing " + key);
  }
  assert.ok(cssSource.includes(".terminal-context-menu"), "terminal context menu styles missing");

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
  /return \(\) => \{\s*mountedRef\.current = false;\s*void browserHide\(sessionKey\)\.catch\(\(\) => undefined\);/,
  "native browser should hide when the side panel unmounts",
);
assert.ok(cssSource.includes(".builtin-browser"), "built-in browser styles missing");
assert.ok(panelsSource.includes("browserFind") && apiSource.includes("browserFind"), "browser find wiring missing");
assert.ok(panelsSource.includes("browserScreenshot") && apiSource.includes("browserScreenshot"), "browser screenshot wiring missing");
assert.ok(panelsSource.includes("browser-settings") && panelsSource.includes("xcoding.browserSettings.v1"), "browser settings panel missing");
assert.ok(panelsSource.includes("browser-devicebar") && panelsSource.includes("deviceToolbar"), "browser device toolbar missing");
assert.ok(
  panelsSource.includes("xcoding.browserHistory.v1") &&
    panelsSource.includes('className="browser-history"') &&
    panelsSource.includes("recordHistory(nextUrl"),
  "browser history panel must persist visits recorded from navigation events",
);
assert.ok(
  panelsSource.includes("const overlayOpen = settingsOpen || historyOpen") &&
    panelsSource.includes("!overlayOpen && !menuOpen && !refreshMenuOpen"),
  "history panel must hide the native webview like the settings panel does",
);
assert.ok(cssSource.includes(".browser-history"), "browser history styles missing");
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

  // Right tools panel must follow the task it was opened in.
  assert.ok(
    appSource.includes("rightPanelOpenBySession") && appSource.includes("rightPanelTabBySession"),
    "App.tsx should track right panel state per session",
  );
  assert.ok(
    !appSource.includes("useState<ToolPanelTab>(\"review\")"),
    "App.tsx should not keep a single global right panel tab",
  );
  assert.ok(appSource.includes("rightPanelStateFor<ToolPanelTab>"), "App.tsx should derive right panel state per session");
  assert.ok(
    appSource.includes("sessionStateKey(activeSessionIdRef.current)"),
    "right panel writers must key off the session currently in view",
  );
  assert.ok(appSource.includes("adoptDraftRightPanelState"), "draft right panel state must migrate to the created session");
  assert.ok(
    appSource.includes("dropSessionKey(current, sessionId)"),
    "deleting a session must drop its right panel state",
  );
  assert.ok(
    appSource.includes("key={sessionStateKey(activeSessionId)}"),
    "RightToolsPanel must remount per session so the embedded browser cannot leak another task's page",
  );
  assert.ok(
    appSource.includes("browserNavigationBySession") && !appSource.includes("setBrowserNavigation("),
    "assistant link navigation must be tracked per session",
  );
  assert.ok(
    appSource.includes("browserStateBySession"),
    "browser tabs and current page must be tracked per session",
  );
  assert.ok(
    appSource.includes("setBrowserStateBySession((current) => adoptDraftSessionKey(current, sessionId))"),
    "draft browser state must migrate to the created session",
  );
  assert.ok(
    appSource.includes("setBrowserStateBySession((current) => dropSessionKey(current, sessionId))"),
    "deleting a session must drop its browser state",
  );
  assert.ok(
    appSource.includes("browserState={browserState}") && appSource.includes("onBrowserStateChange="),
    "App must pass the active session browser state to the right tools panel",
  );
  assert.ok(
    panelsSource.includes("initialState={browserState}") && panelsSource.includes("onStateChange={onBrowserStateChange}"),
    "RightToolsPanel must pass browser state through to BuiltInBrowserPanel",
  );
  assert.ok(
    panelsSource.includes("initialState?.tabs.length") &&
      panelsSource.includes("onStateChangeRef.current({ tabs, activeTabId, handledNavigationId })"),
    "BuiltInBrowserPanel must restore and report its tab state",
  );

  // Returning to a task must not hard-reload the page it already had open.
  const browserRustSource = await readFile(
    resolve(repositoryRoot, "apps/desktop/src-tauri/src/browser.rs"),
    "utf8",
  );
  const mainRustSource = await readFile(
    resolve(repositoryRoot, "apps/desktop/src-tauri/src/main.rs"),
    "utf8",
  );
  assert.ok(
    browserRustSource.includes("pub struct BrowserRegistry") && browserRustSource.includes("fn browser_label(index: u64)"),
    "browser.rs must keep one webview per task instead of a single shared label",
  );
  assert.ok(
    !browserRustSource.includes("const SIDE_BROWSER_LABEL"),
    "the single global side-browser label must be gone",
  );
  const ensureStart = browserRustSource.indexOf("pub async fn browser_ensure");
  const ensureBody = browserRustSource.slice(
    ensureStart,
    browserRustSource.indexOf("#[tauri::command]", ensureStart),
  );
  assert.ok(ensureStart > 0 && ensureBody.length > 0, "browser_ensure must exist");
  assert.ok(
    !ensureBody.includes("browser_navigate_inner"),
    "browser_ensure must not navigate an existing webview",
  );
  assert.ok(
    browserRustSource.includes("pub async fn browser_adopt_session"),
    "a draft task's live webview must be adoptable by the created session",
  );
  assert.ok(
    browserRustSource.includes("if !registry(app).is_active(session)"),
    "only the visible task may write the shared browser-state.json snapshot",
  );
  assert.ok(
    browserRustSource.includes("session: String,") && browserRustSource.includes("BrowserNavigatedPayload {"),
    "browser-navigated must report which task navigated",
  );
  assert.ok(
    mainRustSource.includes("browser::BrowserRegistry::default()") && mainRustSource.includes("browser::browser_adopt_session"),
    "main.rs must register the browser registry state and the adopt command",
  );
  assert.ok(
    apiSource.includes("export async function browserAdoptSession(from: string, to: string)") &&
      apiSource.includes("browserEnsure(\n  session: string,"),
    "workspaceApi must expose per-session browser commands",
  );
  assert.ok(
    panelsSource.includes("sessionKey={sessionKey}") && panelsSource.includes("browserHide(sessionKey)"),
    "BuiltInBrowserPanel must address its own task's webview",
  );
  assert.ok(
    panelsSource.includes("if (!ensured.created && ensured.url)") && panelsSource.includes("adoptLiveUrl"),
    "the panel must adopt the live page after switching back instead of reloading it",
  );
  assert.ok(
    panelsSource.includes("handledNavigationId") && panelsSource.includes("initialState?.handledNavigationId ?? null"),
    "an already-opened assistant link must not be replayed when returning to a task",
  );
  assert.ok(
    panelsSource.includes("if (id === activeTabIdRef.current)"),
    "re-selecting the open tab must not reload it",
  );
  // Exactly one task's page may be on screen: a background task must never float
  // over the chat area.
  assert.ok(
    browserRustSource.includes("fn hide_other_browsers") && browserRustSource.includes("fn hide_inactive_browsers"),
    "browser.rs must be able to sweep every side webview that is not in view",
  );
  assert.ok(
    browserRustSource.includes("for (label, webview) in app.webviews()"),
    "the sweep must walk real webview labels so orphans are caught too",
  );
  const showStart = browserRustSource.indexOf("pub async fn browser_show");
  const showBody = browserRustSource.slice(
    showStart,
    browserRustSource.indexOf("#[tauri::command]", showStart),
  );
  assert.ok(
    showBody.includes("if !registry(&app).is_active(&session)"),
    "a stale show from a task that already switched away must be ignored",
  );
  assert.ok(
    showBody.includes("hide_other_browsers"),
    "showing a task's page must hide every other task's page",
  );
  assert.ok(
    panelsSource.includes("if (!mountedRef.current)") && panelsSource.includes("mountedRef.current = false"),
    "an unmounted browser panel must not finish a pending show",
  );
  assert.ok(
    appSource.includes("browserAdoptSession(DRAFT_SESSION_KEY, sessionStateKey(sessionId))"),
    "promoting a draft task must migrate its webview instead of recreating it",
  );
  assert.ok(
    appSource.includes("browserClose(sessionStateKey(sessionId))"),
    "deleting a task must release its native webview",
  );
  assert.ok(
    appSource.includes("sessionKey={activeSessionStateKey}"),
    "App must tell the right tools panel which task it is rendering",
  );

  assert.equal(formatSessionStatus("need_user"), "needs review");
  assert.equal(formatSessionStatus("running"), "running");
  assert.equal(formatMessageRole("user"), "You");
  assert.equal(formatMessageRole("assistant"), "Assistant");
  assert.equal(formatModeLabel("ask"), "Ask");
  assert.equal(formatModeLabel("auto-edit"), "Auto edit");
  assert.equal(formatModeLabel("full-auto"), "Full auto");
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

  assert.equal(sessionStateKey(null), DRAFT_SESSION_KEY);
  assert.equal(sessionStateKey("  "), DRAFT_SESSION_KEY);
  assert.equal(sessionStateKey("s1"), "s1");
  assert.deepEqual(rightPanelStateFor("s1", {}, {}, "review"), { open: false, tab: "review" });
  assert.deepEqual(
    rightPanelStateFor("s1", { s1: true }, { s1: "browser" }, "review"),
    { open: true, tab: "browser" },
  );
  // Another session's open panel must not leak into the active one.
  assert.deepEqual(
    rightPanelStateFor("s2", { s1: true }, { s1: "browser" }, "review"),
    { open: false, tab: "review" },
  );
  assert.deepEqual(dropSessionKey({ s1: true, s2: true }, "s1"), { s2: true });
  assert.deepEqual(adoptDraftSessionKey({ [DRAFT_SESSION_KEY]: true }, "s1"), { s1: true });
  assert.deepEqual(adoptDraftSessionKey({ [DRAFT_SESSION_KEY]: true, s1: false }, "s1"), { s1: false });
  assert.deepEqual(adoptDraftSessionKey({ s1: true }, "s1"), { s1: true });

  // Finished-task dot: set for background completions, cleared once the task is opened.
  assert.ok(
    appSource.includes("completedUnseenSessionIds") &&
      appSource.includes('className="session-done-dot"'),
    "App.tsx must render a completion dot for finished background tasks",
  );
  assert.ok(
    appSource.includes("setCompletedUnseenSessionIds((current) => markSessionCompletedUnseen(current, sid))") &&
      appSource.includes("setCompletedUnseenSessionIds((current) => clearSessionCompletedUnseen(current, session.id))"),
    "completion dot must be set on task_completed and cleared when the task is selected",
  );
  assert.ok(cssSource.includes(".session-done-dot"), "styles.css missing .session-done-dot");
  assert.deepEqual(markSessionCompletedUnseen([], "s1"), ["s1"]);
  assert.deepEqual(markSessionCompletedUnseen(["s1"], "s1"), ["s1"]);
  assert.deepEqual(markSessionCompletedUnseen(["s1"], "  "), ["s1"]);
  assert.deepEqual(markSessionCompletedUnseen(["s1"], "s2"), ["s1", "s2"]);
  assert.deepEqual(clearSessionCompletedUnseen(["s1", "s2"], "s1"), ["s2"]);
  assert.deepEqual(clearSessionCompletedUnseen(["s2"], "s1"), ["s2"]);

  console.log("Desktop layout UX checks passed.");
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
