import type { PendingAction, PersistedSessionEvent, ToolCall } from "@xcoding/protocol";

export type ReviewPresentation = {
  title: string;
  summary: string;
  highRisk: boolean;
  commandText: string | null;
  bodyKind: "patch" | "command" | "generic";
};

function asString(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value : null;
}

function asStringArray(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.filter((item): item is string => typeof item === "string");
}

export function formatCommandText(toolCall: ToolCall): string | null {
  if (toolCall.name !== "run_command") return null;
  const executable = asString(toolCall.arguments.executable) ?? "<command>";
  const args = asStringArray(toolCall.arguments.args);
  return args.length === 0 ? executable : `${executable} ${args.join(" ")}`;
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

export function buildReviewPresentation(
  action: PendingAction,
  summary: string | null,
  hasPatchPreview: boolean,
): ReviewPresentation {
  const commandText = formatCommandText(action.tool_call);
  const highRisk = isHighRiskSummary(summary);
  if (action.tool_call.name === "apply_patch" || hasPatchPreview) {
    return {
      title: "Patch approval",
      summary: summary ?? "Review and approve the proposed patch.",
      highRisk: false,
      commandText: null,
      bodyKind: "patch",
    };
  }
  if (action.tool_call.name === "run_command") {
    return {
      title: highRisk ? "High-risk command approval" : "Command approval",
      summary:
        summary ??
        (commandText ? `Review and approve command: ${commandText}` : "Review and approve command."),
      highRisk,
      commandText,
      bodyKind: "command",
    };
  }
  return {
    title: "Action approval",
    summary: summary ?? `Review ${action.tool_call.name}`,
    highRisk,
    commandText: null,
    bodyKind: "generic",
  };
}
