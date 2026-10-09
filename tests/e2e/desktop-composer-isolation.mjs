import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const testDirectory = dirname(fileURLToPath(import.meta.url));
const repositoryRoot = resolve(testDirectory, "../..");
const appSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/App.tsx"), "utf8");

function functionBody(name) {
  const startPattern = new RegExp(`^  (?:async )?function ${name}\\b`, "m");
  const start = appSource.search(startPattern);
  assert.notEqual(start, -1, `App.tsx should define ${name}`);
  const next = appSource.slice(start + 1).search(/\r?\n  (?:async )?function \w+\b/);
  return appSource.slice(start, next < 0 ? appSource.length : start + 1 + next);
}

assert.ok(
  appSource.includes("type ComposerDraft") && appSource.includes("composerDraftBySession"),
  "App.tsx should keep prompt and attachments in per-session composer drafts",
);
assert.doesNotMatch(
  appSource,
  /const \[prompt, setPrompt\] = useState/,
  "App.tsx must not keep one global prompt state",
);
assert.ok(
  appSource.includes("const activeComposerDraft = composerDraftBySession[activeSessionStateKey] ?? EMPTY_COMPOSER_DRAFT"),
  "App.tsx should derive composer content from the active session draft",
);

const selectSessionBody = functionBody("selectSession");
assert.ok(
  selectSessionBody.includes("activeSessionIdRef.current = session.id") &&
    selectSessionBody.includes("const nextSessionKey = sessionStateKey(session.id)") &&
    selectSessionBody.includes("setComposerDraftBySession"),
  "selectSession must keep drafts separate while switching tasks",
);

const resetComposerBody = functionBody("resetComposerSession");
assert.ok(
  resetComposerBody.includes("DRAFT_SESSION_KEY") &&
    resetComposerBody.includes("createEmptyComposerDraft()") &&
    !resetComposerBody.includes("setComposerImages([])") &&
    !resetComposerBody.includes("setComposerTextFiles([])"),
  "resetComposerSession must clear only the new-task composer draft",
);

const deleteSessionBody = functionBody("deleteSession");
assert.ok(
  deleteSessionBody.includes("setComposerDraftBySession((current) => dropSessionKey(current, sessionId))"),
  "deleting a session must drop its composer draft",
);

const submitComposerBody = functionBody("submitComposer");
assert.ok(
  submitComposerBody.includes("const composerSessionKey = activeSessionStateKey") &&
    submitComposerBody.includes("restoreComposerDraft(message, images, textFiles, composerSessionKey)"),
  "failed sends must restore content to the session that submitted it",
);

const steerCurrentRunBody = functionBody("steerCurrentRun");
assert.ok(
  steerCurrentRunBody.includes("const composerSessionKey = activeSessionStateKey") &&
    steerCurrentRunBody.includes("restoreComposerDraft(message, images, textFiles, composerSessionKey)"),
  "failed steering must restore content to the session that submitted it",
);

const restoreComposerBody = functionBody("restoreComposerDraft");
assert.ok(
  restoreComposerBody.includes("sessionKey: string = activeSessionStateKey") &&
    restoreComposerBody.includes("[sessionKey]: restored"),
  "restoreComposerDraft must write to the captured session key",
);

console.log("composer draft isolation checks passed");
