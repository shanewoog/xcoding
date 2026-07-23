import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

function isHighRiskSummary(summary) {
  return typeof summary === "string" && summary.toUpperCase().includes("HIGH-RISK");
}

function asString(value) {
  return typeof value === "string" && value.trim() ? value : null;
}

function asStringArray(value) {
  if (!Array.isArray(value)) return [];
  return value.filter((item) => typeof item === "string");
}

function formatCommandText(toolCall) {
  if (toolCall.name !== "run_command") return null;
  const executable = asString(toolCall.arguments?.executable) ?? "<command>";
  const args = asStringArray(toolCall.arguments?.args);
  return args.length === 0 ? executable : executable + " " + args.join(" ");
}

function formatGitDetail(toolCall) {
  const name = toolCall.name;
  const args = toolCall.arguments ?? {};
  switch (name) {
    case "git_add": {
      const paths = asStringArray(args.paths);
      return "paths: " + (paths.length > 0 ? paths.join(", ") : "<paths>");
    }
    case "git_commit": {
      const message = asString(args.message) ?? "<message>";
      const lines = ["message: " + message];
      if (typeof args.allow_empty === "boolean") lines.push("allow_empty: " + args.allow_empty);
      return lines.join("\n");
    }
    case "git_push": {
      const remote = asString(args.remote) ?? "origin";
      const branch = asString(args.branch) ?? "<current-branch>";
      const lines = ["remote: " + remote, "branch: " + branch];
      if (typeof args.set_upstream === "boolean") lines.push("set_upstream: " + args.set_upstream);
      return lines.join("\n");
    }
    case "git_fetch": {
      const remote = asString(args.remote) ?? "origin";
      const branch = asString(args.branch) ?? "<all>";
      return ["remote: " + remote, "branch: " + branch].join("\n");
    }
    case "git_pull": {
      const remote = asString(args.remote) ?? "origin";
      const branch = asString(args.branch) ?? "<current-branch>";
      const ffOnly = typeof args.ff_only === "boolean" ? args.ff_only : true;
      return ["remote: " + remote, "branch: " + branch, "ff_only: " + ffOnly].join("\n");
    }
    default:
      return null;
  }
}

function gitToolTitle(toolName) {
  switch (toolName) {
    case "git_add":
      return "High-risk git add approval";
    case "git_commit":
      return "High-risk git commit approval";
    case "git_push":
      return "High-risk git push approval";
    case "git_fetch":
      return "High-risk git fetch approval";
    case "git_pull":
      return "High-risk git pull approval";
    default:
      return null;
  }
}

function isGitWriteTool(name) {
  return ["git_add", "git_commit", "git_push", "git_fetch", "git_pull"].includes(name);
}

function buildReviewPresentation(action, summary, hasPatchPreview) {
  const toolName = action.tool_call.name;
  const commandText = formatCommandText(action.tool_call);
  const gitDetail = formatGitDetail(action.tool_call);
  const highRiskFromSummary = isHighRiskSummary(summary);
  if (toolName === "apply_patch" || hasPatchPreview) {
    return {
      title: "Patch approval",
      highRisk: false,
      bodyKind: "patch",
      commandText: null,
      gitDetail: null,
      riskHint: null,
    };
  }
  if (toolName === "run_command") {
    return {
      title: highRiskFromSummary ? "High-risk command approval" : "Command approval",
      highRisk: highRiskFromSummary,
      bodyKind: "command",
      commandText,
      gitDetail: null,
      riskHint: highRiskFromSummary
        ? "Shell or force-push style commands can change the system or remote git history. Approve only if you trust the exact command."
        : null,
    };
  }
  if (isGitWriteTool(toolName)) {
    return {
      title: gitToolTitle(toolName) ?? "High-risk git approval",
      highRisk: true,
      bodyKind: "git",
      commandText: null,
      gitDetail,
      riskHint:
        "Git write and remote tools always need approval, even in auto-edit. Check remote, branch, paths, and message before approving.",
    };
  }
  return {
    title: "Action approval",
    highRisk: highRiskFromSummary,
    bodyKind: "generic",
    commandText: null,
    gitDetail: null,
    riskHint: null,
  };
}

