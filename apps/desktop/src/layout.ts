import type { MessageRole, Mode, Session, SessionStatus } from "@xcoding/protocol";

export function formatSessionStatus(status: SessionStatus): string {
  switch (status) {
    case "need_user":
      return "needs review";
    default:
      return status.replaceAll("_", " ");
  }
}

export function formatMessageRole(role: MessageRole): string {
  switch (role) {
    case "user":
      return "You";
    case "assistant":
      return "Assistant";
    case "tool":
      return "Tool";
    case "system":
      return "System";
    default:
      return role;
  }
}

export function formatModeLabel(mode: Mode): string {
  return mode === "auto-edit" ? "Auto edit" : "Ask";
}

export function formatRelativeTime(iso: string, nowMs: number = Date.now()): string {
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return "";
  const deltaSec = Math.max(0, Math.floor((nowMs - then) / 1000));
  if (deltaSec < 45) return "just now";
  if (deltaSec < 3600) return `${Math.floor(deltaSec / 60)}m ago`;
  if (deltaSec < 86400) return `${Math.floor(deltaSec / 3600)}h ago`;
  if (deltaSec < 86400 * 14) return `${Math.floor(deltaSec / 86400)}d ago`;
  return new Date(then).toLocaleDateString();
}

export function sessionMetaLine(
  session: Pick<Session, "mode" | "model" | "updated_at">,
  nowMs: number = Date.now(),
): string {
  return [formatModeLabel(session.mode), session.model, formatRelativeTime(session.updated_at, nowMs)]
    .filter(Boolean)
    .join(" · ");
}

export function hasTraceContent(input: {
  pendingAction: unknown;
  planCount: number;
  activityCount: number;
  restoreCount: number;
  replayCount: number;
  taskSummary: unknown;
}): boolean {
  return Boolean(
    input.pendingAction ||
      input.planCount > 0 ||
      input.activityCount > 0 ||
      input.restoreCount > 0 ||
      input.replayCount > 0 ||
      input.taskSummary,
  );
}
