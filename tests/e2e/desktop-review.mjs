import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

function isHighRiskSummary(summary) {
  return typeof summary === "string" && summary.toUpperCase().includes("HIGH-RISK");
}

function formatCommandText(toolCall) {
  if (toolCall.name !== "run_command") return null;
  const executable =
    typeof toolCall.arguments?.executable === "string" && toolCall.arguments.executable.trim()
      ? toolCall.arguments.executable
      : "<command>";
  const args = Array.isArray(toolCall.arguments?.args)
    ? toolCall.arguments.args.filter((item) => typeof item === "string")
    : [];
  return args.length === 0 ? executable : executable + " " + args.join(" ");
}

function buildReviewPresentation(action, summary, hasPatchPreview) {
  const commandText = formatCommandText(action.tool_call);
  const highRisk = isHighRiskSummary(summary);
  if (action.tool_call.name === "apply_patch" || hasPatchPreview) {
    return { title: "Patch approval", highRisk: false, bodyKind: "patch", commandText: null };
  }
  if (action.tool_call.name === "run_command") {
    return {
      title: highRisk ? "High-risk command approval" : "Command approval",
      highRisk,
      bodyKind: "command",
      commandText,
    };
  }
  return { title: "Action approval", highRisk, bodyKind: "generic", commandText: null };
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
  ]) {
    assert.ok(reviewSource.includes(needle), "review.ts missing " + needle);
  }
  for (const needle of ["risk-badge", "command-preview", "Approve high-risk", "high-risk"]) {
    assert.ok(appSource.includes(needle), "App.tsx missing " + needle);
  }
  for (const needle of [".risk-badge", ".command-preview", ".review-panel.high-risk", ".approve-risk-button"]) {
    assert.ok(cssSource.includes(needle), "styles.css missing " + needle);
  }
  assert.ok(cliSource.includes("WARNING: HIGH-RISK command"), "CLI missing HIGH-RISK warning");

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

  console.log("Desktop review UX checks passed.");
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
