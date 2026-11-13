import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const appSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/App.tsx"), "utf8");
const i18nSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/i18n.ts"), "utf8");
const cssSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/styles.css"), "utf8");
const releaseScriptSource = await readFile(resolve(repositoryRoot, "scripts/package-desktop-portable.ps1"), "utf8");

assert.match(
  appSource,
  /type SettingsTab = [^\n]*"sessions"/,
  "settings must expose a dedicated session-management tab",
);
assert.ok(
  appSource.includes("function SessionManagerPanel") &&
    appSource.includes("groupSessionsByProject(sessions, diskProjects)"),
  "session management must list sessions across every project",
);
assert.ok(
  appSource.includes('type="checkbox"') &&
    appSource.includes("settings.sessions.selectAll") &&
    appSource.includes("selectedSessionIds.length === sessions.length"),
  "session management must support individual and select-all checkboxes",
);
assert.ok(
  appSource.includes("async function deleteSelectedSessions") &&
    appSource.includes('window.confirm(t(locale, "settings.sessions.deleteConfirm", { count: selectedSessionIds.length }))') &&
    appSource.includes("await onDelete(selectedSessionIds)"),
  "batch deletion must require one confirmation for the current selection",
);
assert.ok(
  appSource.includes("async function deleteManagedSessions") &&
    appSource.includes("for (const sessionId of uniqueSessionIds)") &&
    appSource.includes('await invoke("delete_session", { sessionId })') &&
    appSource.includes("await refreshSessions()"),
  "batch deletion must invoke the existing session-delete command and refresh the session list",
);
assert.ok(
  appSource.includes("function forgetDeletedSessions") &&
    appSource.includes("deleted.has(activeSessionIdRef.current)") &&
    appSource.includes("resetComposerSession()"),
  "batch deletion must clear per-session UI state and reset a deleted active session",
);
assert.ok(
  appSource.includes("settings.sessions.deletePartial") &&
    (i18nSource.match(/"settings.tab.sessions"/g)?.length ?? 0) >= 2 &&
    (i18nSource.match(/"settings.sessions.deleteConfirm"/g)?.length ?? 0) >= 2,
  "session-management labels and destructive warnings must be localized in both languages",
);
assert.ok(
  cssSource.includes(".session-management-group") &&
    cssSource.includes(".session-management-row.is-selected") &&
    cssSource.includes('.session-management-row input[type="checkbox"]') &&
    cssSource.includes(".session-management .danger-button"),
  "session management must provide grouped, selected, checkbox, and destructive-action styles",
);

assert.ok(
  releaseScriptSource.includes("tests/e2e/desktop-session-management.mjs"),
  "portable releases must include the session-management regression test",
);

console.log("Desktop session management checks passed.");
