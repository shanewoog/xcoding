import type { MessageRole, Mode, Session, SessionStatus } from "@xcoding/protocol";
import { t, type Locale } from "./i18n";

export function formatSessionStatus(status: SessionStatus, locale: Locale = "en"): string {
  switch (status) {
    case "need_user":
      return t(locale, "status.need_user");
    case "running":
      return t(locale, "status.running");
    case "done":
      return t(locale, "status.done");
    case "cancelled":
      return t(locale, "status.cancelled");
    case "failed":
      return t(locale, "status.failed");
    case "created":
      return t(locale, "status.created");
  }
}

export function formatMessageRole(role: MessageRole, locale: Locale = "en"): string {
  switch (role) {
    case "user":
      return t(locale, "role.user");
    case "assistant":
      return t(locale, "role.assistant");
    case "tool":
      return t(locale, "role.tool");
    case "system":
      return t(locale, "role.system");
    default:
      return role;
  }
}

export function formatModeLabel(mode: Mode, locale: Locale = "en"): string {
  return mode === "auto-edit" ? t(locale, "mode.autoEdit") : t(locale, "mode.ask");
}

export function formatRelativeTime(iso: string, nowMs: number = Date.now(), locale: Locale = "en"): string {
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return "";
  const deltaSec = Math.max(0, Math.floor((nowMs - then) / 1000));
  if (deltaSec < 45) return t(locale, "relative.justNow");
  if (deltaSec < 3600) return t(locale, "relative.mAgo", { n: Math.floor(deltaSec / 60) });
  if (deltaSec < 86400) return t(locale, "relative.hAgo", { n: Math.floor(deltaSec / 3600) });
  if (deltaSec < 86400 * 14) return t(locale, "relative.dAgo", { n: Math.floor(deltaSec / 86400) });
  return new Date(then).toLocaleDateString(locale === "zh-CN" ? "zh-CN" : "en-US");
}

export function sessionMetaLine(
  session: Pick<Session, "mode" | "model" | "updated_at">,
  nowMs: number = Date.now(),
  locale: Locale = "en",
): string {
  return [formatModeLabel(session.mode, locale), session.model, formatRelativeTime(session.updated_at, nowMs, locale)]
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
