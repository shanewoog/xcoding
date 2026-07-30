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
  switch (mode) {
    case "full-auto":
      return t(locale, "mode.fullAuto");
    case "auto-edit":
      return t(locale, "mode.autoEdit");
    default:
      return t(locale, "mode.ask");
  }
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


const RIGHT_PANEL_WIDTH_KEY = "xcoding.rightPanelWidth";
export const DEFAULT_RIGHT_PANEL_WIDTH = 420;
export const MIN_RIGHT_PANEL_WIDTH = 280;
const MIN_CHAT_WIDTH = 360;
const MIN_SIDEBAR_WIDTH = 230;

export function clampRightPanelWidth(
  width: number,
  viewportWidth: number = typeof window !== "undefined" ? window.innerWidth : 1280,
): number {
  // The browser can grow until the sidebar and chat reach their protected minimum widths.
  const max = Math.max(MIN_RIGHT_PANEL_WIDTH, viewportWidth - MIN_SIDEBAR_WIDTH - MIN_CHAT_WIDTH);
  if (!Number.isFinite(width)) return DEFAULT_RIGHT_PANEL_WIDTH;
  return Math.round(Math.min(max, Math.max(MIN_RIGHT_PANEL_WIDTH, width)));
}

export function loadRightPanelWidth(): number {
  try {
    const raw = localStorage.getItem(RIGHT_PANEL_WIDTH_KEY);
    if (raw == null) return DEFAULT_RIGHT_PANEL_WIDTH;
    return clampRightPanelWidth(Number(raw));
  } catch {
    return DEFAULT_RIGHT_PANEL_WIDTH;
  }
}

export function saveRightPanelWidth(width: number): void {
  try {
    localStorage.setItem(RIGHT_PANEL_WIDTH_KEY, String(clampRightPanelWidth(width)));
  } catch {
    // ignore storage failures
  }
}

// Right-panel state is tracked per session so the tools panel follows the task
// it was opened in. The composer draft (no session yet) uses a reserved key.
export const DRAFT_SESSION_KEY = "__draft__";

export function sessionStateKey(sessionId: string | null): string {
  const trimmed = sessionId?.trim();
  return trimmed ? trimmed : DRAFT_SESSION_KEY;
}

export function rightPanelStateFor<Tab extends string>(
  sessionId: string | null,
  openBySession: Record<string, boolean>,
  tabBySession: Record<string, Tab>,
  defaultTab: Tab,
): { open: boolean; tab: Tab } {
  const key = sessionStateKey(sessionId);
  return { open: openBySession[key] === true, tab: tabBySession[key] ?? defaultTab };
}

export function dropSessionKey<T>(map: Record<string, T>, sessionId: string): Record<string, T> {
  const key = sessionStateKey(sessionId);
  if (!(key in map)) return map;
  const { [key]: _removed, ...rest } = map;
  return rest;
}

export function adoptDraftSessionKey<T>(map: Record<string, T>, sessionId: string): Record<string, T> {
  const key = sessionStateKey(sessionId);
  if (key === DRAFT_SESSION_KEY || !(DRAFT_SESSION_KEY in map)) return map;
  const { [DRAFT_SESSION_KEY]: draft, ...rest } = map;
  if (key in rest) return rest;
  return { ...rest, [key]: draft };
}
