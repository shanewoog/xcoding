import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { ChangeEvent, ClipboardEvent, FormEvent, KeyboardEvent, MouseEvent as ReactMouseEvent, ReactNode } from "react";
import type {
  CancelSessionResult,
  ChatImageAttachment,
  ChatParams,
  ChatResult,
  CloudProviderConfig,
  Message,
  Mode,
  PatchPreview,
  PendingAction,
  PersistedSessionEvent,
  PlanStep,
  ResolveActionResult,
  RestorePoint,
  ReplaySessionResult,
  ReplayStep,
  RollbackRestorePointResult,
  Session,
  SessionDetail,
  SessionEvent,
  TaskSummary,
  CreateProjectResult,
  ImportProjectResult,
  ListModelsResult,
  ProjectDir,
  ProviderAuthStatus,
  ProviderModel,
  UserConfig,
  WorkspaceConfig,
} from "@xcoding/protocol";
import { buildActivity, eventActivity, mergeActivity } from "./activity";
import type { ActivityItem } from "./activity";
import { buildReviewPresentation, isRememberableLocalApiRequest, latestApprovalSummary } from "./review";
import {
  buildDesktopDoctorChecks,
  desktopDoctorReady,
  modeHelpText,
  commandAllowlistHelpText,
  parseCommandAllowlistText,
  formatCommandAllowlistText,
  commandDenylistHelpText,
  parseCommandDenylistText,
  formatCommandDenylistText,
} from "./config";
import {
  clampRightPanelWidth,
  formatSessionStatus,
  loadRightPanelWidth,
  saveRightPanelWidth,
  sessionMetaLine,
} from "./layout";
import { applyUiFontSize, loadUiFontSize, saveUiFontSize } from "./appearance";
import { isLocale, loadLocale, saveLocale, t, type Locale } from "./i18n";
import {
  EmptyQuickActions,
  EnvironmentPopover,
  HeaderEnvButton,
  HeaderPanelToggle,
  HeaderRightPanelToggle,
  RightToolsPanel,
  TerminalBottomPanel,
  seedTerminalCommand,
  type PanelTarget,
  type SourceItem,
  type ToolPanelTab,
  type BrowserNavigationRequest,
} from "./panels";
import { fetchGitEnvironment, formatDiffStat, openPath } from "./workspaceApi";

const defaultProvider = "openai";
const DEFAULT_PROVIDER_BASE_URL = "https://ai.v58.dev";
const DEFAULT_MAX_PROVIDER_RETRIES = 6;
const MIN_MAX_PROVIDER_RETRIES = 0;
const MAX_MAX_PROVIDER_RETRIES = 10;
const DEFAULT_MAX_TOOL_ROUNDS = 16;
const MIN_MAX_TOOL_ROUNDS = 1;
const MAX_MAX_TOOL_ROUNDS = 1024;
const DEFAULT_CIRCUIT_FAILURE_THRESHOLD = 3;
const MIN_CIRCUIT_FAILURE_THRESHOLD = 1;
const MAX_CIRCUIT_FAILURE_THRESHOLD = 10;
const DEFAULT_STREAM_FIRST_EVENT_TIMEOUT_SECS = 120;
const MIN_STREAM_FIRST_EVENT_TIMEOUT_SECS = 1;
const MAX_STREAM_FIRST_EVENT_TIMEOUT_SECS = 120;
const DEFAULT_STREAM_IDLE_TIMEOUT_SECS = 180;
const MIN_STREAM_IDLE_TIMEOUT_SECS = 60;
const MAX_STREAM_IDLE_TIMEOUT_SECS = 600;
const DEFAULT_NON_STREAM_TIMEOUT_SECS = 600;
const MIN_NON_STREAM_TIMEOUT_SECS = 60;
const MAX_NON_STREAM_TIMEOUT_SECS = 1200;
const DEFAULT_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD = 2;
const MIN_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD = 1;
const MAX_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD = 20;
const DEFAULT_CIRCUIT_RECOVERY_WAIT_SECS = 60;
const MIN_CIRCUIT_RECOVERY_WAIT_SECS = 30;
const MAX_CIRCUIT_RECOVERY_WAIT_SECS = 120;
const DEFAULT_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT = 60;
const MIN_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT = 1;
const MAX_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT = 100;
const DEFAULT_CIRCUIT_MIN_REQUEST_COUNT = 10;
const MIN_CIRCUIT_MIN_REQUEST_COUNT = 1;
const MAX_CIRCUIT_MIN_REQUEST_COUNT = 100;
const isTauriRuntime = "__TAURI_INTERNALS__" in window;
const REASONING_EFFORTS = ["none", "low", "medium", "high"] as const;
type ReasoningEffort = (typeof REASONING_EFFORTS)[number];

function normalizeReasoningEffort(value: string | undefined | null): ReasoningEffort {
  const trimmed = (value || "").trim().toLowerCase();
  return (REASONING_EFFORTS as readonly string[]).includes(trimmed)
    ? (trimmed as ReasoningEffort)
    : "high";
}

function normalizeBoundedInteger(
  value: number | undefined | null,
  fallback: number,
  minimum: number,
  maximum: number,
): number {
  const numeric = typeof value === "number" ? value : Number.NaN;
  if (!Number.isFinite(numeric)) return fallback;
  return Math.min(maximum, Math.max(minimum, Math.round(numeric)));
}

const MODELS_CACHE_KEY = "xcoding.cachedModels.v1";

type CachedModels = {
  baseUrl: string;
  models: ProviderModel[];
  savedAt: number;
};

function loadCachedModels(baseUrl: string): ProviderModel[] {
  try {
    const raw = localStorage.getItem(MODELS_CACHE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as CachedModels;
    if (!parsed || parsed.baseUrl !== baseUrl || !Array.isArray(parsed.models)) return [];
    return parsed.models.filter((entry) => entry && typeof entry.id === "string" && entry.id.trim());
  } catch {
    return [];
  }
}

function saveCachedModels(baseUrl: string, models: ProviderModel[]): void {
  try {
    const payload: CachedModels = {
      baseUrl,
      models,
      savedAt: Date.now(),
    };
    localStorage.setItem(MODELS_CACHE_KEY, JSON.stringify(payload));
  } catch {
    // ignore quota / private mode failures
  }
}
function normalizeProviderBaseUrl(value: string | undefined | null): string {
  let normalized = (value || "").trim().replace(/\/+$/, "");
  while (/\/v1$/i.test(normalized)) {
    normalized = normalized.slice(0, -3).replace(/\/+$/, "");
  }
  return normalized;
}

function providerId(): string {
  return globalThis.crypto?.randomUUID?.() || `provider-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

function hydrateProviders(config: UserConfig): { providers: CloudProviderConfig[]; activeProviderId: string } {
  const configured = (config.providers || [])
    .filter((item) => item && typeof item.id === "string")
    .map((item, index) => ({
      id: item.id.trim() || `provider-${index + 1}`,
      name: item.name?.trim() || `Provider ${index + 1}`,
      base_url: normalizeProviderBaseUrl(item.base_url) || DEFAULT_PROVIDER_BASE_URL,
      api_key: item.api_key || undefined,
    }));
  const providers = configured.length > 0
    ? configured
    : [{
        id: "default",
        name: defaultProvider,
        base_url: normalizeProviderBaseUrl(config.base_url) || DEFAULT_PROVIDER_BASE_URL,
        api_key: config.api_key || undefined,
      }];
  const activeProviderId = providers.some((item) => item.id === config.active_provider_id)
    ? config.active_provider_id!
    : providers[0].id;
  return { providers, activeProviderId };
}

function projectLabel(workspaceRoot: string): string {
  const parts = workspaceRoot.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] || workspaceRoot || "project";
}


const CHAT_WORKSPACE_DIR = ".xcoding-chat";

function normalizeRoot(value: string): string {
  return value.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
}

function isChatWorkspace(root: string | null | undefined): boolean {
  const n = normalizeRoot((root || "").trim());
  if (!n) return false;
  return n.endsWith(`/${CHAT_WORKSPACE_DIR}`) || n === CHAT_WORKSPACE_DIR;
}

function chatWorkspaceCandidate(workspaceHome: string): string {
  const home = workspaceHome.trim().replace(/[\\/]+$/, "");
  if (!home) return CHAT_WORKSPACE_DIR;
  const sep = home.includes("\\") && !home.includes("/") ? "\\" : "/";
  return `${home}${sep}${CHAT_WORKSPACE_DIR}`;
}


function groupSessionsByProject(
  sessions: Session[],
  diskProjects: ProjectDir[] = [],
): Array<{ root: string; name: string; sessions: Session[] }> {
  const groups: Array<{ root: string; name: string; sessions: Session[] }> = [];
  const index = new Map<string, number>();
  const norm = (value: string) => value.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();

  const ensure = (root: string, name: string) => {
    const key = norm(root);
    const existing = index.get(key);
    if (existing === undefined) {
      index.set(key, groups.length);
      groups.push({ root, name, sessions: [] });
      return groups.length - 1;
    }
    if (name && groups[existing].name === projectLabel(groups[existing].root)) {
      groups[existing].name = name;
    }
    return existing;
  };

  for (const project of diskProjects) {
    ensure(project.path, project.title || project.dir_name || projectLabel(project.path));
  }
  for (const session of sessions) {
    const root = session.workspace_root;
    const idx = ensure(root, projectLabel(root));
    groups[idx].sessions.push(session);
  }
  return groups;
}
function sessionTitle(session: Session, locale: Locale): string {
  if (session.title?.trim()) return session.title.trim();
  if (isChatWorkspace(session.workspace_root)) {
    return t(locale, "session.chatFallback");
  }
  return t(locale, "session.fallbackTitle", {
    name: session.workspace_root.split(/[\\/]/).pop() || t(locale, "session.workspaceFallback"),
  });
}

function toolMessagePreview(content: string): string {
  const line = content
    .split(/\r?\n/)
    .map((part) => part.trim())
    .find((part) => part.length > 0) || content.trim();
  const collapsed = line.replace(/\s+/g, " ");
  if (collapsed.length <= 72) return collapsed;
  return `${collapsed.slice(0, 71)}…`;
}

type ConversationEntry =
  | { kind: "message"; message: Message }
  | { kind: "tool-group"; id: string; messages: Message[] };

function groupConversationMessages(messages: Message[]): ConversationEntry[] {
  const entries: ConversationEntry[] = [];
  for (const message of messages) {
    if (message.role !== "tool") {
      entries.push({ kind: "message", message });
      continue;
    }

    const last = entries.at(-1);
    if (last?.kind === "tool-group") {
      last.messages.push(message);
      continue;
    }
    entries.push({ kind: "tool-group", id: message.id, messages: [message] });
  }
  return entries;
}

type ComposerImage = {
  id: string;
  mime_type: string;
  data_base64: string;
  name?: string;
  previewUrl: string;
};

const IMAGE_BEGIN = "<!-- xcoding-images";
const IMAGE_END = "xcoding-images -->";
const MAX_COMPOSER_IMAGES = 4;
const MAX_IMAGE_BYTES = 4 * 1024 * 1024;
const SYSTEM_CONTEXT_TOKEN_RESERVE = 4_000;
const IMAGE_CONTEXT_TOKEN_ESTIMATE = 2_000;
const DEFAULT_CONTEXT_WINDOW = 128_000;
const CJK_CHARACTER = /[\u2E80-\u9FFF\uF900-\uFAFF]/u;

function estimateTextTokens(value: string): number {
  let estimate = 0;
  for (const character of value) {
    estimate += CJK_CHARACTER.test(character) ? 1.5 : 0.25;
  }
  return Math.ceil(estimate);
}

function estimateMessageTokens(message: Message): number {
  const parsed = message.role === "user"
    ? parseStoredUserMessage(message.content)
    : { text: message.content, images: [] as Array<{ mime_type: string; data_base64: string }> };
  return estimateTextTokens(parsed.text) + parsed.images.length * IMAGE_CONTEXT_TOKEN_ESTIMATE;
}

function contextWindowForModel(model: string): number {
  const normalized = model.trim().toLowerCase();
  if (/gemini/.test(normalized)) return 1_000_000;
  if (/claude/.test(normalized)) return 200_000;
  return DEFAULT_CONTEXT_WINDOW;
}

function formatContextTokenCount(tokens: number): string {
  if (tokens >= 1_000_000) {
    const millions = tokens / 1_000_000;
    return `${Number.isInteger(millions) ? millions : millions.toFixed(1)}m`;
  }
  if (tokens >= 1_000) return `${Math.max(1, Math.round(tokens / 1_000))}k`;
  return String(Math.max(0, Math.round(tokens)));
}

function encodeLocalUserContent(message: string, images: ChatImageAttachment[]): string {
  const text = message.trim();
  if (images.length === 0) return text;
  const payload = images
    .map((image) => `${image.mime_type}|${image.data_base64}`)
    .join(";");
  const marker = `<!-- xcoding-images:${payload} xcoding-images -->`;
  return text ? `${text}\n\n${marker}` : marker;
}

function parseStoredUserMessage(content: string): { text: string; images: Array<{ mime_type: string; data_base64: string }> } {
  const start = content.indexOf(IMAGE_BEGIN);
  if (start < 0) return { text: content, images: [] };
  const endRel = content.indexOf(IMAGE_END, start);
  if (endRel < 0) return { text: content, images: [] };
  const end = endRel + IMAGE_END.length;
  const block = content.slice(start, end);
  const text = `${content.slice(0, start)}${content.slice(end)}`.trim();
  const payload = block
    .replace(IMAGE_BEGIN, "")
    .replace(IMAGE_END, "")
    .replace(/^:\s*/, "")
    .trim();
  const images: Array<{ mime_type: string; data_base64: string }> = [];
  for (const item of payload.split(";")) {
    const [mime, data] = item.split("|");
    if (!mime || !data) continue;
    const mimeType = mime.trim().toLowerCase();
    const dataBase64 = data.trim();
    if (mimeType.startsWith("image/") && dataBase64) {
      images.push({ mime_type: mimeType === "image/jpg" ? "image/jpeg" : mimeType, data_base64: dataBase64 });
    }
  }
  return { text, images };
}

function fileToComposerImage(file: File): Promise<ComposerImage> {
  return new Promise((resolve, reject) => {
    const mime = (file.type || "").toLowerCase();
    if (!["image/png", "image/jpeg", "image/jpg", "image/webp", "image/gif"].includes(mime)) {
      reject(new Error("type"));
      return;
    }
    if (file.size > MAX_IMAGE_BYTES) {
      reject(new Error("size"));
      return;
    }
    const reader = new FileReader();
    reader.onload = () => {
      const result = String(reader.result || "");
      const match = /^data:([^;]+);base64,(.+)$/s.exec(result);
      if (!match) {
        reject(new Error("read"));
        return;
      }
      const mimeType = match[1].toLowerCase() === "image/jpg" ? "image/jpeg" : match[1].toLowerCase();
      const data_base64 = match[2].replace(/\s+/g, "");
      resolve({
        id: `img-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
        mime_type: mimeType,
        data_base64,
        name: file.name || undefined,
        previewUrl: `data:${mimeType};base64,${data_base64}`,
      });
    };
    reader.onerror = () => reject(new Error("read"));
    reader.readAsDataURL(file);
  });
}

function UserMessageBody({ content }: { content: string }) {
  const parsed = parseStoredUserMessage(content);
  return (
    <div className="message-body">
      {parsed.text ? <div className="message-text">{parsed.text}</div> : null}
      {parsed.images.length > 0 ? (
        <div className="message-images">
          {parsed.images.map((image, index) => (
            <img
              key={`${image.mime_type}-${index}`}
              src={`data:${image.mime_type};base64,${image.data_base64}`}
              alt={`attachment-${index + 1}`}
              className="message-image"
            />
          ))}
        </div>
      ) : null}
    </div>
  );
}

function latestPlan(events: PersistedSessionEvent[]): PlanStep[] {
  for (let index = events.length - 1; index >= 0; index -= 1) {
    const event = events[index].event;
    if (event.type === "plan") return event.steps;
  }
  return [];
}

function latestTaskSummary(events: PersistedSessionEvent[]): TaskSummary | null {
  for (let index = events.length - 1; index >= 0; index -= 1) {
    const event = events[index].event;
    if (event.type === "task_completed") return event.summary;
  }
  return null;
}

function latestPatchPreview(events: PersistedSessionEvent[], action: PendingAction | null): PatchPreview | null {
  if (!action || action.tool_call.name !== "apply_patch") return null;
  for (let index = events.length - 1; index >= 0; index -= 1) {
    const event = events[index].event;
    if (event.type === "patch_preview") return event.preview;
  }
  return null;
}

function buildPatchDiffLines(
  preview: PatchPreview,
  locale: Locale,
): Array<{ kind: "add" | "remove" | "meta"; text: string }> {
  const lines: Array<{ kind: "add" | "remove" | "meta"; text: string }> = [];
  if (!preview.old_text) {
    lines.push({ kind: "meta", text: t(locale, "review.newFile") });
  } else {
    for (const line of preview.old_text.split("\n")) {
      lines.push({ kind: "remove", text: line });
    }
  }
  for (const line of preview.new_text.split("\n")) {
    lines.push({ kind: "add", text: line });
  }
  return lines;
}

async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    // Clipboard can fail outside secure contexts; ignore.
  }
}

function formatMessageTime(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "";
  return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false });
}

type ModelCallLog = PersistedSessionEvent & {
  event: Extract<SessionEvent, { type: "model_call" }>;
};

function formatModelCallTime(value: string, locale: Locale): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "medium" }).format(date);
}

function CopyIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden="true">
      <rect x="5.5" y="5.5" width="8" height="8" rx="1.5" stroke="currentColor" strokeWidth="1.4" />
      <path d="M10.5 5.5V4A1.5 1.5 0 0 0 9 2.5H4A1.5 1.5 0 0 0 2.5 4v5A1.5 1.5 0 0 0 4 10.5h1.5" stroke="currentColor" strokeWidth="1.4" />
    </svg>
  );
}

function EditIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden="true">
      <path d="M3.2 12.8l.7-2.6L11 3.1a1.4 1.4 0 0 1 2 0L13 4.1a1.4 1.4 0 0 1 0 2L5.8 13.3 3.2 12.8z" stroke="currentColor" strokeWidth="1.4" strokeLinejoin="round" />
      <path d="M9.6 3.6l2.8 2.8" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
    </svg>
  );
}

