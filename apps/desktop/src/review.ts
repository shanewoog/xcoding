import type { PendingAction, PersistedSessionEvent, ToolCall } from "@xcoding/protocol";

export type ReviewBodyKind = "patch" | "command" | "git" | "generic";

export type ReviewPresentation = {
  title: string;
  summary: string;
  highRisk: boolean;
  commandText: string | null;
  gitDetail: string | null;
  bodyKind: ReviewBodyKind;
  riskHint: string | null;
};

const GIT_WRITE_TOOLS = new Set([
  "git_add",
  "git_commit",
  "git_push",
  "git_fetch",
  "git_pull",
]);

function asString(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value : null;
}

function asStringArray(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.filter((item): item is string => typeof item === "string");
}

function asOptionalBool(value: unknown): boolean | null {
  return typeof value === "boolean" ? value : null;
}

export function formatCommandText(toolCall: ToolCall): string | null {
  if (toolCall.name !== "run_command") return null;
  const executable = asString(toolCall.arguments.executable) ?? "<command>";
  const args = asStringArray(toolCall.arguments.args);
  return args.length === 0 ? executable : `${executable} ${args.join(" ")}`;
}

export function formatGitDetail(toolCall: ToolCall): string | null {
  if (!GIT_WRITE_TOOLS.has(toolCall.name)) return null;
  const args = toolCall.arguments ?? {};
  switch (toolCall.name) {
    case "git_add": {
      const paths = asStringArray(args.paths);
      return `paths: ${paths.length > 0 ? paths.join(", ") : "<paths>"}`;
    }
    case "git_commit": {
      const message = asString(args.message) ?? "<message>";
      const allowEmpty = asOptionalBool(args.allow_empty);
      const lines = [`message: ${message}`];
      if (allowEmpty !== null) lines.push(`allow_empty: ${allowEmpty}`);
      return lines.join("\n");
    }
    case "git_push": {
      const remote = asString(args.remote) ?? "origin";
      const branch = asString(args.branch) ?? "<current-branch>";
      const setUpstream = asOptionalBool(args.set_upstream);
      const lines = [`remote: ${remote}`, `branch: ${branch}`];
      if (setUpstream !== null) lines.push(`set_upstream: ${setUpstream}`);
      return lines.join("\n");
    }
    case "git_fetch": {
      const remote = asString(args.remote) ?? "origin";
      const branch = asString(args.branch) ?? "<all>";
      return [`remote: ${remote}`, `branch: ${branch}`].join("\n");
    }
    case "git_pull": {
      const remote = asString(args.remote) ?? "origin";
      const branch = asString(args.branch) ?? "<current-branch>";
      const ffOnly = asOptionalBool(args.ff_only);
      const lines = [`remote: ${remote}`, `branch: ${branch}`];
      lines.push(`ff_only: ${ffOnly === null ? true : ffOnly}`);
      return lines.join("\n");
    }
    default:
      return null;
  }
}

export function gitToolTitle(toolName: string): string | null {
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

export function latestApprovalSummary(
  events: PersistedSessionEvent[],
  action: PendingAction | null,
): string | null {
  if (!action) return null;
  for (let index = events.length - 1; index >= 0; index -= 1) {
    const event = events[index].event;
    if (event.type === "approval_requested" && event.action.id === action.id) {
      return event.summary;
    }
  }
  return null;
}

export function isHighRiskSummary(summary: string | null | undefined): boolean {
  return typeof summary === "string" && summary.toUpperCase().includes("HIGH-RISK");
}

export function isGitWriteTool(name: string): boolean {
  return GIT_WRITE_TOOLS.has(name);
}

export function buildReviewPresentation(
  action: PendingAction,
  summary: string | null,
  hasPatchPreview: boolean,
): ReviewPresentation {
  const toolName = action.tool_call.name;
  const commandText = formatCommandText(action.tool_call);
  const gitDetail = formatGitDetail(action.tool_call);
  const highRiskFromSummary = isHighRiskSummary(summary);

  if (toolName === "apply_patch" || hasPatchPreview) {
    return {
      title: "Patch approval",
      summary: summary ?? "Review and approve the proposed patch.",
      highRisk: false,
      commandText: null,
      gitDetail: null,
      bodyKind: "patch",
      riskHint: null,
    };
  }

  if (toolName === "run_command") {
    const highRisk = highRiskFromSummary;
    return {
      title: highRisk ? "High-risk command approval" : "Command approval",
      summary:
        summary ??
        (commandText ? `Review and approve command: ${commandText}` : "Review and approve command."),
      highRisk,
      commandText,
      gitDetail: null,
      bodyKind: "command",
      riskHint: highRisk
        ? "Shell or force-push style commands can change the system or remote git history. Approve only if you trust the exact command."
        : null,
    };
  }

  if (isGitWriteTool(toolName)) {
    return {
      title: gitToolTitle(toolName) ?? "High-risk git approval",
      summary: summary ?? `Review HIGH-RISK ${toolName}`,
      highRisk: true,
      commandText: null,
      gitDetail,
      bodyKind: "git",
      riskHint:
        "Git write and remote tools always need approval, even in auto-edit. Check remote, branch, paths, and message before approving.",
    };
  }

  return {
    title: "Action approval",
    summary: summary ?? `Review ${toolName}`,
    highRisk: highRiskFromSummary,
    commandText: null,
    gitDetail: null,
    bodyKind: "generic",
    riskHint: highRiskFromSummary
      ? "This action is marked high-risk. Approve only if you trust the exact operation."
      : null,
  };
}
