import type { SessionEvent } from "@xcoding/protocol";

export type ActivityState = "running" | "done" | "failed";

export type ActivityPolicy =
  | "auto-apply"
  | "auto-run"
  | "awaiting"
  | "blocked"
  | "high-risk"
  | "running"
  | "done"
  | "failed"
  | "generic";

export type ActivityItem = {
  id: string;
  label: string;
  detail: string;
  state: ActivityState;
  policy: ActivityPolicy;
};

const DISTINCTIVE_POLICIES: ReadonlySet<ActivityPolicy> = new Set([
  "auto-apply",
  "auto-run",
  "awaiting",
  "blocked",
  "high-risk",
]);

export function classifyActivitySummary(
  summary: string,
  state: ActivityState = "running",
): ActivityPolicy {
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

export function activityPolicyBadge(policy: ActivityPolicy): string | null {
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

export function mergeActivity(
  previous: ActivityItem | undefined,
  next: ActivityItem,
): ActivityItem {
  if (!previous) return next;
  if (DISTINCTIVE_POLICIES.has(previous.policy) && !DISTINCTIVE_POLICIES.has(next.policy)) {
    return { ...next, policy: previous.policy };
  }
  return next;
}

function toolDetail(argumentsJson: unknown): string {
  try {
    return JSON.stringify(argumentsJson);
  } catch {
    return String(argumentsJson);
  }
}

export function eventActivity(event: SessionEvent, sequence: string): ActivityItem | null {
  if (event.type === "tool_start") {
    const label = event.summary;
    const state: ActivityState = "running";
    return {
      id: event.tool_call.id,
      label,
      detail: toolDetail(event.tool_call.arguments),
      state,
      policy: classifyActivitySummary(label, state),
    };
  }
  if (event.type === "tool_end") {
    const label = event.summary;
    const state: ActivityState = event.success ? "done" : "failed";
    return {
      id: event.tool_call.id,
      label,
      detail: toolDetail(event.tool_call.arguments),
      state,
      policy: classifyActivitySummary(label, state),
    };
  }
  if (event.type === "approval_requested") {
    const label = event.summary;
    const policy = /HIGH-RISK/i.test(label) ? "high-risk" : "awaiting";
    return {
      id: event.action.id,
      label,
      detail: event.action.tool_call.name,
      state: "running",
      policy,
    };
  }
  if (event.type === "restore_point_rolled_back") {
    return {
      id: sequence,
      label: event.summary,
      detail: event.restore_point.path,
      state: "done",
      policy: "done",
    };
  }
  if (event.type === "session_cancelled") {
    return {
      id: sequence,
      label: "Session cancelled",
      detail: event.message,
      state: "failed",
      policy: "failed",
    };
  }
  if (event.type === "error") {
    return {
      id: sequence,
      label: "Agent error",
      detail: event.message,
      state: "failed",
      policy: "failed",
    };
  }
  return null;
}

export function buildActivity(
  events: Array<{ id: string; event: SessionEvent }>,
): ActivityItem[] {
  const items = new Map<string, ActivityItem>();
  for (const item of events) {
    const activity = eventActivity(item.event, item.id);
    if (!activity) continue;
    items.set(activity.id, mergeActivity(items.get(activity.id), activity));
  }
  return [...items.values()];
}
