import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const appSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/App.tsx"), "utf8");
const storeSource = await readFile(resolve(repositoryRoot, "crates/xcoding-store/src/lib.rs"), "utf8");
const coreSource = await readFile(resolve(repositoryRoot, "crates/xcoding-core/src/lib.rs"), "utf8");
const tauriSource = await readFile(resolve(repositoryRoot, "apps/desktop/src-tauri/src/main.rs"), "utf8");

function functionBody(name) {
  const startPattern = new RegExp(`^  (?:(?:async )?function ${name}\\b|const ${name} = useCallback)`, "m");
  const start = appSource.search(startPattern);
  assert.notEqual(start, -1, `App.tsx should define ${name}`);
  const next = appSource.slice(start + 1).search(/\r?\n  (?:(?:async )?function \w+\b|const \w+ = useCallback)/);
  return appSource.slice(start, next < 0 ? appSource.length : start + 1 + next);
}

const hydrateSource = functionBody("hydrateSession");
assert.ok(
  hydrateSource.includes("detail.follow_up_queue") && hydrateSource.includes("queuedFollowUpFromEntry"),
  "opening a session must restore its persisted follow-up queue",
);

const enqueueSource = functionBody("enqueueFollowUp");
assert.ok(
  enqueueSource.includes('invoke<FollowUpQueueEntry>("queue_follow_up"') &&
    enqueueSource.includes("await invoke"),
  "queuing a follow-up must persist it before updating the UI",
);

const submitSource = functionBody("submitComposer");
assert.ok(
  submitSource.includes("await enqueueFollowUp(") &&
    submitSource.includes("restoreComposerDraft("),
  "a failed queue write must keep the composer draft instead of losing it",
);

const notStartedSource = functionBody("markQueuedFollowUpsNotStarted");
assert.ok(
  notStartedSource.includes('item.status === "queued"') &&
    notStartedSource.includes('"mark_follow_ups_not_started"') &&
    !notStartedSource.includes("filter((item) => item.sessionId !== sessionId)"),
  "stopping a turn must mark queued follow-ups as not started, not delete them",
);

const drainSource = functionBody("drainFollowUpQueue");
assert.ok(
  drainSource.includes('if (status !== "done")') &&
    drainSource.includes("markQueuedFollowUpsNotStarted(sessionId)"),
  "failed or cancelled turns must preserve queued follow-ups as not started",
);
assert.ok(
  drainSource.includes('item.status === "queued"') &&
    drainSource.includes("followUpId: next.id"),
  "automatic draining must only consume queued entries and pass their persisted id",
);

const sendSource = functionBody("sendChatMessage");
assert.ok(
  sendSource.includes("options?.followUpId") && sendSource.includes("await deleteFollowUp(options.followUpId)"),
  "a queued entry must only be deleted after send preflight succeeds",
);
assert.ok(sendSource.includes("let turnFailed = false;"), "send-chat must track terminal failure for queue draining");
assert.ok(sendSource.includes("turnFailed = true;"), "send-chat must mark failed turns before finally runs");
assert.ok(
  sendSource.includes("await markQueuedFollowUpsNotStarted(failSid);"),
  "failed sessions must preserve remaining queue entries as not started",
);
assert.ok(
  sendSource.includes("if (!options?.skipDrain && !turnFailed)"),
  "failed turns must never invoke automatic follow-up draining",
);

const cancelSource = functionBody("cancelSession");
assert.ok(
  cancelSource.includes("await markQueuedFollowUpsNotStarted(sessionId)"),
  "manual cancellation must preserve remaining queue entries",
);
assert.ok(
  !cancelSource.includes("drainFollowUpQueue(sessionId)"),
  "manual cancellation must not restart queue draining",
);

const cancelFollowUpSource = functionBody("cancelFollowUp");
assert.ok(
  cancelFollowUpSource.includes('"set_follow_up_queue_status"') &&
    cancelFollowUpSource.includes('status: "cancelled"'),
  "cancelling one queued message must persist a cancelled state",
);

const continueSource = functionBody("continueFollowUp");
assert.ok(
  continueSource.includes("followUpId: item.id") && continueSource.includes("sendChatMessage("),
  "not-started entries must remain persisted until the manual send passes preflight",
);

assert.ok(
  appSource.includes('"composer.queueStateNotStarted"') &&
    appSource.includes('"composer.queueStateCancelled"') &&
    appSource.includes('"action.cancelFollowUp"'),
  "the conversation history must show not-started and cancelled queue states with normal actions",
);

assert.ok(
  storeSource.includes("CREATE TABLE IF NOT EXISTS follow_up_queue") &&
    storeSource.includes("pub fn queue_follow_up(") &&
    storeSource.includes("pub fn set_follow_up_queue_status(") &&
    storeSource.includes("pub fn mark_queued_follow_ups_not_started(") &&
    storeSource.includes("pub fn delete_follow_up_queue_entry("),
  "SQLite must own the durable follow-up queue lifecycle",
);
assert.ok(
  storeSource.includes("UPDATE follow_up_queue") &&
    storeSource.includes("FollowUpQueueStatus::NotStarted"),
  "startup reconciliation and turn failures must preserve queued entries as not started",
);
assert.ok(
  storeSource.includes("SELECT id, text, text_files FROM follow_up_queue"),
  "historical secret redaction must include durable queued follow-ups",
);
assert.ok(
  coreSource.includes("follow_up_queue: self.store.list_follow_up_queue(session_id)?"),
  "session detail must include the persisted queue",
);
assert.ok(
  ["queue_follow_up", "set_follow_up_queue_status", "mark_follow_ups_not_started", "delete_follow_up_queue_entry"]
    .every((command) => tauriSource.includes(command)),
  "the desktop bridge must expose the full durable queue lifecycle",
);

console.log("Follow-up queue persistence and recovery checks passed.");