function SettingsIcon() {
  return (
    <svg width="15" height="15" viewBox="0 0 16 16" fill="none" aria-hidden="true">
      <circle cx="8" cy="8" r="2.2" stroke="currentColor" strokeWidth="1.4" />
      <path
        d="M8 1.6l.7 1.4 1.5-.3.9 1.3 1.4.5-.3 1.5 1.2.9-1.2.9.3 1.5-1.4.5-.9 1.3-1.5-.3L8 14.4l-.7-1.4-1.5.3-.9-1.3-1.4-.5.3-1.5L2.6 9l1.2-.9-.3-1.5 1.4-.5.9-1.3 1.5.3L8 1.6z"
        stroke="currentColor"
        strokeWidth="1.2"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function gitSnapshotText(summary: TaskSummary, locale: Locale): string {
  return [
    summary.git_branch ? t(locale, "summary.branch", { name: summary.git_branch }) : "",
    summary.git_status ? t(locale, "summary.status", { text: summary.git_status }) : "",
    summary.git_diff ? t(locale, "summary.diff", { text: summary.git_diff }) : "",
  ]
    .filter(Boolean)
    .join("\n\n");
}

function formatTaskSummaryText(summary: TaskSummary, locale: Locale): string {
  const added = summary.lines_added ?? 0;
  const removed = summary.lines_removed ?? 0;
  const lines: string[] = [
    t(locale, "summary.taskComplete", {
      files: summary.changed_files.length,
      added,
      removed,
    }),
    t(locale, "summary.commands", {
      ok: summary.commands_succeeded,
      total: summary.commands_run,
    }) + (summary.commands_failed ? t(locale, "summary.commandsFailed", { n: summary.commands_failed }) : ""),
  ];
  const fileChanges = summary.file_changes ?? [];
  if (fileChanges.length > 0) {
    lines.push(t(locale, "summary.files"));
    for (const change of fileChanges) {
      lines.push(`  [${change.kind}] ${change.path} (+${change.lines_added}/-${change.lines_removed})`);
    }
  } else if (summary.changed_files.length > 0) {
    lines.push(t(locale, "summary.changed", { files: summary.changed_files.join(", ") }));
  }
  const git = gitSnapshotText(summary, locale);
  if (git) lines.push(git);
  return lines.join("\n");
}

function fileChangeLabel(kind: string, locale: Locale): string {
  if (kind === "created") return t(locale, "file.created");
  if (kind === "deleted") return t(locale, "file.deleted");
  return t(locale, "file.modified");
}

function mergeMessage(messages: Message[], message: Message): Message[] {
  return messages.some((current) => current.id === message.id) ? messages : [...messages, message];
}

type RunStatus = {
  startedAt: number;
  phase: "thinking" | "retrying" | "failed";
  retryAttempt?: number;
  retryMaxAttempts?: number;
  detail?: string;
};

function formatRunElapsed(startedAt: number, now: number): string {
  const seconds = Math.max(0, Math.floor((now - startedAt) / 1000));
  const minutes = Math.floor(seconds / 60);
  const remainder = seconds % 60;
  return minutes > 0 ? `${minutes}m ${remainder}s` : `${seconds}s`;
}

function completedRunElapsedByMessageId(messages: Message[]): Record<string, string> {
  const elapsedByMessageId: Record<string, string> = {};
  let turnStartedAt: number | null = null;

  for (const message of messages) {
    if (message.role === "user") {
      const createdAt = Date.parse(message.created_at);
      turnStartedAt = Number.isFinite(createdAt) ? createdAt : null;
      continue;
    }
    if (message.role !== "assistant" || turnStartedAt === null) continue;

    const completedAt = Date.parse(message.created_at);
    if (Number.isFinite(completedAt) && completedAt >= turnStartedAt) {
      elapsedByMessageId[message.id] = formatRunElapsed(turnStartedAt, completedAt);
    }
    turnStartedAt = null;
  }

  return elapsedByMessageId;
}

function runPlanStepDescription(step: PlanStep, locale: Locale): string {
  if (step.id === "inspect") return t(locale, "run.plan.inspect");
  if (step.id === "change") return t(locale, "run.plan.change");
  if (step.id === "verify") return t(locale, "run.plan.verify");
  return step.description;
}

function currentRunPlanStep(plan: PlanStep[], activity: ActivityItem[]): number {
  if (plan.length === 0) return -1;
  const activityText = activity.map((item) => `${item.label} ${item.detail}`).join("\n").toLowerCase();
  const changeIndex = plan.findIndex((step) => step.id === "change");
  const verifyIndex = plan.findIndex((step) => step.id === "verify");
  const changedWorkspace = /\bapply_patch\b|auto-applying|\b(?:write|create|update|delete)_file\b/.test(activityText);
  const verifyingWorkspace = /\b(?:cargo|pnpm|npm|node|git)\b[^\n]*(?:test|check|build|fmt|lint|diff --check)|\b(?:test|typecheck|verification)\b/.test(activityText);

  if (verifyingWorkspace && verifyIndex >= 0) return verifyIndex;
  if (changedWorkspace && changeIndex >= 0) return changeIndex;
  return 0;
}

const MESSAGE_LINK_PATTERN = /\[([^\]\r\n]+)\]\((https?:\/\/[^\s)]+)\)|https?:\/\/[^\s<>()\]]+/gi;

