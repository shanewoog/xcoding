import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

function classifyActivitySummary(summary, state = "running") {
  const text = summary.trim();
  if (/^Auto-applying\b/i.test(text)) return "auto-apply";
  if (/^Auto-running\b/i.test(text)) return "auto-run";
  if (/^Awaiting approval\b/i.test(text)) return "awaiting";
  if (/^Blocked\b/i.test(text)) return "blocked";
  if (/HIGH-RISK/i.test(text)) return "high-risk";
  if (state === "failed" && /patch conflict/i.test(text)) return "conflict";
  if (state === "failed") return "failed";
  if (state === "done") return "done";
  if (/^Running\b/i.test(text)) return "running";
  return state === "running" ? "running" : "generic";
}

function activityPolicyBadge(policy) {
  switch (policy) {
    case "auto-apply":
      return "AUTO-APPLY";
    case "auto-run":
      return "AUTO-RUN";
    case "awaiting":
      return "AWAITING";
    case "blocked":
      return "BLOCKED";
    case "high-risk":
      return "HIGH-RISK";
    case "conflict":
      return "CONFLICT";
    default:
      return null;
  }
}


function eventActivity(event, sequence) {
  if (event.type === "tool_end") {
    const label = event.summary;
    const state = event.success ? "done" : "failed";
    const isConflict =
      !event.success &&
      (event.tool_call?.name === "apply_patch" || /patch conflict/i.test(label)) &&
      /patch conflict/i.test(label);
    return {
      id: event.tool_call?.id ?? sequence,
      label,
      detail: isConflict
        ? "Re-read the file and retry apply_patch with updated old_text."
        : "",
      state,
      policy: isConflict ? "conflict" : classifyActivitySummary(label, state),
    };
  }
  return {
    id: sequence,
    label: String(event.summary ?? event.type ?? "activity"),
    detail: "",
    state: "running",
    policy: "generic",
  };
}
function mergeActivity(previous, next) {
  const distinctive = new Set(["auto-apply", "auto-run", "awaiting", "blocked", "high-risk", "conflict"]);
  if (!previous) return next;
  if (distinctive.has(previous.policy) && !distinctive.has(next.policy)) {
    return { ...next, policy: previous.policy };
  }
  return next;
}