async function main() {
  const reviewSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/review.ts"), "utf8");
  const appSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/App.tsx"), "utf8");
  const cssSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/styles.css"), "utf8");
  const cliSource = await readFile(resolve(repositoryRoot, "apps/cli/src/index.ts"), "utf8");

  for (const needle of [
    "export function buildReviewPresentation",
    "isHighRiskSummary",
    "formatCommandText",
    "formatGitDetail",
    "gitToolTitle",
    'bodyKind: "git"',
  ]) {
    assert.ok(reviewSource.includes(needle), "review.ts missing " + needle);
  }
  for (const needle of [
    "risk-badge",
    "command-preview",
    "git-preview",
    "Approve high-risk",
    "high-risk",
    "review.riskHint",
    'review.bodyKind === "git"',
  ]) {
    assert.ok(appSource.includes(needle), "App.tsx missing " + needle);
  }
  for (const needle of [".risk-badge", ".command-preview", ".review-panel.high-risk", ".approve-risk-button"]) {
    assert.ok(cssSource.includes(needle), "styles.css missing " + needle);
  }
  assert.ok(cliSource.includes("WARNING: HIGH-RISK command"), "CLI missing HIGH-RISK command warning");
  assert.ok(
    cliSource.includes("WARNING: HIGH-RISK git operation"),
    "CLI missing HIGH-RISK git operation warning",
  );
  assert.ok(cliSource.includes("formatGitApprovalDetail"), "CLI missing formatGitApprovalDetail");

  const highRisk = buildReviewPresentation(
    {
      tool_call: {
        name: "run_command",
        arguments: { executable: "powershell", args: ["-Command", "Get-ChildItem"] },
      },
    },
    "Review HIGH-RISK command: powershell -Command Get-ChildItem",
    false,
  );
  assert.equal(highRisk.highRisk, true);
  assert.equal(highRisk.bodyKind, "command");
  assert.equal(highRisk.commandText, "powershell -Command Get-ChildItem");
  assert.equal(highRisk.title, "High-risk command approval");

  const normal = buildReviewPresentation(
    {
      tool_call: {
        name: "run_command",
        arguments: { executable: "cargo", args: ["test"] },
      },
    },
    "Review and approve command: cargo test",
    false,
  );
  assert.equal(normal.highRisk, false);
  assert.equal(normal.commandText, "cargo test");

  const patch = buildReviewPresentation(
    { tool_call: { name: "apply_patch", arguments: { path: "src/lib.rs" } } },
    "Review HIGH-RISK command: should-not-matter",
    true,
  );
  assert.equal(patch.highRisk, false);
  assert.equal(patch.bodyKind, "patch");

  const gitAdd = buildReviewPresentation(
    {
      tool_call: {
        name: "git_add",
        arguments: { paths: ["src/lib.rs", "README.md"] },
      },
    },
    "Review HIGH-RISK git add: src/lib.rs, README.md",
    false,
  );
  assert.equal(gitAdd.highRisk, true);
  assert.equal(gitAdd.bodyKind, "git");
  assert.equal(gitAdd.title, "High-risk git add approval");
  assert.equal(gitAdd.gitDetail, "paths: src/lib.rs, README.md");
  assert.match(gitAdd.riskHint ?? "", /always need approval/i);

  const gitCommit = buildReviewPresentation(
    {
      tool_call: {
        name: "git_commit",
        arguments: { message: "Add review UX\n\nDetails", allow_empty: false },
      },
    },
    "Review HIGH-RISK git commit: Add review UX",
    false,
  );
  assert.equal(gitCommit.title, "High-risk git commit approval");
  assert.equal(
    gitCommit.gitDetail,
    "message: Add review UX\n\nDetails\nallow_empty: false",
  );

  const gitPush = buildReviewPresentation(
    {
      tool_call: {
        name: "git_push",
        arguments: { remote: "origin", branch: "main", set_upstream: true },
      },
    },
    "Review HIGH-RISK git push: origin main",
    false,
  );
  assert.equal(gitPush.title, "High-risk git push approval");
  assert.equal(gitPush.gitDetail, "remote: origin\nbranch: main\nset_upstream: true");

  const gitFetch = buildReviewPresentation(
    {
      tool_call: {
        name: "git_fetch",
        arguments: { remote: "origin" },
      },
    },
    "Review HIGH-RISK git fetch: origin <all>",
    false,
  );
  assert.equal(gitFetch.title, "High-risk git fetch approval");
  assert.equal(gitFetch.gitDetail, "remote: origin\nbranch: <all>");

  const gitPull = buildReviewPresentation(
    {
      tool_call: {
        name: "git_pull",
        arguments: { remote: "origin", branch: "main", ff_only: true },
      },
    },
    "Review HIGH-RISK git pull: origin main (ff-only)",
    false,
  );
  assert.equal(gitPull.title, "High-risk git pull approval");
  assert.equal(gitPull.gitDetail, "remote: origin\nbranch: main\nff_only: true");

  console.log("Desktop review UX checks passed.");
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
