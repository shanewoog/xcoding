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
  ModelCapabilities,
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
  LocalPluginItem,
  ProjectDir,
  ProviderAuthStatus,
  ProviderApiKey,
  ProviderKeyStatus,
  ProviderModel,
  ModelRoute,
  ModelRouteStatus,
  UserConfig,
  VisionDelegateConfig,
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
  adoptDraftSessionKey,
  clampRightPanelWidth,
  clearSessionCompletedUnseen,
  DRAFT_SESSION_KEY,
  dropSessionKey,
  formatSessionStatus,
  loadRightPanelWidth,
  markSessionCompletedUnseen,
  rightPanelStateFor,
  saveRightPanelWidth,
  sessionMetaLine,
  sessionStateKey,
} from "./layout";
import {
  applyTheme,
  applyUiFontSize,
  loadTheme,
  loadUiFontSize,
  normalizeTheme,
  saveTheme,
  saveUiFontSize,
  THEMES,
  type Theme,
} from "./appearance";
import { isLocale, loadLocale, saveLocale, t, type Locale, type MessageKey } from "./i18n";
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
  type BrowserSessionState,
} from "./panels";
import {
  browserAdoptSession,
  browserClose,
  fetchGitEnvironment,
  formatDiffStat,
  openPath,
} from "./workspaceApi";

const defaultProvider = "openai";
const DEFAULT_PROVIDER_BASE_URL = "https://ai.v58.dev";
const DEFAULT_PERSONALITY = "default";
const PERSONALITY_OPTIONS = ["default", "pragmatic", "friendly", "concise", "teaching"] as const;
const MAX_CUSTOM_INSTRUCTIONS_CHARS = 4000;
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
const MIN_CONTEXT_WINDOW_TOKENS = 1_024;
const MAX_CONTEXT_WINDOW_TOKENS = 10_000_000;
const DEFAULT_CONTEXT_COMPACTION_THRESHOLD_PERCENT = 80;
const MIN_CONTEXT_COMPACTION_THRESHOLD_PERCENT = 50;
const MAX_CONTEXT_COMPACTION_THRESHOLD_PERCENT = 95;
const DEFAULT_VISION_TIMEOUT_SECS = 30;
const MIN_VISION_TIMEOUT_SECS = 5;
const MAX_VISION_TIMEOUT_SECS = 300;
const isTauriRuntime = "__TAURI_INTERNALS__" in window;
const REASONING_EFFORTS = ["none", "low", "medium", "high"] as const;
type ReasoningEffort = (typeof REASONING_EFFORTS)[number];

function normalizeReasoningEffort(value: string | undefined | null): ReasoningEffort {
  const trimmed = (value || "").trim().toLowerCase();
  return (REASONING_EFFORTS as readonly string[]).includes(trimmed)
    ? (trimmed as ReasoningEffort)
    : "high";
}