async function main() {
  const activitySource = await readFile(resolve(repositoryRoot, "apps/desktop/src/activity.ts"), "utf8");
  const appSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/App.tsx"), "utf8");
  const cssSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/styles.css"), "utf8");
  const configSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/config.ts"), "utf8");
  const i18nSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/i18n.ts"), "utf8");
  const docsDesktop = await readFile(resolve(repositoryRoot, "docs/desktop.md"), "utf8");
  const roadmapEn = await readFile(resolve(repositoryRoot, "docs/en/roadmap.md"), "utf8");
  const roadmapZh = await readFile(resolve(repositoryRoot, "docs/zh/roadmap.md"), "utf8");

  for (const needle of [
    "export function classifyActivitySummary",
    "export function activityPolicyBadge",
    "export function mergeActivity",
    "export function eventActivity",
    "export function buildActivity",
    "export function isPatchConflictSummary",
    "approval_requested",
    '"conflict"',
  ]) {
    assert.ok(activitySource.includes(needle), "activity.ts missing " + needle);
  }

  // Activity is still derived from session events, but the former activity panel
  // was intentionally removed from the desktop chat surface.
  for (const needle of [
    'from "./activity"',
    "buildActivity",
    "eventActivity",
    "mergeActivity",
  ]) {
    assert.ok(appSource.includes(needle), "App.tsx missing " + needle);
  }
  assert.ok(!appSource.includes("activity-badge"), "the removed activity badge must not be rendered");
  assert.ok(appSource.includes("run-plan-list"), "the compact run status should expose task steps when expanded");
  assert.ok(appSource.includes("currentRunPlanStep(plan, activity)"), "run status should derive the current task step");
  assert.ok(cssSource.includes("position: sticky"), "run status should stay visible while the conversation scrolls");

  // A new task is collapsed by default, but incoming stream updates must not
  // override a user opening the live thinking/status details panel.
  assert.ok(appSource.includes("const [runStatusExpanded, setRunStatusExpanded] = useState(false)"), "run status should default to collapsed");
  assert.ok(appSource.includes("open={runStatusExpanded}"), "run status details should remain controlled");
  assert.ok(
    appSource.includes("onToggle={(event) => setRunStatusExpanded(event.currentTarget.open)}"),
    "toggling run status details should preserve the user's selection",
  );
  const patchRunStatusSource = appSource.match(/const patchRunStatus =[\s\S]*?const patchStream/)?.[0] ?? "";
  assert.ok(patchRunStatusSource, "App.tsx should define patchRunStatus before patchStream");
  assert.ok(
    !patchRunStatusSource.includes("setRunStatusExpanded(false)"),
    "streamed run-status updates must not collapse details opened by the user",
  );
  assert.ok(
    /runStatus\.phase === "failed"\s*\? t\(locale, "activity\.agentError"\)/.test(appSource),
    "a failed run must show a compact Agent error summary instead of task progress",
  );
  assert.ok(
    appSource.includes('setRunStatusExpanded(false);') && appSource.includes('if (payload.type === "error")'),
    "provider errors must collapse the process details by default",
  );
  assert.match(
    appSource,
    /setSessionRunStatus\(sid, \{ startedAt: Date\.now\(\), phase: "thinking" \}\);\s+if \(touchesActive\(sid\)\) \{\s+commitStreamedAssistant\(sid\);\s+setActivity\(\[\]\);/,
    "continuing a session must clear stale activity from the previous cancelled run",
  );
  assert.ok(
    appSource.includes('type === "session_cancelled" || type === "task_completed" || type === "error"') &&
      appSource.includes("activityEvents = detail.events.slice(previousRunEnd + 1)") &&
      appSource.includes("setActivity(buildActivity(activityEvents, locale))"),
    "rehydrating a running session must not restore activity from an earlier cancelled run",
  );
  assert.ok(cssSource.includes(".run-status-dots.failed"), "failed run status should use a static error indicator");
  assert.ok(
    appSource.includes("function completedRunElapsedByMessageId(messages: Message[]): Record<string, string>"),
    "completed assistant messages should derive a persistent elapsed time from message timestamps",
  );
  assert.ok(
    appSource.includes("elapsedByMessageId[message.id] = formatRunElapsed(turnStartedAt, completedAt)"),
    "completed elapsed time should be measured from the preceding user message to its assistant reply",
  );
  assert.ok(
    appSource.includes('t(locale, "run.processed", { elapsed })'),
    "completed assistant messages should render their elapsed time in the conversation",
  );
  assert.ok(cssSource.includes(".message-completed-elapsed"), "completed elapsed time should have a persistent message style");
  assert.ok(i18nSource.includes('"run.processed": "Processed {elapsed}"'), "completed elapsed time should be localized in English");
  assert.ok(i18nSource.includes('"run.processed": "已处理 {elapsed}"'), "completed elapsed time should be localized in Chinese");
  assert.ok(!appSource.includes("activity-header"), "the removed activity panel header must not be rendered");
  assert.ok(!cssSource.includes(".activity-header"), "styles must not retain the removed activity panel header");
  assert.ok(appSource.includes("const [conversationAtBottom, setConversationAtBottom] = useState(true)"), "conversation should track whether the user is reading the latest content");
  assert.ok(appSource.includes("!node || !conversationAtBottomRef.current"), "stream updates must not pull a user back to the bottom");
  assert.ok(appSource.includes("onScroll={(event) => updateConversationBottomState(event.currentTarget)}"), "conversation scrolling should update the pinned state");
  assert.ok(appSource.includes('className="scroll-to-bottom-button"'), "a jump-to-latest control should be rendered above the composer");
  assert.ok(cssSource.includes(".scroll-to-bottom-button"), "jump-to-latest control should be styled");
  assert.ok(i18nSource.includes('"run.scrollBottom"'), "jump-to-latest control should be localized");
  assert.ok(i18nSource.includes('"activity.fileCreateFailed": "Create failed"'), "failed file creation should be localized in English");
  assert.ok(i18nSource.includes('"activity.fileEditFailed": "Edit failed"'), "failed file editing should be localized in English");
  assert.ok(i18nSource.includes('"activity.fileCreateFailed": "创建失败"'), "failed file creation should be localized in Chinese");
  assert.ok(i18nSource.includes('"activity.fileEditFailed": "编辑失败"'), "failed file editing should be localized in Chinese");
  assert.ok(appSource.includes('function inlineActivityLabel('), "failed inline file actions should use a distinct label");
  assert.ok(appSource.includes('entry.fileExisted ? "activity.fileEditFailed" : "activity.fileCreateFailed"'), "failed inline file labels should distinguish edits from creates");
  assert.ok(appSource.includes('label: inlineActivityLabel(existing, event.success ? "done" : "failed", locale)'), "session replay should show failed file labels");
  assert.ok(appSource.includes('label: inlineActivityLabel(next, state, locale)'), "live activity updates should show failed file labels");
  const inlineActivityListSource = appSource.match(/function InlineActivityList[\s\S]*?\n}\n\ntype SettingsTab/)?.[0] ?? "";
  assert.ok(inlineActivityListSource, "App.tsx should define InlineActivityList");
  assert.ok(
    inlineActivityListSource.includes("<details className={`inline-activity-group ${groupState}`}>") &&
      !inlineActivityListSource.includes("<details className={`inline-activity-group ${groupState}`} open"),
    "inline tool activity groups should be collapsed by default",
  );
  assert.ok(
    inlineActivityListSource.includes('className="inline-activity-group-summary"'),
    "inline tool activity groups should expose a clickable summary",
  );
  assert.ok(
    inlineActivityListSource.includes('t(locale, "activity.toolCalls", { count: items.length })'),
    "inline tool activity summaries should show the item count",
  );
  assert.ok(cssSource.includes(".inline-activity-group[open]"), "expanded inline tool activity groups should be styled");
  assert.ok(i18nSource.includes('"activity.toolCalls": "Tool calls: {count}"'), "tool activity count should be localized in English");
  assert.ok(i18nSource.includes('"activity.toolCalls": "工具调用 {count} 项"'), "tool activity count should be localized in Chinese");

  assert.ok(
    i18nSource.includes("allowlisted safe commands") || configSource.includes("allowlisted safe commands"),
    "modeHelpText should mention allowlisted commands",
  );
  assert.ok(
    !configSource.includes("Commands still need approval."),
    "stale commands-always-need-approval mode help should be gone",
  );
  assert.ok(docsDesktop.includes("allowlisted safe commands"), "desktop.md should mention allowlist");
  assert.ok(roadmapEn.includes("allowlisted commands"), "roadmap EN item 6 should mention allowlist");
  assert.ok(roadmapZh.includes("白名单命令"), "roadmap ZH item 6 should mention allowlist");

  assert.equal(classifyActivitySummary("Auto-applying apply_patch"), "auto-apply");
  assert.equal(classifyActivitySummary("Auto-running run_command"), "auto-run");
  assert.equal(classifyActivitySummary("Awaiting approval for run_command"), "awaiting");
  assert.equal(classifyActivitySummary("Blocked run_command"), "blocked");
  assert.equal(
    classifyActivitySummary("Review HIGH-RISK command: powershell -Command dir"),
    "high-risk",
  );
  assert.equal(activityPolicyBadge("auto-apply"), "AUTO-APPLY");
  assert.equal(activityPolicyBadge("auto-run"), "AUTO-RUN");
  assert.equal(activityPolicyBadge("awaiting"), "AWAITING");
  assert.equal(activityPolicyBadge("high-risk"), "HIGH-RISK");
  assert.equal(activityPolicyBadge("conflict"), "CONFLICT");
  assert.equal(
    classifyActivitySummary(
      "patch conflict on notes.txt: file contents changed; re-read the file and retry with updated old_text",
      "failed",
    ),
    "conflict",
  );
  const conflictItem = eventActivity(
    {
      type: "tool_end",
      tool_call: { id: "call_conflict", name: "apply_patch", arguments: { path: "notes.txt" } },
      success: false,
      summary:
        "patch conflict on notes.txt: file contents changed; re-read the file and retry with updated old_text",
    },
    "seq-conflict",
  );
  assert.equal(conflictItem.policy, "conflict");
  assert.match(conflictItem.detail, /re-read the file/i);
  assert.equal(activityPolicyBadge("running"), null);
  const conflictMerged = mergeActivity(
    {
      id: "conflict-1",
      label: "patch conflict on notes.txt: file contents changed; re-read the file and retry with updated old_text",
      detail: "Re-read the file and retry apply_patch with updated old_text.",
      state: "failed",
      policy: "conflict",
    },
    {
      id: "conflict-1",
      label: "done",
      detail: "",
      state: "done",
      policy: "done",
    },
  );
  assert.equal(conflictMerged.policy, "conflict");


  const merged = mergeActivity(
    {
      id: "1",
      label: "Auto-applying apply_patch",
      detail: "{}",
      state: "running",
      policy: "auto-apply",
    },
    {
      id: "1",
      label: "Applied patch to src/a.ts",
      detail: "{}",
      state: "done",
      policy: "done",
    },
  );
  assert.equal(merged.state, "done");
  assert.equal(merged.policy, "auto-apply");
  assert.equal(merged.label, "Applied patch to src/a.ts");

  console.log("Desktop activity policy UX checks passed.");
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