function splitLinkPunctuation(value: string): { url: string; suffix: string } {
  const suffix = value.match(/(?:\*\*|[.,!?;:'"\u2018\u2019\u201C\u201D，。！？；：])+$/)?.[0] || "";
  return { url: suffix ? value.slice(0, -suffix.length) : value, suffix };
}

function AssistantMessageBody({ content, onOpenLink }: { content: string; onOpenLink: (url: string) => void }) {
  const parts: ReactNode[] = [];
  let cursor = 0;
  let key = 0;

  for (const match of content.matchAll(MESSAGE_LINK_PATTERN)) {
    const start = match.index ?? 0;
    let end = start + match[0].length;
    const markdownLabel = match[1];
    const { url, suffix } = splitLinkPunctuation(match[2] || match[0]);
    if (!url) continue;

    const trailingBareLinkBoldDelimiter = !markdownLabel && suffix.includes("**") && start >= cursor + 2 && content.slice(start - 2, start) === "**";
    const boldLink = trailingBareLinkBoldDelimiter || (Boolean(markdownLabel) && start >= cursor + 2 && content.slice(start - 2, start) === "**" && content.slice(end, end + 2) === "**");
    const visibleSuffix = trailingBareLinkBoldDelimiter ? suffix.replace("**", "") : suffix;
    const textEnd = boldLink ? start - 2 : start;
    if (textEnd > cursor) parts.push(content.slice(cursor, textEnd));

    const link = (
      <a
        key={`message-link-${key}`}
        className="assistant-message-link"
        href={url}
        onClick={(event) => {
          event.preventDefault();
          onOpenLink(url);
        }}
      >
        {markdownLabel || url}
      </a>
    );
    parts.push(boldLink ? <strong key={`message-link-strong-${key}`}>{link}</strong> : link);
    if (visibleSuffix) parts.push(visibleSuffix);
    cursor = boldLink && markdownLabel ? end + 2 : end;
    key += 1;
  }

  if (cursor < content.length) parts.push(content.slice(cursor));
  return <>{parts.length > 0 ? parts : content}</>;
}

export function App() {
  const [locale, setLocale] = useState<Locale>(() => loadLocale());
  const [uiFontSize, setUiFontSize] = useState(() => loadUiFontSize());
  const [workspaceHome, setWorkspaceHome] = useState("");
  const [workspaceRoot, setWorkspaceRoot] = useState("");
  const [composerIntent, setComposerIntent] = useState<"chat" | "task">("task");
  const [diskProjects, setDiskProjects] = useState<ProjectDir[]>([]);
  const [createProjectOpen, setCreateProjectOpen] = useState(false);
  const [createProjectName, setCreateProjectName] = useState("");
  const [creatingProject, setCreatingProject] = useState(false);
  const [hiddenProjectPaths, setHiddenProjectPaths] = useState<string[]>([]);
  const [projectMenu, setProjectMenu] = useState<{ root: string; x: number; y: number } | null>(null);
  const [prompt, setPrompt] = useState("");
  const [composerImages, setComposerImages] = useState<ComposerImage[]>([]);
  const imageInputRef = useRef<HTMLInputElement | null>(null);
  const [mode, setMode] = useState<Mode>("ask");
  const [model, setModel] = useState("");
  const [reasoningEffort, setReasoningEffort] = useState<ReasoningEffort>("high");
  const [maxProviderRetries, setMaxProviderRetries] = useState(DEFAULT_MAX_PROVIDER_RETRIES);
  const [maxToolRounds, setMaxToolRounds] = useState(DEFAULT_MAX_TOOL_ROUNDS);
  const [circuitFailureThreshold, setCircuitFailureThreshold] = useState(DEFAULT_CIRCUIT_FAILURE_THRESHOLD);
  const [streamFirstEventTimeoutSecs, setStreamFirstEventTimeoutSecs] = useState(DEFAULT_STREAM_FIRST_EVENT_TIMEOUT_SECS);
  const [streamIdleTimeoutSecs, setStreamIdleTimeoutSecs] = useState(DEFAULT_STREAM_IDLE_TIMEOUT_SECS);
  const [nonStreamTimeoutSecs, setNonStreamTimeoutSecs] = useState(DEFAULT_NON_STREAM_TIMEOUT_SECS);
  const [circuitRecoverySuccessThreshold, setCircuitRecoverySuccessThreshold] = useState(DEFAULT_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD);
  const [circuitRecoveryWaitSecs, setCircuitRecoveryWaitSecs] = useState(DEFAULT_CIRCUIT_RECOVERY_WAIT_SECS);
  const [circuitErrorRateThresholdPercent, setCircuitErrorRateThresholdPercent] = useState(DEFAULT_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT);
  const [circuitMinRequestCount, setCircuitMinRequestCount] = useState(DEFAULT_CIRCUIT_MIN_REQUEST_COUNT);
  const [availableModels, setAvailableModels] = useState<ProviderModel[]>([]);
  const [modelsLoading, setModelsLoading] = useState(false);
  const [modelsError, setModelsError] = useState<string | null>(null);
  const [providerModelsById, setProviderModelsById] = useState<Record<string, ProviderModel[]>>({});
  const [providerModelsLoadingById, setProviderModelsLoadingById] = useState<Record<string, boolean>>({});
  const [providerModelErrorsById, setProviderModelErrorsById] = useState<Record<string, string>>({});
  const [commandAllowlistText, setCommandAllowlistText] = useState("");
  const [commandDenylistText, setCommandDenylistText] = useState("");
  const [sessions, setSessions] = useState<Session[]>([]);
  const [activeSessionId, setActiveSessionId] = useState<string | null>(null);
  const [sessionMenu, setSessionMenu] = useState<{ sessionId: string; x: number; y: number } | null>(null);
  const [collapsedProjects, setCollapsedProjects] = useState<Record<string, boolean>>({});
  const [followUpQueue, setFollowUpQueue] = useState<Array<{ id: string; sessionId: string; text: string; images: ChatImageAttachment[] }>>([]);
  const followUpQueueRef = useRef<Array<{ id: string; sessionId: string; text: string; images: ChatImageAttachment[] }>>([]);
  const [composerSendMode, setComposerSendMode] = useState<"queue" | "steer">("queue");
  // Per-session in-flight workers so multiple tasks can run in parallel.
  const chatInFlightBySessionRef = useRef<Map<string, Promise<string | null>>>(new Map());
  const chatGenerationBySessionRef = useRef<Map<string, number>>(new Map());
  const draftInFlightRef = useRef<Promise<string | null> | null>(null);
  const draftGenerationRef = useRef(0);
  // Monotonic counter shared by draft + session turns so draft→session handoff cannot collide with steer gens.
  const chatGenerationMonoRef = useRef(0);
  const streamedTextBySessionRef = useRef<Map<string, string>>(new Map());
  const drainFollowUpsBySessionRef = useRef<Set<string>>(new Set());
  const activeSessionIdRef = useRef<string | null>(null);
  const sessionsRef = useRef<Session[]>([]);
  const [messages, setMessages] = useState<Message[]>([]);
  const [streamedText, setStreamedText] = useState("");
  const [streamedTextBySession, setStreamedTextBySession] = useState<Record<string, string>>({});
  const [plan, setPlan] = useState<PlanStep[]>([]);
  const [activity, setActivity] = useState<ActivityItem[]>([]);
  const [pendingAction, setPendingAction] = useState<PendingAction | null>(null);
  const [approvalSummary, setApprovalSummary] = useState<string | null>(null);
  const [patchPreview, setPatchPreview] = useState<PatchPreview | null>(null);
  const [rememberLocalApiApproval, setRememberLocalApiApproval] = useState(false);
  const [restorePoints, setRestorePoints] = useState<RestorePoint[]>([]);
  const [taskSummary, setTaskSummary] = useState<TaskSummary | null>(null);
  const [replaySteps, setReplaySteps] = useState<ReplayStep[]>([]);
  const [providerStatus, setProviderStatus] = useState<ProviderAuthStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [modelCallLogs, setModelCallLogs] = useState<ModelCallLog[]>([]);
  const [modelCallLogsLoading, setModelCallLogsLoading] = useState(false);
  const [modelCallLogsError, setModelCallLogsError] = useState<string | null>(null);
  const [runningSessionIds, setRunningSessionIds] = useState<string[]>([]);
  const [draftRunning, setDraftRunning] = useState(false);
  const [runStatusBySession, setRunStatusBySession] = useState<Record<string, RunStatus>>({});
  const [draftRunStatus, setDraftRunStatus] = useState<RunStatus | null>(null);
  const [runStatusExpanded, setRunStatusExpanded] = useState(false);
  const [runStatusClock, setRunStatusClock] = useState(() => Date.now());
  const [conversationAtBottom, setConversationAtBottom] = useState(true);
  const [isSavingConfig, setIsSavingConfig] = useState(false);
  const [view, setView] = useState<"workbench" | "settings" | "model-logs">("workbench");
  const [providers, setProviders] = useState<CloudProviderConfig[]>([
    { id: "default", name: defaultProvider, base_url: DEFAULT_PROVIDER_BASE_URL },
  ]);
  const [activeProviderId, setActiveProviderId] = useState("default");
  const [selectedProviderId, setSelectedProviderId] = useState("default");
  const [showApiKey, setShowApiKey] = useState(false);
  const [diagnosticsOpen, setDiagnosticsOpen] = useState(false);
  const [userConfigReady, setUserConfigReady] = useState(false); // used to delay hydration
  const conversationRef = useRef<HTMLDivElement | null>(null);
  const conversationAtBottomRef = useRef(true);
  const pendingConversationScrollTopRef = useRef<number | null>(null);
  const pendingConversationScrollToBottomRef = useRef(false);
  const envButtonRef = useRef<HTMLButtonElement | null>(null);
  const [bottomPanelOpen, setBottomPanelOpen] = useState(false);
  const [rightPanelOpen, setRightPanelOpen] = useState(false);
  const [rightPanelTab, setRightPanelTab] = useState<ToolPanelTab>("review");
  const [browserNavigation, setBrowserNavigation] = useState<BrowserNavigationRequest | null>(null);
  const [rightPanelWidth, setRightPanelWidth] = useState(() => loadRightPanelWidth());
  const [envPopoverOpen, setEnvPopoverOpen] = useState(false);
  const [contextUsageOpen, setContextUsageOpen] = useState(false);

  useEffect(() => {
    setRememberLocalApiApproval(false);
  }, [pendingAction?.id]);

  useEffect(() => {
    const onResize = () => {
      setRightPanelWidth((current) => {
        const next = clampRightPanelWidth(current);
        if (next !== current) saveRightPanelWidth(next);
        return next;
      });
    };
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);
  const [envSummary, setEnvSummary] = useState("···");

  const activeSession = useMemo(
    () => sessions.find((session) => session.id === activeSessionId) ?? null,
    [activeSessionId, sessions],
  );
  useEffect(() => {
    activeSessionIdRef.current = activeSessionId;
  }, [activeSessionId]);
  useEffect(() => {
    sessionsRef.current = sessions;
  }, [sessions]);

  const activeSessionRunning = !!(
    (!activeSessionId && draftRunning)
    || (!!activeSessionId && runningSessionIds.includes(activeSessionId))
    || activeSession?.status === "running"
  );
  const anySessionRunning = draftRunning || runningSessionIds.length > 0;
  // Active-session aliases keep the rest of the workbench logic readable.
  const isRunning = activeSessionRunning;
  const runStatus = activeSessionId
    ? (runStatusBySession[activeSessionId] ?? null)
    : draftRunStatus;
  const completedRunElapsed = useMemo(
    () => completedRunElapsedByMessageId(messages),
    [messages],
  );
  const currentPlanStepIndex = currentRunPlanStep(plan, activity);
  const latestActivity = activity.length > 0 ? activity[activity.length - 1] : null;

  const activeProvider = useMemo(
    () => providers.find((item) => item.id === activeProviderId) ?? providers[0] ?? null,
    [activeProviderId, providers],
  );
  const selectedProvider = useMemo(
    () => providers.find((item) => item.id === selectedProviderId) ?? activeProvider,
    [activeProvider, providers, selectedProviderId],
  );
  const selectedProviderModels = selectedProvider ? providerModelsById[selectedProvider.id] ?? [] : [];
  const selectedProviderModelsLoading = selectedProvider ? Boolean(providerModelsLoadingById[selectedProvider.id]) : false;
  const selectedProviderModelsError = selectedProvider ? providerModelErrorsById[selectedProvider.id] : undefined;

  function updateProvider(id: string, patch: Partial<CloudProviderConfig>): void {
    setProviders((current) => current.map((item) => (item.id === id ? { ...item, ...patch } : item)));
  }

  function addProvider(): void {
    const id = providerId();
    setProviders((current) => [
      ...current,
      { id, name: `Provider ${current.length + 1}`, base_url: DEFAULT_PROVIDER_BASE_URL },
    ]);
    setSelectedProviderId(id);
  }

  function activateProvider(id: string): void {
    const next = providers.find((item) => item.id === id);
    if (!next) return;
    setActiveProviderId(id);
    setSelectedProviderId(id);
    setAvailableModels(loadCachedModels(normalizeProviderBaseUrl(next.base_url) || DEFAULT_PROVIDER_BASE_URL));
    setModelsError(null);
    setProviderStatus(null);
  }

  function deleteProvider(id: string): void {
    if (providers.length <= 1) return;
    const next = providers.filter((item) => item.id !== id);
    const fallback = next.find((item) => item.id === activeProviderId) ?? next[0];
    setProviders(next);
    if (selectedProviderId === id) setSelectedProviderId(fallback.id);
    if (activeProviderId === id) activateProvider(fallback.id);
  }

  const chatSessions = useMemo(
    () => sessions.filter((session) => isChatWorkspace(session.workspace_root)),
    [sessions],
  );
  const hiddenProjectSet = useMemo(
    () => new Set(hiddenProjectPaths.map((item) => normalizeRoot(item)).filter(Boolean)),
    [hiddenProjectPaths],
  );
  const projectGroups = useMemo(
    () =>
      groupSessionsByProject(
        sessions.filter(
          (session) =>
            !isChatWorkspace(session.workspace_root) &&
            !hiddenProjectSet.has(normalizeRoot(session.workspace_root)),
        ),
        diskProjects.filter((project) => !hiddenProjectSet.has(normalizeRoot(project.path))),
      ),
    [sessions, diskProjects, hiddenProjectSet],
  );

  const envSources = useMemo<SourceItem[]>(
    () =>
      composerImages.map((image) => ({
        id: image.id,
        name: image.name || image.id,
        kind: "image" as const,
      })),
    [composerImages],
  );
  const contextUsage = useMemo(() => {
    const limit = contextWindowForModel(model);
    const used = SYSTEM_CONTEXT_TOKEN_RESERVE
      + messages.reduce((total, message) => total + estimateMessageTokens(message), 0)
      + estimateTextTokens(streamedText)
      + estimateTextTokens(prompt)
      + composerImages.length * IMAGE_CONTEXT_TOKEN_ESTIMATE;
    return {
      limit,
      percent: Math.min(100, Math.round((used / limit) * 100)),
      used,
    };
  }, [composerImages.length, messages, model, prompt, streamedText]);

  useEffect(() => {
    followUpQueueRef.current = followUpQueue;
  }, [followUpQueue]);

  useEffect(() => {
    saveLocale(locale);
    document.documentElement.lang = locale === "zh-CN" ? "zh-CN" : "en";
  }, [locale]);

  useEffect(() => {
    applyUiFontSize(uiFontSize);
    saveUiFontSize(uiFontSize);
  }, [uiFontSize]);

  useEffect(() => {
    let cancelled = false;
    if (!isTauriRuntime || !workspaceRoot.trim()) {
      setEnvSummary("···");
      return;
    }
    // Header only needs branch + diffstat; skip local branch enumeration on the hot path.
    // Debounce so startup paint and rapid isRunning flips do not stampede git.exe.
    const delayMs = isRunning ? 800 : 160;
    const timer = window.setTimeout(() => {
      void (async () => {
        try {
          const env = await fetchGitEnvironment(workspaceRoot.trim(), false);
          if (cancelled) return;
          if (!env.is_repo) {
            setEnvSummary(t(locale, "env.notRepo"));
            return;
          }
          const branch = env.branch || t(locale, "env.noBranch");
          const stat = formatDiffStat(env.insertions, env.deletions);
          setEnvSummary(env.insertions || env.deletions ? `${branch} ${stat}` : branch);
        } catch {
          if (!cancelled) setEnvSummary("git");
        }
      })();
    }, delayMs);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [workspaceRoot, locale, taskSummary, isRunning]);

  useEffect(() => {
    const onKey = (event: globalThis.KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      const tag = (target?.tagName || "").toLowerCase();
      const typing =
        tag === "input" ||
        tag === "textarea" ||
        tag === "select" ||
        Boolean(target?.isContentEditable);
      const mod = event.ctrlKey || event.metaKey;
      if (!mod) return;

      if (event.key.toLowerCase() === "j" && !event.shiftKey && !event.altKey) {
        event.preventDefault();
        toggleBottomPanel();
        return;
      }
      if (event.key.toLowerCase() === "g" && event.shiftKey && !event.altKey) {
        event.preventDefault();
        openRightPanel("review");
        return;
      }
      if (event.key.toLowerCase() === "t" && !event.shiftKey && !event.altKey && !typing) {
        event.preventDefault();
        openRightPanel("browser");
        return;
      }
      if (event.key.toLowerCase() === "p" && !event.shiftKey && !event.altKey && !typing) {
        event.preventDefault();
        openRightPanel("files");
        return;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  useEffect(() => {
    if (!isTauriRuntime) {
      setUserConfigReady(true);
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const config = await invoke<UserConfig>("get_user_config");
        if (cancelled) return;
        if (isLocale(config.locale)) setLocale(config.locale);
        setMode(config.mode);
        setModel((config.model || "").trim());
        setReasoningEffort(normalizeReasoningEffort(config.reasoning_effort));
        setMaxProviderRetries(normalizeBoundedInteger(config.max_provider_retries, DEFAULT_MAX_PROVIDER_RETRIES, MIN_MAX_PROVIDER_RETRIES, MAX_MAX_PROVIDER_RETRIES));
        setMaxToolRounds(normalizeBoundedInteger(config.max_tool_rounds, DEFAULT_MAX_TOOL_ROUNDS, MIN_MAX_TOOL_ROUNDS, MAX_MAX_TOOL_ROUNDS));
        setCircuitFailureThreshold(normalizeBoundedInteger(config.circuit_failure_threshold, DEFAULT_CIRCUIT_FAILURE_THRESHOLD, MIN_CIRCUIT_FAILURE_THRESHOLD, MAX_CIRCUIT_FAILURE_THRESHOLD));
        setStreamFirstEventTimeoutSecs(normalizeBoundedInteger(config.stream_first_event_timeout_secs, DEFAULT_STREAM_FIRST_EVENT_TIMEOUT_SECS, MIN_STREAM_FIRST_EVENT_TIMEOUT_SECS, MAX_STREAM_FIRST_EVENT_TIMEOUT_SECS));
        setStreamIdleTimeoutSecs(normalizeBoundedInteger(config.stream_idle_timeout_secs, DEFAULT_STREAM_IDLE_TIMEOUT_SECS, MIN_STREAM_IDLE_TIMEOUT_SECS, MAX_STREAM_IDLE_TIMEOUT_SECS));
        setNonStreamTimeoutSecs(normalizeBoundedInteger(config.non_stream_timeout_secs, DEFAULT_NON_STREAM_TIMEOUT_SECS, MIN_NON_STREAM_TIMEOUT_SECS, MAX_NON_STREAM_TIMEOUT_SECS));
        setCircuitRecoverySuccessThreshold(normalizeBoundedInteger(config.circuit_recovery_success_threshold, DEFAULT_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD, MIN_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD, MAX_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD));
        setCircuitRecoveryWaitSecs(normalizeBoundedInteger(config.circuit_recovery_wait_secs, DEFAULT_CIRCUIT_RECOVERY_WAIT_SECS, MIN_CIRCUIT_RECOVERY_WAIT_SECS, MAX_CIRCUIT_RECOVERY_WAIT_SECS));
        setCircuitErrorRateThresholdPercent(normalizeBoundedInteger(config.circuit_error_rate_threshold_percent, DEFAULT_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT, MIN_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT, MAX_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT));
        setCircuitMinRequestCount(normalizeBoundedInteger(config.circuit_min_request_count, DEFAULT_CIRCUIT_MIN_REQUEST_COUNT, MIN_CIRCUIT_MIN_REQUEST_COUNT, MAX_CIRCUIT_MIN_REQUEST_COUNT));
        const hydratedProviders = hydrateProviders(config);
        setProviders(hydratedProviders.providers);
        setActiveProviderId(hydratedProviders.activeProviderId);
        setSelectedProviderId(hydratedProviders.activeProviderId);
        const selectedProvider = hydratedProviders.providers.find((item) => item.id === hydratedProviders.activeProviderId)
          ?? hydratedProviders.providers[0];
        // Instant model picker from last successful list; network refresh is deferred.
        const cachedModels = loadCachedModels(selectedProvider.base_url);
        if (cachedModels.length > 0) {
          setAvailableModels(cachedModels);
        }
        if (config.workspace_home?.trim()) {
          setWorkspaceHome(config.workspace_home.trim());
        }
        if (config.last_workspace_root?.trim()) {
          setWorkspaceRoot(config.last_workspace_root.trim());
        }
        setHiddenProjectPaths(
          Array.isArray(config.hidden_project_paths)
            ? config.hidden_project_paths.map((item) => String(item || "").trim()).filter(Boolean)
            : [],
        );
      } catch (cause) {
        if (!cancelled) {
          setError(cause instanceof Error ? cause.message : String(cause));
        }
      } finally {
        if (!cancelled) setUserConfigReady(true);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const refreshProviderStatus = useCallback(async () => {
    if (!isTauriRuntime) return;
    try {
      const status = await invoke<ProviderAuthStatus>("provider_status");
      setProviderStatus(status);
    } catch (cause) {
      setProviderStatus({
        ready: false,
        has_api_key: false,
        base_url: "https://ai.v58.dev/v1",
        message: cause instanceof Error ? cause.message : String(cause),
      });
    }
  }, []);

  const maskApiKeyHint = useCallback((key: string): string | undefined => {
    const trimmed = key.trim();
    if (!trimmed) return undefined;
    if (trimmed.length <= 4) return "****";
    return `...${trimmed.slice(-4)}`;
  }, []);

  const refreshModels = useCallback(async (options?: {
    silent?: boolean;
    provider?: CloudProviderConfig | null;
    updateComposerModels?: boolean;
  }): Promise<boolean> => {
    const provider = options?.provider ?? activeProvider;
    const providerId = provider?.id;
    const configuredBase = normalizeProviderBaseUrl(provider?.base_url) || DEFAULT_PROVIDER_BASE_URL;
    const configuredKey = provider?.api_key || "";
    const silent = Boolean(options?.silent);
    const updateComposerModels = options?.updateComposerModels
      ?? (!options?.provider || provider?.id === activeProvider?.id);
    if (!isTauriRuntime) {
      const message = t(locale, "models.tauriOnly");
      if (providerId) {
        setProviderModelErrorsById((current) => ({ ...current, [providerId]: message }));
      }
      if (updateComposerModels) {
        setModelsError(message);
        setProviderStatus({
          ready: false,
          has_api_key: Boolean(configuredKey.trim()),
          base_url: configuredBase,
          key_hint: maskApiKeyHint(configuredKey),
          message,
        });
      }
      return false;
    }
    if (providerId) {
      setProviderModelsLoadingById((current) => ({ ...current, [providerId]: true }));
      setProviderModelErrorsById((current) => ({ ...current, [providerId]: "" }));
    }
    if (updateComposerModels) {
      setModelsLoading(true);
      if (!silent) {
        setModelsError(null);
        setProviderStatus((current) => ({
          ready: false,
          has_api_key: Boolean(configuredKey.trim() || current?.has_api_key),
          base_url: configuredBase,
          key_hint: maskApiKeyHint(configuredKey) || current?.key_hint,
          message: t(locale, "auth.checking"),
        }));
      }
    }
    try {
      const result = await invoke<ListModelsResult>("list_provider_models", {
        baseUrl: configuredBase,
        apiKey: configuredKey.trim() || null,
      });
      if (providerId) {
        setProviderModelsById((current) => ({ ...current, [providerId]: result.models }));
        setProviderModelErrorsById((current) => ({ ...current, [providerId]: "" }));
      }
      if (updateComposerModels) {
        saveCachedModels(configuredBase, result.models);
        setAvailableModels(result.models);
        setModelsError(null);
        setProviderStatus({
          ready: true,
          has_api_key: Boolean(configuredKey.trim()),
          base_url: result.base_url || configuredBase,
          key_hint: maskApiKeyHint(configuredKey),
          message: t(locale, "auth.modelsOk", { count: String(result.models.length) }),
        });
      }
      return true;
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause);
      if (providerId) {
        setProviderModelErrorsById((current) => ({ ...current, [providerId]: message }));
      }
      if (updateComposerModels) {
        if (!silent) {
          setAvailableModels([]);
          setModelsError(message);
        } else {
          // Keep cached models on background refresh failure.
          setModelsError((current) => current ?? message);
        }
        setProviderStatus((current) => ({
          ready: silent ? Boolean(current?.ready) : false,
          has_api_key: Boolean(configuredKey.trim() || current?.has_api_key),
          base_url: configuredBase,
          key_hint: maskApiKeyHint(configuredKey) || current?.key_hint,
          message: silent && current?.ready ? (current.message || message) : message,
        }));
      }
      return false;
    } finally {
      if (providerId) {
        setProviderModelsLoadingById((current) => ({ ...current, [providerId]: false }));
      }
      if (updateComposerModels) setModelsLoading(false);
    }
  }, [activeProvider, locale, maskApiKeyHint]);
  const loadWorkspaceConfig = useCallback(async () => {
    const root = workspaceRoot.trim();
    if (!isTauriRuntime || !root) return;
    try {
      const config = await invoke<WorkspaceConfig>("workspace_config", { workspaceRoot: root });
      setMode(config.mode);
      // Model lives in the composer / user config. Workspace defaults must not overwrite it
      // (missing workspace rows synthesize model=gpt-5.5 and would pin the picker).
      setCommandAllowlistText(formatCommandAllowlistText(config.command_allowlist));
      setCommandDenylistText(formatCommandDenylistText(config.command_denylist));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }, [workspaceRoot]);

  const refreshDiskProjects = useCallback(async () => {
    if (!isTauriRuntime) return;
    const home = workspaceHome.trim();
    if (!home) {
      setDiskProjects([]);
      return;
    }
    try {
      const projects = await invoke<ProjectDir[]>("list_projects", { workspaceHome: home });
      setDiskProjects(projects);
    } catch {
      // Keep the last known project list if scanning fails.
    }
  }, [workspaceHome]);

  const refreshSessions = useCallback(async () => {
    if (!isTauriRuntime) return;
    try {
      const nextSessions = await invoke<Session[]>("list_sessions", { workspaceRoot: null });
      setSessions(nextSessions);
    } catch {
      // Keep the last known session list if refresh fails.
    }
  }, []);


  const refreshWorkspace = useCallback(async () => {
    await refreshDiskProjects();
    await Promise.all([refreshSessions(), loadWorkspaceConfig(), refreshProviderStatus()]);
  }, [loadWorkspaceConfig, refreshDiskProjects, refreshProviderStatus, refreshSessions]);

  useEffect(() => {
    if (!userConfigReady) return;
    if (!isTauriRuntime) {
      setModelsError(t(locale, "models.tauriOnly"));
      return;
    }
    // Defer network model list so first paint / session list are not competing with it.
    const timer = window.setTimeout(() => {
      void refreshModels({ silent: true });
    }, 120);
    return () => window.clearTimeout(timer);
  }, [userConfigReady, refreshModels]);

  const hydrateSession = useCallback(async (sessionId: string) => {
    if (!isTauriRuntime) return;
    try {
      const detail = await invoke<SessionDetail>("session_detail", { sessionId });
      const pending = detail.session.status === "need_user"
        ? detail.pending_actions.find((action) => action.status === "pending") ?? null
        : null;
      setMessages(detail.messages);
      const liveStream = streamedTextBySessionRef.current.get(sessionId) ?? "";
      setStreamedText(liveStream);
      setPlan(latestPlan(detail.events));
      setActivity(buildActivity(detail.events, locale));
      setPendingAction(pending);
      setApprovalSummary(latestApprovalSummary(detail.events, pending));
      setPatchPreview(latestPatchPreview(detail.events, pending));
      setRestorePoints(detail.restore_points);
      setTaskSummary(latestTaskSummary(detail.events));
      setReplaySteps([]);
      setSessions((current) => current.some((session) => session.id === detail.session.id)
        ? current.map((session) => session.id === detail.session.id ? detail.session : session)
        : [detail.session, ...current]);
      if (detail.session.status === "done" || detail.session.status === "cancelled") {
        try {
          const replay = await invoke<ReplaySessionResult>("session_replay", { sessionId });
          setReplaySteps(replay.steps);
        } catch {
          // Keep the session usable when replay reconstruction is unavailable.
        }
      }
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }, [locale]);

  const loadModelCallLogs = useCallback(async (sessionId: string | null = activeSessionId) => {
    if (!sessionId) {
      setModelCallLogs([]);
      setModelCallLogsError(null);
      return;
    }
    if (!isTauriRuntime) {
      setModelCallLogsError(t(locale, "models.tauriOnly"));
      return;
    }

    setModelCallLogsLoading(true);
    setModelCallLogsError(null);
    try {
      const detail = await invoke<SessionDetail>("session_detail", { sessionId });
      setModelCallLogs(
        detail.events.filter(
          (item): item is ModelCallLog => item.event.type === "model_call",
        ),
      );
    } catch (cause) {
      setModelCallLogsError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setModelCallLogsLoading(false);
    }
  }, [activeSessionId, locale]);

  useEffect(() => {
    if (!userConfigReady) return;
    void refreshWorkspace();
  }, [refreshWorkspace, userConfigReady]);
  useEffect(() => {
    if (activeSessionId) void hydrateSession(activeSessionId);
  }, [activeSessionId, hydrateSession]);

  useEffect(() => {
    const sid = activeSessionId;
    if (!sid) return;
    streamedTextBySessionRef.current.set(sid, streamedText);
  }, [streamedText, activeSessionId]);

  const updateConversationBottomState = useCallback((node: HTMLDivElement) => {
    const atBottom = node.scrollHeight - node.scrollTop - node.clientHeight <= 48;
    conversationAtBottomRef.current = atBottom;
    setConversationAtBottom(atBottom);
  }, []);

  const scrollConversationToBottom = useCallback((behavior: ScrollBehavior = "smooth") => {
    const node = conversationRef.current;
    if (!node) return;
    conversationAtBottomRef.current = true;
    setConversationAtBottom(true);
    node.scrollTo({ top: node.scrollHeight, behavior });
  }, []);

  useEffect(() => {
    conversationAtBottomRef.current = true;
    setConversationAtBottom(true);
  }, [activeSessionId]);

  useEffect(() => {
    const node = conversationRef.current;
    if (!node || !conversationAtBottomRef.current) return;
    node.scrollTop = node.scrollHeight;
    setConversationAtBottom(true);
  }, [messages, streamedText, error, isRunning, runStatus]);

  useLayoutEffect(() => {
    if (view !== "workbench") return;
    const node = conversationRef.current;
    if (!node) return;
    if (pendingConversationScrollToBottomRef.current) {
      node.scrollTop = node.scrollHeight;
      conversationAtBottomRef.current = true;
      setConversationAtBottom(true);
      pendingConversationScrollToBottomRef.current = false;
      pendingConversationScrollTopRef.current = null;
      return;
    }
    const scrollTop = pendingConversationScrollTopRef.current;
    if (scrollTop === null) return;
    const maxScrollTop = Math.max(0, node.scrollHeight - node.clientHeight);
    node.scrollTop = Math.min(scrollTop, maxScrollTop);
    updateConversationBottomState(node);
    pendingConversationScrollTopRef.current = null;
  }, [updateConversationBottomState, view]);

  useEffect(() => {
    if (view !== "workbench" || typeof ResizeObserver === "undefined") return;
    const node = conversationRef.current;
    if (!node) return;
    const observer = new ResizeObserver(() => {
      if (!conversationAtBottomRef.current) return;
      node.scrollTop = node.scrollHeight;
    });
    observer.observe(node);
    return () => observer.disconnect();
  }, [view]);

  function openSettings(): void {
    pendingConversationScrollToBottomRef.current = conversationAtBottomRef.current;
    pendingConversationScrollTopRef.current = conversationRef.current?.scrollTop ?? null;
    setView("settings");
  }

  function returnToWorkbench(): void {
    setView("workbench");
  }

  function returnToConversationFromModelLogs(): void {
    pendingConversationScrollToBottomRef.current = true;
    setView("workbench");
  }

  useEffect(() => {
    if (!runStatus || runStatus.phase === "failed") return;
    const timer = window.setInterval(() => setRunStatusClock(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [runStatus?.phase, runStatus?.startedAt]);

  useEffect(() => {
    if (!isTauriRuntime) return;
    let unlisten: (() => void) | undefined;
    void listen<SessionEvent>("session-event", (event) => {
      const payload = event.payload;
      const sid = payload.session_id;
      const isActive = activeSessionIdRef.current === sid;

      const markRunning = (running: boolean) => {
        if (running) setDraftRunning(false);
        setRunningSessionIds((current) => {
          if (running) return current.includes(sid) ? current : [...current, sid];
          return current.filter((id) => id !== sid);
        });
        if (running) {
          setSessions((current) =>
            current.map((session) =>
              session.id === sid && session.status !== "running" && session.status !== "need_user"
                ? { ...session, status: "running" }
                : session,
            ),
          );
        }
      };
      const patchRunStatus = (next: RunStatus | null | ((current: RunStatus | undefined) => RunStatus | null)) => {
        setRunStatusBySession((current) => {
          const prev = current[sid];
          const value = typeof next === "function" ? next(prev) : next;
          if (!value) {
            if (!(sid in current)) return current;
            const { [sid]: _removed, ...rest } = current;
            return rest;
          }
          return { ...current, [sid]: value };
        });
      };
      const patchStream = (updater: (current: string) => string) => {
        setStreamedTextBySession((current) => {
          const value = updater(current[sid] ?? "");
          streamedTextBySessionRef.current.set(sid, value);
          return { ...current, [sid]: value };
        });
        if (isActive) {
          setStreamedText((current) => updater(current));
        }
      };
      const clearStream = () => {
        streamedTextBySessionRef.current.set(sid, "");
        setStreamedTextBySession((current) => {
          if (!(sid in current)) return current;
          const { [sid]: _removed, ...rest } = current;
          return rest;
        });
        if (isActive) setStreamedText("");
      };

      // Only auto-focus a session when nothing is selected yet (first event of a new task).
      // Never steal focus from another parallel session the user is viewing.
      if (!activeSessionIdRef.current) {
        setActiveSessionId(sid);
      }

      if (payload.type === "text_delta") {
        markRunning(true);
        patchStream((current) => current + payload.delta);
        patchRunStatus((current) => current?.phase === "retrying"
          ? { ...current, phase: "thinking", detail: undefined }
          : (current ?? null));
      }
      if (payload.type === "retrying") {
        markRunning(true);
        patchRunStatus((current) => ({
          startedAt: current?.startedAt ?? Date.now(),
          phase: "retrying",
          retryAttempt: payload.attempt,
          retryMaxAttempts: payload.max_attempts,
          detail: payload.message,
        }));
      }
      if (payload.type === "message_completed") {
        // Defensive fallback for sessions produced by an older backend: never render an
        // empty assistant bubble. The current backend turns an empty provider response
        // into an explicit failed session instead.
        if (isActive && payload.message.content.trim()) {
          setMessages((current) => mergeMessage(current, payload.message));
        }
        clearStream();
        // The message is ready, but the session stays running until task_completed.
        // New input must remain queued while the backend finishes this task.
      }
      if (payload.type === "plan") {
        if (isActive) setPlan(payload.steps);
      }
      if (payload.type === "patch_preview") {
        if (isActive) setPatchPreview(payload.preview);
      }
      if (payload.type === "approval_requested") {
        markRunning(false);
        setSessions((current) =>
          current.map((session) =>
            session.id === sid ? { ...session, status: "need_user" } : session,
          ),
        );
        if (isActive) {
          setPendingAction(payload.action);
          setApprovalSummary(payload.summary);
        }
      }
      if (payload.type === "session_cancelled") {
        // Keep whatever the model already streamed so steer/cancel does not blank the transcript.
        if (isActive) {
          commitStreamedAssistant(sid);
          setPendingAction(null);
          setApprovalSummary(null);
          setRunStatusExpanded(false);
        } else {
          clearStream();
        }
        markRunning(false);
        patchRunStatus(null);
        setSessions((current) =>
          current.map((session) =>
            session.id === sid ? { ...session, status: "cancelled" } : session,
          ),
        );
      }
      if (payload.type === "task_completed") {
        markRunning(false);
        patchRunStatus(null);
        setSessions((current) =>
          current.map((session) =>
            session.id === sid ? { ...session, status: "done" } : session,
          ),
        );
        if (isActive) {
          setTaskSummary(payload.summary);
          setRunStatusExpanded(false);
        }
      }
      if (payload.type === "error") {
        // Remote/provider failures end the turn as failed (after provider retries).
        markRunning(false);
        patchRunStatus((current) => ({
          startedAt: current?.startedAt ?? Date.now(),
          phase: "failed",
          detail: payload.message,
        }));
        setSessions((current) =>
          current.map((session) =>
            session.id === sid ? { ...session, status: "failed" } : session,
          ),
        );
        if (isActive) {
          setError(payload.message);
          setRunStatusExpanded(false);
          setPendingAction(null);
          setApprovalSummary(null);
        }
      }
      if (isActive) {
        const nextActivity = eventActivity(payload, `${payload.type}-${Date.now()}`, locale);
        if (nextActivity) {
          setActivity((current) => {
            const index = current.findIndex((item) => item.id === nextActivity.id);
            if (index < 0) return [...current, nextActivity];
            return current.map((item) =>
              item.id === nextActivity.id ? mergeActivity(item, nextActivity) : item,
            );
          });
        }
      }
    }).then((stop) => { unlisten = stop; });
    return () => unlisten?.();
  }, [locale]);

  function commitStreamedAssistant(sessionId?: string | null): void {
    const sid = sessionId || activeSessionIdRef.current || "";
    const text = (sid ? (streamedTextBySessionRef.current.get(sid) ?? "") : streamedText).trim()
      ? (sid ? (streamedTextBySessionRef.current.get(sid) ?? streamedText) : streamedText)
      : streamedText;
    if (!text.trim()) {
      if (sid) {
        streamedTextBySessionRef.current.set(sid, "");
        setStreamedTextBySession((current) => {
          if (!(sid in current)) return current;
          const { [sid]: _removed, ...rest } = current;
          return rest;
        });
      }
      if (!sid || activeSessionIdRef.current === sid) setStreamedText("");
      return;
    }
    if (!sid || activeSessionIdRef.current === sid) {
      setMessages((current) => {
        const last = current[current.length - 1];
        if (last?.role === "assistant" && last.content === text) return current;
        return [
          ...current,
          {
            id: `local-assistant-${Date.now()}`,
            session_id: sid,
            role: "assistant",
            content: text,
            created_at: new Date().toISOString(),
          },
        ];
      });
      setStreamedText("");
    }
    if (sid) {
      streamedTextBySessionRef.current.set(sid, "");
      setStreamedTextBySession((current) => {
        if (!(sid in current)) return current;
        const { [sid]: _removed, ...rest } = current;
        return rest;
      });
    }
  }


  function canContinueSession(session: Session | null): boolean {
    return !!session && (
      session.status === "done"
      || session.status === "failed"
      || session.status === "created"
      || session.status === "cancelled"
    );
  }

  function selectProject(root: string): void {
    const next = root.trim();
    if (!next) return;
    if (next !== workspaceRoot.trim()) {
      setWorkspaceRoot(next);
    }
    startNewTask();
    if (isTauriRuntime) {
      void (async () => {
        try {
          const current = await invoke<UserConfig>("get_user_config");
          await invoke<UserConfig>("set_user_config", {
            config: { ...current, last_workspace_root: next, workspace_home: workspaceHome.trim() || current.workspace_home } satisfies UserConfig,
          });
        } catch {
          // Ignore preference persistence failures.
        }
      })();
    }
  }

  function selectSession(session: Session): void {
    setSessionMenu(null);
    setView("workbench");
    setActiveSessionId(session.id);
    // Keep draftRunning for background new-task creation; activeSessionRunning ignores it when a session is selected.
    const live = streamedTextBySessionRef.current.get(session.id) ?? streamedTextBySession[session.id] ?? "";
    setStreamedText(live);
    setError(null);
    setRunStatusExpanded(false);
    if (isChatWorkspace(session.workspace_root)) {
      setComposerIntent("chat");
      // Keep current project workspace for terminal/files; chat is unbound.
      return;
    }
    setComposerIntent("task");
    const root = session.workspace_root.trim();
    if (root && root !== workspaceRoot.trim()) {
      setWorkspaceRoot(root);
    }
  }

  function toggleProjectCollapsed(root: string): void {
    setCollapsedProjects((current) => ({ ...current, [root]: !current[root] }));
  }

  function openTerminalPanel(seedCommand?: string): void {
    setBottomPanelOpen(true);
    setEnvPopoverOpen(false);
    if (seedCommand?.trim()) {
      seedTerminalCommand(seedCommand.trim());
    }
  }

  function openRightPanel(tab: ToolPanelTab): void {
    setRightPanelTab(tab);
    setRightPanelOpen(true);
    setEnvPopoverOpen(false);
  }

  function openAssistantLink(url: string): void {
    setBrowserNavigation((current) => ({ url, id: (current?.id ?? 0) + 1 }));
    openRightPanel("browser");
  }

  function openPanel(target: PanelTarget, seedCommand?: string): void {
    if (target === "terminal") {
      openTerminalPanel(seedCommand);
      return;
    }
    openRightPanel(target);
  }

  function toggleBottomPanel(): void {
    setBottomPanelOpen((current) => !current);
  }

  function toggleRightPanel(): void {
    setRightPanelOpen((current) => !current);
  }

  function resetComposerSession(): void {
    // Leave other sessions running in the background; only clear the composer view.
    setActiveSessionId(null);
    setComposerImages([]);
    setMessages([]);
    setStreamedText("");
    if (!draftInFlightRef.current) {
      setDraftRunning(false);
      setDraftRunStatus(null);
    }
    setPlan([]);
    setActivity([]);
    setPendingAction(null);
    setApprovalSummary(null);
    setPatchPreview(null);
    setRestorePoints([]);
    setTaskSummary(null);
    setReplaySteps([]);
    setError(null);
    setRunStatusExpanded(false);
  }

  function startNewTask(): void {
    setComposerIntent("task");
    setView("workbench");
    resetComposerSession();
  }

  function startNewChat(): void {
    setComposerIntent("chat");
    setView("workbench");
    resetComposerSession();
  }

  function enqueueFollowUp(sessionId: string, text: string, images: ChatImageAttachment[] = []): void {
    const item = {
      id: `queue-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
      sessionId,
      text,
      images,
    };
    setFollowUpQueue((current) => {
      const next = [...current, item];
      followUpQueueRef.current = next;
      return next;
    });
  }

  function removeFollowUp(id: string): void {
    setFollowUpQueue((current) => {
      const next = current.filter((item) => item.id !== id);
      followUpQueueRef.current = next;
      return next;
    });
  }

  function visibleFollowUps(sessionId: string | null): Array<{ id: string; sessionId: string; text: string; images: ChatImageAttachment[] }> {
    if (!sessionId) return [];
    return followUpQueue.filter((item) => item.sessionId === sessionId);
  }

  function editFollowUp(id: string): void {
    const item = followUpQueueRef.current.find((entry) => entry.id === id) || followUpQueue.find((entry) => entry.id === id);
    if (!item) return;
    removeFollowUp(id);
    setPrompt(item.text);
    setComposerImages(
      item.images.map((image, index) => ({
        id: `edit-${Date.now()}-${index}`,
        mime_type: image.mime_type,
        data_base64: image.data_base64,
        name: image.name,
        previewUrl: `data:${image.mime_type};base64,${image.data_base64}`,
      })),
    );
    setError(null);
  }

  function editSentUserMessage(content: string): void {
    const parsed = parseStoredUserMessage(content);
    setPrompt(parsed.text);
    setComposerImages(
      parsed.images.map((image, index) => ({
        id: `edit-sent-${Date.now()}-${index}`,
        mime_type: image.mime_type,
        data_base64: image.data_base64,
        name: undefined,
        previewUrl: `data:${image.mime_type};base64,${image.data_base64}`,
      })),
    );
    setError(null);
  }

  async function steerFollowUp(id: string): Promise<void> {
    const item = followUpQueueRef.current.find((entry) => entry.id === id) || followUpQueue.find((entry) => entry.id === id);
    if (!item) return;
    removeFollowUp(id);
    await sendChatMessage(item.text, { steer: true, sessionId: item.sessionId, images: item.images });
  }

  useEffect(() => {
    if (!sessionMenu && !projectMenu) return;
    const close = () => {
      setSessionMenu(null);
      setProjectMenu(null);
    };
    window.addEventListener("click", close);
    window.addEventListener("blur", close);
    window.addEventListener("resize", close);
    return () => {
      window.removeEventListener("click", close);
      window.removeEventListener("blur", close);
      window.removeEventListener("resize", close);
    };
  }, [sessionMenu, projectMenu]);

  function openSessionMenu(event: ReactMouseEvent, sessionId: string): void {
    event.preventDefault();
    event.stopPropagation();
    setProjectMenu(null);
    setSessionMenu({ sessionId, x: event.clientX, y: event.clientY });
  }

  async function renameSession(sessionId: string): Promise<void> {
    if (!isTauriRuntime) return;
    const session = sessionsRef.current.find((item) => item.id === sessionId);
    const requested = window.prompt(
      t(locale, "history.renamePrompt"),
      session?.title?.trim() ?? "",
    );
    setSessionMenu(null);
    if (requested === null) return;

    const title = requested.trim();
    if (!title) {
      setError(t(locale, "history.renameEmpty"));
      return;
    }

    setError(null);
    try {
      const renamed = await invoke<Session>("rename_session", { sessionId, title });
      setSessions((current) => current.map((item) => (item.id === renamed.id ? renamed : item)));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : t(locale, "history.renameFailed"));
    }
  }

  async function deleteSession(sessionId: string): Promise<void> {
    if (!isTauriRuntime) return;
    const confirmed = window.confirm(t(locale, "history.deleteConfirm"));
    setSessionMenu(null);
    if (!confirmed) return;
    setError(null);
    try {
      await invoke("delete_session", { sessionId });
      setSessions((current) => current.filter((session) => session.id !== sessionId));
      if (activeSessionId === sessionId) {
        resetComposerSession();
      }
      await refreshSessions();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : t(locale, "history.deleteFailed"));
    }
  }

  const persistComposerPrefs = useCallback(
    async (nextModel: string, nextReasoning: ReasoningEffort) => {
      if (!isTauriRuntime) return;
      const trimmedModel = nextModel.trim();
      try {
        const current = await invoke<UserConfig>("get_user_config");
        const saved = await invoke<UserConfig>("set_user_config", {
          config: {
            ...current,
            model: trimmedModel,
            reasoning_effort: nextReasoning,
          } satisfies UserConfig,
        });
        setModel((saved.model || "").trim());
        setReasoningEffort(normalizeReasoningEffort(saved.reasoning_effort));
        const root = workspaceRoot.trim();
        if (root && trimmedModel) {
          await invoke<WorkspaceConfig>("set_workspace_config", {
            params: {
              workspace_root: root,
              mode,
              provider: defaultProvider,
              model: trimmedModel,
            },
          });
        }
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause));
      }
    },
    [mode, workspaceRoot],
  );

  async function sendChatMessage(
    message: string,
    options?: {
      steer?: boolean;
      sessionId?: string | null;
      skipDrain?: boolean;
      images?: ChatImageAttachment[];
    },
  ): Promise<string | null> {
    const targetSessionIdEarly = options?.sessionId ?? activeSessionId;
    const existingForRoot =
      (targetSessionIdEarly
        ? sessions.find((session) => session.id === targetSessionIdEarly) ?? activeSession
        : activeSession);
    const continuingRoot = (existingForRoot?.workspace_root || "").trim();
    const wantsChat = composerIntent === "chat" || isChatWorkspace(continuingRoot);

    let root = continuingRoot || workspaceRoot.trim();
    let usedChatWorkspace = isChatWorkspace(root);
    if (!root || (wantsChat && !continuingRoot)) {
      // New chat (or chat without project): ensure dedicated chat workspace.
      if (wantsChat) {
        try {
          if (isTauriRuntime) {
            root = await invoke<string>("ensure_chat_workspace", {
              workspaceHome: workspaceHome.trim() || null,
            });
          } else {
            root = chatWorkspaceCandidate(workspaceHome);
          }
          usedChatWorkspace = true;
        } catch (cause) {
          setError(cause instanceof Error ? cause.message : String(cause));
          return null;
        }
      }
    }
    if (!root) {
      setError(t(locale, "error.needWorkspace"));
      return null;
    }
    const images = options?.images ?? [];
    if (!message.trim() && images.length === 0) {
      setError(t(locale, "error.needPrompt"));
      return null;
    }
    if (providerStatus && !providerStatus.ready) {
      setError(t(locale, "error.needProvider"));
      return null;
    }
    if (!model.trim()) {
      setError(t(locale, "error.needModel"));
      return null;
    }
    if (!isTauriRuntime) {
      setError(t(locale, "error.tauriOnly"));
      return null;
    }

    const targetSessionId = options?.sessionId ?? activeSessionId;
    const previousInFlight = targetSessionId
      ? (chatInFlightBySessionRef.current.get(targetSessionId) ?? null)
      : draftInFlightRef.current;
    chatGenerationMonoRef.current += 1;
    const generation = chatGenerationMonoRef.current;
    // ownerSessionId starts as the explicit target; draft turns adopt the real id when chat returns.
    const ownerSessionIdRef = { current: targetSessionId as string | null };
    if (targetSessionId) chatGenerationBySessionRef.current.set(targetSessionId, generation);
    else draftGenerationRef.current = generation;

    const isCurrentGeneration = () => {
      const ownerId = ownerSessionIdRef.current;
      if (ownerId) return chatGenerationBySessionRef.current.get(ownerId) === generation;
      return draftGenerationRef.current === generation;
    };
    const adoptSessionId = (sessionId: string) => {
      if (!sessionId) return;
      const previousOwner = ownerSessionIdRef.current;
      if (previousOwner === sessionId) return;
      if (!previousOwner) {
        const existing = chatGenerationBySessionRef.current.get(sessionId);
        // Only claim the session if nothing newer already owns it (e.g. a steer).
        if (existing == null || existing < generation) {
          chatGenerationBySessionRef.current.set(sessionId, generation);
        }
        const inflight = draftInFlightRef.current;
        if (inflight && chatInFlightBySessionRef.current.get(sessionId) !== inflight) {
          chatInFlightBySessionRef.current.set(sessionId, inflight);
        }
      }
      ownerSessionIdRef.current = sessionId;
    };
    const markSessionRunning = (sessionId: string, running: boolean) => {
      setRunningSessionIds((current) => {
        if (running) return current.includes(sessionId) ? current : [...current, sessionId];
        return current.filter((id) => id !== sessionId);
      });
      if (running) {
        setSessions((current) =>
          current.map((session) =>
            session.id === sessionId && session.status !== "running" && session.status !== "need_user"
              ? { ...session, status: "running" }
              : session,
          ),
        );
      }
    };
    const setSessionRunStatus = (sessionId: string | null, next: RunStatus | null) => {
      if (!sessionId) {
        setDraftRunStatus(next);
        return;
      }
      setRunStatusBySession((current) => {
        if (!next) {
          if (!(sessionId in current)) return current;
          const { [sessionId]: _removed, ...rest } = current;
          return rest;
        }
        return { ...current, [sessionId]: next };
      });
    };
    const touchesActive = (sessionId: string | null | undefined) => (
      !sessionId || activeSessionIdRef.current === sessionId
    );

    const inFlight = (async (): Promise<string | null> => {
          let sessionForContinue =
            (targetSessionId
              ? sessionsRef.current.find((session) => session.id === targetSessionId)
                ?? sessions.find((session) => session.id === targetSessionId)
                ?? activeSession
              : activeSession);

          if (
            options?.steer
            && targetSessionId
            && (
              runningSessionIds.includes(targetSessionId)
              || sessionForContinue?.status === "running"
              || sessionForContinue?.status === "need_user"
            )
          ) {
            try {
              const partialAssistant = streamedTextBySessionRef.current.get(targetSessionId) ?? "";
              commitStreamedAssistant(targetSessionId);
              await invoke<CancelSessionResult>("cancel_session", {
                params: {
                  session_id: targetSessionId,
                  partial_assistant: partialAssistant.trim() ? partialAssistant : undefined,
                },
              });
              // Wait for the cancelled in-flight worker to exit so it cannot re-cancel
              // or fail the steered follow-up turn.
              if (previousInFlight) {
                try { await previousInFlight; } catch { /* cancelled/failed prior turn */ }
              }
              await refreshSessions();
              sessionForContinue = sessionForContinue
                ? { ...sessionForContinue, status: "cancelled" }
                : null;
            } catch (cause) {
              if (touchesActive(targetSessionId)) {
                setError(cause instanceof Error ? cause.message : String(cause));
              }
              return null;
            }
          }

          // Explicit sessionId (queue drain / follow-up) must continue the same task.
          // Do not rely only on canContinueSession(sessionForContinue): drain runs inside
          // the previous turn's async closure where `sessions`/`activeSession` are stale,
          // so status may still look non-continuable and we'd omit session_id → new task.
          const continuing =
            canContinueSession(sessionForContinue)
            || (!!targetSessionId && !!options?.steer)
            || !!options?.sessionId;
          if (touchesActive(targetSessionId)) setError(null);

          if (!continuing) {
            // Starting a brand-new task: focus the draft composer and leave other sessions alone.
            setActiveSessionId(null);
            setMessages([]);
            setStreamedText("");
            setPlan([]);
            setActivity([]);
            setPendingAction(null);
            setApprovalSummary(null);
            setPatchPreview(null);
            setRestorePoints([]);
            setTaskSummary(null);
            setReplaySteps([]);
            setDraftRunning(true);
            setDraftRunStatus({ startedAt: Date.now(), phase: "thinking" });
            setRunStatusExpanded(false);
          } else {
            const sid = (sessionForContinue?.id || targetSessionId) as string;
            markSessionRunning(sid, true);
            setSessionRunStatus(sid, { startedAt: Date.now(), phase: "thinking" });
            if (touchesActive(sid)) {
              commitStreamedAssistant(sid);
              setPendingAction(null);
              setApprovalSummary(null);
              setPatchPreview(null);
              setTaskSummary(null);
              setRunStatusExpanded(false);
              const localContent = encodeLocalUserContent(message, images);
              setMessages((current) => [
                ...current,
                {
                  id: `local-user-${Date.now()}`,
                  session_id: sid,
                  role: "user",
                  content: localContent,
                  created_at: new Date().toISOString(),
                },
              ]);
            }
          }

          const params: ChatParams = {
            workspace_root: root,
            message: message.trim() ? message : (images.length > 0 ? " " : message),
            mode,
            provider: defaultProvider,
            model,
            session_id: continuing ? (sessionForContinue?.id || targetSessionId || undefined) : undefined,
            images: images.length > 0 ? images : undefined,
          };

          let finishedSessionId: string | null = continuing ? (sessionForContinue?.id || targetSessionId) : null;
          try {
            const result = await invoke<ChatResult>("chat", { params });
            finishedSessionId = result.session.id;
            adoptSessionId(result.session.id);
            // A steered/cancelled worker must not wipe the newer turn's transcript.
            if (!isCurrentGeneration()) {
              return finishedSessionId;
            }
            // Only jump focus to this session if the user is still on the draft that started it,
            // or already viewing this session. Never steal focus from another parallel task.
            if (!activeSessionIdRef.current || activeSessionIdRef.current === result.session.id) {
              setActiveSessionId(result.session.id);
              const completedMessage = result.message;
              if (completedMessage?.content.trim()) {
                setMessages((current) => mergeMessage(current, completedMessage));
              }
              await hydrateSession(result.session.id);
            }
            await refreshSessions();
            // The invoke result is authoritative even if a desktop event was lost. Do
            // not leave the composer in a perpetual “thinking” state after a completed
            // or cancelled turn.
            if (result.session.status !== "running") {
              setSessionRunStatus(result.session.id, null);
              if (touchesActive(result.session.id)) setRunStatusExpanded(false);
            }
            markSessionRunning(result.session.id, result.session.status === "running");
            setDraftRunning(false);
            setDraftRunStatus(null);
            try {
              if (!usedChatWorkspace && !isChatWorkspace(root)) {
                const current = await invoke<UserConfig>("get_user_config");
                const previous = (current.last_workspace_root || "").trim();
                if (previous !== root) {
                  await invoke<UserConfig>("set_user_config", {
                    config: { ...current, last_workspace_root: root } satisfies UserConfig,
                  });
                }
              }
            } catch {
              // Non-fatal: chat already succeeded.
            }
          } catch (cause) {
            if (!isCurrentGeneration()) {
              return finishedSessionId;
            }
            const failMessage = cause instanceof Error ? cause.message : String(cause);
            if (touchesActive(finishedSessionId || targetSessionId)) {
              setError(failMessage);
              setRunStatusExpanded(false);
            }
            const failSid = finishedSessionId || targetSessionId;
            if (failSid) {
              setSessionRunStatus(failSid, {
                startedAt: Date.now(),
                phase: "failed",
                detail: failMessage,
              });
              markSessionRunning(failSid, false);
            } else {
              setDraftRunStatus({
                startedAt: Date.now(),
                phase: "failed",
                detail: failMessage,
              });
              setDraftRunning(false);
            }
            // Ensure sidebar/history leave Running and reflect Failed after provider errors.
            try {
              await refreshSessions();
            } catch {
              // Non-fatal: error banner already set.
            }
          } finally {
            if (isCurrentGeneration()) {
              if (finishedSessionId) {
                // Chat invoke is blocking: when it returns this turn is no longer running
                // (done / failed / cancelled / need_user). Clear only if we still own the session.
                markSessionRunning(finishedSessionId, false);
                // Keep failed runStatus until user starts another turn / switches away.
              } else if (!targetSessionId) {
                setDraftRunning(false);
              }
              if (!options?.skipDrain) {
                await drainFollowUpQueue(finishedSessionId);
              }
            }
          }
          return finishedSessionId;
    })();
    if (targetSessionId) chatInFlightBySessionRef.current.set(targetSessionId, inFlight);
    else draftInFlightRef.current = inFlight;
    try {
      return await inFlight;
    } finally {
      if (targetSessionId) {
        if (chatInFlightBySessionRef.current.get(targetSessionId) === inFlight) {
          chatInFlightBySessionRef.current.delete(targetSessionId);
        }
      } else if (draftInFlightRef.current === inFlight) {
        draftInFlightRef.current = null;
      }
    }
  }

  async function drainFollowUpQueue(sessionId: string | null): Promise<void> {
    if (!sessionId || drainFollowUpsBySessionRef.current.has(sessionId)) return;

    let status: Session["status"] | undefined;
    try {
      const latest = await invoke<Session[]>("list_sessions", {
        workspaceRoot: null,
      });
      setSessions(latest);
      status = latest.find((session) => session.id === sessionId)?.status;
    } catch {
      status = sessionsRef.current.find((session) => session.id === sessionId)?.status;
    }

    if (status === "need_user" || status === "running") return;

    const next = followUpQueueRef.current.find((item) => item.sessionId === sessionId);
    if (!next) return;

    drainFollowUpsBySessionRef.current.add(sessionId);
    try {
      setFollowUpQueue((current) => {
        const remaining = current.filter((item) => item.id !== next.id);
        followUpQueueRef.current = remaining;
        return remaining;
      });
      // Keep the guard until this send finishes, then chain the next item explicitly.
      await sendChatMessage(next.text, { sessionId, skipDrain: true, images: next.images });
    } finally {
      drainFollowUpsBySessionRef.current.delete(sessionId);
    }
    await drainFollowUpQueue(sessionId);
  }

  async function addComposerFiles(fileList: FileList | File[] | null | undefined): Promise<void> {
    if (!fileList) return;
    const files = Array.from(fileList).filter((file) => file.type.startsWith("image/"));
    if (files.length === 0) return;
    const next: ComposerImage[] = [];
    for (const file of files) {
      if (composerImages.length + next.length >= MAX_COMPOSER_IMAGES) {
        setError(t(locale, "composer.imageLimit"));
        break;
      }
      try {
        next.push(await fileToComposerImage(file));
      } catch (cause) {
        const code = cause instanceof Error ? cause.message : "read";
        if (code === "size") setError(t(locale, "composer.imageTooLarge"));
        else if (code === "type") setError(t(locale, "composer.imageType"));
        else setError(t(locale, "composer.imageType"));
      }
    }
    if (next.length > 0) {
      setComposerImages((current) => [...current, ...next].slice(0, MAX_COMPOSER_IMAGES));
      setError(null);
    }
  }

  function removeComposerImage(id: string): void {
    setComposerImages((current) => current.filter((image) => image.id !== id));
  }

  async function onComposerPaste(event: ClipboardEvent<HTMLTextAreaElement>): Promise<void> {
    const items = event.clipboardData?.items;
    if (!items) return;
    const files: File[] = [];
    for (const item of Array.from(items)) {
      if (item.kind === "file" && item.type.startsWith("image/")) {
        const file = item.getAsFile();
        if (file) files.push(file);
      }
    }
    if (files.length === 0) return;
    event.preventDefault();
    await addComposerFiles(files);
  }

  async function onComposerImagePick(event: ChangeEvent<HTMLInputElement>): Promise<void> {
    await addComposerFiles(event.target.files);
    event.target.value = "";
  }

  async function submit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    const message = prompt.trim();
    const images = composerImages.map(({ mime_type, data_base64, name }) => ({
      mime_type,
      data_base64,
      name,
    }));
    if (!message && images.length === 0) {
      setError(t(locale, "error.needPrompt"));
      return;
    }

    // While a turn is in flight or waiting for approval, default to queue; optional steer.
    if (isRunning || activeSession?.status === "running" || activeSession?.status === "need_user") {
      if (!activeSessionId) {
        setError(t(locale, "composer.needActiveSession"));
        return;
      }
      if (composerSendMode === "steer") {
        setPrompt("");
        setComposerImages([]);
        await sendChatMessage(message, { steer: true, sessionId: activeSessionId, images });
        setComposerSendMode("queue");
        return;
      }
      enqueueFollowUp(activeSessionId, message, images);
      setPrompt("");
      setComposerImages([]);
      setError(null);
      return;
    }

    setPrompt("");
    setComposerImages([]);
    await sendChatMessage(message, { images });
  }

  async function steerCurrentRun(): Promise<void> {
    const message = prompt.trim();
    const images = composerImages.map(({ mime_type, data_base64, name }) => ({
      mime_type,
      data_base64,
      name,
    }));
    if (!message && images.length === 0) {
      setError(t(locale, "error.needPrompt"));
      return;
    }
    if (!activeSessionId) {
      setError(t(locale, "composer.needActiveSession"));
      return;
    }
    setPrompt("");
    setComposerImages([]);
    await sendChatMessage(message, { steer: true, sessionId: activeSessionId, images });
  }

  async function resolveAction(approved: boolean): Promise<void> {
    if (!pendingAction || !activeSessionId) return;
    const sessionId = activeSessionId;
    const actionId = pendingAction.id;
    setError(null);
    // Hide the approval card immediately after the user decides; hydrate will
    // restore it only if the backend still reports a pending action.
    setPendingAction(null);
    setApprovalSummary(null);
    setPatchPreview(null);
    setRunningSessionIds((current) => current.includes(sessionId) ? current : [...current, sessionId]);
    setRunStatusBySession((current) => ({
      ...current,
      [sessionId]: { startedAt: Date.now(), phase: "thinking" },
    }));
    try {
      if (approved && rememberLocalApiApproval) {
        if (!isTauriRuntime) throw new Error(t(locale, "error.tauriOnly"));
        const current = await invoke<UserConfig>("get_user_config");
        await invoke<UserConfig>("set_user_config", {
          config: { ...current, skip_local_api_confirmation: true } satisfies UserConfig,
        });
      }
      const result = await invoke<ResolveActionResult>("resolve_action", {
        params: { session_id: sessionId, action_id: actionId, approved },
      });
      const completedMessage = result.message;
      if (completedMessage && activeSessionIdRef.current === sessionId) {
        setMessages((current) => mergeMessage(current, completedMessage));
      }
      await refreshSessions();
      if (activeSessionIdRef.current === sessionId) await hydrateSession(sessionId);
    } catch (cause) {
      if (activeSessionIdRef.current === sessionId) {
        setError(cause instanceof Error ? cause.message : String(cause));
        // Restore the real pending state if the resolve call failed.
        await hydrateSession(sessionId);
      }
    } finally {
      setRunningSessionIds((current) => current.filter((id) => id !== sessionId));
      await drainFollowUpQueue(sessionId);
    }
  }

  async function loadReplay(): Promise<void> {
    if (!activeSessionId || !isTauriRuntime) return;
    setError(null);
    try {
      const replay = await invoke<ReplaySessionResult>("session_replay", { sessionId: activeSessionId });
      setReplaySteps(replay.steps);
    } catch (errorValue) {
      setError(errorValue instanceof Error ? errorValue.message : String(errorValue));
    }
  }

  async function rollbackRestorePoint(restorePoint: RestorePoint): Promise<void> {
    if (!activeSessionId || isRunning) return;
    const sessionId = activeSessionId;
    setError(null);
    setRunningSessionIds((current) => current.includes(sessionId) ? current : [...current, sessionId]);
    try {
      await invoke<RollbackRestorePointResult>("rollback_restore_point", {
        params: { session_id: sessionId, restore_point_id: restorePoint.id },
      });
      await refreshSessions();
      if (activeSessionIdRef.current === sessionId) await hydrateSession(sessionId);
    } catch (cause) {
      if (activeSessionIdRef.current === sessionId) {
        setError(cause instanceof Error ? cause.message : String(cause));
      }
    } finally {
      setRunningSessionIds((current) => current.filter((id) => id !== sessionId));
    }
  }

  async function cancelSession(): Promise<void> {
    if (!activeSessionId) return;
    const sessionId = activeSessionId;
    setError(null);
    setRunningSessionIds((current) => current.includes(sessionId) ? current : [...current, sessionId]);
    const previousInFlight = chatInFlightBySessionRef.current.get(sessionId) ?? null;
    try {
      const partialAssistant = streamedTextBySessionRef.current.get(sessionId) ?? streamedText;
      commitStreamedAssistant(sessionId);
      await invoke<CancelSessionResult>("cancel_session", {
        params: {
          session_id: sessionId,
          partial_assistant: partialAssistant.trim() ? partialAssistant : undefined,
        },
      });
      if (previousInFlight) {
        try { await previousInFlight; } catch { /* cancelled/failed prior turn */ }
      }
      await refreshSessions();
      if (activeSessionIdRef.current === sessionId) await hydrateSession(sessionId);
    } catch (cause) {
      if (activeSessionIdRef.current === sessionId) {
        setError(cause instanceof Error ? cause.message : String(cause));
      }
    } finally {
      setRunningSessionIds((current) => current.filter((id) => id !== sessionId));
      setRunStatusBySession((current) => {
        if (!(sessionId in current)) return current;
        const { [sessionId]: _removed, ...rest } = current;
        return rest;
      });
      await drainFollowUpQueue(sessionId);
    }
  }

  function unhideProjectPath(projectPath: string, currentHidden: string[] = hiddenProjectPaths): string[] {
    const key = normalizeRoot(projectPath);
    return currentHidden.filter((item) => normalizeRoot(item) !== key);
  }

  async function persistHiddenProjectPaths(nextHidden: string[], extra: Partial<UserConfig> = {}): Promise<void> {
    setHiddenProjectPaths(nextHidden);
    if (!isTauriRuntime) return;
    try {
      const current = await invoke<UserConfig>("get_user_config");
      await invoke<UserConfig>("set_user_config", {
        config: {
          ...current,
          ...extra,
          hidden_project_paths: nextHidden,
        } satisfies UserConfig,
      });
    } catch {
      // Ignore preference persistence failures.
    }
  }

  async function removeProjectFromArea(root: string): Promise<void> {
    const target = root.trim();
    if (!target) return;
    setProjectMenu(null);
    if (!window.confirm(t(locale, "project.removeConfirm"))) return;
    const key = normalizeRoot(target);
    const nextHidden = Array.from(
      new Set([...hiddenProjectPaths.map((item) => item.trim()).filter(Boolean), target]),
    );
    const clearingCurrent = normalizeRoot(workspaceRoot) === key;
    const removedSessionIds = new Set(
      sessionsRef.current
        .filter((session) => normalizeRoot(session.workspace_root) === key)
        .map((session) => session.id),
    );

    setError(null);
    try {
      if (isTauriRuntime) {
        await invoke<number>("delete_workspace_sessions", { workspaceRoot: target });
      }
      setSessions((current) =>
        current.filter((session) => normalizeRoot(session.workspace_root) !== key),
      );
      if (activeSessionId && removedSessionIds.has(activeSessionId)) {
        resetComposerSession();
      }
      if (clearingCurrent) {
        setWorkspaceRoot("");
      }
      await persistHiddenProjectPaths(
        nextHidden,
        clearingCurrent ? { last_workspace_root: undefined } : {},
      );
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : t(locale, "history.deleteFailed"));
    }
  }

  function openProjectMenu(event: ReactMouseEvent, root: string): void {
    event.preventDefault();
    event.stopPropagation();
    setSessionMenu(null);
    setProjectMenu({ root, x: event.clientX, y: event.clientY });
  }

  async function submitCreateProject(): Promise<void> {
    if (creatingProject) return;
    const home = workspaceHome.trim();
    const name = createProjectName.trim();
    if (!home) {
      setError(t(locale, "error.needWorkspaceHome"));
      setCreateProjectOpen(false);
      openSettings();
      return;
    }
    if (!name) {
      setError(t(locale, "error.needProjectName"));
      return;
    }
    if (!isTauriRuntime) {
      setError(t(locale, "error.tauriOnly"));
      return;
    }
    setCreatingProject(true);
    setError(null);
    try {
      const result = await invoke<CreateProjectResult>("create_project", {
        params: { workspace_home: home, name },
      });
      const nextHidden = unhideProjectPath(result.project.path);
      setWorkspaceRoot(result.project.path);
      setCreateProjectOpen(false);
      setCreateProjectName("");
      startNewTask();
      await persistHiddenProjectPaths(nextHidden, {
        workspace_home: home,
        last_workspace_root: result.project.path,
      });
      await refreshDiskProjects();
    } catch (error) {
      setError(error instanceof Error ? error.message : String(error));
    } finally {
      setCreatingProject(false);
    }
  }

  async function chooseExistingProjectFolder(): Promise<void> {
    if (creatingProject) return;
    const home = workspaceHome.trim();
    if (!home) {
      setError(t(locale, "error.needWorkspaceHome"));
      setCreateProjectOpen(false);
      openSettings();
      return;
    }
    if (!isTauriRuntime) {
      setError(t(locale, "error.tauriOnly"));
      return;
    }
    setError(null);
    let selected: string | null = null;
    try {
      selected = await invoke<string | null>("pick_directory", {
        title: t(locale, "action.chooseProjectFolder"),
      });
    } catch (error) {
      setError(error instanceof Error ? error.message : String(error));
      return;
    }
    if (!selected?.trim()) return;

    const selectedKey = normalizeRoot(selected);
    const visible = diskProjects.some(
      (project) =>
        normalizeRoot(project.path) === selectedKey &&
        !hiddenProjectSet.has(normalizeRoot(project.path)),
    );
    if (visible) {
      setWorkspaceRoot(selected.trim());
      setCreateProjectOpen(false);
      setCreateProjectName("");
      startNewTask();
            if (isTauriRuntime) {
        try {
          const current = await invoke<UserConfig>("get_user_config");
          await invoke<UserConfig>("set_user_config", {
            config: {
              ...current,
              workspace_home: home,
              last_workspace_root: selected.trim(),
            } satisfies UserConfig,
          });
        } catch {
          // Ignore preference persistence failures.
        }
      }
      return;
    }

    setCreatingProject(true);
    try {
      const result = await invoke<ImportProjectResult>("import_project", {
        params: {
          workspace_home: home,
          source_path: selected.trim(),
        },
      });
      const nextHidden = unhideProjectPath(result.project.path);
      setWorkspaceRoot(result.project.path);
      setCreateProjectOpen(false);
      setCreateProjectName("");
      startNewTask();
      await persistHiddenProjectPaths(nextHidden, {
        workspace_home: home,
        last_workspace_root: result.project.path,
      });
      await refreshDiskProjects();
      if (result.copied) {
        setError(t(locale, "project.importedCopy"));
      } else if (result.already_existed) {
        setError(t(locale, "project.importedExisting"));
      }
    } catch (error) {
      setError(error instanceof Error ? error.message : String(error));
    } finally {
      setCreatingProject(false);
    }
  }

  async function saveAllSettings(): Promise<void> {
    if (isSavingConfig || anySessionRunning) return;
    if (!isTauriRuntime) {
      setError(t(locale, "error.tauriOnly"));
      return;
    }
    if (!model.trim()) {
      setError(t(locale, "error.needModel"));
      return;
    }
    setError(null);
    setIsSavingConfig(true);
    try {
      const root = workspaceRoot.trim();
      const home = workspaceHome.trim();
      const normalizedProviders = providers.map((item) => ({
        ...item,
        name: item.name.trim() || "Provider",
        base_url: normalizeProviderBaseUrl(item.base_url) || DEFAULT_PROVIDER_BASE_URL,
        api_key: item.api_key?.trim() || undefined,
      }));
      const selectedProvider = normalizedProviders.find((item) => item.id === activeProviderId) ?? normalizedProviders[0];
      if (!selectedProvider) {
        setError(t(locale, "providers.empty"));
        return;
      }
      const savedUser = await invoke<UserConfig>("set_user_config", {
        config: {
          locale,
          mode,
          provider: defaultProvider,
          model: model.trim(),
          reasoning_effort: reasoningEffort,
          max_provider_retries: normalizeBoundedInteger(maxProviderRetries, DEFAULT_MAX_PROVIDER_RETRIES, MIN_MAX_PROVIDER_RETRIES, MAX_MAX_PROVIDER_RETRIES),
          max_tool_rounds: normalizeBoundedInteger(maxToolRounds, DEFAULT_MAX_TOOL_ROUNDS, MIN_MAX_TOOL_ROUNDS, MAX_MAX_TOOL_ROUNDS),
          circuit_failure_threshold: normalizeBoundedInteger(circuitFailureThreshold, DEFAULT_CIRCUIT_FAILURE_THRESHOLD, MIN_CIRCUIT_FAILURE_THRESHOLD, MAX_CIRCUIT_FAILURE_THRESHOLD),
          stream_first_event_timeout_secs: normalizeBoundedInteger(streamFirstEventTimeoutSecs, DEFAULT_STREAM_FIRST_EVENT_TIMEOUT_SECS, MIN_STREAM_FIRST_EVENT_TIMEOUT_SECS, MAX_STREAM_FIRST_EVENT_TIMEOUT_SECS),
          stream_idle_timeout_secs: normalizeBoundedInteger(streamIdleTimeoutSecs, DEFAULT_STREAM_IDLE_TIMEOUT_SECS, MIN_STREAM_IDLE_TIMEOUT_SECS, MAX_STREAM_IDLE_TIMEOUT_SECS),
          non_stream_timeout_secs: normalizeBoundedInteger(nonStreamTimeoutSecs, DEFAULT_NON_STREAM_TIMEOUT_SECS, MIN_NON_STREAM_TIMEOUT_SECS, MAX_NON_STREAM_TIMEOUT_SECS),
          circuit_recovery_success_threshold: normalizeBoundedInteger(circuitRecoverySuccessThreshold, DEFAULT_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD, MIN_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD, MAX_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD),
          circuit_recovery_wait_secs: normalizeBoundedInteger(circuitRecoveryWaitSecs, DEFAULT_CIRCUIT_RECOVERY_WAIT_SECS, MIN_CIRCUIT_RECOVERY_WAIT_SECS, MAX_CIRCUIT_RECOVERY_WAIT_SECS),
          circuit_error_rate_threshold_percent: normalizeBoundedInteger(circuitErrorRateThresholdPercent, DEFAULT_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT, MIN_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT, MAX_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT),
          circuit_min_request_count: normalizeBoundedInteger(circuitMinRequestCount, DEFAULT_CIRCUIT_MIN_REQUEST_COUNT, MIN_CIRCUIT_MIN_REQUEST_COUNT, MAX_CIRCUIT_MIN_REQUEST_COUNT),
          // Compatibility mirror for the current active provider.
          base_url: selectedProvider.base_url,
          api_key: selectedProvider.api_key,
          providers: normalizedProviders,
          active_provider_id: selectedProvider.id,
          last_workspace_root: root || undefined,
          workspace_home: home || undefined,
          hidden_project_paths: hiddenProjectPaths,
        } satisfies UserConfig,
      });
      setWorkspaceHome((savedUser.workspace_home || "").trim());
      setMode(savedUser.mode);
      setModel((savedUser.model || "").trim());
      setReasoningEffort(normalizeReasoningEffort(savedUser.reasoning_effort));
      setMaxProviderRetries(normalizeBoundedInteger(savedUser.max_provider_retries, DEFAULT_MAX_PROVIDER_RETRIES, MIN_MAX_PROVIDER_RETRIES, MAX_MAX_PROVIDER_RETRIES));
      setMaxToolRounds(normalizeBoundedInteger(savedUser.max_tool_rounds, DEFAULT_MAX_TOOL_ROUNDS, MIN_MAX_TOOL_ROUNDS, MAX_MAX_TOOL_ROUNDS));
      setCircuitFailureThreshold(normalizeBoundedInteger(savedUser.circuit_failure_threshold, DEFAULT_CIRCUIT_FAILURE_THRESHOLD, MIN_CIRCUIT_FAILURE_THRESHOLD, MAX_CIRCUIT_FAILURE_THRESHOLD));
      setStreamFirstEventTimeoutSecs(normalizeBoundedInteger(savedUser.stream_first_event_timeout_secs, DEFAULT_STREAM_FIRST_EVENT_TIMEOUT_SECS, MIN_STREAM_FIRST_EVENT_TIMEOUT_SECS, MAX_STREAM_FIRST_EVENT_TIMEOUT_SECS));
      setStreamIdleTimeoutSecs(normalizeBoundedInteger(savedUser.stream_idle_timeout_secs, DEFAULT_STREAM_IDLE_TIMEOUT_SECS, MIN_STREAM_IDLE_TIMEOUT_SECS, MAX_STREAM_IDLE_TIMEOUT_SECS));
      setNonStreamTimeoutSecs(normalizeBoundedInteger(savedUser.non_stream_timeout_secs, DEFAULT_NON_STREAM_TIMEOUT_SECS, MIN_NON_STREAM_TIMEOUT_SECS, MAX_NON_STREAM_TIMEOUT_SECS));
      setCircuitRecoverySuccessThreshold(normalizeBoundedInteger(savedUser.circuit_recovery_success_threshold, DEFAULT_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD, MIN_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD, MAX_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD));
      setCircuitRecoveryWaitSecs(normalizeBoundedInteger(savedUser.circuit_recovery_wait_secs, DEFAULT_CIRCUIT_RECOVERY_WAIT_SECS, MIN_CIRCUIT_RECOVERY_WAIT_SECS, MAX_CIRCUIT_RECOVERY_WAIT_SECS));
      setCircuitErrorRateThresholdPercent(normalizeBoundedInteger(savedUser.circuit_error_rate_threshold_percent, DEFAULT_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT, MIN_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT, MAX_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT));
      setCircuitMinRequestCount(normalizeBoundedInteger(savedUser.circuit_min_request_count, DEFAULT_CIRCUIT_MIN_REQUEST_COUNT, MIN_CIRCUIT_MIN_REQUEST_COUNT, MAX_CIRCUIT_MIN_REQUEST_COUNT));
      const hydratedProviders = hydrateProviders(savedUser);
      setProviders(hydratedProviders.providers);
      setActiveProviderId(hydratedProviders.activeProviderId);
        setSelectedProviderId(hydratedProviders.activeProviderId);
      if (isLocale(savedUser.locale)) setLocale(savedUser.locale);
      if (root) {
        const config = await invoke<WorkspaceConfig>("set_workspace_config", {
          params: {
            workspace_root: root,
            mode: savedUser.mode,
            provider: defaultProvider,
            model: savedUser.model,
            command_allowlist: parseCommandAllowlistText(commandAllowlistText),
            command_denylist: parseCommandDenylistText(commandDenylistText),
          },
        });
        setMode(config.mode);
        // Keep composer/user model preference; workspace config is synced for allowlists only.
        setCommandAllowlistText(formatCommandAllowlistText(config.command_allowlist));
        setCommandDenylistText(formatCommandDenylistText(config.command_denylist));
      }
      const savedActiveProvider = hydratedProviders.providers.find((item) => item.id === hydratedProviders.activeProviderId)
        ?? hydratedProviders.providers[0];
      await refreshModels({ provider: savedActiveProvider, updateComposerModels: true });
      await refreshSessions();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setIsSavingConfig(false);
    }
  }


  const doctorChecks = buildDesktopDoctorChecks({
    workspaceRoot,
    providerStatus,
    mode,
    model,
    provider: defaultProvider,
    locale,
  });
  const doctorReady = desktopDoctorReady(doctorChecks);
  const activeIsChat = isChatWorkspace(activeSession?.workspace_root) || composerIntent === "chat";
  const workspaceMissing = activeIsChat ? false : !workspaceRoot.trim();
  const modelMissing = !model.trim();
  const modelNotInList =
    !!model.trim() &&
    availableModels.length > 0 &&
    !availableModels.some((entry) => entry.id === model.trim());
  const queueMode = !!(isRunning || activeSession?.status === "running" || activeSession?.status === "need_user");
  const hasComposerContent = !!(prompt.trim() || composerImages.length > 0);
  const sendBlockReason = workspaceMissing
      ? "workspace"
      : (!prompt.trim() && composerImages.length === 0)
        ? "prompt"
        : providerStatus && !providerStatus.ready
          ? "provider"
          : modelMissing
            ? "model"
            : queueMode && !activeSessionId
              ? "session"
            : null;
  const sendHint =
    sendBlockReason === "workspace"
      ? t(locale, "composer.needWorkspace")
      : sendBlockReason === "prompt"
        ? t(locale, "composer.needPrompt")
        : sendBlockReason === "provider"
          ? t(locale, "composer.needProvider")
          : sendBlockReason === "model"
            ? t(locale, "composer.needModel")
            : sendBlockReason === "session"
              ? t(locale, "composer.needActiveSession")
            : queueMode
              ? t(locale, "composer.queueHint")
            : null;
  const sendTitle = sendHint
    || (queueMode
      ? (composerSendMode === "steer" ? t(locale, "action.steer") : t(locale, "action.queue"))
      : canContinueSession(activeSession)
        ? t(locale, "action.continue")
        : t(locale, "action.send"));
  const activeFollowUps = visibleFollowUps(activeSessionId);

  useEffect(() => {
    if (!queueMode) setComposerSendMode("queue");
  }, [queueMode]);

  function onComposerKeyDown(event: KeyboardEvent<HTMLTextAreaElement>): void {
    if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
      event.preventDefault();
      if (queueMode) {
        void steerCurrentRun();
        return;
      }
      event.currentTarget.form?.requestSubmit();
    }
  }

  if (view === "model-logs") {
    return (
      <main className="settings-page model-logs-page">
        <header className="settings-header">
          <div>
            <p className="eyebrow">{t(locale, "brand.eyebrow")}</p>
            <h1>{t(locale, "logs.title")}</h1>
            <p className="mode-help">{t(locale, "logs.subtitle")}</p>
          </div>
          <div className="settings-header-actions">
            <button
              type="button"
              className="quiet-button"
              onClick={() => void loadModelCallLogs(activeSessionId)}
              disabled={!activeSessionId || modelCallLogsLoading}
            >
              {modelCallLogsLoading ? t(locale, "logs.loading") : t(locale, "logs.refresh")}
            </button>
            <button type="button" className="quiet-button" onClick={returnToConversationFromModelLogs}>
              {t(locale, "logs.back")}
            </button>
          </div>
        </header>

        {modelCallLogsError ? (
          <p className="error-message settings-error">
            {t(locale, "logs.loadFailed", { error: modelCallLogsError })}
          </p>
        ) : null}

        <section className="model-logs-list" aria-label={t(locale, "logs.title")}>
          {!activeSessionId ? (
            <div className="empty-state model-logs-empty"><p>{t(locale, "logs.noSession")}</p></div>
          ) : modelCallLogsLoading ? (
            <div className="empty-state model-logs-empty"><p>{t(locale, "logs.loading")}</p></div>
          ) : modelCallLogs.length === 0 ? (
            <div className="empty-state model-logs-empty"><p>{t(locale, "logs.empty")}</p></div>
          ) : (
            modelCallLogs.slice().reverse().map((item) => {
              const event = item.event;
              const isCompaction = event.purpose === "context_compaction";
              return (
                <article className={`model-log-card ${event.success ? "success" : "failure"}`} key={item.id}>
                  <div className="model-log-card-header">
                    <div>
                      <strong>{event.success ? t(locale, "logs.success") : t(locale, "logs.failure")}</strong>
                      <span>{isCompaction ? t(locale, "logs.purpose.contextCompaction") : t(locale, "logs.purpose.chat")}</span>
                    </div>
                    <time dateTime={item.created_at}>{formatModelCallTime(item.created_at, locale)}</time>
                  </div>
                  <dl className="model-log-meta">
                    <div><dt>{t(locale, "logs.provider")}</dt><dd>{event.provider}</dd></div>
                    <div><dt>{t(locale, "logs.model")}</dt><dd>{event.model}</dd></div>
                    <div><dt>{t(locale, "logs.endpoint")}</dt><dd>{event.endpoint}</dd></div>
                    <div><dt>{t(locale, "logs.attempt")}</dt><dd>{event.attempt} / {event.max_attempts}</dd></div>
                    {!isCompaction ? <div><dt>{t(locale, "logs.round")}</dt><dd>{event.round}</dd></div> : null}
                    {event.success ? (
                      <div><dt>{t(locale, "logs.output")}</dt><dd>{t(locale, "logs.outputSummary", { chars: event.output_chars, tools: event.tool_calls })}</dd></div>
                    ) : null}
                  </dl>
                  {!event.success && event.error ? (
                    <div className="model-log-error-wrap">
                      <strong>{t(locale, "logs.error")}</strong>
                      <pre className="model-log-error">{event.error}</pre>
                    </div>
                  ) : null}
                </article>
              );
            })
          )}
        </section>
      </main>
    );
  }

  if (view === "settings") {
    return (
      <main className="settings-page">
        <header className="settings-header">
          <div>
            <p className="eyebrow">{t(locale, "brand.eyebrow")}</p>
            <h1>{t(locale, "settings.title")}</h1>
            <p className="mode-help">{t(locale, "settings.subtitle")}</p>
          </div>
          <div className="settings-header-actions">
            <div className="settings-diagnostics-menu">
              <button
                type="button"
                className={`quiet-button settings-diagnostics-trigger ${doctorReady ? "ready" : "blocked"}`}
                aria-controls="settings-diagnostics-popover"
                aria-expanded={diagnosticsOpen}
                onClick={() => setDiagnosticsOpen((current) => !current)}
              >
                {t(locale, "doctor.title")}
              </button>
              {diagnosticsOpen ? (
                <section
                  id="settings-diagnostics-popover"
                  className="settings-diagnostics-popover"
                  aria-label={t(locale, "aria.diagnostics")}
                >
                  <p className="panel-title">{t(locale, "doctor.title")}</p>
                  <div className={`doctor-panel ${doctorReady ? "ready" : "blocked"}`}>
                    <ul className="doctor-list">
                      {doctorChecks.map((check) => (
                        <li key={check.name} className={check.ok ? "ok" : "bad"}>
                          <strong>{check.name}</strong>
                          <small>{check.detail}</small>
                        </li>
                      ))}
                    </ul>
                  </div>
                </section>
              ) : null}
            </div>
            <label className="settings-locale-control" htmlFor="ui-locale">
              <span>{t(locale, "settings.section.language")}</span>
              <select
                id="ui-locale"
                aria-label={t(locale, "lang.label")}
                value={locale}
                onChange={(event) => setLocale(event.target.value as Locale)}
                disabled={anySessionRunning || isSavingConfig}
              >
                <option value="zh-CN">{t(locale, "lang.zhCN")}</option>
                <option value="en">{t(locale, "lang.en")}</option>
              </select>
            </label>
            <label className="settings-locale-control settings-font-size-control" htmlFor="ui-font-size">
              <span>{t(locale, "settings.uiFontSize")}</span>
              <select
                id="ui-font-size"
                aria-label={t(locale, "settings.uiFontSize")}
                value={uiFontSize}
                onChange={(event) => setUiFontSize(Number(event.target.value))}
              >
                {Array.from({ length: 7 }, (_, index) => 14 + index).map((size) => (
                  <option key={size} value={size}>{size}px</option>
                ))}
              </select>
            </label>
            <button type="button" className="quiet-button" onClick={returnToWorkbench}>
              {t(locale, "action.back")}
            </button>
            <button
              type="button"
              className="primary-button"
              onClick={() => void saveAllSettings()}
              disabled={anySessionRunning || isSavingConfig}
            >
              {isSavingConfig ? t(locale, "action.saving") : t(locale, "action.saveSettings")}
            </button>
          </div>
        </header>

        {error ? <p className="error-message settings-error">{error}</p> : null}

        <div className="settings-grid">
          <section className="settings-card provider-settings-card" aria-label={t(locale, "settings.section.provider")}>
            <div className="provider-manager-header">
              <p className="panel-title">{t(locale, "settings.section.provider")}</p>
              <button type="button" className="quiet-button" onClick={addProvider} disabled={anySessionRunning || isSavingConfig}>
                {t(locale, "action.addProvider")}
              </button>
            </div>
            <p className="mode-help">{t(locale, "providers.onlyOneActive")}</p>
            <div className="provider-manager-body">
              <div id="provider-list" className="provider-list" role="listbox" aria-label={t(locale, "settings.section.provider")}>
                {providers.map((item) => (
                  <button
                    key={item.id}
                    type="button"
                    className={`provider-list-item ${item.id === selectedProvider?.id ? "selected" : ""}`}
                    aria-pressed={item.id === selectedProvider?.id}
                    onClick={() => setSelectedProviderId(item.id)}
                    disabled={anySessionRunning || isSavingConfig}
                    title={item.name.trim() || "Provider"}
                  >
                    {item.name.trim() || "Provider"}
                  </button>
                ))}
              </div>
              {selectedProvider ? (
                <div className={`provider-editor ${selectedProvider.id === activeProviderId ? "active" : ""}`}>
                  <div className="provider-entry-header">
                    <label className="provider-enable">
                      <input
                        type="radio"
                        name="active-provider"
                        checked={selectedProvider.id === activeProviderId}
                        onChange={() => activateProvider(selectedProvider.id)}
                        disabled={anySessionRunning || isSavingConfig}
                      />
                      <span>{t(locale, "action.activateProvider")}</span>
                    </label>
                    <button
                      type="button"
                      className="danger-button"
                      onClick={() => deleteProvider(selectedProvider.id)}
                      disabled={anySessionRunning || isSavingConfig || providers.length <= 1}
                      title={providers.length <= 1 ? t(locale, "providers.cannotDeleteLast") : undefined}
                    >
                      {t(locale, "action.deleteProvider")}
                    </button>
                  </div>
                  <label className="field-label" htmlFor={`provider-name-${selectedProvider.id}`}>{t(locale, "field.providerName")}</label>
                  <input
                    id={`provider-name-${selectedProvider.id}`}
                    value={selectedProvider.name}
                    onChange={(event) => updateProvider(selectedProvider.id, { name: event.target.value })}
                    disabled={anySessionRunning || isSavingConfig}
                    spellCheck={false}
                  />
                  <label className="field-label" htmlFor={`provider-base-url-${selectedProvider.id}`}>{t(locale, "field.baseUrl")}</label>
                  <input
                    id={`provider-base-url-${selectedProvider.id}`}
                    value={selectedProvider.base_url}
                    onChange={(event) => updateProvider(selectedProvider.id, { base_url: event.target.value })}
                    disabled={anySessionRunning || isSavingConfig}
                    spellCheck={false}
                    placeholder={t(locale, "field.baseUrlPlaceholder")}
                  />
                  <label className="field-label" htmlFor={`provider-api-key-${selectedProvider.id}`}>{t(locale, "field.apiKey")}</label>
                  <div className="secret-field">
                    <input
                      id={`provider-api-key-${selectedProvider.id}`}
                      type={showApiKey ? "text" : "password"}
                      value={selectedProvider.api_key || ""}
                      onChange={(event) => updateProvider(selectedProvider.id, { api_key: event.target.value })}
                      disabled={anySessionRunning || isSavingConfig}
                      spellCheck={false}
                      autoComplete="off"
                      placeholder={t(locale, "field.apiKeyPlaceholder")}
                    />
                    <button type="button" className="quiet-button" onClick={() => setShowApiKey((current) => !current)}>
                      {showApiKey ? t(locale, "action.hideKey") : t(locale, "action.showKey")}
                    </button>
                  </div>
                  </div>
                ) : null}
            </div>
            <div className={`auth-status ${providerStatus?.ready ? "ready" : "missing"}`} role="status">
              <strong>{providerStatus?.ready ? t(locale, "auth.ready") : t(locale, "auth.missing")}</strong>
              <small>{providerStatus?.message || t(locale, "auth.checking")}</small>
              <small>
                {t(locale, "auth.base", { url: providerStatus?.base_url || activeProvider?.base_url || DEFAULT_PROVIDER_BASE_URL })}
                {providerStatus?.key_hint ? ` · ${t(locale, "auth.key", { hint: providerStatus.key_hint })}` : ""}
              </small>
            </div>
            <div className="provider-model-actions">
              <button
                type="button"
                className="quiet-button"
                onClick={() => {
                  if (!selectedProvider) return;
                  void refreshModels({
                    provider: selectedProvider,
                    updateComposerModels: selectedProvider.id === activeProvider?.id,
                  });
                }}
                disabled={anySessionRunning || isSavingConfig || !selectedProvider || selectedProviderModelsLoading}
              >
                {selectedProviderModelsLoading ? t(locale, "auth.checking") : t(locale, "action.refreshAuth")}
              </button>
            </div>
            <p className="provider-models-label">{t(locale, "models.fetched")}</p>
            {selectedProviderModels.length > 0 ? (
              <div className="provider-model-list" role="list" aria-label={t(locale, "models.fetched")}>
                {selectedProviderModels.map((entry) => (
                  <span className="provider-model-item" role="listitem" key={entry.id}>
                    {entry.owned_by ? `${entry.id} · ${entry.owned_by}` : entry.id}
                  </span>
                ))}
              </div>
            ) : (
              <small className="provider-model-empty">{t(locale, "models.empty")}</small>
            )}
            {selectedProviderModelsError ? <small className="models-error">{selectedProviderModelsError}</small> : null}
            <p className="mode-help">{t(locale, "settings.providerHelp")}</p>
          </section>

          <section className="settings-card resilience-settings-card" aria-label={t(locale, "settings.resilience.title")}>
            <p className="panel-title">{t(locale, "settings.resilience.title")}</p>

            <div className="resilience-setting-group">
              <p className="resilience-setting-group-title">{t(locale, "settings.resilience.retryTimeout")}</p>
              <div className="resilience-setting-grid">
                <label htmlFor="provider-max-retries">
                  <span className="field-label">{t(locale, "field.maxProviderRetries")}</span>
                  <input id="provider-max-retries" type="number" min={MIN_MAX_PROVIDER_RETRIES} max={MAX_MAX_PROVIDER_RETRIES} step={1} value={maxProviderRetries} onChange={(event) => setMaxProviderRetries(normalizeBoundedInteger(Number(event.target.value), DEFAULT_MAX_PROVIDER_RETRIES, MIN_MAX_PROVIDER_RETRIES, MAX_MAX_PROVIDER_RETRIES))} disabled={anySessionRunning || isSavingConfig} />
                </label>
                <label htmlFor="provider-max-tool-rounds">
                  <span className="field-label">{t(locale, "field.maxToolRounds")}</span>
                  <input id="provider-max-tool-rounds" type="number" min={MIN_MAX_TOOL_ROUNDS} max={MAX_MAX_TOOL_ROUNDS} step={1} value={maxToolRounds} onChange={(event) => setMaxToolRounds(normalizeBoundedInteger(Number(event.target.value), DEFAULT_MAX_TOOL_ROUNDS, MIN_MAX_TOOL_ROUNDS, MAX_MAX_TOOL_ROUNDS))} disabled={anySessionRunning || isSavingConfig} />
                </label>
                <label htmlFor="provider-circuit-failure-threshold">
                  <span className="field-label">{t(locale, "field.circuitFailureThreshold")}</span>
                  <input id="provider-circuit-failure-threshold" type="number" min={MIN_CIRCUIT_FAILURE_THRESHOLD} max={MAX_CIRCUIT_FAILURE_THRESHOLD} step={1} value={circuitFailureThreshold} onChange={(event) => setCircuitFailureThreshold(normalizeBoundedInteger(Number(event.target.value), DEFAULT_CIRCUIT_FAILURE_THRESHOLD, MIN_CIRCUIT_FAILURE_THRESHOLD, MAX_CIRCUIT_FAILURE_THRESHOLD))} disabled={anySessionRunning || isSavingConfig} />
                </label>
              </div>
              <p className="mode-help">{t(locale, "field.maxProviderRetriesHint")}</p>
              <p className="mode-help">{t(locale, "field.maxToolRoundsHint")}</p>
            </div>

            <div className="resilience-setting-group">
              <p className="resilience-setting-group-title">{t(locale, "settings.resilience.timeout")}</p>
              <div className="resilience-setting-grid">
                <label htmlFor="stream-first-event-timeout">
                  <span className="field-label">{t(locale, "field.streamFirstEventTimeout")}</span>
                  <input id="stream-first-event-timeout" type="number" min={MIN_STREAM_FIRST_EVENT_TIMEOUT_SECS} max={MAX_STREAM_FIRST_EVENT_TIMEOUT_SECS} step={1} value={streamFirstEventTimeoutSecs} onChange={(event) => setStreamFirstEventTimeoutSecs(normalizeBoundedInteger(Number(event.target.value), DEFAULT_STREAM_FIRST_EVENT_TIMEOUT_SECS, MIN_STREAM_FIRST_EVENT_TIMEOUT_SECS, MAX_STREAM_FIRST_EVENT_TIMEOUT_SECS))} disabled={anySessionRunning || isSavingConfig} />
                </label>
                <label htmlFor="stream-idle-timeout">
                  <span className="field-label">{t(locale, "field.streamIdleTimeout")}</span>
                  <input id="stream-idle-timeout" type="number" min={MIN_STREAM_IDLE_TIMEOUT_SECS} max={MAX_STREAM_IDLE_TIMEOUT_SECS} step={1} value={streamIdleTimeoutSecs} onChange={(event) => setStreamIdleTimeoutSecs(normalizeBoundedInteger(Number(event.target.value), DEFAULT_STREAM_IDLE_TIMEOUT_SECS, MIN_STREAM_IDLE_TIMEOUT_SECS, MAX_STREAM_IDLE_TIMEOUT_SECS))} disabled={anySessionRunning || isSavingConfig} />
                </label>
                <label htmlFor="non-stream-timeout">
                  <span className="field-label">{t(locale, "field.nonStreamTimeout")}</span>
                  <input id="non-stream-timeout" type="number" min={MIN_NON_STREAM_TIMEOUT_SECS} max={MAX_NON_STREAM_TIMEOUT_SECS} step={1} value={nonStreamTimeoutSecs} onChange={(event) => setNonStreamTimeoutSecs(normalizeBoundedInteger(Number(event.target.value), DEFAULT_NON_STREAM_TIMEOUT_SECS, MIN_NON_STREAM_TIMEOUT_SECS, MAX_NON_STREAM_TIMEOUT_SECS))} disabled={anySessionRunning || isSavingConfig} />
                </label>
              </div>
              <p className="mode-help">{t(locale, "field.timeoutSettingsHint")}</p>
            </div>

            <div className="resilience-setting-group">
              <p className="resilience-setting-group-title">{t(locale, "settings.resilience.circuitBreaker")}</p>
              <div className="resilience-setting-grid">
                <label htmlFor="circuit-recovery-success-threshold">
                  <span className="field-label">{t(locale, "field.circuitRecoverySuccessThreshold")}</span>
                  <input id="circuit-recovery-success-threshold" type="number" min={MIN_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD} max={MAX_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD} step={1} value={circuitRecoverySuccessThreshold} onChange={(event) => setCircuitRecoverySuccessThreshold(normalizeBoundedInteger(Number(event.target.value), DEFAULT_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD, MIN_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD, MAX_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD))} disabled={anySessionRunning || isSavingConfig} />
                </label>
                <label htmlFor="circuit-recovery-wait">
                  <span className="field-label">{t(locale, "field.circuitRecoveryWait")}</span>
                  <input id="circuit-recovery-wait" type="number" min={MIN_CIRCUIT_RECOVERY_WAIT_SECS} max={MAX_CIRCUIT_RECOVERY_WAIT_SECS} step={1} value={circuitRecoveryWaitSecs} onChange={(event) => setCircuitRecoveryWaitSecs(normalizeBoundedInteger(Number(event.target.value), DEFAULT_CIRCUIT_RECOVERY_WAIT_SECS, MIN_CIRCUIT_RECOVERY_WAIT_SECS, MAX_CIRCUIT_RECOVERY_WAIT_SECS))} disabled={anySessionRunning || isSavingConfig} />
                </label>
                <label htmlFor="circuit-error-rate-threshold">
                  <span className="field-label">{t(locale, "field.circuitErrorRateThreshold")}</span>
                  <input id="circuit-error-rate-threshold" type="number" min={MIN_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT} max={MAX_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT} step={1} value={circuitErrorRateThresholdPercent} onChange={(event) => setCircuitErrorRateThresholdPercent(normalizeBoundedInteger(Number(event.target.value), DEFAULT_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT, MIN_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT, MAX_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT))} disabled={anySessionRunning || isSavingConfig} />
                </label>
                <label htmlFor="circuit-min-request-count">
                  <span className="field-label">{t(locale, "field.circuitMinRequestCount")}</span>
                  <input id="circuit-min-request-count" type="number" min={MIN_CIRCUIT_MIN_REQUEST_COUNT} max={MAX_CIRCUIT_MIN_REQUEST_COUNT} step={1} value={circuitMinRequestCount} onChange={(event) => setCircuitMinRequestCount(normalizeBoundedInteger(Number(event.target.value), DEFAULT_CIRCUIT_MIN_REQUEST_COUNT, MIN_CIRCUIT_MIN_REQUEST_COUNT, MAX_CIRCUIT_MIN_REQUEST_COUNT))} disabled={anySessionRunning || isSavingConfig} />
                </label>
              </div>
              <p className="mode-help">{t(locale, "field.circuitBreakerHint")}</p>
            </div>
          </section>

          <section className="settings-card settings-defaults-card" aria-label={t(locale, "aria.workspaceDefaults")}>
            <p className="panel-title">{t(locale, "field.defaults")}</p>
            <label className="field-label" htmlFor="settings-workspace">{t(locale, "field.workspace")}</label>
            <input
              id="settings-workspace"
              className={!workspaceHome.trim() ? "workspace-missing" : undefined}
              value={workspaceHome}
              onChange={(event) => setWorkspaceHome(event.target.value)}
              placeholder={t(locale, "field.workspacePlaceholder")}
              spellCheck={false}
              disabled={anySessionRunning || isSavingConfig}
            />
            <p className="mode-help">{t(locale, "field.workspaceHint")}</p>
            <label className="field-label" htmlFor="default-mode">{t(locale, "field.mode")}</label>
            <select
              id="default-mode"
              value={mode}
              onChange={(event) => setMode(event.target.value as Mode)}
              disabled={anySessionRunning || isSavingConfig}
            >
              <option value="ask">{t(locale, "mode.ask")}</option>
              <option value="auto-edit">{t(locale, "mode.autoEdit")}</option>
            </select>
            <p className="mode-help">{modeHelpText(mode, locale)}</p>
            <label className="field-label" htmlFor="command-allowlist">{t(locale, "field.allowlist")}</label>
            <textarea
              id="command-allowlist"
              className="command-allowlist-input"
              value={commandAllowlistText}
              onChange={(event) => setCommandAllowlistText(event.target.value)}
              disabled={anySessionRunning || isSavingConfig || !workspaceRoot.trim()}
              spellCheck={false}
              rows={4}
              placeholder={"rg\nmake:test\ngit:--version"}
            />
            <p className="mode-help">{commandAllowlistHelpText(locale)}</p>
            <label className="field-label" htmlFor="command-denylist">{t(locale, "field.denylist")}</label>
            <textarea
              id="command-denylist"
              className="command-allowlist-input"
              value={commandDenylistText}
              onChange={(event) => setCommandDenylistText(event.target.value)}
              disabled={anySessionRunning || isSavingConfig || !workspaceRoot.trim()}
              spellCheck={false}
              rows={3}
              placeholder={"powershell\ncurl"}
            />
            <p className="mode-help">{commandDenylistHelpText(locale)}</p>
            {!workspaceRoot.trim() ? (
              <p className="mode-help">{t(locale, "settings.workspacePolicyHint")}</p>
            ) : null}
          </section>
        </div>
      </main>
    );
  }

  return (
    <main
      className={`workbench${rightPanelOpen ? " has-right-panel" : ""}`}
      style={
        rightPanelOpen
          ? ({ ["--right-panel-width"]: `${rightPanelWidth}px` } as Record<string, string>)
          : undefined
      }
    >
      {createProjectOpen ? (
        <div className="modal-backdrop" role="presentation" onClick={() => !creatingProject && setCreateProjectOpen(false)}>
          <div
            className="modal-card"
            role="dialog"
            aria-modal="true"
            aria-label={t(locale, "project.createTitle")}
            onClick={(event) => event.stopPropagation()}
          >
            <div className="modal-header">
              <p className="panel-title">{t(locale, "project.createTitle")}</p>
              <button
                type="button"
                className="quiet-button"
                onClick={() => setCreateProjectOpen(false)}
                disabled={creatingProject}
                aria-label={t(locale, "action.cancel")}
              >
                ×
              </button>
            </div>
            <label className="field-label" htmlFor="create-project-name">{t(locale, "field.projectName")}</label>
            <input
              id="create-project-name"
              value={createProjectName}
              onChange={(event) => setCreateProjectName(event.target.value)}
              placeholder={t(locale, "field.projectNamePlaceholder")}
              disabled={creatingProject}
              autoFocus
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  void submitCreateProject();
                }
              }}
            />
            <p className="mode-help">
              {workspaceHome.trim()
                ? t(locale, "project.createHelp", { home: workspaceHome.trim() })
                : t(locale, "project.createNeedHome")}
            </p>
            <p className="mode-help">{t(locale, "project.chooseHelp")}</p>
            <div className="modal-actions modal-actions-split">
              <button
                type="button"
                className="quiet-button"
                onClick={() => void chooseExistingProjectFolder()}
                disabled={creatingProject || !workspaceHome.trim()}
              >
                {t(locale, "action.chooseProjectFolder")}
              </button>
              <div className="modal-actions-right">
                <button type="button" className="quiet-button" onClick={() => setCreateProjectOpen(false)} disabled={creatingProject}>
                  {t(locale, "action.cancel")}
                </button>
                <button type="button" className="primary-button" onClick={() => void submitCreateProject()} disabled={creatingProject || !createProjectName.trim()}>
                  {creatingProject ? t(locale, "action.creatingProject") : t(locale, "action.createProject")}
                </button>
              </div>
            </div>
          </div>
        </div>
      ) : null}

      <aside className="sessions-panel" aria-label={t(locale, "aria.sessions")}>
        <div className="sessions-top">
          <div className="sidebar-actions">
            <button
              type="button"
              className="quiet-button sidebar-settings-button"
              onClick={openSettings}
              aria-label={t(locale, "aria.openSettings")}
              title={t(locale, "action.settings")}
            >
              <SettingsIcon />
            </button>
            <button
              type="button"
              className="quiet-button sidebar-action-button"
              onClick={() => startNewTask()}
              aria-label={t(locale, "action.newTask")}
            >
              {t(locale, "action.newTask")}
            </button>
            <button
              type="button"
              className="quiet-button sidebar-action-button"
              onClick={() => startNewChat()}
              aria-label={t(locale, "action.newChat")}
            >
              {t(locale, "action.newChat")}
            </button>
          </div>
        </div>
        <nav className="session-list" aria-label={t(locale, "aria.savedSessions")}>
          <div className="project-group chat-group">
            <div className="projects-header-row chat-header-row">
              <p className="panel-title session-list-title">{t(locale, "field.chats")}</p>
              <button
                type="button"
                className="quiet-button projects-add-button"
                onClick={() => startNewChat()}
                aria-label={t(locale, "action.newChat")}
                title={t(locale, "action.newChat")}
              >
                +
              </button>
            </div>
            {chatSessions.length === 0 ? (
              <div className="empty-state projects-empty chat-empty">
                <p>{t(locale, "history.emptyChats")}</p>
              </div>
            ) : (
              chatSessions.map((session) => (
                <button
                  type="button"
                  className={`session-item ${session.id === activeSessionId ? "is-active" : ""} status-${session.status}`}
                  key={session.id}
                  onClick={() => selectSession(session)}
                  onContextMenu={(event) => openSessionMenu(event, session.id)}
                  title={sessionMetaLine(session, Date.now(), locale)}
                >
                  <span className="session-item-title">{sessionTitle(session, locale)}</span>
                </button>
              ))
            )}
          </div>
          <div className="projects-header-row">
            <p className="panel-title session-list-title">{t(locale, "field.projects")}</p>
            <button
              type="button"
              className="quiet-button projects-add-button"
              onClick={() => {
                setError(null);
                setCreateProjectName("");
                setCreateProjectOpen(true);
              }}
              aria-label={t(locale, "action.createProject")}
              title={t(locale, "action.createProject")}
            >
              +
            </button>
          </div>
          {projectGroups.length === 0 ? (
            <div className="empty-state projects-empty">
              <p>{t(locale, "history.emptyProjects")}</p>
              <button
                type="button"
                className="quiet-button"
                onClick={() => {
                  setError(null);
                  setCreateProjectName("");
                  setCreateProjectOpen(true);
                }}
              >
                {t(locale, "action.createProject")}
              </button>
            </div>
          ) : null}
          {projectGroups.map((group) => {
            const collapsed = !!collapsedProjects[group.root];
            const isCurrentProject =
              !activeIsChat &&
              normalizeRoot(group.root) === normalizeRoot(workspaceRoot);
            return (
              <div className={`project-group ${isCurrentProject ? "is-current" : ""}`} key={group.root}>
                <button
                  type="button"
                  className="project-header"
                  onClick={() => {
                    setCollapsedProjects((current) => ({ ...current, [group.root]: false }));
                    selectProject(group.root);
                  }}
                  onContextMenu={(event) => openProjectMenu(event, group.root)}
                  title={group.root}
                >
                  <span className="project-chevron" aria-hidden="true">{collapsed ? "▸" : "▾"}</span>
                  <span className="project-icon" aria-hidden="true">📁</span>
                  <span className="project-name">{group.name}</span>
                  <span className="project-count">{group.sessions.length}</span>
                </button>
                {collapsed ? null : group.sessions.map((session) => (
                  <button
                    type="button"
                    className={`session-item ${session.id === activeSessionId ? "is-active" : ""} status-${session.status}`}
                    key={session.id}
                    onClick={() => selectSession(session)}
                    onContextMenu={(event) => openSessionMenu(event, session.id)}
                    title={sessionMetaLine(session, Date.now(), locale)}
                  >
                    <span className="session-item-title">{sessionTitle(session, locale)}</span>
                  </button>
                ))}
              </div>
            );
          })}
        </nav>
        {sessionMenu ? (
          <div
            className="session-context-menu"
            style={{ left: sessionMenu.x, top: sessionMenu.y }}
            role="menu"
            onClick={(event) => event.stopPropagation()}
            onContextMenu={(event) => event.preventDefault()}
          >
            <button
              type="button"
              role="menuitem"
              onClick={() => void renameSession(sessionMenu.sessionId)}
            >
              {t(locale, "action.rename")}
            </button>
            <button
              type="button"
              className="danger"
              role="menuitem"
              onClick={() => void deleteSession(sessionMenu.sessionId)}
            >
              {t(locale, "action.delete")}
            </button>
          </div>
        ) : null}
        {projectMenu ? (
          <div
            className="session-context-menu"
            style={{ left: projectMenu.x, top: projectMenu.y }}
            role="menu"
            onClick={(event) => event.stopPropagation()}
            onContextMenu={(event) => event.preventDefault()}
          >
            <button
              type="button"
              role="menuitem"
              onClick={() => {
                const root = projectMenu.root;
                setProjectMenu(null);
                void openPath(root).catch((cause) => {
                  setError(cause instanceof Error ? cause.message : String(cause));
                });
              }}
            >
              {t(locale, "action.openInExplorer")}
            </button>
            <button
              type="button"
              className="danger"
              role="menuitem"
              onClick={() => void removeProjectFromArea(projectMenu.root)}
            >
              {t(locale, "action.removeProject")}
            </button>
          </div>
        ) : null}
      </aside>

      <section className={`chat-panel${bottomPanelOpen ? " has-bottom-panel" : ""}`} aria-label={t(locale, "aria.conversation")}>
        <header className="chat-header">
          <h2>
            {activeSession ? sessionTitle(activeSession, locale) : t(locale, "chat.newTask")}
            {canContinueSession(activeSession) ? ` · ${t(locale, "chat.followUp")}` : ""}
          </h2>
          <div className="chat-header-actions">
            <HeaderEnvButton
              locale={locale}
              open={envPopoverOpen}
              onToggle={() => setEnvPopoverOpen((value) => !value)}
              buttonRef={envButtonRef}
              summary={envSummary}
            />
            <HeaderPanelToggle
              locale={locale}
              open={bottomPanelOpen}
              onToggle={toggleBottomPanel}
            />
            <HeaderRightPanelToggle
              locale={locale}
              open={rightPanelOpen}
              onToggle={toggleRightPanel}
            />
            <EnvironmentPopover
              locale={locale}
              open={envPopoverOpen}
              onClose={() => setEnvPopoverOpen(false)}
              workspaceRoot={workspaceRoot}
              sources={envSources}
              anchorRef={envButtonRef}
              onOpenTerminal={(seed) => openTerminalPanel(seed)}
            />
          </div>
        </header>
        <div
          className="conversation"
          aria-live="polite"
          ref={conversationRef}
          onScroll={(event) => updateConversationBottomState(event.currentTarget)}
        >
          {groupConversationMessages(messages).map((entry) => {
            if (entry.kind === "tool-group") {
              return (
                <details className="message message-tool-group" key={entry.id}>
                  <summary>
                    <span>{t(locale, "role.tool")} · {entry.messages.length}</span>
                  </summary>
                  <div className="tool-message-list">
                    {entry.messages.map((message) => (
                      <section className="tool-message-item" key={message.id}>
                        <span className="message-preview">{toolMessagePreview(message.content)}</span>
                        <div className="message-body">{message.content}</div>
                      </section>
                    ))}
                  </div>
                </details>
              );
            }

            const message = entry.message;
            const elapsed = message.role === "assistant" ? completedRunElapsed[message.id] : null;
            return message.role === "user" ? (
              <article className={`message message-${message.role}`} key={message.id}>
                <div className="message-bubble">
                  <UserMessageBody content={message.content} />
                </div>
                <div className="message-meta">
                  {formatMessageTime(message.created_at) ? (
                    <time dateTime={message.created_at}>{formatMessageTime(message.created_at)}</time>
                  ) : null}
                  <button
                    type="button"
                    className="message-meta-button"
                    onClick={() => void copyText(parseStoredUserMessage(message.content).text || message.content)}
                    title={t(locale, "action.copy")}
                    aria-label={t(locale, "action.copy")}
                  >
                    <CopyIcon />
                  </button>
                  <button
                    type="button"
                    className="message-meta-button"
                    onClick={() => editSentUserMessage(message.content)}
                    title={t(locale, "action.editMessage")}
                    aria-label={t(locale, "action.editMessage")}
                  >
                    <EditIcon />
                  </button>
                </div>
              </article>
            ) : (
              <article className={`message message-${message.role}`} key={message.id}>
                <div className="message-bubble">
                  <div className="message-body">{message.role === "assistant" ? <AssistantMessageBody content={message.content} onOpenLink={openAssistantLink} /> : message.content}</div>
                </div>
                {elapsed ? <p className="message-completed-elapsed">{t(locale, "run.processed", { elapsed })}</p> : null}
                {message.role === "assistant" ? (
                  <div className="message-meta">
                    {formatMessageTime(message.created_at) ? (
                      <time dateTime={message.created_at}>{formatMessageTime(message.created_at)}</time>
                    ) : null}
                    <button
                      type="button"
                      className="message-meta-button"
                      onClick={() => void copyText(message.content)}
                      title={t(locale, "action.copy")}
                      aria-label={t(locale, "action.copy")}
                    >
                      <CopyIcon />
                    </button>
                  </div>
                ) : null}
              </article>
            );
          })}
          {runStatus ? (
            <details
              className={`run-status run-status-${runStatus.phase}`}
              open={runStatusExpanded}
              onToggle={(event) => setRunStatusExpanded(event.currentTarget.open)}
            >
              <summary
                title={`${t(locale, "run.elapsed", { elapsed: formatRunElapsed(runStatus.startedAt, runStatusClock) })} · ${
                  runStatus.phase === "thinking"
                    ? t(locale, "run.thinking")
                    : runStatus.phase === "retrying"
                      ? t(locale, "run.reconnecting", {
                        attempt: runStatus.retryAttempt ?? 1,
                        max: runStatus.retryMaxAttempts ?? 5,
                      })
                      : t(locale, "activity.agentError")
                }`}
              >
                <span
                  className={`run-status-dots${runStatus.phase === "failed" ? " failed" : ""}`}
                  aria-hidden="true"
                >
                  {runStatus.phase === "failed" ? "!" : <><i /><i /><i /></>}
                </span>
                {runStatus.phase !== "failed" && plan.length > 0 && currentPlanStepIndex >= 0 ? (
                  <span>{t(locale, "run.stepProgress", {
                    current: String(currentPlanStepIndex + 1),
                    total: String(plan.length),
                  })}</span>
                ) : (
                  <span>
                    {runStatus.phase === "failed"
                      ? t(locale, "activity.agentError")
                      : t(locale, "run.elapsed", { elapsed: formatRunElapsed(runStatus.startedAt, runStatusClock) })}
                  </span>
                )}
              </summary>
              <div className="run-status-popover">
                {plan.length > 0 && currentPlanStepIndex >= 0 ? (
                  <ol className="run-plan-list" aria-label={t(locale, "run.planLabel")}>
                    {plan.map((step, index) => {
                      const state = index < currentPlanStepIndex
                        ? "done"
                        : index === currentPlanStepIndex
                          ? (runStatus.phase === "failed" ? "failed" : "current")
                          : "pending";
                      return (
                        <li className={`run-plan-step ${state}`} key={step.id}>
                          <span className="run-plan-marker" aria-hidden="true">
                            {state === "done" ? "✓" : state === "failed" ? "!" : state === "current" ? "●" : "○"}
                          </span>
                          <span>{runPlanStepDescription(step, locale)}</span>
                        </li>
                      );
                    })}
                  </ol>
                ) : null}
                {latestActivity ? <p className={`run-status-activity ${latestActivity.state}`}>{latestActivity.label}</p> : null}
                {runStatus.detail ? <p className="run-status-detail">{runStatus.detail}</p> : null}
                {streamedText ? (
                  <div className="run-status-process streaming">
                    <div className="message-body">
                      <AssistantMessageBody content={streamedText} onOpenLink={openAssistantLink} />
                    </div>
                  </div>
                ) : null}
              </div>
            </details>
          ) : streamedText ? (
            <article className="message message-assistant streaming">
              <div className="message-body"><AssistantMessageBody content={streamedText} onOpenLink={openAssistantLink} /></div>
            </article>
          ) : null}
          {messages.length === 0 && !streamedText && !isRunning ? (
            <div className="empty-chat">
              <p className="empty-state">{t(locale, activeIsChat ? "chat.emptyChat" : "chat.empty")}</p>
              <ul className="empty-hints">
                <li>{t(locale, activeIsChat ? "chat.hint.leftChat" : "chat.hint.left")}</li>
                <li>{t(locale, "chat.hint.center")}</li>
              </ul>
              <EmptyQuickActions locale={locale} onOpen={(tab) => openPanel(tab)} />
              <p className="empty-state composer-hint">{t(locale, "chat.tip")}</p>
            </div>
          ) : null}
          {error && runStatus?.phase !== "failed" ? <p className="error-message">{error}</p> : null}
        </div>
        <div className="chat-bottom">
        {pendingAction ? (() => {
          const review = buildReviewPresentation(pendingAction, approvalSummary, Boolean(patchPreview), locale);
          return (
            <section className={`review-panel${review.highRisk ? " high-risk" : ""}`} aria-label={review.title}>
              <p className="panel-title">{t(locale, "trace.review")}</p>
              <div className="review-header">
                <strong>{review.title}</strong>
                {review.highRisk ? <span className="risk-badge">{t(locale, "risk.high")}</span> : null}
              </div>
              <p className="review-summary">{review.summary}</p>
              {review.bodyKind === "patch" && patchPreview ? (
                <>
                  <code>{patchPreview.path}</code>
                  <pre className="diff-preview">
                    {buildPatchDiffLines(patchPreview, locale).map((line, index) => (
                      <span key={index} className={`diff-line ${line.kind}`}>
                        {line.kind === "remove" ? `- ${line.text}` : line.kind === "add" ? `+ ${line.text}` : line.text}
                      </span>
                    ))}
                  </pre>
                </>
              ) : null}
              {review.bodyKind === "command" ? (
                <pre className="command-preview" aria-label="Command to approve">
                  {review.commandText ?? JSON.stringify(pendingAction.tool_call.arguments, null, 2)}
                </pre>
              ) : null}
              {review.bodyKind === "git" ? (
                <pre className="command-preview git-preview" aria-label="Git operation to approve">
                  {review.gitDetail ?? JSON.stringify(pendingAction.tool_call.arguments, null, 2)}
                </pre>
              ) : null}
              {review.bodyKind === "generic" ? <code>{JSON.stringify(pendingAction.tool_call.arguments)}</code> : null}
              {review.riskHint ? <p className="risk-hint">{review.riskHint}</p> : null}
              {isTauriRuntime && review.highRisk && isRememberableLocalApiRequest(pendingAction) ? (
                <label className="local-api-confirmation">
                  <input
                    type="checkbox"
                    checked={rememberLocalApiApproval}
                    onChange={(event) => setRememberLocalApiApproval(event.target.checked)}
                    disabled={isRunning}
                  />
                  <span>
                    <strong>{t(locale, "review.localApiRemember")}</strong>
                    <small>{t(locale, "review.localApiRememberHint")}</small>
                  </span>
                </label>
              ) : null}
              <div className="review-actions">
                <button type="button" className="reject-button" onClick={() => void resolveAction(false)} disabled={isRunning}>
                  {t(locale, "action.reject")}
                </button>
                <button
                  type="button"
                  className={review.highRisk ? "approve-risk-button" : undefined}
                  onClick={() => void resolveAction(true)}
                  disabled={isRunning}
                >
                  {review.highRisk ? t(locale, "action.approveRisk") : t(locale, "action.approve")}
                </button>
              </div>
            </section>
          );
        })() : null}
        <form className="composer" onSubmit={submit}>
          {!conversationAtBottom ? (
            <button
              type="button"
              className="scroll-to-bottom-button"
              onClick={() => scrollConversationToBottom()}
              title={t(locale, "run.scrollBottom")}
              aria-label={t(locale, "run.scrollBottom")}
            >
              <span aria-hidden="true">↓</span>
            </button>
          ) : null}
          {activeFollowUps.length > 0 ? (
            <div className="followup-queue" aria-label={t(locale, "composer.queueList")}>
              <div className="followup-queue-header">
                <strong>{t(locale, "composer.queueTitle", { count: String(activeFollowUps.length) })}</strong>
                <span>{t(locale, "composer.queueHelp")}</span>
              </div>
              <ul>
                {activeFollowUps.map((item, index) => (
                  <li key={item.id}>
                    <span className="followup-index">#{index + 1}</span>
                    <span className="followup-text">
                      {item.text || (item.images.length > 0 ? t(locale, "message.imageCount", { count: String(item.images.length) }) : "")}
                      {item.text && item.images.length > 0 ? ` · ${t(locale, "message.imageCount", { count: String(item.images.length) })}` : ""}
                    </span>
                    <div className="followup-actions">
                      <button
                        type="button"
                        className="quiet-button"
                        onClick={() => void steerFollowUp(item.id)}
                        title={t(locale, "action.steerHelp")}
                      >
                        {t(locale, "action.steerMode")}
                      </button>
                      <button
                        type="button"
                        className="quiet-button"
                        onClick={() => editFollowUp(item.id)}
                        title={t(locale, "action.editMessage")}
                      >
                        {t(locale, "action.editMessage")}
                      </button>
                      <button
                        type="button"
                        className="quiet-button"
                        onClick={() => removeFollowUp(item.id)}
                        title={t(locale, "action.closeQueue")}
                        aria-label={t(locale, "action.closeQueue")}
                      >
                        {t(locale, "action.closeQueue")}
                      </button>
                    </div>
                  </li>
                ))}
              </ul>
            </div>
          ) : null}
          {composerImages.length > 0 ? (
            <div className="composer-images" aria-label={t(locale, "composer.imagesLabel")}>
              {composerImages.map((image) => (
                <div className="composer-image" key={image.id}>
                  <img src={image.previewUrl} alt={image.name || t(locale, "message.imageAttachment")} />
                  <button
                    type="button"
                    className="composer-image-remove"
                    onClick={() => removeComposerImage(image.id)}
                    title={t(locale, "composer.removeImage")}
                    aria-label={t(locale, "composer.removeImage")}
                  >
                    ×
                  </button>
                </div>
              ))}
            </div>
          ) : null}
          <textarea
            value={prompt}
            onChange={(event) => setPrompt(event.target.value)}
            onKeyDown={onComposerKeyDown}
            onPaste={(event) => void onComposerPaste(event)}
            placeholder={
              queueMode
                ? t(locale, "composer.queuePlaceholder")
                : canContinueSession(activeSession)
                  ? t(locale, "composer.continuePlaceholder")
                  : t(locale, activeIsChat ? "composer.placeholderChat" : "composer.placeholder")
            }
            rows={4}
          />
          <input
            ref={imageInputRef}
            type="file"
            accept="image/png,image/jpeg,image/webp,image/gif"
            multiple
            hidden
            onChange={(event) => void onComposerImagePick(event)}
          />
          <div className="composer-footer">
            <div className="composer-footer-meta">
              <button
                type="button"
                className="composer-model-log-button"
                onClick={() => {
                  setView("model-logs");
                  void loadModelCallLogs(activeSessionId);
                }}
                title={t(locale, "logs.open")}
              >
                {t(locale, "logs.open")}
              </button>
              <span title={sendHint || undefined}>
              {queueMode
                ? t(locale, "composer.queueModeLabel")
                : canContinueSession(activeSession)
                  ? t(locale, "composer.continueId", { id: activeSession!.id.slice(0, 8) })
                  : sendHint
                    ? sendHint
                    : workspaceRoot.trim()
                      ? workspaceRoot
                      : t(locale, "composer.chooseWorkspace")}
              </span>
            </div>
            <div className="composer-actions">
              <select
                id="composer-model"
                className="composer-select model"
                value={model}
                onChange={(event) => {
                  const next = event.target.value;
                  setModel(next);
                  if (activeSessionId && next.trim()) {
                    setSessions((current) =>
                      current.map((session) =>
                        session.id === activeSessionId ? { ...session, model: next.trim() } : session,
                      ),
                    );
                  }
                  void persistComposerPrefs(next, reasoningEffort);
                }}
                disabled={isSavingConfig || (modelsLoading && availableModels.length === 0)}
                title={t(locale, "field.model")}
                aria-label={t(locale, "field.model")}
              >
                <option value="">
                  {modelsLoading
                    ? t(locale, "models.loading")
                    : t(locale, "models.placeholder")}
                </option>
                {modelNotInList ? (
                  <option value={model}>
                    {model} ({t(locale, "models.notInList")})
                  </option>
                ) : null}
                {availableModels.map((entry) => (
                  <option key={entry.id} value={entry.id}>
                    {entry.owned_by ? `${entry.id} · ${entry.owned_by}` : entry.id}
                  </option>
                ))}
              </select>
              <div className="context-usage">
                <button
                  type="button"
                  className="context-usage-button"
                  onClick={() => setContextUsageOpen((open) => !open)}
                  aria-expanded={contextUsageOpen}
                  aria-haspopup="dialog"
                  aria-label={t(locale, "context.title")}
                  title={t(locale, "context.title")}
                >
                  <span aria-hidden="true">i</span>
                </button>
                {contextUsageOpen ? (
                  <div className="context-usage-popover" role="dialog" aria-label={t(locale, "context.title")}>
                    <strong>{t(locale, "context.title")}</strong>
                    <div className="context-usage-percent">{t(locale, "context.usage", { percent: contextUsage.percent })}</div>
                    <div className="context-usage-meter" aria-hidden="true">
                      <span style={{ width: `${contextUsage.percent}%` }} />
                    </div>
                    <span className="context-usage-detail">
                      {t(locale, "context.usedOf", {
                        used: formatContextTokenCount(contextUsage.used),
                        limit: formatContextTokenCount(contextUsage.limit),
                      })}
                    </span>
                    <span className="context-usage-estimate">{t(locale, "context.estimated")}</span>
                  </div>
                ) : null}
              </div>
              <select
                id="composer-reasoning"
                className="composer-select reasoning"
                value={reasoningEffort}
                onChange={(event) => {
                  const next = normalizeReasoningEffort(event.target.value);
                  setReasoningEffort(next);
                  void persistComposerPrefs(model, next);
                }}
                disabled={isSavingConfig}
                title={t(locale, "field.reasoning")}
                aria-label={t(locale, "field.reasoning")}
              >
                {REASONING_EFFORTS.map((effort) => (
                  <option key={effort} value={effort}>
                    {t(locale, `reasoning.${effort}`)}
                  </option>
                ))}
              </select>

              <select
                id="composer-mode"
                className="composer-select mode"
                value={mode}
                onChange={(event) => setMode(event.target.value as Mode)}
                disabled={isRunning || isSavingConfig}
                title={t(locale, "field.mode")}
                aria-label={t(locale, "field.mode")}
              >
                <option value="ask">{t(locale, "mode.ask")}</option>
                <option value="auto-edit">{t(locale, "mode.autoEdit")}</option>
              </select>
              <button
                type="button"
                className="quiet-button composer-attach"
                onClick={() => imageInputRef.current?.click()}
                disabled={composerImages.length >= MAX_COMPOSER_IMAGES}
                title={t(locale, "action.attachImage")}
                aria-label={t(locale, "action.attachImage")}
              >
                +
              </button>
              {queueMode && hasComposerContent ? (
                <div className="composer-run-mode" role="group" aria-label={t(locale, "composer.queueModeLabel")}>
                  <button
                    type="button"
                    className={composerSendMode === "queue" ? "active" : undefined}
                    onClick={() => setComposerSendMode("queue")}
                    title={t(locale, "composer.queueHint")}
                  >
                    {t(locale, "action.queueMode")}
                  </button>
                  <button
                    type="button"
                    className={composerSendMode === "steer" ? "active" : undefined}
                    onClick={() => setComposerSendMode("steer")}
                    title={t(locale, "action.steerHelp")}
                  >
                    {t(locale, "action.steerMode")}
                    <kbd>Ctrl</kbd>
                  </button>
                </div>
              ) : null}
              {queueMode && !hasComposerContent ? (
                <button
                  type="button"
                  className="composer-icon-button stop"
                  onClick={() => void cancelSession()}
                  disabled={!activeSessionId}
                  title={t(locale, "action.stop")}
                  aria-label={t(locale, "action.stop")}
                >
                  <span className="composer-stop-square" aria-hidden="true" />
                </button>
              ) : (
                <button
                  type="submit"
                  className={`composer-icon-button send${sendBlockReason ? " send-needs-setup" : ""}`}
                  disabled={!!sendBlockReason}
                  title={sendTitle}
                  aria-label={sendTitle}
                >
                  <span className="composer-send-arrow" aria-hidden="true">↑</span>
                </button>
              )}
            </div>
          </div>
        </form>
        </div>
        <TerminalBottomPanel
          locale={locale}
          open={bottomPanelOpen}
          onClose={() => setBottomPanelOpen(false)}
          workspaceRoot={workspaceRoot}
        />
      </section>

      <RightToolsPanel
        locale={locale}
        open={rightPanelOpen}
        tab={rightPanelTab}
        browserNavigation={browserNavigation}
        onTabChange={setRightPanelTab}
        onClose={() => setRightPanelOpen(false)}
        workspaceRoot={workspaceRoot}
        width={rightPanelWidth}
        onWidthChange={(next) => {
          const clamped = clampRightPanelWidth(next);
          setRightPanelWidth(clamped);
          saveRightPanelWidth(clamped);
        }}
        reviewContent={
          pendingAction ? (
            <p className="env-muted">{t(locale, "trace.review")}: {pendingAction.tool_call.name}</p>
          ) : null
        }
      />
    </main>
  );
}

