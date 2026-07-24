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
  const layoutSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/layout.ts"), "utf8");

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
    "status-badge",
    "empty-chat",
    "empty-hints",
    "onComposerKeyDown",
    "conversationRef",
    "showTraceContent",
    "chat.tip",
    'from "./layout"',
    'from "./i18n"',
    'id="ui-locale"',
    "workspace-missing",
    "loadLocale",
    "saveLocale",
    "field.workspaceHint",
  ]) {
    assert.ok(appSource.includes(needle), "App.tsx missing " + needle);
  }

  for (const needle of [
    ".status-badge",
    ".sessions-top",
    ".empty-chat",
    ".empty-hints",
    ".trace-empty",
    "position: sticky",
    ".review-panel",
    "status-need_user",
    "input.workspace-missing",
    "#ui-locale",
  ]) {
    assert.ok(cssSource.includes(needle), "styles.css missing " + needle);
  }

  assert.match(cssSource, /\.review-panel\s*\{[\s\S]*?position:\s*sticky/, "review panel should be sticky");
  assert.ok(
    appSource.includes("event.ctrlKey || event.metaKey") && appSource.includes('event.key === "Enter"'),
    "composer should submit on Ctrl/Cmd+Enter",
  );

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
