import type { PendingAction, PersistedSessionEvent, ToolCall } from "@xcoding/protocol";
import { t, type Locale } from "./i18n";

const GIT_WRITE_TOOLS = new Set([
  "git_add",
  "git_commit",
  "git_push",
  "git_fetch",
  "git_pull",
]);

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
  const executable = asString(toolCall.arguments?.executable) ?? "<command>";
  const args = asStringArray(toolCall.arguments?.args);
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

export function gitToolTitle(toolName: string, locale: Locale = "en"): string | null {
  switch (toolName) {
    case "git_add":
      return t(locale, "review.gitAdd");
    case "git_commit":
      return t(locale, "review.gitCommit");
    case "git_push":
      return t(locale, "review.gitPush");
    case "git_fetch":
      return t(locale, "review.gitFetch");
    case "git_pull":
      return t(locale, "review.gitPull");
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

const POWERSHELL_EXECUTABLES = new Set(["powershell", "powershell.exe", "pwsh", "pwsh.exe"]);
const LOCAL_API_FORBIDDEN_COMMANDS = [
  "remove-item",
  "move-item",
  "copy-item",
  "new-item",
  "set-content",
  "add-content",
  "clear-content",
  "out-file",
  "set-itemproperty",
  "invoke-expression",
  "start-process",
  "stop-process",
  "restart-computer",
  "set-executionpolicy",
];

function isLoopbackHttpUrl(value: string): boolean {
  try {
    const url = new URL(value);
    if ((url.protocol !== "http:" && url.protocol !== "https:") || url.username || url.password) return false;
    return ["127.0.0.1", "localhost", "[::1]", "::1"].includes(url.hostname.toLowerCase());
  } catch {
    return false;
  }
}

function isLocalApiOutputReference(statement: string): boolean {
  return /^[$][A-Za-z0-9_$.]+$/.test(statement);
}

function isRememberableLocalApiScript(script: string): boolean {
  const normalized = script.trim();
  if (!normalized) return false;
  const lower = normalized.toLowerCase();
  if (["$(", "|", "&", "`"].some((token) => lower.includes(token))) return false;
  if (LOCAL_API_FORBIDDEN_COMMANDS.some((command) => lower.includes(command))) return false;

  const urls = [...normalized.matchAll(/https?:\/\/[^\s'"`]+/gi)].map((match) => match[0]);
  if (urls.length === 0 || urls.some((url) => !isLoopbackHttpUrl(url))) return false;

  let invokeCount = 0;
  for (const rawStatement of normalized.split(/[;{}\r\n]/)) {
    const statement = rawStatement.trim();
    if (!statement) continue;
    const statementLower = statement.toLowerCase();
    if (statementLower === "try" || statementLower === "catch") continue;
    if (statementLower.startsWith("invoke-webrequest") || statementLower.startsWith("invoke-restmethod")) {
      invokeCount += 1;
      continue;
    }
    const assignment = statementLower.match(/^[$][a-z_][a-z0-9_]*\s*=\s*(.+)$/);
    if (assignment && (assignment[1].startsWith("invoke-webrequest") || assignment[1].startsWith("invoke-restmethod"))) {
      invokeCount += 1;
      continue;
    }
    if (isLocalApiOutputReference(statement)) continue;
    return false;
  }
  return invokeCount === 1;
}

/**
 * Controls only whether the Desktop UI offers to remember an approval.
 * The Rust agent repeats this validation before it bypasses any high-risk prompt.
 */
export function isRememberableLocalApiRequest(action: PendingAction): boolean {
  if (action.tool_call.name !== "run_command") return false;
  const executable = asString(action.tool_call.arguments?.executable);
  if (!executable) return false;
  const executableName = executable.split(/[\\/]/).pop()?.trim().toLowerCase();
  if (!executableName || !POWERSHELL_EXECUTABLES.has(executableName)) return false;
  const args = asStringArray(action.tool_call.arguments?.args);
  const commandIndex = args.findIndex((argument) => /^(?:-command|-c)$/i.test(argument));
  return commandIndex >= 0 && commandIndex + 2 === args.length && isRememberableLocalApiScript(args[commandIndex + 1]);
}

export function buildReviewPresentation(
  action: PendingAction,
  summary: string | null,
  hasPatchPreview: boolean,
  locale: Locale = "en",
): ReviewPresentation {
  const toolName = action.tool_call.name;
  const commandText = formatCommandText(action.tool_call);
  const gitDetail = formatGitDetail(action.tool_call);
  const highRiskFromSummary = isHighRiskSummary(summary);

  if (toolName === "apply_patch" || hasPatchPreview) {
    return {
      title: t(locale, "review.patchTitle"),
      summary: summary ?? t(locale, "review.patchSummary"),
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
      title: highRisk ? t(locale, "review.commandRiskTitle") : t(locale, "review.commandTitle"),
      summary:
        summary ??
        (commandText
          ? t(locale, "review.commandSummaryWith", { command: commandText })
          : t(locale, "review.commandSummary")),
      highRisk,
      commandText,
      gitDetail: null,
      bodyKind: "command",
      riskHint: highRisk ? t(locale, "review.commandRiskHint") : null,
    };
  }

  if (isGitWriteTool(toolName)) {
    return {
      title: gitToolTitle(toolName, locale) ?? t(locale, "review.gitTitle"),
      summary: summary ?? t(locale, "review.gitSummary", { tool: toolName }),
      highRisk: true,
      commandText: null,
      gitDetail,
      bodyKind: "git",
      riskHint: t(locale, "review.gitRiskHint"),
    };
  }

  return {
    title: t(locale, "review.genericTitle"),
    summary: summary ?? t(locale, "review.genericSummary", { tool: toolName }),
    highRisk: highRiskFromSummary,
    commandText: null,
    gitDetail: null,
    bodyKind: "generic",
    riskHint: highRiskFromSummary ? t(locale, "review.genericRiskHint") : null,
  };
}
