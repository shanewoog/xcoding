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
    default:
      return null;
  }
}

function mergeActivity(previous, next) {
  const distinctive = new Set(["auto-apply", "auto-run", "awaiting", "blocked", "high-risk"]);
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
  const docsDesktop = await readFile(resolve(repositoryRoot, "docs/desktop.md"), "utf8");
  const roadmapEn = await readFile(resolve(repositoryRoot, "docs/en/roadmap.md"), "utf8");
  const roadmapZh = await readFile(resolve(repositoryRoot, "docs/zh/roadmap.md"), "utf8");

  for (const needle of [
    "export function classifyActivitySummary",
    "export function activityPolicyBadge",
    "export function mergeActivity",
    "export function eventActivity",
    "export function buildActivity",
    "approval_requested",
  ]) {
    assert.ok(activitySource.includes(needle), "activity.ts missing " + needle);
  }

  for (const needle of [
    'from "./activity"',
    "activityPolicyBadge",
    "mergeActivity",
    "activity-badge",
    "activity-header",
    "policy-${item.policy}",
  ]) {
    assert.ok(appSource.includes(needle), "App.tsx missing " + needle);
  }

  for (const needle of [
    ".activity-badge",
    ".activity-header",
    ".activity-badge.policy-auto-apply",
    ".activity-badge.policy-auto-run",
    ".activity-badge.policy-awaiting",
    ".activity-badge.policy-high-risk",
  ]) {
    assert.ok(cssSource.includes(needle), "styles.css missing " + needle);
  }

  assert.ok(
    configSource.includes("allowlisted safe commands"),
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
  assert.equal(activityPolicyBadge("running"), null);

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