function normalizePersonality(value: string | undefined | null): string {
  const trimmed = (value || "").trim().toLowerCase();
  return (PERSONALITY_OPTIONS as readonly string[]).includes(trimmed) ? trimmed : DEFAULT_PERSONALITY;
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

const MIN_PROVIDER_KEY_WEIGHT = 0;
const MAX_PROVIDER_KEY_WEIGHT = 1000;

function providerKeyId(): string {
  return globalThis.crypto?.randomUUID?.() || `key-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

/** Keeps the stored key pool well formed: stable ids, bounded weights, no blank entries. */
function normalizeProviderApiKeys(items: ProviderApiKey[] | undefined): ProviderApiKey[] {
  return (items || [])
    .filter((item) => item && typeof item.key === "string")
    .map((item, index) => ({
      id: (typeof item.id === "string" ? item.id.trim() : "") || `key-${index + 1}`,
      label: item.label?.trim() || undefined,
      key: item.key.trim(),
      weight: normalizeBoundedInteger(item.weight, 1, MIN_PROVIDER_KEY_WEIGHT, MAX_PROVIDER_KEY_WEIGHT),
      enabled: item.enabled !== false,
    }))
    .filter((item) => item.key.length > 0);
}

/** Key actually used for single-key flows such as the model list probe. */
function primaryProviderKey(provider: CloudProviderConfig | null | undefined): string {
  if (!provider) return "";
  const pool = (provider.api_keys || []).filter((item) => item.enabled !== false && (item.key || "").trim());
  if (pool.length > 0) return (pool[0].key || "").trim();
  return (provider.api_key || "").trim();
}

function hydrateProviders(config: UserConfig): { providers: CloudProviderConfig[]; activeProviderId: string } {
  const configured = (config.providers || [])
    .filter((item) => item && typeof item.id === "string")
    .map((item, index) => ({
      id: item.id.trim() || `provider-${index + 1}`,
      name: item.name?.trim() || `Provider ${index + 1}`,
      base_url: normalizeProviderBaseUrl(item.base_url) || DEFAULT_PROVIDER_BASE_URL,
      wire_api: item.wire_api === "responses" ? "responses" as const : "chat_completions" as const,
      trust_level: item.trust_level === "local" || item.trust_level === "official" ? item.trust_level : "relay" as const,
      api_key: item.api_key || undefined,
      api_keys: normalizeProviderApiKeys(item.api_keys),
    }));
  const providers = configured.length > 0
    ? configured
    : [{
        id: "default",
        name: defaultProvider,
        base_url: normalizeProviderBaseUrl(config.base_url) || DEFAULT_PROVIDER_BASE_URL,
        wire_api: "chat_completions" as const,
        trust_level: "relay" as const,
        api_key: config.api_key || undefined,
        api_keys: [],
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

type ToolCallLike = { id: string; name: string; arguments: Record<string, unknown> };

type InlineActivityEntry = {
  id: string;
  kind: "file" | "command";
  label: string;
  path?: string;
  command?: string;
  state: "running" | "done" | "failed";
  created_at: string;
  added?: number;
  removed?: number;
  fileExisted?: boolean;
  summary?: string;
};

function applyPatchCounts(argumentsJson: Record<string, unknown>): { path: string; fileExisted: boolean; added: number; removed: number } {
  const path = typeof argumentsJson.path === "string" ? argumentsJson.path : "";
  const oldText = typeof argumentsJson.old_text === "string" ? argumentsJson.old_text : "";
  const newText = typeof argumentsJson.new_text === "string" ? argumentsJson.new_text : "";
  const fileExisted = oldText.trim().length > 0;
  const removed = fileExisted ? oldText.split("\n").length : 0;
  const added = newText ? newText.split("\n").length : 0;
  return { path, fileExisted, added, removed };
}

function runCommandPreview(argumentsJson: Record<string, unknown>): string {
  const executable = typeof argumentsJson.executable === "string" ? argumentsJson.executable : "";
  const rawArgs = argumentsJson.args;
  const pieces = Array.isArray(rawArgs) ? rawArgs.map((item) => String(item)) : [];
  const parts = [executable, ...pieces].filter((part) => part.length > 0);
  return parts.length > 0 ? parts.join(" ") : JSON.stringify(argumentsJson);
}

function buildInlineActivityStart(toolCall: ToolCallLike, createdAt: string, locale: Locale): InlineActivityEntry | null {
  if (toolCall.name === "apply_patch") {
    const { path, fileExisted, added, removed } = applyPatchCounts(toolCall.arguments);
    if (!path) return null;
    return {
      id: toolCall.id,
      kind: "file",
      label: t(locale, fileExisted ? "activity.fileEdited" : "activity.fileCreated"),
      path,
      state: "running",
      created_at: createdAt,
      added,
      removed,
      fileExisted,
    };
  }
  if (toolCall.name === "run_command") {
    return {
      id: toolCall.id,
      kind: "command",
      label: t(locale, "activity.commandRan"),
      command: runCommandPreview(toolCall.arguments),
      state: "running",
      created_at: createdAt,
    };
  }
  return null;
}

function inlineActivityLabel(entry: InlineActivityEntry, state: InlineActivityEntry["state"], locale: Locale): string {
  if (entry.kind !== "file" || state !== "failed") return entry.label;
  return t(locale, entry.fileExisted ? "activity.fileEditFailed" : "activity.fileCreateFailed");
}

function upsertInlineActivity(
  current: InlineActivityEntry[],
  toolCall: ToolCallLike,
  createdAt: string,
  state: "running" | "done" | "failed",
  summary: string,
  locale: Locale,
): InlineActivityEntry[] {
  const next = buildInlineActivityStart(toolCall, createdAt, locale);
  if (!next) return current;
  const index = current.findIndex((item) => item.id === toolCall.id);
  const list = index >= 0
    ? current.map((item, itemIndex) => (
        itemIndex === index
          ? { ...item, ...next, label: inlineActivityLabel(next, state, locale), state, summary, created_at: item.created_at }
          : item
      ))
    : [...current, { ...next, label: inlineActivityLabel(next, state, locale), state, summary }];
  return list.sort((a, b) => a.created_at.localeCompare(b.created_at));
}

function buildInlineActivity(events: PersistedSessionEvent[], locale: Locale): InlineActivityEntry[] {
  const byId = new Map<string, InlineActivityEntry>();
  const ordered = [...events].sort((a, b) => a.created_at.localeCompare(b.created_at));
  for (const item of ordered) {
    const event = item.event;
    if (event.type === "tool_start") {
      const entry = buildInlineActivityStart(event.tool_call, item.created_at, locale);
      if (entry) byId.set(entry.id, entry);
    } else if (event.type === "tool_end") {
      const existing = byId.get(event.tool_call.id);
      if (existing) {
        byId.set(event.tool_call.id, {
          ...existing,
          label: inlineActivityLabel(existing, event.success ? "done" : "failed", locale),
          state: event.success ? "done" : "failed",
          summary: event.summary,
        });
      }
    }
  }
  return [...byId.values()].sort((a, b) => a.created_at.localeCompare(b.created_at));
}

function splitInlineActivityByMessage(
  messages: Message[],
  entries: InlineActivityEntry[],
): { buckets: Map<string, InlineActivityEntry[]>; pending: InlineActivityEntry[] } {
  const buckets = new Map<string, InlineActivityEntry[]>();
  let previousAssistantAt: string | null = null;
  let lastAssistantAt: string | null = null;
  for (const message of messages) {
    if (message.role !== "assistant") continue;
    const bucket: InlineActivityEntry[] = [];
    for (const entry of entries) {
      if ((previousAssistantAt === null || entry.created_at > previousAssistantAt) && entry.created_at <= message.created_at) {
        bucket.push(entry);
      }
    }
    if (bucket.length > 0) buckets.set(message.id, bucket);
    previousAssistantAt = message.created_at;
    lastAssistantAt = message.created_at;
  }
  const pending = lastAssistantAt === null
    ? entries
    : entries.filter((entry) => entry.created_at > lastAssistantAt);
  return { buckets, pending };
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

function contextWindowForModel(model: string, overrides?: Record<string, number>): number {
  const normalized = model.trim().toLowerCase();
  if (overrides && normalized in overrides) return overrides[normalized];
  if (/gemini/.test(normalized)) return 1_000_000;
  if (/deepseek/.test(normalized)) return 1_048_576;
  if (/grok/.test(normalized)) return 256_000;
  if (/claude/.test(normalized)) return 200_000;
  if (/gpt-5/.test(normalized) || /gpt-4\.1/.test(normalized)) return 272_000;
  if (/qwen/.test(normalized) || /kimi/.test(normalized) || /mimo/.test(normalized)) return 256_000;
  return DEFAULT_CONTEXT_WINDOW;
}

function normalizeModelContextWindows(value: Record<string, number> | undefined | null): Record<string, number> {
  const result: Record<string, number> = {};
  if (!value) return result;
  for (const [model, window] of Object.entries(value)) {
    const normalized = model.trim().toLowerCase();
    if (!normalized) continue;
    result[normalized] = normalizeBoundedInteger(
      window,
      DEFAULT_CONTEXT_WINDOW,
      MIN_CONTEXT_WINDOW_TOKENS,
      MAX_CONTEXT_WINDOW_TOKENS,
    );
  }
  return result;
}

interface ModelContextWindowEntry {
  id: string;
  model: string;
  window: number;
}

function contextWindowEntriesFromMap(
  value: Record<string, number> | undefined | null,
): ModelContextWindowEntry[] {
  const normalized = normalizeModelContextWindows(value);
  return Object.entries(normalized).map(([model, window], index) => ({
    id: `context-window-${index}`,
    model,
    window,
  }));
}

function contextWindowMapFromEntries(entries: ModelContextWindowEntry[]): Record<string, number> {
  const result: Record<string, number> = {};
  for (const entry of entries) {
    const model = entry.model.trim().toLowerCase();
    if (!model) continue;
    result[model] = normalizeBoundedInteger(
      entry.window,
      DEFAULT_CONTEXT_WINDOW,
      MIN_CONTEXT_WINDOW_TOKENS,
      MAX_CONTEXT_WINDOW_TOKENS,
    );
  }
  return result;
}

/** One editable row of the model routing table; flat so the form stays simple. */
interface ModelRouteEntry {
  id: string;
  model: string;
  providerId: string;
  weight: number;
  enabled: boolean;
  modelOverride: string;
}

function modelRouteEntriesFromMap(
  value: Record<string, ModelRoute[]> | undefined | null,
): ModelRouteEntry[] {
  const entries: ModelRouteEntry[] = [];
  for (const [model, routes] of Object.entries(value || {})) {
    const normalizedModel = model.trim().toLowerCase();
    if (!normalizedModel || !Array.isArray(routes)) continue;
    routes.forEach((route, index) => {
      if (!route || typeof route.provider_id !== "string") return;
      entries.push({
        id: `model-route-${normalizedModel}-${index}`,
        model: normalizedModel,
        providerId: route.provider_id,
        weight: normalizeBoundedInteger(route.weight, 1, MIN_PROVIDER_KEY_WEIGHT, MAX_PROVIDER_KEY_WEIGHT),
        enabled: route.enabled !== false,
        modelOverride: route.model_override?.trim() || "",
      });
    });
  }
  return entries;
}

function modelRouteMapFromEntries(entries: ModelRouteEntry[]): Record<string, ModelRoute[]> {
  const result: Record<string, ModelRoute[]> = {};
  for (const entry of entries) {
    const model = entry.model.trim().toLowerCase();
    const providerId = entry.providerId.trim();
    if (!model || !providerId) continue;
    const routes = result[model] || (result[model] = []);
    // The backend keeps the first route per provider; drop duplicates here so the
    // saved form matches what routing will actually use.
    if (routes.some((route) => route.provider_id === providerId)) continue;
    routes.push({
      provider_id: providerId,
      weight: normalizeBoundedInteger(entry.weight, 1, MIN_PROVIDER_KEY_WEIGHT, MAX_PROVIDER_KEY_WEIGHT),
      enabled: entry.enabled,
      model_override: entry.modelOverride.trim() || undefined,
    });
  }
  return result;
}

interface VisionDelegateForm {
  enabled: boolean;
  providerId: string;
  model: string;
  timeoutSeconds: number;
}

const EMPTY_VISION_DELEGATE_FORM: VisionDelegateForm = {
  enabled: false,
  providerId: "",
  model: "",
  timeoutSeconds: DEFAULT_VISION_TIMEOUT_SECS,
};

function visionDelegateFormFromConfig(
  value: VisionDelegateConfig | undefined | null,
): VisionDelegateForm {
  if (!value) return EMPTY_VISION_DELEGATE_FORM;
  return {
    enabled: value.enabled === true,
    providerId: (value.provider_id || "").trim(),
    model: (value.model || "").trim(),
    timeoutSeconds: normalizeBoundedInteger(
      value.timeout_seconds,
      DEFAULT_VISION_TIMEOUT_SECS,
      MIN_VISION_TIMEOUT_SECS,
      MAX_VISION_TIMEOUT_SECS,
    ),
  };
}

/** Omitted entirely while untouched so config.json stays clean. */
function visionDelegateConfigFromForm(
  form: VisionDelegateForm,
): VisionDelegateConfig | undefined {
  const providerId = form.providerId.trim();
  const model = form.model.trim();
  if (!form.enabled && !providerId && !model) return undefined;
  return {
    enabled: form.enabled,
    provider_id: providerId,
    model,
    timeout_seconds: normalizeBoundedInteger(
      form.timeoutSeconds,
      DEFAULT_VISION_TIMEOUT_SECS,
      MIN_VISION_TIMEOUT_SECS,
      MAX_VISION_TIMEOUT_SECS,
    ),
  };
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

function splitLinkPunctuation(value: string): { url: string; suffix: string } {
  const suffix = value.match(/(?:\*\*|[.,!?;:'"\u2018\u2019\u201C\u201D，。！？；：])+$/)?.[0] || "";
  return { url: suffix ? value.slice(0, -suffix.length) : value, suffix };
}

function renderInlineMarkdown(
  text: string,
  onOpenLink: (url: string) => void,
  keyBase: string,
): ReactNode[] {
  const parts: ReactNode[] = [];
  let cursor = 0;
  let key = 0;
  for (const match of text.matchAll(INLINE_TOKEN_PATTERN)) {
    const start = match.index ?? 0;
    if (start > cursor) parts.push(text.slice(cursor, start));
    const end = start + match[0].length;
    const markdownLabel = match[1];
    const markdownUrl = match[2];
    const code = match[3];
    const bold = match[4];
    const bareUrl = match[5];
    if (markdownUrl) {
      const { url, suffix } = splitLinkPunctuation(markdownUrl);
      if (url) {
        parts.push(
          <a
            key={`${keyBase}-link-${key}`}
            className="assistant-message-link"
            href={url}
            onClick={(event) => {
              event.preventDefault();
              onOpenLink(url);
            }}
          >
            {markdownLabel || url}
          </a>,
        );
        if (suffix) parts.push(suffix);
      } else {
        parts.push(match[0]);
      }
      cursor = end;
    } else if (code) {
      // A model may still fence a URL in backticks. Keep the monospace look but
      // make it open like any other link instead of rendering dead text.
      const fenced = splitLinkPunctuation(code.trim());
      if (BARE_URL_PATTERN.test(fenced.url) && !fenced.suffix) {
        const url = fenced.url;
        parts.push(
          <a
            key={`${keyBase}-code-link-${key}`}
            className="assistant-message-link"
            href={url}
            onClick={(event) => {
              event.preventDefault();
              onOpenLink(url);
            }}
          >
            <code>{url}</code>
          </a>,
        );
      } else {
        parts.push(<code key={`${keyBase}-code-${key}`}>{code}</code>);
      }
      cursor = end;
    } else if (bold) {
      parts.push(
        <strong key={`${keyBase}-strong-${key}`}>
          {renderInlineMarkdown(bold, onOpenLink, `${keyBase}-bold-${key}`)}
        </strong>,
      );
      cursor = end;
    } else if (bareUrl) {
      const { url, suffix } = splitLinkPunctuation(bareUrl);
      if (url) {
        parts.push(
          <a
            key={`${keyBase}-url-${key}`}
            className="assistant-message-link"
            href={url}
            onClick={(event) => {
              event.preventDefault();
              onOpenLink(url);
            }}
          >
            {url}
          </a>,
        );
        if (suffix) parts.push(suffix);
      } else {
        parts.push(match[0]);
      }
      cursor = end;
    } else {
      cursor = end;
    }
    key += 1;
  }
  if (cursor < text.length) parts.push(text.slice(cursor));
  return parts;
}

function renderMarkdownBlocks(content: string, onOpenLink: (url: string) => void): ReactNode[] {
  const nodes: ReactNode[] = [];
  const lines = content.split("\n");
  let index = 0;
  let blockKey = 0;
  while (index < lines.length) {
    const line = lines[index];
    if (!line.trim()) {
      index += 1;
      continue;
    }
    const orderedMatch = ORDERED_ITEM_PATTERN.exec(line);
    const unorderedMatch = !orderedMatch && UNORDERED_ITEM_PATTERN.exec(line);
    if (orderedMatch || unorderedMatch) {
      const itemPattern = orderedMatch ? ORDERED_ITEM_PATTERN : UNORDERED_ITEM_PATTERN;
      const items: string[] = [];
      while (index < lines.length) {
        const current = lines[index];
        if (!current.trim()) break;
        const match = itemPattern.exec(current);
        if (!match) break;
        items.push(match[1]);
        index += 1;
      }
      const list = items.map((item, itemIndex) => (
        <li key={`md-item-${blockKey}-${itemIndex}`}>
          {renderInlineMarkdown(item, onOpenLink, `md-${blockKey}-${itemIndex}`)}
        </li>
      ));
      nodes.push(
        orderedMatch
          ? <ol key={`md-block-${blockKey}`}>{list}</ol>
          : <ul key={`md-block-${blockKey}`}>{list}</ul>,
      );
    } else {
      const paragraph: string[] = [];
      while (index < lines.length) {
        const current = lines[index];
        if (!current.trim()) break;
        if (ORDERED_ITEM_PATTERN.test(current) || UNORDERED_ITEM_PATTERN.test(current)) break;
        paragraph.push(current);
        index += 1;
      }
      nodes.push(
        <p key={`md-block-${blockKey}`}>
          {renderInlineMarkdown(paragraph.join("\n"), onOpenLink, `md-${blockKey}`)}
        </p>,
      );
    }
    blockKey += 1;
  }
  return nodes;
}

const INLINE_TOKEN_PATTERN = /\[([^\]\r\n]+)\]\((https?:\/\/[^\s)]+)\)|`([^`\r\n]+)`|\*\*([^*\r\n]+)\*\*|(https?:\/\/[^\s<>()\]]+)/g;
// Anchored and non-global on purpose: a /g regex keeps lastIndex between tests.
const BARE_URL_PATTERN = /^https?:\/\/[^\s<>()\]]+$/;
const UNORDERED_ITEM_PATTERN = /^\s*[-*]\s+(.*)$/;
const ORDERED_ITEM_PATTERN = /^\s*\d+[.)]\s+(.*)$/;

function AssistantMessageBody({ content, onOpenLink }: { content: string; onOpenLink: (url: string) => void }) {
  if (!content) return <>{content}</>;
  return <>{renderMarkdownBlocks(content, onOpenLink)}</>;
}

function InlineActivityList({ items, locale }: { items: InlineActivityEntry[]; locale: Locale }) {
  if (items.length === 0) return null;
  const groupState = items.some((entry) => entry.state === "running")
    ? "running"
    : items.some((entry) => entry.state === "failed")
      ? "failed"
      : "done";
  const groupStatus = t(
    locale,
    groupState === "running" ? "status.running" : groupState === "failed" ? "status.failed" : "status.done",
  );
  return (
    <details className={`inline-activity-group ${groupState}`}>
      <summary className="inline-activity-group-summary">
        <span className="inline-activity-group-label">{t(locale, "activity.toolCalls", { count: items.length })}</span>
        {groupState === "failed" ? (
          <span className="inline-activity-dot failed" aria-hidden="true">!</span>
        ) : (
          <span className={`inline-activity-dot ${groupState}`} aria-hidden="true" />
        )}
        <span className="inline-activity-group-status">{groupStatus}</span>
      </summary>
      <div className="inline-activity">
        {items.map((entry) => {
          if (entry.kind === "command") {
            return (
              <details className={`inline-activity-item inline-activity-command ${entry.state}`} key={entry.id}>
                <summary>
                  <span className="inline-activity-label">{t(locale, "activity.commandRan")}</span>
                  <code className="inline-activity-command-preview">{entry.command}</code>
                  {entry.state === "running" ? <span className="inline-activity-dot running" aria-label="running" /> : null}
                  {entry.state === "failed" ? <span className="inline-activity-dot failed" aria-hidden="true">!</span> : null}
                </summary>
                <div className="inline-activity-detail">
                  <code>{entry.command}</code>
                  {entry.summary ? <p className="inline-activity-summary">{entry.summary}</p> : null}
                </div>
              </details>
            );
          }
          return (
            <div className={`inline-activity-item inline-activity-file ${entry.state}`} key={entry.id} title={entry.path}>
              <span className="inline-activity-label">{entry.label}</span>
              <code className="inline-activity-file-path">{entry.path}</code>
              {(entry.added ?? 0) > 0 || (entry.removed ?? 0) > 0 ? (
                <span className="inline-activity-delta">
                  {(entry.added ?? 0) > 0 ? <span className="delta-add">+{entry.added}</span> : null}
                  {(entry.removed ?? 0) > 0 ? <span className="delta-remove">−{entry.removed}</span> : null}
                </span>
              ) : null}
              {entry.state === "running" ? <span className="inline-activity-dot running" aria-label="running" /> : null}
              {entry.state === "failed" ? <span className="inline-activity-dot failed" aria-hidden="true">!</span> : null}
            </div>
          );
        })}
      </div>
    </details>
  );
}

type SettingsTab = "provider" | "resilience" | "context" | "vision" | "personalization" | "plugins" | "defaults";

// Prefer the tool call that is still running, so the hint names what is happening now rather than
// the last thing that finished. Falls back to the most recent entry once nothing is in flight.
function runningActivity(activity: ActivityItem[]): ActivityItem | null {
  for (let index = activity.length - 1; index >= 0; index -= 1) {
    if (activity[index].state === "running") return activity[index];
  }
  return activity.length > 0 ? activity[activity.length - 1] : null;
}

function shortActivityLabel(label: string): string {
  const single = label.replace(/\s+/g, " ").trim();
  return single.length > 72 ? `${single.slice(0, 71)}…` : single;
}

// One-line "current step" hint that lives in the conversation while a turn runs. The full step
// list stays in the run-status popover; this only answers "which step am I on right now".
function RunPlanProgress({
  plan,
  currentIndex,
  phase,
  activity,
  locale,
}: {
  plan: PlanStep[];
  currentIndex: number;
  phase: RunStatus["phase"] | null;
  activity: ActivityItem[];
  locale: Locale;
}) {
  if (phase === null || plan.length === 0 || currentIndex < 0) return null;
  const step = plan[currentIndex];
  if (!step) return null;
  const failed = phase === "failed";
  const current = runningActivity(activity);
  return (
    <p className={`run-plan-progress${failed ? " failed" : ""}`}>
      <span className="run-plan-marker" aria-hidden="true">{failed ? "!" : "●"}</span>
      <span className="run-plan-progress-step">
        {t(locale, "run.stepProgress", { current: String(currentIndex + 1), total: String(plan.length) })}
      </span>
      <span className="run-plan-progress-text">{runPlanStepDescription(step, locale)}</span>
      {current ? (
        <span className={`run-plan-progress-now ${current.state}`} title={current.detail || current.label}>
          {shortActivityLabel(current.label)}
        </span>
      ) : null}
    </p>
  );
}

export function App() {
  useEffect(() => {
    const preventDefaultContextMenu = (event: MouseEvent) => {
      event.preventDefault();
    };
    document.addEventListener("contextmenu", preventDefaultContextMenu, true);
    return () => document.removeEventListener("contextmenu", preventDefaultContextMenu, true);
  }, []);

  const [locale, setLocale] = useState<Locale>(() => loadLocale());
  const [uiFontSize, setUiFontSize] = useState(() => loadUiFontSize());
  const [theme, setTheme] = useState<Theme>(() => loadTheme());
  const [workspaceHome, setWorkspaceHome] = useState("");
  const [workspaceRoot, setWorkspaceRoot] = useState("");
  const workspaceRootRef = useRef("");
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
  const [providerFallbackEnabled, setProviderFallbackEnabled] = useState(false);
  const [maxToolRounds, setMaxToolRounds] = useState(DEFAULT_MAX_TOOL_ROUNDS);
  const [circuitFailureThreshold, setCircuitFailureThreshold] = useState(DEFAULT_CIRCUIT_FAILURE_THRESHOLD);
  const [streamFirstEventTimeoutSecs, setStreamFirstEventTimeoutSecs] = useState(DEFAULT_STREAM_FIRST_EVENT_TIMEOUT_SECS);
  const [streamIdleTimeoutSecs, setStreamIdleTimeoutSecs] = useState(DEFAULT_STREAM_IDLE_TIMEOUT_SECS);
  const [nonStreamTimeoutSecs, setNonStreamTimeoutSecs] = useState(DEFAULT_NON_STREAM_TIMEOUT_SECS);
  const [circuitRecoverySuccessThreshold, setCircuitRecoverySuccessThreshold] = useState(DEFAULT_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD);
  const [circuitRecoveryWaitSecs, setCircuitRecoveryWaitSecs] = useState(DEFAULT_CIRCUIT_RECOVERY_WAIT_SECS);
  const [circuitErrorRateThresholdPercent, setCircuitErrorRateThresholdPercent] = useState(DEFAULT_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT);
  const [circuitMinRequestCount, setCircuitMinRequestCount] = useState(DEFAULT_CIRCUIT_MIN_REQUEST_COUNT);
  const [contextCompactionThresholdPercent, setContextCompactionThresholdPercent] = useState(DEFAULT_CONTEXT_COMPACTION_THRESHOLD_PERCENT);
  const [modelContextWindowEntries, setModelContextWindowEntries] = useState<ModelContextWindowEntry[]>([]);
  const [modelRouteEntries, setModelRouteEntries] = useState<ModelRouteEntry[]>([]);
  const [modelRouteStatuses, setModelRouteStatuses] = useState<ModelRouteStatus[]>([]);
  const [visionDelegate, setVisionDelegate] = useState<VisionDelegateForm>(EMPTY_VISION_DELEGATE_FORM);
  // Kept verbatim so saving from Settings never drops hand-written entries.
  const [modelCapabilities, setModelCapabilities] = useState<Record<string, ModelCapabilities>>({});
  const [customInstructions, setCustomInstructions] = useState("");
  const [personality, setPersonality] = useState(DEFAULT_PERSONALITY);
  const [localMemoryEnabled, setLocalMemoryEnabled] = useState(false);
  const [toolMemoryEnabled, setToolMemoryEnabled] = useState(true);
  const [localMemoryCount, setLocalMemoryCount] = useState<number | null>(null);
  const [isClearingMemories, setIsClearingMemories] = useState(false);
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
  const autoRestoredSessionRef = useRef(false);
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
  // Epoch barrier: bumped on every user-initiated composer reset (new task / new chat / project switch).
  // Background sessions must not steal focus from a composer opened in a newer epoch.
  const composerEpochRef = useRef(0);
  // Epoch the in-flight draft turn was started in; null when no draft turn is running.
  const draftEpochRef = useRef<number | null>(null);
  // Sessions that already existed when the current draft turn started. A brand-new task's id
  // cannot be in this set, so events from older running tasks are rejected as focus candidates.
  const draftKnownSessionIdsRef = useRef<Set<string>>(new Set());
  // Monotonic counter shared by draft + session turns so draft→session handoff cannot collide with steer gens.
  const chatGenerationMonoRef = useRef(0);
  const streamedTextBySessionRef = useRef<Map<string, string>>(new Map());
  const drainFollowUpsBySessionRef = useRef<Set<string>>(new Set());
  const activeSessionIdRef = useRef<string | null>(null);
  const sessionsRef = useRef<Session[]>([]);
  const workspaceModeRevisionRef = useRef(0);
  const workspaceModeSaveChainRef = useRef<Promise<void>>(Promise.resolve());

  const updateWorkspaceRoot = useCallback((nextRoot: string): void => {
    const normalized = nextRoot.trim();
    workspaceRootRef.current = normalized;
    setWorkspaceRoot(normalized);
  }, []);
  const [messages, setMessages] = useState<Message[]>([]);
  const [streamedText, setStreamedText] = useState("");
  const [streamedTextBySession, setStreamedTextBySession] = useState<Record<string, string>>({});
  const [plan, setPlan] = useState<PlanStep[]>([]);
  const [activity, setActivity] = useState<ActivityItem[]>([]);
  const [inlineActivityBySession, setInlineActivityBySession] = useState<Record<string, InlineActivityEntry[]>>({});
  const [pendingAction, setPendingAction] = useState<PendingAction | null>(null);
  const [approvalSummary, setApprovalSummary] = useState<string | null>(null);
  const [patchPreview, setPatchPreview] = useState<PatchPreview | null>(null);
  const [rememberLocalApiApproval, setRememberLocalApiApproval] = useState(false);
  const [restorePoints, setRestorePoints] = useState<RestorePoint[]>([]);
  const [taskSummary, setTaskSummary] = useState<TaskSummary | null>(null);
  const [replaySteps, setReplaySteps] = useState<ReplayStep[]>([]);
  const [providerStatus, setProviderStatus] = useState<ProviderAuthStatus | null>(null);
  const [providerKeyStatuses, setProviderKeyStatuses] = useState<ProviderKeyStatus[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [modelCallLogs, setModelCallLogs] = useState<ModelCallLog[]>([]);
  const [modelCallLogsLoading, setModelCallLogsLoading] = useState(false);
  const [modelCallLogsError, setModelCallLogsError] = useState<string | null>(null);
  const [runningSessionIds, setRunningSessionIds] = useState<string[]>([]);
  // Tasks that completed while the user was not looking at them; drives the sidebar green dot.
  const [completedUnseenSessionIds, setCompletedUnseenSessionIds] = useState<string[]>([]);
  const [draftRunning, setDraftRunning] = useState(false);
  const [runStatusBySession, setRunStatusBySession] = useState<Record<string, RunStatus>>({});
  const [draftRunStatus, setDraftRunStatus] = useState<RunStatus | null>(null);
  const [runStatusExpanded, setRunStatusExpanded] = useState(false);
  const [runStatusClock, setRunStatusClock] = useState(() => Date.now());
  const [conversationAtBottom, setConversationAtBottom] = useState(true);
  const [isSavingConfig, setIsSavingConfig] = useState(false);
  const [view, setView] = useState<"workbench" | "settings" | "model-logs">("workbench");
  const [providers, setProviders] = useState<CloudProviderConfig[]>([
    { id: "default", name: defaultProvider, base_url: DEFAULT_PROVIDER_BASE_URL, wire_api: "chat_completions", trust_level: "relay" },
  ]);
  const [activeProviderId, setActiveProviderId] = useState("default");
  const [selectedProviderId, setSelectedProviderId] = useState("default");
  const [showApiKey, setShowApiKey] = useState(false);
  const [diagnosticsOpen, setDiagnosticsOpen] = useState(false);
  const [settingsTab, setSettingsTab] = useState<SettingsTab>("provider");
  const [pluginItems, setPluginItems] = useState<LocalPluginItem[]>([]);
  const [pluginFilter, setPluginFilter] = useState<"all" | "mcp" | "skill">("all");
  const [pluginSearch, setPluginSearch] = useState("");
  const [pluginLoading, setPluginLoading] = useState(false);
  const [pluginNotice, setPluginNotice] = useState<string | null>(null);
  const [pluginEditorOpen, setPluginEditorOpen] = useState(false);
  const [mcpName, setMcpName] = useState("");
  const [mcpCommand, setMcpCommand] = useState("");
  const [mcpArgs, setMcpArgs] = useState("");
  const [mcpEnv, setMcpEnv] = useState("");
  const [userConfigReady, setUserConfigReady] = useState(false); // used to delay hydration
  const conversationRef = useRef<HTMLDivElement | null>(null);
  const conversationAtBottomRef = useRef(true);
  const pendingConversationScrollTopRef = useRef<number | null>(null);
  const pendingConversationScrollToBottomRef = useRef(false);
  const envButtonRef = useRef<HTMLButtonElement | null>(null);
  const [bottomPanelOpen, setBottomPanelOpen] = useState(false);
  // Right panel state is per session: the tools panel follows the task it was opened in.
  const [rightPanelOpenBySession, setRightPanelOpenBySession] = useState<Record<string, boolean>>({});
  const [rightPanelTabBySession, setRightPanelTabBySession] = useState<Record<string, ToolPanelTab>>({});
  const [browserNavigationBySession, setBrowserNavigationBySession] = useState<Record<string, BrowserNavigationRequest>>({});
  const [browserStateBySession, setBrowserStateBySession] = useState<Record<string, BrowserSessionState>>({});
  const [rightPanelWidth, setRightPanelWidth] = useState(() => loadRightPanelWidth());
  const [envPopoverOpen, setEnvPopoverOpen] = useState(false);
  const [contextUsageOpen, setContextUsageOpen] = useState(false);
  const [compactedMessageCount, setCompactedMessageCount] = useState(0);
  const [contextCompactionSummary, setContextCompactionSummary] = useState("");

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
  const { open: rightPanelOpen, tab: rightPanelTab } = rightPanelStateFor<ToolPanelTab>(
    activeSessionId,
    rightPanelOpenBySession,
    rightPanelTabBySession,
    "review",
  );
  const activeSessionStateKey = sessionStateKey(activeSessionId);
  const browserNavigation = browserNavigationBySession[activeSessionStateKey] ?? null;
  const browserState = browserStateBySession[activeSessionStateKey] ?? null;
  const completedRunElapsed = useMemo(
    () => completedRunElapsedByMessageId(messages),
    [messages],
  );
  const currentInlineActivity = inlineActivityBySession[activeSessionId || ""] || [];
  const { buckets: inlineActivityBuckets, pending: pendingInlineActivity } = useMemo(
    () => splitInlineActivityByMessage(messages, currentInlineActivity),
    [messages, currentInlineActivity],
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
  const providerKeyStatusById = useMemo(() => {
    const map = new Map<string, ProviderKeyStatus>();
    for (const status of providerKeyStatuses) {
      map.set(`${status.provider_id}|${status.key_id}`, status);
    }
    return map;
  }, [providerKeyStatuses]);
  const modelRouteStatusByKey = useMemo(() => {
    const map = new Map<string, ModelRouteStatus>();
    for (const status of modelRouteStatuses) {
      map.set(`${status.model}|${status.provider_id}`, status);
    }
    return map;
  }, [modelRouteStatuses]);
  const selectedProviderModelsLoading = selectedProvider ? Boolean(providerModelsLoadingById[selectedProvider.id]) : false;
  const selectedProviderModelsError = selectedProvider ? providerModelErrorsById[selectedProvider.id] : undefined;

  function updateProvider(id: string, patch: Partial<CloudProviderConfig>): void {
    setProviders((current) => current.map((item) => (item.id === id ? { ...item, ...patch } : item)));
  }

  function updateProviderKeys(
    id: string,
    update: (keys: ProviderApiKey[]) => ProviderApiKey[],
  ): void {
    setProviders((current) => current.map((item) => (
      item.id === id ? { ...item, api_keys: update(item.api_keys || []) } : item
    )));
  }

  function addProviderKey(id: string): void {
    updateProviderKeys(id, (keys) => [
      ...keys,
      { id: providerKeyId(), label: "", key: "", weight: 1, enabled: true },
    ]);
  }

  function updateProviderKey(providerId: string, keyId: string, patch: Partial<ProviderApiKey>): void {
    updateProviderKeys(providerId, (keys) => keys.map((item) => (
      item.id === keyId ? { ...item, ...patch } : item
    )));
  }

  function deleteProviderKey(providerId: string, keyId: string): void {
    updateProviderKeys(providerId, (keys) => keys.filter((item) => item.id !== keyId));
  }

  function addProvider(): void {
    const id = providerId();
    setProviders((current) => [
      ...current,
      { id, name: `Provider ${current.length + 1}`, base_url: DEFAULT_PROVIDER_BASE_URL, wire_api: "chat_completions" },
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
  const modelContextWindows = useMemo(
    () => contextWindowMapFromEntries(modelContextWindowEntries),
    [modelContextWindowEntries],
  );
  const contextUsage = useMemo(() => {
    const limit = contextWindowForModel(model, modelContextWindows);
    const retainedMessages = messages.slice(Math.min(compactedMessageCount, messages.length));
    const used = SYSTEM_CONTEXT_TOKEN_RESERVE
      + (contextCompactionSummary ? estimateTextTokens(contextCompactionSummary) : 0)
      + retainedMessages.reduce((total, message) => total + estimateMessageTokens(message), 0)
      + estimateTextTokens(streamedText)
      + estimateTextTokens(prompt)
      + composerImages.length * IMAGE_CONTEXT_TOKEN_ESTIMATE;
    return {
      limit,
      percent: Math.min(100, Math.round((used / limit) * 100)),
      used,
    };
  }, [compactedMessageCount, composerImages.length, contextCompactionSummary, messages, model, modelContextWindows, prompt, streamedText]);

  const filteredPluginItems = useMemo(() => {
    const query = pluginSearch.trim().toLowerCase();
    return pluginItems.filter((item) => {
      if (pluginFilter !== "all" && item.kind !== pluginFilter) return false;
      if (!query) return true;
      return `${item.name} ${item.description} ${item.source}`.toLowerCase().includes(query);
    });
  }, [pluginFilter, pluginItems, pluginSearch]);

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
    applyTheme(theme);
    saveTheme(theme);
  }, [theme]);

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
        setProviderFallbackEnabled(config.provider_fallback_enabled === true);
        setMaxToolRounds(normalizeBoundedInteger(config.max_tool_rounds, DEFAULT_MAX_TOOL_ROUNDS, MIN_MAX_TOOL_ROUNDS, MAX_MAX_TOOL_ROUNDS));
        setCircuitFailureThreshold(normalizeBoundedInteger(config.circuit_failure_threshold, DEFAULT_CIRCUIT_FAILURE_THRESHOLD, MIN_CIRCUIT_FAILURE_THRESHOLD, MAX_CIRCUIT_FAILURE_THRESHOLD));
        setStreamFirstEventTimeoutSecs(normalizeBoundedInteger(config.stream_first_event_timeout_secs, DEFAULT_STREAM_FIRST_EVENT_TIMEOUT_SECS, MIN_STREAM_FIRST_EVENT_TIMEOUT_SECS, MAX_STREAM_FIRST_EVENT_TIMEOUT_SECS));
        setStreamIdleTimeoutSecs(normalizeBoundedInteger(config.stream_idle_timeout_secs, DEFAULT_STREAM_IDLE_TIMEOUT_SECS, MIN_STREAM_IDLE_TIMEOUT_SECS, MAX_STREAM_IDLE_TIMEOUT_SECS));
        setNonStreamTimeoutSecs(normalizeBoundedInteger(config.non_stream_timeout_secs, DEFAULT_NON_STREAM_TIMEOUT_SECS, MIN_NON_STREAM_TIMEOUT_SECS, MAX_NON_STREAM_TIMEOUT_SECS));
        setCircuitRecoverySuccessThreshold(normalizeBoundedInteger(config.circuit_recovery_success_threshold, DEFAULT_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD, MIN_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD, MAX_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD));
        setCircuitRecoveryWaitSecs(normalizeBoundedInteger(config.circuit_recovery_wait_secs, DEFAULT_CIRCUIT_RECOVERY_WAIT_SECS, MIN_CIRCUIT_RECOVERY_WAIT_SECS, MAX_CIRCUIT_RECOVERY_WAIT_SECS));
        setCircuitErrorRateThresholdPercent(normalizeBoundedInteger(config.circuit_error_rate_threshold_percent, DEFAULT_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT, MIN_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT, MAX_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT));
        setCircuitMinRequestCount(normalizeBoundedInteger(config.circuit_min_request_count, DEFAULT_CIRCUIT_MIN_REQUEST_COUNT, MIN_CIRCUIT_MIN_REQUEST_COUNT, MAX_CIRCUIT_MIN_REQUEST_COUNT));
        setContextCompactionThresholdPercent(normalizeBoundedInteger(config.context_compaction_threshold_percent, DEFAULT_CONTEXT_COMPACTION_THRESHOLD_PERCENT, MIN_CONTEXT_COMPACTION_THRESHOLD_PERCENT, MAX_CONTEXT_COMPACTION_THRESHOLD_PERCENT));
        setModelContextWindowEntries(contextWindowEntriesFromMap(config.model_context_windows));
        setModelRouteEntries(modelRouteEntriesFromMap(config.model_routes));
        setVisionDelegate(visionDelegateFormFromConfig(config.vision_delegate));
        setModelCapabilities(config.model_capabilities ?? {});
        setCustomInstructions(config.custom_instructions ?? "");
        setPersonality(normalizePersonality(config.personality));
        setLocalMemoryEnabled(config.local_memory_enabled === true);
        setToolMemoryEnabled(config.tool_memory_enabled !== false);
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
          updateWorkspaceRoot(config.last_workspace_root);
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
  }, [updateWorkspaceRoot]);

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

  const refreshProviderKeyStatuses = useCallback(async () => {
    if (!isTauriRuntime) return;
    try {
      const statuses = await invoke<ProviderKeyStatus[]>("provider_key_status");
      setProviderKeyStatuses(Array.isArray(statuses) ? statuses : []);
    } catch {
      // Rotation stats are advisory; a failure here must not block the settings view.
      setProviderKeyStatuses([]);
    }
  }, []);

  const refreshModelRouteStatuses = useCallback(async () => {
    if (!isTauriRuntime) return;
    try {
      const statuses = await invoke<ModelRouteStatus[]>("model_route_status");
      setModelRouteStatuses(Array.isArray(statuses) ? statuses : []);
    } catch {
      // Same as key stats: routing state is advisory and must not block settings.
      setModelRouteStatuses([]);
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
    const configuredKey = primaryProviderKey(provider);
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
    const modeRevision = workspaceModeRevisionRef.current;
    try {
      const config = await invoke<WorkspaceConfig>("workspace_config", { workspaceRoot: root });
      if (workspaceRootRef.current !== root || workspaceModeRevisionRef.current !== modeRevision) return;
      setMode(config.mode);
      // Model lives in the composer / user config. Workspace defaults must not overwrite it
      // (unconfigured workspaces return an empty model and would clear the picker).
      setCommandAllowlistText(formatCommandAllowlistText(config.command_allowlist));
      setCommandDenylistText(formatCommandDenylistText(config.command_denylist));
    } catch (cause) {
      if (workspaceRootRef.current === root && workspaceModeRevisionRef.current === modeRevision) {
        setError(cause instanceof Error ? cause.message : String(cause));
      }
    }
  }, [workspaceRoot]);

  const loadLocalMemoryCount = useCallback(async () => {
    const root = workspaceRoot.trim();
    if (!isTauriRuntime || !root) {
      setLocalMemoryCount(null);
      return;
    }
    try {
      const count = await invoke<number>("count_local_memories", { workspaceRoot: root });
      setLocalMemoryCount(count);
    } catch (cause) {
      console.error("Failed to load memory count:", cause);
      setLocalMemoryCount(null);
    }
  }, [workspaceRoot]);

  const loadLocalPlugins = useCallback(async () => {
    if (!isTauriRuntime) {
      setPluginItems([]);
      return;
    }
    setPluginLoading(true);
    setPluginNotice(null);
    try {
      const items = await invoke<LocalPluginItem[]>("list_local_plugins", {
        workspaceRoot: workspaceRoot.trim() || null,
      });
      setPluginItems(items);
    } catch (cause) {
      setPluginNotice(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setPluginLoading(false);
    }
  }, [workspaceRoot]);

  const toggleLocalPlugin = useCallback(async (item: LocalPluginItem) => {
    if (!isTauriRuntime) return;
    setPluginNotice(null);
    try {
      await invoke("set_plugin_enabled", {
        kind: item.kind,
        source: item.source,
        name: item.name,
        enabled: !item.enabled,
      });
      setPluginItems((current) => current.map((entry) =>
        entry.id === item.id
          ? { ...entry, enabled: !item.enabled, status: !item.enabled ? "enabled" : "disabled" }
          : entry,
      ));
      setPluginNotice(t(locale, "settings.plugins.nextTurn"));
    } catch (cause) {
      setPluginNotice(cause instanceof Error ? cause.message : String(cause));
    }
  }, [locale]);

  const importLocalSkill = useCallback(async () => {
    if (!isTauriRuntime) {
      setPluginNotice(t(locale, "error.tauriOnly"));
      return;
    }
    setPluginNotice(null);
    try {
      const sourcePath = await invoke<string | null>("pick_directory", {
        title: t(locale, "settings.plugins.skillFolder"),
      });
      if (!sourcePath?.trim()) return;
      await invoke("import_local_skill", { sourcePath: sourcePath.trim() });
      await loadLocalPlugins();
      setPluginNotice(t(locale, "settings.plugins.nextTurn"));
    } catch (cause) {
      setPluginNotice(cause instanceof Error ? cause.message : String(cause));
    }
  }, [loadLocalPlugins, locale]);

  const saveLocalMcp = useCallback(async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!isTauriRuntime) {
      setPluginNotice(t(locale, "error.tauriOnly"));
      return;
    }
    const args = mcpArgs.split(/\r?\n/).map((value) => value.trim()).filter(Boolean);
    const env: Record<string, string> = {};
    for (const line of mcpEnv.split(/\r?\n/).map((value) => value.trim()).filter(Boolean)) {
      const separator = line.indexOf("=");
      const key = separator > 0 ? line.slice(0, separator).trim() : "";
      if (separator <= 0 || !/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) {
        setPluginNotice(t(locale, "settings.plugins.invalidEnv"));
        return;
      }
      env[key] = line.slice(separator + 1);
    }
    setPluginNotice(null);
    try {
      await invoke("add_local_mcp", {
        name: mcpName.trim(),
        command: mcpCommand.trim(),
        args,
        env,
      });
      setMcpName("");
      setMcpCommand("");
      setMcpArgs("");
      setMcpEnv("");
      setPluginEditorOpen(false);
      await loadLocalPlugins();
      setPluginNotice(t(locale, "settings.plugins.nextTurn"));
    } catch (cause) {
      setPluginNotice(cause instanceof Error ? cause.message : String(cause));
    }
  }, [loadLocalPlugins, locale, mcpArgs, mcpCommand, mcpEnv, mcpName]);

  const refreshDiskProjects = useCallback(async () => {
    void 0;
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

  const refreshSessions = useCallback(async (options: { autoRestore?: boolean } = {}) => {
    if (!isTauriRuntime) return;
    try {
      const nextSessions = await invoke<Session[]>("list_sessions", { workspaceRoot: null });
      setSessions(nextSessions);
      if (options.autoRestore && !autoRestoredSessionRef.current) {
        autoRestoredSessionRef.current = true;
        if (nextSessions.length > 0) {
          setActiveSessionId(nextSessions[0].id);
        }
      }
    } catch {
      // Keep the last known session list if refresh fails.
    }
  }, []);


  const refreshWorkspace = useCallback(async () => {
    await refreshDiskProjects();
    await Promise.all([refreshSessions({ autoRestore: true }), loadWorkspaceConfig(), refreshProviderStatus()]);
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

  useEffect(() => {
    if (view === "settings" && settingsTab === "personalization") {
      void loadLocalMemoryCount();
    }
  }, [view, settingsTab, workspaceRoot, loadLocalMemoryCount]);

  useEffect(() => {
    if (view !== "settings" || settingsTab !== "provider") return;
    void refreshProviderKeyStatuses();
    void refreshModelRouteStatuses();
    // Cooldown counters tick down locally, so poll while the pane is visible.
    const timer = window.setInterval(() => {
      void refreshProviderKeyStatuses();
      void refreshModelRouteStatuses();
    }, 5000);
    return () => window.clearInterval(timer);
  }, [view, settingsTab, refreshProviderKeyStatuses, refreshModelRouteStatuses]);

  useEffect(() => {
    if (view === "settings" && settingsTab === "plugins") {
      void loadLocalPlugins();
    }
  }, [view, settingsTab, loadLocalPlugins]);

  const hydrateSession = useCallback(async (sessionId: string) => {
    if (!isTauriRuntime) return;
    try {
      const detail = await invoke<SessionDetail>("session_detail", { sessionId });
      const pending = detail.session.status === "need_user"
        ? detail.pending_actions.find((action) => action.status === "pending") ?? null
        : null;
      setMessages(detail.messages);
      const latestCompaction = [...detail.events]
        .reverse()
        .map((item) => item.event)
        .find((event): event is Extract<SessionEvent, { type: "context_compacted" }> => event.type === "context_compacted");
      setCompactedMessageCount(latestCompaction?.compacted_message_count ?? 0);
      setContextCompactionSummary(latestCompaction?.summary ?? "");
      const liveStream = streamedTextBySessionRef.current.get(sessionId) ?? "";
      setStreamedText(liveStream);
      setPlan(latestPlan(detail.events));
      let activityEvents = detail.events;
      if (detail.session.status === "running" || detail.session.status === "need_user") {
        let previousRunEnd = -1;
        for (let index = detail.events.length - 1; index >= 0; index -= 1) {
          const type = detail.events[index].event.type;
          if (type === "session_cancelled" || type === "task_completed" || type === "error") {
            previousRunEnd = index;
            break;
          }
        }
        activityEvents = detail.events.slice(previousRunEnd + 1);
      }
      setActivity(buildActivity(activityEvents, locale));
      setInlineActivityBySession((current) => ({ ...current, [sessionId]: buildInlineActivity(detail.events, locale) }));
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
          // A new turn supersedes the previous completion signal.
          setCompletedUnseenSessionIds((current) => clearSessionCompletedUnseen(current, sid));
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
      // Also never adopt the composer on behalf of a task the user did not just start: the
      // draft turn must belong to the current composer epoch, and the session must be one this
      // turn created (older running tasks are already in the pre-turn snapshot).
      if (
        !activeSessionIdRef.current
        && draftEpochRef.current === composerEpochRef.current
        && !draftKnownSessionIdsRef.current.has(sid)
      ) {
        setActiveSessionId(sid);
        adoptDraftRightPanelState(sid);
      }

      if (payload.type === "text_delta") {
        markRunning(true);
        patchStream((current) => current + payload.delta);
        patchRunStatus((current) => current?.phase === "retrying"
          ? { ...current, phase: "thinking", detail: undefined }
          : (current ?? null));
      }
      if (payload.type === "context_compacted" && isActive) {
        setCompactedMessageCount(payload.compacted_message_count);
        setContextCompactionSummary(payload.summary);
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
      if (payload.type === "stream_reset") {
        // The interrupted attempt's text was never persisted; drop it so the
        // restarted answer does not render on top of a half-finished one.
        markRunning(true);
        clearStream();
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
        // Only a task the user is not currently viewing needs the sidebar completion dot.
        if (!isActive) {
          setCompletedUnseenSessionIds((current) => markSessionCompletedUnseen(current, sid));
        }
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
      if (payload.type === "tool_start" || payload.type === "tool_end") {
        if (payload.tool_call.name === "apply_patch" || payload.tool_call.name === "run_command") {
          const state: InlineActivityEntry["state"] = payload.type === "tool_start"
            ? "running"
            : payload.success ? "done" : "failed";
          setInlineActivityBySession((current) => ({
            ...current,
            [sid]: upsertInlineActivity(current[sid] || [], payload.tool_call, new Date().toISOString(), state, payload.summary, locale),
          }));
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
      updateWorkspaceRoot(next);
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
    setCompletedUnseenSessionIds((current) => clearSessionCompletedUnseen(current, session.id));
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
      updateWorkspaceRoot(root);
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

  function setRightPanelState(open: boolean, tab?: ToolPanelTab): void {
    // Read the ref so callbacks registered once (shortcuts) still target the visible session.
    const key = sessionStateKey(activeSessionIdRef.current);
    setRightPanelOpenBySession((current) => ({ ...current, [key]: open }));
    if (tab) {
      setRightPanelTabBySession((current) => ({ ...current, [key]: tab }));
    }
  }

  function adoptDraftRightPanelState(sessionId: string): void {
    setRightPanelOpenBySession((current) => adoptDraftSessionKey(current, sessionId));
    setRightPanelTabBySession((current) => adoptDraftSessionKey(current, sessionId));
    setBrowserNavigationBySession((current) => adoptDraftSessionKey(current, sessionId));
    setBrowserStateBySession((current) => adoptDraftSessionKey(current, sessionId));
    // Hand the draft's live webview to the created session instead of rebuilding it.
    void browserAdoptSession(DRAFT_SESSION_KEY, sessionStateKey(sessionId)).catch(() => undefined);
  }

  function openRightPanel(tab: ToolPanelTab): void {
    setRightPanelState(true, tab);
    setEnvPopoverOpen(false);
  }

  function openAssistantLink(url: string): void {
    const key = sessionStateKey(activeSessionIdRef.current);
    setBrowserNavigationBySession((current) => ({
      ...current,
      [key]: { url, id: (current[key]?.id ?? 0) + 1 },
    }));
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
    const key = sessionStateKey(activeSessionIdRef.current);
    setRightPanelOpenBySession((current) => ({ ...current, [key]: current[key] !== true }));
  }

  function resetComposerSession(): void {
    // Leave other sessions running in the background; only clear the composer view.
    // Bump the epoch first: turns started before this reset must not adopt the fresh composer.
    composerEpochRef.current += 1;
    setActiveSessionId(null);
    setComposerImages([]);
    setMessages([]);
    setCompactedMessageCount(0);
    setContextCompactionSummary("");
    setStreamedText("");
    // Always clear draft run state. A still-in-flight draft belongs to the previous epoch, and
    // leaving draftRunning=true here would keep the new composer in queue mode and block sending.
    setDraftRunning(false);
    setDraftRunStatus(null);
    setPlan([]);
    setActivity([]);
    setInlineActivityBySession({});
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
      setCompletedUnseenSessionIds((current) => clearSessionCompletedUnseen(current, sessionId));
      setRightPanelOpenBySession((current) => dropSessionKey(current, sessionId));
      setRightPanelTabBySession((current) => dropSessionKey(current, sessionId));
      setBrowserNavigationBySession((current) => dropSessionKey(current, sessionId));
      setBrowserStateBySession((current) => dropSessionKey(current, sessionId));
      // Release the task's native webview so deleted tasks stop holding one.
      void browserClose(sessionStateKey(sessionId)).catch(() => undefined);
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

  const persistWorkspaceMode = useCallback(
    async (nextMode: Mode, previousMode: Mode): Promise<void> => {
      if (!isTauriRuntime) return;
      const root = workspaceRoot.trim();
      if (!root) return;
      const revision = ++workspaceModeRevisionRef.current;
      const save = async (): Promise<void> => {
        try {
          const current = await invoke<WorkspaceConfig>("workspace_config", { workspaceRoot: root });
          const saved = await invoke<WorkspaceConfig>("set_workspace_config", {
            params: {
              workspace_root: root,
              mode: nextMode,
              provider: current.provider,
              model: current.model,
            },
          });
          if (workspaceRootRef.current === root && workspaceModeRevisionRef.current === revision) {
            setMode(saved.mode);
          }
        } catch (cause) {
          if (workspaceRootRef.current === root && workspaceModeRevisionRef.current === revision) {
            setMode(previousMode);
            setError(cause instanceof Error ? cause.message : String(cause));
          }
        }
      };
      const queuedSave = workspaceModeSaveChainRef.current.then(save, save);
      workspaceModeSaveChainRef.current = queuedSave.then(() => undefined, () => undefined);
      await queuedSave;
    },
    [workspaceRoot],
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
            // Claim the current epoch so only this turn may adopt the composer, and snapshot the
            // sessions that already exist so an older running task cannot pose as this new task.
            draftEpochRef.current = composerEpochRef.current;
            draftKnownSessionIdsRef.current = new Set(sessionsRef.current.map((item) => item.id));
            setActiveSessionId(null);
            setMessages([]);
            setCompactedMessageCount(0);
            setContextCompactionSummary("");
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
              setActivity([]);
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
            // An empty composer is only adoptable by the turn that owns the current epoch, so a
            // background task finishing after "new task" cannot hijack the fresh composer.
            const mayAdoptEmptyComposer =
              draftEpochRef.current === composerEpochRef.current
              && !draftKnownSessionIdsRef.current.has(result.session.id);
            if (
              activeSessionIdRef.current === result.session.id
              || (!activeSessionIdRef.current && mayAdoptEmptyComposer)
            ) {
              setActiveSessionId(result.session.id);
              adoptDraftRightPanelState(result.session.id);
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
        // This draft turn is over: it must no longer count as the composer-adoption owner.
        draftEpochRef.current = null;
        draftKnownSessionIdsRef.current = new Set();
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

  // Restores composer content when a send never reached the model, so an
  // interrupted steer cannot silently swallow the user's message.
  function restoreComposerDraft(message: string, images: ChatImageAttachment[]): void {
    setPrompt((current) => (current.trim() ? current : message));
    setComposerImages((current) =>
      current.length > 0
        ? current
        : images.map((image, index) => ({
            id: `restore-${Date.now()}-${index}`,
            mime_type: image.mime_type,
            data_base64: image.data_base64,
            name: image.name,
            previewUrl: `data:${image.mime_type};base64,${image.data_base64}`,
          })),
    );
  }

  async function submit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    await submitComposer();
  }

  async function submitComposer(): Promise<void> {
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
        const sent = await sendChatMessage(message, { steer: true, sessionId: activeSessionId, images });
        if (!sent) restoreComposerDraft(message, images);
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
    const sent = await sendChatMessage(message, { steer: true, sessionId: activeSessionId, images });
    if (!sent) restoreComposerDraft(message, images);
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
        updateWorkspaceRoot("");
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
      updateWorkspaceRoot(result.project.path);
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
      updateWorkspaceRoot(selected);
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
      updateWorkspaceRoot(result.project.path);
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

  async function clearWorkspaceMemories(): Promise<void> {
    const root = workspaceRoot.trim();
    if (!isTauriRuntime || !root || isClearingMemories) return;
    setError(null);
    setIsClearingMemories(true);
    try {
      await invoke<number>("clear_local_memories", { workspaceRoot: root });
      setLocalMemoryCount(0);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setIsClearingMemories(false);
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
    if (visionDelegate.enabled && (!visionDelegate.providerId.trim() || !visionDelegate.model.trim())) {
      setError(t(locale, "error.visionDelegateIncomplete"));
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
        wire_api: item.wire_api === "responses" ? "responses" as const : "chat_completions" as const,
        trust_level: item.trust_level === "local" || item.trust_level === "official" ? item.trust_level : "relay" as const,
        api_key: item.api_key?.trim() || undefined,
        api_keys: normalizeProviderApiKeys(item.api_keys),
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
          provider_fallback_enabled: providerFallbackEnabled,
          max_tool_rounds: normalizeBoundedInteger(maxToolRounds, DEFAULT_MAX_TOOL_ROUNDS, MIN_MAX_TOOL_ROUNDS, MAX_MAX_TOOL_ROUNDS),
          circuit_failure_threshold: normalizeBoundedInteger(circuitFailureThreshold, DEFAULT_CIRCUIT_FAILURE_THRESHOLD, MIN_CIRCUIT_FAILURE_THRESHOLD, MAX_CIRCUIT_FAILURE_THRESHOLD),
          stream_first_event_timeout_secs: normalizeBoundedInteger(streamFirstEventTimeoutSecs, DEFAULT_STREAM_FIRST_EVENT_TIMEOUT_SECS, MIN_STREAM_FIRST_EVENT_TIMEOUT_SECS, MAX_STREAM_FIRST_EVENT_TIMEOUT_SECS),
          stream_idle_timeout_secs: normalizeBoundedInteger(streamIdleTimeoutSecs, DEFAULT_STREAM_IDLE_TIMEOUT_SECS, MIN_STREAM_IDLE_TIMEOUT_SECS, MAX_STREAM_IDLE_TIMEOUT_SECS),
          non_stream_timeout_secs: normalizeBoundedInteger(nonStreamTimeoutSecs, DEFAULT_NON_STREAM_TIMEOUT_SECS, MIN_NON_STREAM_TIMEOUT_SECS, MAX_NON_STREAM_TIMEOUT_SECS),
          circuit_recovery_success_threshold: normalizeBoundedInteger(circuitRecoverySuccessThreshold, DEFAULT_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD, MIN_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD, MAX_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD),
          circuit_recovery_wait_secs: normalizeBoundedInteger(circuitRecoveryWaitSecs, DEFAULT_CIRCUIT_RECOVERY_WAIT_SECS, MIN_CIRCUIT_RECOVERY_WAIT_SECS, MAX_CIRCUIT_RECOVERY_WAIT_SECS),
          circuit_error_rate_threshold_percent: normalizeBoundedInteger(circuitErrorRateThresholdPercent, DEFAULT_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT, MIN_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT, MAX_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT),
          circuit_min_request_count: normalizeBoundedInteger(circuitMinRequestCount, DEFAULT_CIRCUIT_MIN_REQUEST_COUNT, MIN_CIRCUIT_MIN_REQUEST_COUNT, MAX_CIRCUIT_MIN_REQUEST_COUNT),
          context_compaction_threshold_percent: normalizeBoundedInteger(contextCompactionThresholdPercent, DEFAULT_CONTEXT_COMPACTION_THRESHOLD_PERCENT, MIN_CONTEXT_COMPACTION_THRESHOLD_PERCENT, MAX_CONTEXT_COMPACTION_THRESHOLD_PERCENT),
          // Compatibility mirror for the current active provider.
          base_url: selectedProvider.base_url,
          api_key: selectedProvider.api_key,
          providers: normalizedProviders,
          active_provider_id: selectedProvider.id,
          last_workspace_root: root || undefined,
          workspace_home: home || undefined,
          hidden_project_paths: hiddenProjectPaths,
          model_context_windows: contextWindowMapFromEntries(modelContextWindowEntries),
          model_routes: modelRouteMapFromEntries(modelRouteEntries),
          vision_delegate: visionDelegateConfigFromForm(visionDelegate),
          model_capabilities: modelCapabilities,
          custom_instructions: customInstructions.trim() || undefined,
          personality,
          local_memory_enabled: localMemoryEnabled,
          tool_memory_enabled: toolMemoryEnabled,
        } satisfies UserConfig,
      });
      setWorkspaceHome((savedUser.workspace_home || "").trim());
      setMode(savedUser.mode);
      setModel((savedUser.model || "").trim());
      setReasoningEffort(normalizeReasoningEffort(savedUser.reasoning_effort));
      setMaxProviderRetries(normalizeBoundedInteger(savedUser.max_provider_retries, DEFAULT_MAX_PROVIDER_RETRIES, MIN_MAX_PROVIDER_RETRIES, MAX_MAX_PROVIDER_RETRIES));
      setProviderFallbackEnabled(savedUser.provider_fallback_enabled === true);
      setMaxToolRounds(normalizeBoundedInteger(savedUser.max_tool_rounds, DEFAULT_MAX_TOOL_ROUNDS, MIN_MAX_TOOL_ROUNDS, MAX_MAX_TOOL_ROUNDS));
      setCircuitFailureThreshold(normalizeBoundedInteger(savedUser.circuit_failure_threshold, DEFAULT_CIRCUIT_FAILURE_THRESHOLD, MIN_CIRCUIT_FAILURE_THRESHOLD, MAX_CIRCUIT_FAILURE_THRESHOLD));
      setStreamFirstEventTimeoutSecs(normalizeBoundedInteger(savedUser.stream_first_event_timeout_secs, DEFAULT_STREAM_FIRST_EVENT_TIMEOUT_SECS, MIN_STREAM_FIRST_EVENT_TIMEOUT_SECS, MAX_STREAM_FIRST_EVENT_TIMEOUT_SECS));
      setStreamIdleTimeoutSecs(normalizeBoundedInteger(savedUser.stream_idle_timeout_secs, DEFAULT_STREAM_IDLE_TIMEOUT_SECS, MIN_STREAM_IDLE_TIMEOUT_SECS, MAX_STREAM_IDLE_TIMEOUT_SECS));
      setNonStreamTimeoutSecs(normalizeBoundedInteger(savedUser.non_stream_timeout_secs, DEFAULT_NON_STREAM_TIMEOUT_SECS, MIN_NON_STREAM_TIMEOUT_SECS, MAX_NON_STREAM_TIMEOUT_SECS));
      setCircuitRecoverySuccessThreshold(normalizeBoundedInteger(savedUser.circuit_recovery_success_threshold, DEFAULT_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD, MIN_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD, MAX_CIRCUIT_RECOVERY_SUCCESS_THRESHOLD));
      setCircuitRecoveryWaitSecs(normalizeBoundedInteger(savedUser.circuit_recovery_wait_secs, DEFAULT_CIRCUIT_RECOVERY_WAIT_SECS, MIN_CIRCUIT_RECOVERY_WAIT_SECS, MAX_CIRCUIT_RECOVERY_WAIT_SECS));
      setCircuitErrorRateThresholdPercent(normalizeBoundedInteger(savedUser.circuit_error_rate_threshold_percent, DEFAULT_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT, MIN_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT, MAX_CIRCUIT_ERROR_RATE_THRESHOLD_PERCENT));
      setCircuitMinRequestCount(normalizeBoundedInteger(savedUser.circuit_min_request_count, DEFAULT_CIRCUIT_MIN_REQUEST_COUNT, MIN_CIRCUIT_MIN_REQUEST_COUNT, MAX_CIRCUIT_MIN_REQUEST_COUNT));
      setContextCompactionThresholdPercent(normalizeBoundedInteger(savedUser.context_compaction_threshold_percent, DEFAULT_CONTEXT_COMPACTION_THRESHOLD_PERCENT, MIN_CONTEXT_COMPACTION_THRESHOLD_PERCENT, MAX_CONTEXT_COMPACTION_THRESHOLD_PERCENT));
      setModelContextWindowEntries(contextWindowEntriesFromMap(savedUser.model_context_windows));
      setModelRouteEntries(modelRouteEntriesFromMap(savedUser.model_routes));
      setVisionDelegate(visionDelegateFormFromConfig(savedUser.vision_delegate));
      setModelCapabilities(savedUser.model_capabilities ?? {});
      setCustomInstructions(savedUser.custom_instructions ?? "");
      setPersonality(normalizePersonality(savedUser.personality));
      setLocalMemoryEnabled(savedUser.local_memory_enabled === true);
      setToolMemoryEnabled(savedUser.tool_memory_enabled !== false);
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

  function addContextWindowEntry(): void {
    setModelContextWindowEntries((current) => [
      ...current,
      {
        id: `context-window-${Date.now()}-${current.length}`,
        model: "",
        window: DEFAULT_CONTEXT_WINDOW,
      },
    ]);
  }

  function updateContextWindowEntry(
    id: string,
    patch: Partial<Pick<ModelContextWindowEntry, "model" | "window">>,
  ): void {
    setModelContextWindowEntries((current) =>
      current.map((entry) => (entry.id === id ? { ...entry, ...patch } : entry)),
    );
  }

  function removeContextWindowEntry(id: string): void {
    setModelContextWindowEntries((current) => current.filter((entry) => entry.id !== id));
  }

  function addModelRouteEntry(): void {
    setModelRouteEntries((current) => [
      ...current,
      {
        id: `model-route-${Date.now()}-${current.length}`,
        model: model.trim().toLowerCase(),
        providerId: activeProviderId || providers[0]?.id || "",
        weight: 1,
        enabled: true,
        modelOverride: "",
      },
    ]);
  }

  function updateModelRouteEntry(id: string, patch: Partial<Omit<ModelRouteEntry, "id">>): void {
    setModelRouteEntries((current) =>
      current.map((entry) => (entry.id === id ? { ...entry, ...patch } : entry)),
    );
  }

  function removeModelRouteEntry(id: string): void {
    setModelRouteEntries((current) => current.filter((entry) => entry.id !== id));
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
    if (!(event.ctrlKey || event.metaKey) || event.key !== "Enter") return;
    event.preventDefault();
    // Ctrl+Shift+Enter is the only keyboard interrupt; plain Ctrl+Enter must
    // behave exactly like the send button and honour the queue/steer toggle.
    if (event.shiftKey) {
      if (queueMode) void steerCurrentRun();
      return;
    }
    if (sendBlockReason) return;
    void submitComposer();
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
    const settingsTabs: { id: SettingsTab; labelKey: MessageKey }[] = [
      { id: "provider", labelKey: "settings.tab.provider" },
      { id: "resilience", labelKey: "settings.tab.resilience" },
      { id: "context", labelKey: "settings.tab.context" },
      { id: "vision", labelKey: "settings.tab.vision" },
      { id: "personalization", labelKey: "settings.tab.personalization" },
      { id: "plugins", labelKey: "settings.tab.plugins" },
      { id: "defaults", labelKey: "settings.tab.defaults" },
    ];
    const focusSettingsTab = (tab: SettingsTab) => {
      requestAnimationFrame(() => {
        const el = document.getElementById(`settings-tab-${tab}`);
        if (el instanceof HTMLElement) el.focus();
      });
    };
    const handleSettingsTabKeyDown = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
      let nextIndex: number | null = null;
      if (event.key === "ArrowRight" || event.key === "ArrowDown") {
        nextIndex = (index + 1) % settingsTabs.length;
      } else if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
        nextIndex = (index - 1 + settingsTabs.length) % settingsTabs.length;
      } else if (event.key === "Home") {
        nextIndex = 0;
      } else if (event.key === "End") {
        nextIndex = settingsTabs.length - 1;
      }
      if (nextIndex === null) return;
      event.preventDefault();
      const next = settingsTabs[nextIndex].id;
      setSettingsTab(next);
      focusSettingsTab(next);
    };
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
            <label className="settings-locale-control" htmlFor="ui-theme">
              <span>{t(locale, "settings.theme")}</span>
              <select
                id="ui-theme"
                aria-label={t(locale, "settings.theme")}
                value={theme}
                onChange={(event) => setTheme(normalizeTheme(event.target.value))}
              >
                {THEMES.map((option) => (
                  <option key={option} value={option}>
                    {t(locale, `settings.theme.${option}` as MessageKey)}
                  </option>
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

        <nav className="settings-tabs" role="tablist" aria-label={t(locale, "settings.tabsLabel")}>
          {settingsTabs.map((tab, index) => {
            const active = settingsTab === tab.id;
            return (
              <button
                key={tab.id}
                type="button"
                role="tab"
                id={`settings-tab-${tab.id}`}
                className={`settings-tab${active ? " active" : ""}`}
                aria-selected={active}
                aria-controls={`settings-panel-${tab.id}`}
                tabIndex={active ? 0 : -1}
                onClick={() => setSettingsTab(tab.id)}
                onKeyDown={(event) => handleSettingsTabKeyDown(event, index)}
              >
                {t(locale, tab.labelKey)}
              </button>
            );
          })}
        </nav>

        <div className="settings-tabs-container">
          <section
            className="settings-card provider-settings-card"
            role="tabpanel"
            id="settings-panel-provider"
            aria-labelledby="settings-tab-provider"
            aria-label={t(locale, "settings.section.provider")}
            hidden={settingsTab !== "provider"}
          >
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
                  <label className="field-label" htmlFor={`provider-wire-api-${selectedProvider.id}`}>{t(locale, "field.providerProtocol")}</label>
                  <select
                    id={`provider-wire-api-${selectedProvider.id}`}
                    value={selectedProvider.wire_api || "chat_completions"}
                    onChange={(event) => updateProvider(selectedProvider.id, {
                      wire_api: event.target.value === "responses" ? "responses" : "chat_completions",
                    })}
                    disabled={anySessionRunning || isSavingConfig}
                  >
                    <option value="chat_completions">{t(locale, "providerProtocol.chatCompletions")}</option>
                    <option value="responses">{t(locale, "providerProtocol.responses")}</option>
                  </select>
                  <label className="field-label" htmlFor={`provider-trust-level-${selectedProvider.id}`}>{locale === "zh-CN" ? "Provider 信任级别" : "Provider trust level"}</label>
                  <select
                    id={`provider-trust-level-${selectedProvider.id}`}
                    value={selectedProvider.trust_level || "relay"}
                    onChange={(event) => updateProvider(selectedProvider.id, {
                      trust_level: event.target.value === "local" || event.target.value === "official" || event.target.value === "relay"
                        ? event.target.value
                        : "relay",
                    })}
                    disabled={anySessionRunning || isSavingConfig}
                  >
                    <option value="relay">{locale === "zh-CN" ? "中转站（低信任）" : "Relay (untrusted)"}</option>
                    <option value="official">{locale === "zh-CN" ? "官方直连" : "Official direct"}</option>
                    <option value="local">{locale === "zh-CN" ? "本地/私有化" : "Local / private"}</option>
                  </select>
                  <p className="mode-help">{t(locale, "field.providerProtocolHint")}</p>
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
                  {(selectedProvider.api_keys || []).length === 0 ? (
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
                  ) : null}
                  <div className="provider-key-pool" id={`provider-key-pool-${selectedProvider.id}`}>
                    {(selectedProvider.api_keys || []).map((keyEntry, keyIndex) => {
                      const keyStatus = providerKeyStatusById.get(`${selectedProvider.id}|${keyEntry.id}`);
                      const stateLabelKey: MessageKey = keyStatus?.state === "rejected"
                        ? "keyState.rejected"
                        : keyStatus?.state === "rate_limited"
                          ? "keyState.rateLimited"
                          : keyStatus?.state === "unstable"
                            ? "keyState.unstable"
                            : keyStatus?.state === "disabled"
                              ? "keyState.disabled"
                              : "keyState.ready";
                      return (
                      <div className="provider-key-entry" key={keyEntry.id}>
                      <div className={`provider-key-row ${keyEntry.enabled === false ? "disabled" : ""}`}>
                        <input
                          className="provider-key-label"
                          value={keyEntry.label || ""}
                          onChange={(event) => updateProviderKey(selectedProvider.id, keyEntry.id, { label: event.target.value })}
                          disabled={anySessionRunning || isSavingConfig}
                          spellCheck={false}
                          placeholder={t(locale, "field.keyLabelPlaceholder", { index: String(keyIndex + 1) })}
                          aria-label={t(locale, "field.keyLabel")}
                        />
                        <input
                          className="provider-key-secret"
                          type={showApiKey ? "text" : "password"}
                          value={keyEntry.key || ""}
                          onChange={(event) => updateProviderKey(selectedProvider.id, keyEntry.id, { key: event.target.value })}
                          disabled={anySessionRunning || isSavingConfig}
                          spellCheck={false}
                          autoComplete="off"
                          placeholder={t(locale, "field.apiKeyPlaceholder")}
                          aria-label={t(locale, "field.apiKey")}
                        />
                        <input
                          className="provider-key-weight"
                          type="number"
                          min={MIN_PROVIDER_KEY_WEIGHT}
                          max={MAX_PROVIDER_KEY_WEIGHT}
                          step={1}
                          value={keyEntry.weight ?? 1}
                          onChange={(event) => updateProviderKey(selectedProvider.id, keyEntry.id, {
                            weight: normalizeBoundedInteger(Number(event.target.value), 1, MIN_PROVIDER_KEY_WEIGHT, MAX_PROVIDER_KEY_WEIGHT),
                          })}
                          disabled={anySessionRunning || isSavingConfig}
                          aria-label={t(locale, "field.keyWeight")}
                          title={t(locale, "field.keyWeight")}
                        />
                        <label className="provider-key-enabled">
                          <input
                            type="checkbox"
                            checked={keyEntry.enabled !== false}
                            onChange={(event) => updateProviderKey(selectedProvider.id, keyEntry.id, { enabled: event.target.checked })}
                            disabled={anySessionRunning || isSavingConfig}
                          />
                          <span>{t(locale, "field.keyEnabled")}</span>
                        </label>
                        <button
                          type="button"
                          className="quiet-button"
                          onClick={() => deleteProviderKey(selectedProvider.id, keyEntry.id)}
                          disabled={anySessionRunning || isSavingConfig}
                          title={t(locale, "action.deleteApiKey")}
                          aria-label={t(locale, "action.deleteApiKey")}
                        >
                          x
                        </button>
                      </div>
                      {keyStatus ? (
                        <div className={`provider-key-status ${keyStatus.state}`}>
                          <span className="provider-key-state">{t(locale, stateLabelKey)}</span>
                          <span className="provider-key-usage">
                            {t(locale, "keyStats.usage", {
                              ok: String(keyStatus.success_count),
                              fail: String(keyStatus.failure_count),
                            })}
                          </span>
                          {keyStatus.cooldown_secs ? (
                            <span className="provider-key-cooldown">
                              {t(locale, "keyStats.cooldown", { seconds: String(keyStatus.cooldown_secs) })}
                            </span>
                          ) : null}
                        </div>
                      ) : null}
                      </div>
                      );
                    })}
                  </div>
                  <div className="provider-key-pool-actions">
                    <button
                      type="button"
                      className="quiet-button"
                      onClick={() => {
                        const legacy = (selectedProvider.api_key || "").trim();
                        const pool = selectedProvider.api_keys || [];
                        if (pool.length === 0 && legacy) {
                          // Promote the single configured key so the weighted pool starts from
                          // the credential that already works, then add the new empty row.
                          updateProvider(selectedProvider.id, {
                            api_key: undefined,
                            api_keys: [
                              { id: providerKeyId(), label: "", key: legacy, weight: 1, enabled: true },
                              { id: providerKeyId(), label: "", key: "", weight: 1, enabled: true },
                            ],
                          });
                          return;
                        }
                        addProviderKey(selectedProvider.id);
                      }}
                      disabled={anySessionRunning || isSavingConfig}
                    >
                      {t(locale, "action.addApiKey")}
                    </button>
                    <span className="provider-key-pool-hint">{t(locale, "help.apiKeyPool")}</span>
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
                    {entry.id}
                  </span>
                ))}
              </div>
            ) : (
              <small className="provider-model-empty">{t(locale, "models.empty")}</small>
            )}
            {selectedProviderModelsError ? <small className="models-error">{selectedProviderModelsError}</small> : null}
            <div className="model-route-section">
              <p className="model-route-title">{t(locale, "settings.modelRoutes.title")}</p>
              <div className="model-route-list" id="model-route-list">
                {modelRouteEntries.length === 0 ? (
                  <small className="model-route-empty">{t(locale, "settings.modelRoutes.empty")}</small>
                ) : (
                  modelRouteEntries.map((entry) => {
                    const routeStatus = modelRouteStatusByKey.get(
                      `${entry.model.trim().toLowerCase()}|${entry.providerId}`,
                    );
                    const routeStateLabelKey: MessageKey = routeStatus?.state === "disabled"
                      ? "routeState.disabled"
                      : routeStatus?.state === "unknown_provider"
                        ? "routeState.unknownProvider"
                        : routeStatus?.state === "no_credential"
                          ? "routeState.noCredential"
                          : routeStatus?.state === "blocked"
                            ? "routeState.blocked"
                            : routeStatus?.state === "cooling_down"
                              ? "routeState.coolingDown"
                              : routeStatus?.state === "trust_mismatch"
                                ? "routeState.trustMismatch"
                                : "routeState.ready";
                    return (
                      <div className="model-route-entry" key={entry.id}>
                        <div className={`model-route-row ${entry.enabled ? "" : "disabled"}`}>
                          <input
                            className="model-route-model"
                            value={entry.model}
                            onChange={(event) => updateModelRouteEntry(entry.id, { model: event.target.value })}
                            disabled={anySessionRunning || isSavingConfig}
                            spellCheck={false}
                            placeholder={t(locale, "field.routeModelPlaceholder")}
                            aria-label={t(locale, "field.routeModel")}
                          />
                          <select
                            className="model-route-provider"
                            value={entry.providerId}
                            onChange={(event) => updateModelRouteEntry(entry.id, { providerId: event.target.value })}
                            disabled={anySessionRunning || isSavingConfig}
                            aria-label={t(locale, "field.routeProvider")}
                          >
                            {providers.some((item) => item.id === entry.providerId) ? null : (
                              <option value={entry.providerId}>{entry.providerId || "-"}</option>
                            )}
                            {providers.map((item) => (
                              <option value={item.id} key={item.id}>
                                {item.name}
                              </option>
                            ))}
                          </select>
                          <input
                            className="model-route-override"
                            value={entry.modelOverride}
                            onChange={(event) => updateModelRouteEntry(entry.id, { modelOverride: event.target.value })}
                            disabled={anySessionRunning || isSavingConfig}
                            spellCheck={false}
                            placeholder={t(locale, "field.routeOverridePlaceholder")}
                            aria-label={t(locale, "field.routeOverride")}
                          />
                          <input
                            className="model-route-weight"
                            type="number"
                            min={MIN_PROVIDER_KEY_WEIGHT}
                            max={MAX_PROVIDER_KEY_WEIGHT}
                            step={1}
                            value={entry.weight}
                            onChange={(event) => updateModelRouteEntry(entry.id, {
                              weight: normalizeBoundedInteger(Number(event.target.value), 1, MIN_PROVIDER_KEY_WEIGHT, MAX_PROVIDER_KEY_WEIGHT),
                            })}
                            disabled={anySessionRunning || isSavingConfig}
                            aria-label={t(locale, "field.keyWeight")}
                            title={t(locale, "field.keyWeight")}
                          />
                          <label className="model-route-enabled">
                            <input
                              type="checkbox"
                              checked={entry.enabled}
                              onChange={(event) => updateModelRouteEntry(entry.id, { enabled: event.target.checked })}
                              disabled={anySessionRunning || isSavingConfig}
                            />
                            <span>{t(locale, "field.keyEnabled")}</span>
                          </label>
                          <button
                            type="button"
                            className="quiet-button"
                            onClick={() => removeModelRouteEntry(entry.id)}
                            disabled={anySessionRunning || isSavingConfig}
                            title={t(locale, "action.deleteModelRoute")}
                            aria-label={t(locale, "action.deleteModelRoute")}
                          >
                            x
                          </button>
                        </div>
                        {routeStatus ? (
                          <div className={`model-route-status ${routeStatus.state}`}>
                            <span className="model-route-state">{t(locale, routeStateLabelKey)}</span>
                            <span className="model-route-usage">
                              {t(locale, "keyStats.usage", {
                                ok: String(routeStatus.success_count),
                                fail: String(routeStatus.failure_count),
                              })}
                            </span>
                            <span className="model-route-effective">{routeStatus.effective_model}</span>
                          </div>
                        ) : null}
                      </div>
                    );
                  })
                )}
              </div>
              <div className="model-route-actions">
                <button
                  type="button"
                  className="quiet-button"
                  onClick={addModelRouteEntry}
                  disabled={anySessionRunning || isSavingConfig || providers.length === 0}
                >
                  {t(locale, "action.addModelRoute")}
                </button>
              </div>
              <p className="mode-help">{t(locale, "settings.modelRoutes.help")}</p>
            </div>
            <p className="mode-help">{t(locale, "settings.providerHelp")}</p>
          </section>

          <section
            className="settings-card resilience-settings-card"
            role="tabpanel"
            id="settings-panel-resilience"
            aria-labelledby="settings-tab-resilience"
            aria-label={t(locale, "settings.resilience.title")}
            hidden={settingsTab !== "resilience"}
          >
            <p className="panel-title">{t(locale, "settings.resilience.title")}</p>

            <div className="resilience-setting-group">
              <p className="resilience-setting-group-title">{t(locale, "settings.resilience.retryTimeout")}</p>
              <div className="resilience-toggle-row">
                <div>
                  <span className="field-label">{t(locale, "field.providerFallback")}</span>
                  <p className="mode-help">{t(locale, "field.providerFallbackHint")}</p>
                </div>
                <button
                  type="button"
                  className={`browser-toggle${providerFallbackEnabled ? " on" : ""}`}
                  aria-label={t(locale, "field.providerFallback")}
                  aria-pressed={providerFallbackEnabled}
                  onClick={() => setProviderFallbackEnabled((enabled) => !enabled)}
                  disabled={anySessionRunning || isSavingConfig}
                >
                  <span className="browser-toggle-knob" />
                </button>
              </div>
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

          <section
            className="settings-card plugins-settings-card"
            role="tabpanel"
            id="settings-panel-plugins"
            aria-labelledby="settings-tab-plugins"
            aria-label={t(locale, "settings.plugins.title")}
            hidden={settingsTab !== "plugins"}
          >
            <div className="plugins-header">
              <div>
                <p className="panel-title">{t(locale, "settings.plugins.title")}</p>
                <p className="mode-help">{t(locale, "settings.plugins.subtitle")}</p>
              </div>
              <div className="plugins-actions">
                <button type="button" className="quiet-button" onClick={() => void importLocalSkill()} disabled={pluginLoading}>
                  {t(locale, "settings.plugins.importSkill")}
                </button>
                <button type="button" className="primary-button" onClick={() => setPluginEditorOpen((open) => !open)} disabled={pluginLoading}>
                  {t(locale, "settings.plugins.addMcp")}
                </button>
              </div>
            </div>

            {pluginEditorOpen ? (
              <form className="plugin-editor" onSubmit={(event) => void saveLocalMcp(event)}>
                <div className="plugin-editor-grid">
                  <label>
                    <span className="field-label">{t(locale, "settings.plugins.mcpName")}</span>
                    <input value={mcpName} onChange={(event) => setMcpName(event.target.value)} required spellCheck={false} />
                  </label>
                  <label>
                    <span className="field-label">{t(locale, "settings.plugins.mcpCommand")}</span>
                    <input value={mcpCommand} onChange={(event) => setMcpCommand(event.target.value)} required spellCheck={false} />
                  </label>
                  <label>
                    <span className="field-label">{t(locale, "settings.plugins.mcpArgs")}</span>
                    <textarea rows={3} value={mcpArgs} onChange={(event) => setMcpArgs(event.target.value)} spellCheck={false} />
                  </label>
                  <label>
                    <span className="field-label">{t(locale, "settings.plugins.mcpEnv")}</span>
                    <textarea rows={3} value={mcpEnv} onChange={(event) => setMcpEnv(event.target.value)} spellCheck={false} />
                  </label>
                </div>
                <div className="plugin-editor-actions">
                  <button type="button" className="quiet-button" onClick={() => setPluginEditorOpen(false)}>{t(locale, "action.cancel")}</button>
                  <button type="submit" className="primary-button">{t(locale, "action.saveSettings")}</button>
                </div>
              </form>
            ) : null}

            <div className="plugin-toolbar">
              <div className="plugin-filter" role="tablist" aria-label={t(locale, "settings.plugins.title")}>
                {(["all", "mcp", "skill"] as const).map((filter) => (
                  <button
                    key={filter}
                    type="button"
                    className={pluginFilter === filter ? "active" : undefined}
                    aria-selected={pluginFilter === filter}
                    onClick={() => setPluginFilter(filter)}
                  >
                    {t(locale, `settings.plugins.${filter}` as MessageKey)}
                  </button>
                ))}
              </div>
              <input
                className="plugin-search"
                value={pluginSearch}
                onChange={(event) => setPluginSearch(event.target.value)}
                placeholder={t(locale, "settings.plugins.search")}
                aria-label={t(locale, "settings.plugins.search")}
              />
            </div>

            {pluginNotice ? <p className="plugin-notice" role="status">{pluginNotice}</p> : null}
            {pluginLoading ? <p className="mode-help">{t(locale, "settings.plugins.loading")}</p> : null}
            {!pluginLoading && filteredPluginItems.length === 0 ? (
              <p className="plugin-empty">{t(locale, "settings.plugins.empty")}</p>
            ) : (
              <div className="plugin-list" role="list">
                {filteredPluginItems.map((item) => (
                  <div className="plugin-list-item" role="listitem" key={item.id}>
                    <div className={`plugin-icon ${item.kind}`} aria-hidden="true">{item.kind === "mcp" ? "M" : "S"}</div>
                    <div className="plugin-meta">
                      <strong>{item.name}</strong>
                      <span>{item.description}</span>
                      <small className="plugin-source">
                        {item.source === "user" ? t(locale, "settings.plugins.userSource") : t(locale, "settings.plugins.workspaceSource")}
                        {item.kind === "mcp" && item.env_keys?.length ? ` · ${item.env_keys.join(", ")}` : ""}
                      </small>
                    </div>
                    <button
                      type="button"
                      className={`plugin-toggle${item.enabled ? " on" : ""}`}
                      aria-label={`${item.name}: ${item.enabled ? t(locale, "settings.plugins.enabled") : t(locale, "settings.plugins.disabled")}`}
                      aria-pressed={item.enabled}
                      onClick={() => void toggleLocalPlugin(item)}
                      disabled={pluginLoading}
                    >
                      <span className="plugin-toggle-knob" />
                    </button>
                  </div>
                ))}
              </div>
            )}
          </section>

          <section
            className="settings-card context-windows-settings-card"
            role="tabpanel"
            id="settings-panel-context"
            aria-labelledby="settings-tab-context"
            aria-label={t(locale, "settings.contextWindows.title")}
            hidden={settingsTab !== "context"}
          >
            <div className="provider-manager-header">
              <p className="panel-title">{t(locale, "settings.contextWindows.title")}</p>
              <button
                type="button"
                className="quiet-button"
                onClick={addContextWindowEntry}
                disabled={anySessionRunning || isSavingConfig}
              >
                {t(locale, "action.addContextWindow")}
              </button>
            </div>
            <p className="mode-help">{t(locale, "settings.contextWindows.hint")}</p>
            <div className="resilience-setting-grid">
              <label htmlFor="context-compaction-threshold">
                <span className="field-label">{t(locale, "field.contextCompactionThreshold")}</span>
                <input
                  id="context-compaction-threshold"
                  type="number"
                  min={MIN_CONTEXT_COMPACTION_THRESHOLD_PERCENT}
                  max={MAX_CONTEXT_COMPACTION_THRESHOLD_PERCENT}
                  step={1}
                  value={contextCompactionThresholdPercent}
                  onChange={(event) => setContextCompactionThresholdPercent(normalizeBoundedInteger(Number(event.target.value), DEFAULT_CONTEXT_COMPACTION_THRESHOLD_PERCENT, MIN_CONTEXT_COMPACTION_THRESHOLD_PERCENT, MAX_CONTEXT_COMPACTION_THRESHOLD_PERCENT))}
                  disabled={anySessionRunning || isSavingConfig}
                />
              </label>
            </div>
            <div id="context-window-list" className="context-window-list">
              {modelContextWindowEntries.length === 0 ? (
                <p className="mode-help">{t(locale, "settings.contextWindows.empty")}</p>
              ) : (
                modelContextWindowEntries.map((entry) => (
                  <div className="context-window-row" key={entry.id}>
                    <div className="context-window-field">
                      <label className="field-label" htmlFor={`context-window-model-${entry.id}`}>
                        {t(locale, "field.contextWindowModel")}
                      </label>
                      <input
                        id={`context-window-model-${entry.id}`}
                        value={entry.model}
                        onChange={(event) => updateContextWindowEntry(entry.id, { model: event.target.value })}
                        disabled={anySessionRunning || isSavingConfig}
                        spellCheck={false}
                        placeholder={t(locale, "field.contextWindowModelPlaceholder")}
                      />
                    </div>
                    <div className="context-window-field">
                      <label className="field-label" htmlFor={`context-window-tokens-${entry.id}`}>
                        {t(locale, "field.contextWindowTokens")}
                      </label>
                      <input
                        id={`context-window-tokens-${entry.id}`}
                        type="number"
                        min={MIN_CONTEXT_WINDOW_TOKENS}
                        max={MAX_CONTEXT_WINDOW_TOKENS}
                        step={1_024}
                        value={entry.window}
                        onChange={(event) => updateContextWindowEntry(entry.id, { window: Number(event.target.value) })}
                        disabled={anySessionRunning || isSavingConfig}
                      />
                    </div>
                    <button
                      type="button"
                      className="danger-button"
                      onClick={() => removeContextWindowEntry(entry.id)}
                      disabled={anySessionRunning || isSavingConfig}
                    >
                      {t(locale, "action.remove")}
                    </button>
                  </div>
                ))
              )}
            </div>
            <p className="mode-help">{t(locale, "settings.contextWindows.fallbackHint")}</p>
          </section>

          <section
            className="settings-card vision-settings-card"
            role="tabpanel"
            id="settings-panel-vision"
            aria-labelledby="settings-tab-vision"
            aria-label={t(locale, "settings.vision.title")}
            hidden={settingsTab !== "vision"}
          >
            <p className="panel-title">{t(locale, "settings.vision.title")}</p>
            <p className="mode-help">{t(locale, "settings.vision.hint")}</p>
            <div className="resilience-toggle-row">
              <div>
                <span className="field-label">{t(locale, "field.visionDelegateEnabled")}</span>
                <p className="mode-help">{t(locale, "field.visionDelegateEnabledHint")}</p>
              </div>
              <button
                type="button"
                className={`browser-toggle${visionDelegate.enabled ? " on" : ""}`}
                aria-label={t(locale, "field.visionDelegateEnabled")}
                aria-pressed={visionDelegate.enabled}
                onClick={() => setVisionDelegate((current) => ({ ...current, enabled: !current.enabled }))}
                disabled={anySessionRunning || isSavingConfig}
              >
                <span className="browser-toggle-knob" />
              </button>
            </div>
            <div className="resilience-setting-grid">
              <label htmlFor="vision-delegate-provider">
                <span className="field-label">{t(locale, "field.visionDelegateProvider")}</span>
                <select
                  id="vision-delegate-provider"
                  value={visionDelegate.providerId}
                  onChange={(event) => setVisionDelegate((current) => ({ ...current, providerId: event.target.value }))}
                  disabled={anySessionRunning || isSavingConfig}
                >
                  <option value="">{t(locale, "field.visionDelegateProviderUnset")}</option>
                  {providers.map((item) => (
                    <option value={item.id} key={item.id}>
                      {item.name.trim() || "Provider"}
                    </option>
                  ))}
                </select>
              </label>
              <label htmlFor="vision-delegate-model">
                <span className="field-label">{t(locale, "field.visionDelegateModel")}</span>
                <input
                  id="vision-delegate-model"
                  value={visionDelegate.model}
                  onChange={(event) => setVisionDelegate((current) => ({ ...current, model: event.target.value }))}
                  disabled={anySessionRunning || isSavingConfig}
                  spellCheck={false}
                  placeholder={t(locale, "field.visionDelegateModelPlaceholder")}
                />
              </label>
              <label htmlFor="vision-delegate-timeout">
                <span className="field-label">{t(locale, "field.visionDelegateTimeout")}</span>
                <input
                  id="vision-delegate-timeout"
                  type="number"
                  min={MIN_VISION_TIMEOUT_SECS}
                  max={MAX_VISION_TIMEOUT_SECS}
                  step={1}
                  value={visionDelegate.timeoutSeconds}
                  onChange={(event) => setVisionDelegate((current) => ({
                    ...current,
                    timeoutSeconds: normalizeBoundedInteger(
                      Number(event.target.value),
                      DEFAULT_VISION_TIMEOUT_SECS,
                      MIN_VISION_TIMEOUT_SECS,
                      MAX_VISION_TIMEOUT_SECS,
                    ),
                  }))}
                  disabled={anySessionRunning || isSavingConfig}
                />
              </label>
            </div>
            <p className="mode-help">{t(locale, "settings.vision.capabilityHint")}</p>
          </section>

          <section
            className="settings-card resilience-settings-card"
            role="tabpanel"
            id="settings-panel-personalization"
            aria-labelledby="settings-tab-personalization"
            aria-label={t(locale, "settings.personalization.title")}
            hidden={settingsTab !== "personalization"}
          >
            <p className="panel-title">{t(locale, "settings.personalization.title")}</p>

            <div className="resilience-setting-group">
              <p className="resilience-setting-group-title">{t(locale, "settings.personalization.instructions")}</p>
              <label htmlFor="custom-instructions">
                <span className="field-label">{t(locale, "field.customInstructions")}</span>
                <textarea
                  id="custom-instructions"
                  rows={6}
                  value={customInstructions}
                  maxLength={MAX_CUSTOM_INSTRUCTIONS_CHARS}
                  onChange={(event) => setCustomInstructions(event.target.value)}
                  disabled={anySessionRunning || isSavingConfig}
                  placeholder={t(locale, "field.customInstructionsPlaceholder")}
                />
              </label>
              <p className="mode-help">{t(locale, "field.customInstructionsHint")}</p>
            </div>

            <div className="resilience-setting-group">
              <p className="resilience-setting-group-title">{t(locale, "settings.personalization.personality")}</p>
              <label htmlFor="personality">
                <span className="field-label">{t(locale, "field.personality")}</span>
                <select
                  id="personality"
                  value={personality}
                  onChange={(event) => setPersonality(normalizePersonality(event.target.value))}
                  disabled={anySessionRunning || isSavingConfig}
                >
                  {PERSONALITY_OPTIONS.map((option) => (
                    <option key={option} value={option}>
                      {t(locale, `field.personality.${option}` as MessageKey)}
                    </option>
                  ))}
                </select>
              </label>
              <p className="mode-help">{t(locale, "field.personalityHint")}</p>
            </div>

            <div className="resilience-setting-group">
              <p className="resilience-setting-group-title">{t(locale, "settings.personalization.memory")}</p>
              <div className="resilience-toggle-row">
                <div>
                  <span className="field-label">{t(locale, "field.localMemory")}</span>
                  <p className="mode-help">{t(locale, "field.localMemoryHint")}</p>
                </div>
                <button
                  type="button"
                  className={`browser-toggle${localMemoryEnabled ? " on" : ""}`}
                  aria-label={t(locale, "field.localMemory")}
                  aria-pressed={localMemoryEnabled}
                  onClick={() => setLocalMemoryEnabled((enabled) => !enabled)}
                  disabled={anySessionRunning || isSavingConfig}
                >
                  <span className="browser-toggle-knob" />
                </button>
              </div>
              <div className="resilience-toggle-row">
                <div>
                  <span className="field-label">{t(locale, "field.toolMemory")}</span>
                  <p className="mode-help">{t(locale, "field.toolMemoryHint")}</p>
                </div>
                <button
                  type="button"
                  className={`browser-toggle${toolMemoryEnabled ? " on" : ""}`}
                  aria-label={t(locale, "field.toolMemory")}
                  aria-pressed={toolMemoryEnabled}
                  onClick={() => setToolMemoryEnabled((enabled) => !enabled)}
                  disabled={anySessionRunning || isSavingConfig || !localMemoryEnabled}
                >
                  <span className="browser-toggle-knob" />
                </button>
              </div>
              <div className="resilience-toggle-row">
                <div>
                  <span className="field-label">{t(locale, "field.storedMemories")}</span>
                  <p className="mode-help">
                    {localMemoryCount === null
                      ? t(locale, "field.storedMemoriesUnknown")
                      : `${localMemoryCount}`}
                  </p>
                </div>
                <button
                  type="button"
                  className="danger-button"
                  onClick={() => void clearWorkspaceMemories()}
                  disabled={anySessionRunning || isSavingConfig || isClearingMemories || !workspaceRoot.trim() || localMemoryCount === 0}
                >
                  {t(locale, "action.clearMemories")}
                </button>
              </div>
              <p className="mode-help">{t(locale, "field.storedMemoriesHint")}</p>
            </div>
          </section>

          <section
            className="settings-card settings-defaults-card"
            role="tabpanel"
            id="settings-panel-defaults"
            aria-labelledby="settings-tab-defaults"
            aria-label={t(locale, "aria.workspaceDefaults")}
            hidden={settingsTab !== "defaults"}
          >
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
              <option value="full-auto">{t(locale, "mode.fullAuto")}</option>
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
             placeholder={"bash\ncurl"}
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
                  className={`session-item ${session.id === activeSessionId ? "is-active" : ""} ${
                    completedUnseenSessionIds.includes(session.id) ? "has-done-dot" : ""
                  } status-${session.status}`}
                  key={session.id}
                  onClick={() => selectSession(session)}
                  onContextMenu={(event) => openSessionMenu(event, session.id)}
                  title={sessionMetaLine(session, Date.now(), locale)}
                >
                  <span className="session-item-title">{sessionTitle(session, locale)}</span>
                  {runningSessionIds.includes(session.id) || session.status === "running" ? (
                    <span className="session-running-spinner" role="img" aria-label={t(locale, "status.running")} />
                  ) : completedUnseenSessionIds.includes(session.id) ? (
                    <span className="session-done-dot" role="img" aria-label={t(locale, "status.done")} />
                  ) : null}
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
                    className={`session-item ${session.id === activeSessionId ? "is-active" : ""} ${
                      completedUnseenSessionIds.includes(session.id) ? "has-done-dot" : ""
                    } status-${session.status}`}
                    key={session.id}
                    onClick={() => selectSession(session)}
                    onContextMenu={(event) => openSessionMenu(event, session.id)}
                    title={sessionMetaLine(session, Date.now(), locale)}
                  >
                    <span className="session-item-title">{sessionTitle(session, locale)}</span>
                    {runningSessionIds.includes(session.id) || session.status === "running" ? (
                      <span className="session-running-spinner" role="img" aria-label={t(locale, "status.running")} />
                    ) : completedUnseenSessionIds.includes(session.id) ? (
                      <span className="session-done-dot" role="img" aria-label={t(locale, "status.done")} />
                    ) : null}
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
                {message.role === "assistant" ? (
                  <InlineActivityList items={inlineActivityBuckets.get(message.id) || []} locale={locale} />
                ) : null}
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
          {pendingInlineActivity.length > 0 ? <InlineActivityList items={pendingInlineActivity} locale={locale} /> : null}
          {activeFollowUps.map((item, index) => (
            <article className="message message-user message-queued" key={item.id}>
              <div className="message-bubble">
                <UserMessageBody
                  content={item.text || (item.images.length > 0 ? t(locale, "message.imageCount", { count: String(item.images.length) }) : "")}
                />
              </div>
              <div className="message-meta">
                <span className="message-queued-state">
                  #{index + 1} · {t(locale, "composer.queueHint")}
                </span>
                <button
                  type="button"
                  className="quiet-button message-queued-button"
                  onClick={() => void steerFollowUp(item.id)}
                  title={t(locale, "action.steerHelp")}
                >
                  {t(locale, "action.steerMode")}
                </button>
                <button
                  type="button"
                  className="quiet-button message-queued-button"
                  onClick={() => editFollowUp(item.id)}
                  title={t(locale, "action.editMessage")}
                >
                  {t(locale, "action.editMessage")}
                </button>
                <button
                  type="button"
                  className="quiet-button message-queued-button"
                  onClick={() => removeFollowUp(item.id)}
                  title={t(locale, "action.closeQueue")}
                  aria-label={t(locale, "action.closeQueue")}
                >
                  {t(locale, "action.closeQueue")}
                </button>
              </div>
            </article>
          ))}
          <RunPlanProgress
            plan={plan}
            currentIndex={currentPlanStepIndex}
            phase={runStatus?.phase ?? null}
            activity={activity}
            locale={locale}
          />
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
                    {entry.id}
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
                onChange={(event) => {
                  const nextMode = event.target.value as Mode;
                  const previousMode = mode;
                  setMode(nextMode);
                  void persistWorkspaceMode(nextMode, previousMode);
                }}
                disabled={isRunning || isSavingConfig}
                title={t(locale, "field.mode")}
                aria-label={t(locale, "field.mode")}
              >
                <option value="ask">{t(locale, "mode.ask")}</option>
                <option value="auto-edit">{t(locale, "mode.autoEdit")}</option>
                <option value="full-auto">{t(locale, "mode.fullAuto")}</option>
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
                    <kbd>Ctrl+Shift</kbd>
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
        // Remount per session so the embedded browser never shows another task's page.
        key={sessionStateKey(activeSessionId)}
        locale={locale}
        open={rightPanelOpen}
        tab={rightPanelTab}
        sessionKey={activeSessionStateKey}
        browserNavigation={browserNavigation}
        browserState={browserState}
        onBrowserStateChange={(next) => {
          setBrowserStateBySession((current) => ({ ...current, [activeSessionStateKey]: next }));
        }}
        onTabChange={(next) => setRightPanelState(true, next)}
        onClose={() => setRightPanelState(false)}
        workspaceRoot={workspaceRoot}
        width={rightPanelWidth}
        onWidthChange={(next) => {
          const clamped = clampRightPanelWidth(next);
          setRightPanelWidth(clamped);
          saveRightPanelWidth(clamped);
        }}
        reviewContent={
          pendingAction ? (() => {
            // The side panel mirrors the composer review so a patch can be read
            // in full width instead of the cramped approval strip.
            const review = buildReviewPresentation(pendingAction, approvalSummary, Boolean(patchPreview), locale);
            return (
              <div className="review-preview">
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
                  <pre className="command-preview">
                    {review.commandText ?? JSON.stringify(pendingAction.tool_call.arguments, null, 2)}
                  </pre>
                ) : null}
                {review.bodyKind === "git" ? (
                  <pre className="command-preview git-preview">
                    {review.gitDetail ?? JSON.stringify(pendingAction.tool_call.arguments, null, 2)}
                  </pre>
                ) : null}
                {review.bodyKind === "generic" ? (
                  <pre className="command-preview">{JSON.stringify(pendingAction.tool_call.arguments, null, 2)}</pre>
                ) : null}
                {review.riskHint ? <p className="risk-hint">{review.riskHint}</p> : null}
                <p className="env-muted">{t(locale, "panel.reviewActionsHint")}</p>
              </div>
            );
          })() : null
        }
      />
    </main>
  );
}
