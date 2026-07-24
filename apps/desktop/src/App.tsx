import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { FormEvent, KeyboardEvent, MouseEvent as ReactMouseEvent } from "react";
import type {
  CancelSessionResult,
  ChatParams,
  ChatResult,
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
  ListModelsResult,
  ProviderAuthStatus,
  ProviderModel,
  UserConfig,
  WorkspaceConfig,
} from "@xcoding/protocol";
import { activityPolicyBadge, buildActivity, eventActivity, mergeActivity } from "./activity";
import type { ActivityItem } from "./activity";
import { buildReviewPresentation, latestApprovalSummary } from "./review";
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
  formatMessageRole,
  formatSessionStatus,
  hasTraceContent,
  sessionMetaLine,
} from "./layout";
import { isLocale, loadLocale, saveLocale, t, type Locale } from "./i18n";

const defaultProvider = "openai";
const isTauriRuntime = "__TAURI_INTERNALS__" in window;

function sessionTitle(session: Session, locale: Locale): string {
  return (
    session.title?.trim() ||
    t(locale, "session.fallbackTitle", {
      name: session.workspace_root.split(/[\\/]/).pop() || t(locale, "session.workspaceFallback"),
    })
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

export function App() {
  const [locale, setLocale] = useState<Locale>(() => loadLocale());
  const [workspaceRoot, setWorkspaceRoot] = useState("");
  const [prompt, setPrompt] = useState("");
  const [mode, setMode] = useState<Mode>("ask");
  const [model, setModel] = useState("");
  const [availableModels, setAvailableModels] = useState<ProviderModel[]>([]);
  const [modelsLoading, setModelsLoading] = useState(false);
  const [modelsError, setModelsError] = useState<string | null>(null);
  const [commandAllowlistText, setCommandAllowlistText] = useState("");
  const [commandDenylistText, setCommandDenylistText] = useState("");
  const [sessions, setSessions] = useState<Session[]>([]);
  const [activeSessionId, setActiveSessionId] = useState<string | null>(null);
  const [sessionMenu, setSessionMenu] = useState<{ sessionId: string; x: number; y: number } | null>(null);
  const [messages, setMessages] = useState<Message[]>([]);
  const [streamedText, setStreamedText] = useState("");
  const [plan, setPlan] = useState<PlanStep[]>([]);
  const [activity, setActivity] = useState<ActivityItem[]>([]);
  const [pendingAction, setPendingAction] = useState<PendingAction | null>(null);
  const [approvalSummary, setApprovalSummary] = useState<string | null>(null);
  const [patchPreview, setPatchPreview] = useState<PatchPreview | null>(null);
  const [restorePoints, setRestorePoints] = useState<RestorePoint[]>([]);
  const [taskSummary, setTaskSummary] = useState<TaskSummary | null>(null);
  const [replaySteps, setReplaySteps] = useState<ReplayStep[]>([]);
  const [providerStatus, setProviderStatus] = useState<ProviderAuthStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [isRunning, setIsRunning] = useState(false);
  const [isSavingConfig, setIsSavingConfig] = useState(false);
  const [view, setView] = useState<"workbench" | "settings">("workbench");
  const [apiKey, setApiKey] = useState("");
  const [baseUrl, setBaseUrl] = useState("https://ai.v58.dev/v1");
  const [showApiKey, setShowApiKey] = useState(false);
  const [userConfigReady, setUserConfigReady] = useState(false); // used to delay hydration
  const conversationRef = useRef<HTMLDivElement | null>(null);

  const activeSession = useMemo(
    () => sessions.find((session) => session.id === activeSessionId) ?? null,
    [activeSessionId, sessions],
  );

  useEffect(() => {
    saveLocale(locale);
    document.documentElement.lang = locale === "zh-CN" ? "zh-CN" : "en";
  }, [locale]);

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
        setBaseUrl(config.base_url || "https://ai.v58.dev/v1");
        setApiKey(config.api_key || "");
        if (config.last_workspace_root?.trim()) {
          setWorkspaceRoot(config.last_workspace_root.trim());
        }
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

  const refreshModels = useCallback(async (): Promise<boolean> => {
    const resolvedBase = baseUrl.trim() || "https://ai.v58.dev/v1";
    if (!isTauriRuntime) {
      const message = t(locale, "models.tauriOnly");
      setModelsError(message);
      setProviderStatus({
        ready: false,
        has_api_key: Boolean(apiKey.trim()),
        base_url: resolvedBase,
        key_hint: maskApiKeyHint(apiKey),
        message,
      });
      return false;
    }
    setModelsLoading(true);
    setModelsError(null);
    setProviderStatus((current) => ({
      ready: false,
      has_api_key: Boolean(apiKey.trim() || current?.has_api_key),
      base_url: resolvedBase,
      key_hint: maskApiKeyHint(apiKey) || current?.key_hint,
      message: t(locale, "auth.checking"),
    }));
    try {
      const result = await invoke<ListModelsResult>("list_provider_models", {
        baseUrl: baseUrl.trim() || null,
        apiKey: apiKey.trim() || null,
      });
      setAvailableModels(result.models);
      setModel((current) => {
        const trimmed = current.trim();
        if (trimmed && result.models.some((entry) => entry.id === trimmed)) {
          return trimmed;
        }
        if (trimmed) {
          return trimmed;
        }
        return "";
      });
      setProviderStatus({
        ready: true,
        has_api_key: true,
        base_url: result.base_url || resolvedBase,
        key_hint: maskApiKeyHint(apiKey),
        message: t(locale, "auth.modelsOk", { count: String(result.models.length) }),
      });
      return true;
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause);
      setAvailableModels([]);
      setModelsError(message);
      setProviderStatus({
        ready: false,
        has_api_key: Boolean(apiKey.trim()),
        base_url: resolvedBase,
        key_hint: maskApiKeyHint(apiKey),
        message,
      });
      return false;
    } finally {
      setModelsLoading(false);
    }
  }, [apiKey, baseUrl, locale, maskApiKeyHint]);

  const loadWorkspaceConfig = useCallback(async () => {
    const root = workspaceRoot.trim();
    if (!isTauriRuntime || !root) return;
    try {
      const config = await invoke<WorkspaceConfig>("workspace_config", { workspaceRoot: root });
      setMode(config.mode);
      setModel(config.model);
      setCommandAllowlistText(formatCommandAllowlistText(config.command_allowlist));
      setCommandDenylistText(formatCommandDenylistText(config.command_denylist));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }, [workspaceRoot]);

  const refreshSessions = useCallback(async () => {
    if (!isTauriRuntime) return;
    try {
      const nextSessions = await invoke<Session[]>("list_sessions", { workspaceRoot: workspaceRoot.trim() || null });
      setSessions(nextSessions);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }, [workspaceRoot]);

  const refreshWorkspace = useCallback(async () => {
    await Promise.all([refreshSessions(), loadWorkspaceConfig(), refreshProviderStatus()]);
  }, [loadWorkspaceConfig, refreshProviderStatus, refreshSessions]);

  useEffect(() => {
    if (view !== "settings") return;
    if (!isTauriRuntime) {
      setModelsError(t(locale, "models.tauriOnly"));
      return;
    }
    if (!userConfigReady) return;
    // Real connectivity check: can we list models with current form/env credentials?
    void refreshModels();
  }, [view, userConfigReady, refreshModels]);

  const hydrateSession = useCallback(async (sessionId: string) => {
    if (!isTauriRuntime) return;
    try {
      const detail = await invoke<SessionDetail>("session_detail", { sessionId });
      const pending = detail.session.status === "need_user"
        ? detail.pending_actions.find((action) => action.status === "pending") ?? null
        : null;
      setMessages(detail.messages);
      setStreamedText("");
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

  useEffect(() => {
    if (!userConfigReady) return;
    void refreshWorkspace();
  }, [refreshWorkspace, userConfigReady]);
  useEffect(() => {
    if (activeSessionId) void hydrateSession(activeSessionId);
  }, [activeSessionId, hydrateSession]);

  useEffect(() => {
    const node = conversationRef.current;
    if (!node) return;
    node.scrollTop = node.scrollHeight;
  }, [messages, streamedText, error, isRunning]);

  useEffect(() => {
    if (!isTauriRuntime) return;
    let unlisten: (() => void) | undefined;
    void listen<SessionEvent>("session-event", (event) => {
      const payload = event.payload;
      setActiveSessionId((current) => current ?? payload.session_id);
      if (payload.type === "text_delta") setStreamedText((current) => current + payload.delta);
      if (payload.type === "message_completed") {
        setMessages((current) => mergeMessage(current, payload.message));
        setStreamedText("");
        // Unlock composer as soon as the assistant turn is complete, even if the
        // chat invoke is still finishing post-summary work.
        setIsRunning(false);
        setSessions((current) =>
          current.map((session) =>
            session.id === payload.session_id ? { ...session, status: "done" } : session,
          ),
        );
      }
      if (payload.type === "plan") setPlan(payload.steps);
      if (payload.type === "patch_preview") setPatchPreview(payload.preview);
      if (payload.type === "approval_requested") {
        setPendingAction(payload.action);
        setApprovalSummary(payload.summary);
        setIsRunning(false);
        setSessions((current) =>
          current.map((session) =>
            session.id === payload.session_id ? { ...session, status: "need_user" } : session,
          ),
        );
      }
      if (payload.type === "session_cancelled") {
        setPendingAction(null);
        setApprovalSummary(null);
        setIsRunning(false);
        setSessions((current) =>
          current.map((session) =>
            session.id === payload.session_id ? { ...session, status: "cancelled" } : session,
          ),
        );
      }
      if (payload.type === "task_completed") {
        setTaskSummary(payload.summary);
        setIsRunning(false);
        setSessions((current) =>
          current.map((session) =>
            session.id === payload.session_id ? { ...session, status: "done" } : session,
          ),
        );
      }
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
    }).then((stop) => { unlisten = stop; });
    return () => unlisten?.();
  }, [locale]);

  function canContinueSession(session: Session | null): boolean {
    return !!session && (session.status === "done" || session.status === "failed" || session.status === "created");
  }

  function startNewChat(): void {
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
    setError(null);
  }

  useEffect(() => {
    if (!sessionMenu) return;
    const close = () => setSessionMenu(null);
    window.addEventListener("click", close);
    window.addEventListener("blur", close);
    window.addEventListener("resize", close);
    return () => {
      window.removeEventListener("click", close);
      window.removeEventListener("blur", close);
      window.removeEventListener("resize", close);
    };
  }, [sessionMenu]);

  function openSessionMenu(event: ReactMouseEvent, sessionId: string): void {
    event.preventDefault();
    event.stopPropagation();
    setSessionMenu({ sessionId, x: event.clientX, y: event.clientY });
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
        startNewChat();
      }
      await refreshSessions();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : t(locale, "history.deleteFailed"));
    }
  }

  async function submit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    const root = workspaceRoot.trim();
    const message = prompt.trim();
    if (isRunning) return;
    // Always accept the click so grey-out is not a dead end: surface the exact missing prerequisite.
    if (!root) {
      setError(t(locale, "error.needWorkspace"));
      return;
    }
    if (!message) {
      setError(t(locale, "error.needPrompt"));
      return;
    }
    if (providerStatus && !providerStatus.ready) {
      setError(t(locale, "error.needProvider"));
      return;
    }
    if (!model.trim()) {
      setError(t(locale, "error.needModel"));
      return;
    }
    if (!isTauriRuntime) {
      setError(t(locale, "error.tauriOnly"));
      return;
    }

    const continuing = canContinueSession(activeSession);
    setError(null);
    setIsRunning(true);
    setPrompt("");
    if (!continuing) {
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
    } else {
      setStreamedText("");
      setPendingAction(null);
      setApprovalSummary(null);
      setPatchPreview(null);
      setTaskSummary(null);
      setMessages((current) => [
        ...current,
        {
          id: `local-user-${Date.now()}`,
          session_id: activeSession!.id,
          role: "user",
          content: message,
          created_at: new Date().toISOString(),
        },
      ]);
    }
    const params: ChatParams = {
      workspace_root: root,
      message,
      mode,
      provider: defaultProvider,
      model,
      session_id: continuing ? activeSession!.id : undefined,
    };
    try {
      const result = await invoke<ChatResult>("chat", { params });
      setActiveSessionId(result.session.id);
      const completedMessage = result.message;
      if (completedMessage) setMessages((current) => mergeMessage(current, completedMessage));
      await refreshSessions();
      await hydrateSession(result.session.id);
      try {
        const current = await invoke<UserConfig>("get_user_config");
        const previous = (current.last_workspace_root || "").trim();
        if (previous !== root) {
          await invoke<UserConfig>("set_user_config", {
            config: { ...current, last_workspace_root: root } satisfies UserConfig,
          });
        }
      } catch {
        // Non-fatal: chat already succeeded.
      }
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setIsRunning(false);
    }
  }

  async function resolveAction(approved: boolean): Promise<void> {
    if (!pendingAction || !activeSessionId) return;
    setError(null);
    setIsRunning(true);
    try {
      const result = await invoke<ResolveActionResult>("resolve_action", {
        params: { session_id: activeSessionId, action_id: pendingAction.id, approved },
      });
      const completedMessage = result.message;
      if (completedMessage) setMessages((current) => mergeMessage(current, completedMessage));
      await refreshSessions();
      await hydrateSession(activeSessionId);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setIsRunning(false);
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
    setError(null);
    setIsRunning(true);
    try {
      await invoke<RollbackRestorePointResult>("rollback_restore_point", {
        params: { session_id: activeSessionId, restore_point_id: restorePoint.id },
      });
      await refreshSessions();
      await hydrateSession(activeSessionId);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setIsRunning(false);
    }
  }

  async function cancelSession(): Promise<void> {
    if (!activeSessionId) return;
    setError(null);
    setIsRunning(true);
    try {
      await invoke<CancelSessionResult>("cancel_session", { params: { session_id: activeSessionId } });
      await refreshSessions();
      await hydrateSession(activeSessionId);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setIsRunning(false);
    }
  }

  async function saveAllSettings(): Promise<void> {
    if (isSavingConfig || isRunning) return;
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
      const savedUser = await invoke<UserConfig>("set_user_config", {
        config: {
          locale,
          mode,
          provider: defaultProvider,
          model: model.trim(),
          base_url: baseUrl.trim() || "https://ai.v58.dev/v1",
          api_key: apiKey.trim() || undefined,
          last_workspace_root: root || undefined,
        } satisfies UserConfig,
      });
      setMode(savedUser.mode);
      setModel((savedUser.model || "").trim());
      setBaseUrl(savedUser.base_url || "https://ai.v58.dev/v1");
      setApiKey(savedUser.api_key || "");
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
        setModel(config.model);
        setCommandAllowlistText(formatCommandAllowlistText(config.command_allowlist));
        setCommandDenylistText(formatCommandDenylistText(config.command_denylist));
      }
      await refreshModels();
      await refreshSessions();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setIsSavingConfig(false);
    }
  }

  const showTraceContent = hasTraceContent({
    pendingAction,
    planCount: plan.length,
    activityCount: activity.length,
    restoreCount: restorePoints.length,
    replayCount: replaySteps.length,
    taskSummary,
  });

  const doctorChecks = buildDesktopDoctorChecks({
    workspaceRoot,
    providerStatus,
    mode,
    model,
    provider: defaultProvider,
    locale,
  });
  const doctorReady = desktopDoctorReady(doctorChecks);
  const workspaceMissing = !workspaceRoot.trim();
  const modelMissing = !model.trim();
  const modelNotInList =
    !!model.trim() &&
    availableModels.length > 0 &&
    !availableModels.some((entry) => entry.id === model.trim());
  const sendBlockReason = isRunning
    ? "running"
    : workspaceMissing
      ? "workspace"
      : !prompt.trim()
        ? "prompt"
        : providerStatus && !providerStatus.ready
          ? "provider"
          : modelMissing
            ? "model"
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
            : null;
  const sendTitle =
    sendBlockReason === "running"
      ? t(locale, "action.working")
      : sendHint || t(locale, "action.send");

  function onComposerKeyDown(event: KeyboardEvent<HTMLTextAreaElement>): void {
    if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
      event.preventDefault();
      event.currentTarget.form?.requestSubmit();
    }
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
            <button type="button" className="quiet-button" onClick={() => setView("workbench")}>
              {t(locale, "action.back")}
            </button>
            <button
              type="button"
              className="primary-button"
              onClick={() => void saveAllSettings()}
              disabled={isRunning || isSavingConfig}
            >
              {isSavingConfig ? t(locale, "action.saving") : t(locale, "action.saveSettings")}
            </button>
          </div>
        </header>

        {error ? <p className="error-message settings-error">{error}</p> : null}

        <div className="settings-grid">
          <section className="settings-card" aria-label={t(locale, "settings.section.language")}>
            <p className="panel-title">{t(locale, "settings.section.language")}</p>
            <select
              id="ui-locale"
              aria-label={t(locale, "lang.label")}
              value={locale}
              onChange={(event) => setLocale(event.target.value as Locale)}
              disabled={isRunning || isSavingConfig}
            >
              <option value="zh-CN">{t(locale, "lang.zhCN")}</option>
              <option value="en">{t(locale, "lang.en")}</option>
            </select>
          </section>

          <section className="settings-card" aria-label={t(locale, "settings.section.provider")}>
            <p className="panel-title">{t(locale, "settings.section.provider")}</p>
            <label className="field-label" htmlFor="default-provider">{t(locale, "field.provider")}</label>
            <input
              id="default-provider"
              value={defaultProvider}
              readOnly
              spellCheck={false}
              title={t(locale, "provider.readonlyTitle")}
            />
            <label className="field-label" htmlFor="provider-base-url">{t(locale, "field.baseUrl")}</label>
            <input
              id="provider-base-url"
              value={baseUrl}
              onChange={(event) => setBaseUrl(event.target.value)}
              disabled={isRunning || isSavingConfig}
              spellCheck={false}
              placeholder="https://ai.v58.dev/v1"
            />
            <label className="field-label" htmlFor="provider-api-key">{t(locale, "field.apiKey")}</label>
            <div className="secret-field">
              <input
                id="provider-api-key"
                type={showApiKey ? "text" : "password"}
                value={apiKey}
                onChange={(event) => setApiKey(event.target.value)}
                disabled={isRunning || isSavingConfig}
                spellCheck={false}
                autoComplete="off"
                placeholder={t(locale, "field.apiKeyPlaceholder")}
              />
              <button
                type="button"
                className="quiet-button"
                onClick={() => setShowApiKey((current) => !current)}
              >
                {showApiKey ? t(locale, "action.hideKey") : t(locale, "action.showKey")}
              </button>
            </div>
            <div className={`auth-status ${providerStatus?.ready ? "ready" : "missing"}`} role="status">
              <strong>{providerStatus?.ready ? t(locale, "auth.ready") : t(locale, "auth.missing")}</strong>
              <small>{providerStatus?.message || t(locale, "auth.checking")}</small>
              <small>
                {t(locale, "auth.base", { url: providerStatus?.base_url || baseUrl || "https://ai.v58.dev/v1" })}
                {providerStatus?.key_hint ? ` · ${t(locale, "auth.key", { hint: providerStatus.key_hint })}` : ""}
              </small>
              <button type="button" className="quiet-button" onClick={() => void refreshModels()} disabled={isRunning || isSavingConfig || modelsLoading}>
                {modelsLoading ? t(locale, "auth.checking") : t(locale, "action.refreshAuth")}
              </button>
            </div>
            <p className="mode-help">{t(locale, "settings.providerHelp")}</p>
          </section>

          <section className="settings-card" aria-label={t(locale, "aria.workspaceDefaults")}>
            <p className="panel-title">{t(locale, "field.defaults")}</p>
            <label className="field-label" htmlFor="settings-workspace">{t(locale, "field.workspace")}</label>
            <input
              id="settings-workspace"
              className={workspaceMissing ? "workspace-missing" : undefined}
              value={workspaceRoot}
              onChange={(event) => setWorkspaceRoot(event.target.value)}
              placeholder={t(locale, "field.workspacePlaceholder")}
              spellCheck={false}
              disabled={isRunning || isSavingConfig}
            />
            <p className="mode-help">{t(locale, "field.workspaceHint")}</p>
            <label className="field-label" htmlFor="default-mode">{t(locale, "field.mode")}</label>
            <select
              id="default-mode"
              value={mode}
              onChange={(event) => setMode(event.target.value as Mode)}
              disabled={isRunning || isSavingConfig}
            >
              <option value="ask">{t(locale, "mode.ask")}</option>
              <option value="auto-edit">{t(locale, "mode.autoEdit")}</option>
            </select>
            <p className="mode-help">{modeHelpText(mode, locale)}</p>
            <label className="field-label" htmlFor="default-model">{t(locale, "field.model")}</label>
            <div className="secret-field">
              <select
                id="default-model"
                value={model}
                onChange={(event) => setModel(event.target.value)}
                disabled={isRunning || isSavingConfig || modelsLoading}
              >
                <option value="">
                  {modelsLoading
                    ? t(locale, "models.loading")
                    : availableModels.length === 0
                      ? t(locale, "models.placeholder")
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
              <button
                type="button"
                className="quiet-button"
                onClick={() => void refreshModels()}
                disabled={isRunning || isSavingConfig || modelsLoading}
              >
                {modelsLoading ? t(locale, "models.loading") : t(locale, "models.refresh")}
              </button>
            </div>
            <p className="mode-help">{t(locale, "models.help")}</p>
            {modelsError ? <p className="mode-help" role="alert">{modelsError}</p> : null}
            {modelNotInList ? <p className="mode-help" role="status">{t(locale, "models.notInList")}</p> : null}
            {availableModels.length === 0 && !modelsLoading && !modelsError ? (
              <p className="mode-help">{t(locale, "models.empty")}</p>
            ) : null}
            <label className="field-label" htmlFor="command-allowlist">{t(locale, "field.allowlist")}</label>
            <textarea
              id="command-allowlist"
              className="command-allowlist-input"
              value={commandAllowlistText}
              onChange={(event) => setCommandAllowlistText(event.target.value)}
              disabled={isRunning || isSavingConfig || !workspaceRoot.trim()}
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
              disabled={isRunning || isSavingConfig || !workspaceRoot.trim()}
              spellCheck={false}
              rows={3}
              placeholder={"powershell\ncurl"}
            />
            <p className="mode-help">{commandDenylistHelpText(locale)}</p>
            {!workspaceRoot.trim() ? (
              <p className="mode-help">{t(locale, "settings.workspacePolicyHint")}</p>
            ) : null}
          </section>

          <section className="settings-card" aria-label={t(locale, "aria.diagnostics")}>
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
        </div>
      </main>
    );
  }

  return (
    <main className="workbench">
      <aside className="sessions-panel" aria-label={t(locale, "aria.sessions")}>
        <div className="sessions-top">
          <div className="brand-row">
            <div>
              <p className="eyebrow">{t(locale, "brand.eyebrow")}</p>
              <h1>{t(locale, "brand.sessions")}</h1>
            </div>
            <div className="brand-actions">
              <button
                type="button"
                className="quiet-button"
                onClick={() => setView("settings")}
                aria-label={t(locale, "aria.openSettings")}
              >
                {t(locale, "action.settings")}
              </button>
              <button
                type="button"
                className="quiet-button"
                onClick={() => void refreshWorkspace()}
                aria-label={t(locale, "aria.refreshWorkspace")}
              >
                {t(locale, "action.refresh")}
              </button>
            </div>
          </div>
          <label className="field-label" htmlFor="workspace-root">{t(locale, "field.workspace")}</label>
          <input
            id="workspace-root"
            className={workspaceMissing ? "workspace-missing" : undefined}
            value={workspaceRoot}
            onChange={(event) => setWorkspaceRoot(event.target.value)}
            placeholder={t(locale, "field.workspacePlaceholder")}
            spellCheck={false}
          />
          <p className="mode-help">{t(locale, "field.workspaceHint")}</p>
          <div className={`auth-status compact ${providerStatus?.ready ? "ready" : "missing"}`} role="status">
            <strong>{providerStatus?.ready ? t(locale, "auth.ready") : t(locale, "auth.missing")}</strong>
            <small>
              {providerStatus?.key_hint
                ? t(locale, "auth.key", { hint: providerStatus.key_hint })
                : (providerStatus?.message || t(locale, "auth.checking"))}
            </small>
          </div>
        </div>
        <nav className="session-list" aria-label={t(locale, "aria.savedSessions")}>
          <p className="panel-title session-list-title">{t(locale, "field.history")}</p>
          {sessions.length === 0 ? <p className="empty-state">{t(locale, "history.empty")}</p> : null}
          {sessions.map((session) => (
            <button
              type="button"
              className={`session-item ${session.id === activeSessionId ? "is-active" : ""} status-${session.status}`}
              key={session.id}
              onClick={() => setActiveSessionId(session.id)}
              onContextMenu={(event) => openSessionMenu(event, session.id)}
            >
              <span className="session-item-title">{sessionTitle(session, locale)}</span>
              <span className={`status-badge status-${session.status}`}>{formatSessionStatus(session.status, locale)}</span>
              <small>{sessionMetaLine(session, Date.now(), locale)}</small>
            </button>
          ))}
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
              className="danger"
              role="menuitem"
              onClick={() => void deleteSession(sessionMenu.sessionId)}
            >
              {t(locale, "action.delete")}
            </button>
          </div>
        ) : null}
      </aside>

      <section className="chat-panel" aria-label={t(locale, "aria.conversation")}>
        <header className="chat-header">
          <div>
            <p className="eyebrow">
              {t(locale, "chat.cloudModel")} · {activeSession?.model || model}
              {activeSession ? ` · ${formatSessionStatus(activeSession.status, locale)}` : ""}
            </p>
            <h2>
              {activeSession ? sessionTitle(activeSession, locale) : t(locale, "chat.newTask")}
              {canContinueSession(activeSession) ? ` · ${t(locale, "chat.followUp")}` : ""}
            </h2>
          </div>
          <div className="header-controls">
            {activeSession ? (
              <span className={`status-badge status-${activeSession.status}`}>
                {formatSessionStatus(activeSession.status, locale)}
              </span>
            ) : null}
            {activeSession ? (
              <button type="button" className="quiet-button" onClick={() => startNewChat()} disabled={isRunning}>
                {t(locale, "action.newChat")}
              </button>
            ) : null}
            {activeSession && (activeSession.status === "need_user" || activeSession.status === "running" || isRunning) ? (
              <button type="button" className="quiet-button" onClick={() => void cancelSession()}>
                {t(locale, "action.cancel")}
              </button>
            ) : null}
            <label className="mode-control">
              {t(locale, "field.mode")}
              <select value={mode} onChange={(event) => setMode(event.target.value as Mode)} disabled={isRunning}>
                <option value="ask">{t(locale, "mode.ask")}</option>
                <option value="auto-edit">{t(locale, "mode.autoEdit")}</option>
              </select>
            </label>
          </div>
        </header>
        <div className="conversation" aria-live="polite" ref={conversationRef}>
          {messages.map((message) => (
            <article className={`message message-${message.role}`} key={message.id}>
              <p>{formatMessageRole(message.role, locale)}</p>
              <div>{message.content}</div>
            </article>
          ))}
          {streamedText ? (
            <article className="message message-assistant streaming">
              <p>{t(locale, "role.assistant")}</p>
              <div>{streamedText}</div>
            </article>
          ) : null}
          {messages.length === 0 && !streamedText && !isRunning ? (
            <div className="empty-chat">
              <p className="empty-state">{t(locale, "chat.empty")}</p>
              <ul className="empty-hints">
                <li>{t(locale, "chat.hint.left")}</li>
                <li>{t(locale, "chat.hint.center")}</li>
                <li>{t(locale, "chat.hint.right")}</li>
              </ul>
              <p className="empty-state composer-hint">{t(locale, "chat.tip")}</p>
            </div>
          ) : null}
          {error ? <p className="error-message">{error}</p> : null}
        </div>
        <form className="composer" onSubmit={submit}>
          <textarea
            value={prompt}
            onChange={(event) => setPrompt(event.target.value)}
            onKeyDown={onComposerKeyDown}
            placeholder={
              canContinueSession(activeSession)
                ? t(locale, "composer.continuePlaceholder")
                : t(locale, "composer.placeholder")
            }
            rows={4}
            disabled={isRunning}
          />
          <div className="composer-footer">
            <span title={sendHint || undefined}>
              {canContinueSession(activeSession)
                ? t(locale, "composer.continueId", { id: activeSession!.id.slice(0, 8) })
                : sendHint
                  ? sendHint
                  : workspaceRoot.trim()
                    ? workspaceRoot
                    : t(locale, "composer.chooseWorkspace")}
            </span>
            <button
              type="submit"
              className={sendBlockReason && sendBlockReason !== "running" ? "send-needs-setup" : undefined}
              disabled={isRunning}
              title={sendTitle}
              aria-label={sendTitle}
            >
              {isRunning
                ? t(locale, "action.working")
                : canContinueSession(activeSession)
                  ? t(locale, "action.continue")
                  : t(locale, "action.send")}
            </button>
          </div>
        </form>
      </section>

      <aside className="trace-panel" aria-label={t(locale, "aria.trace")}>
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

        {!showTraceContent && !pendingAction ? (
          <section className="trace-empty">
            <p className="panel-title">{t(locale, "trace.emptyTitle")}</p>
            <p className="empty-state">{t(locale, "trace.emptyBody")}</p>
          </section>
        ) : null}

        {taskSummary ? (
          <section className="task-summary">
            <div className="summary-header">
              <p className="panel-title">{t(locale, "trace.summary")}</p>
              <div className="summary-actions">
                <button
                  type="button"
                  className="quiet-button"
                  onClick={() => void copyText(formatTaskSummaryText(taskSummary, locale))}
                >
                  {t(locale, "action.copy")}
                </button>
                {taskSummary.git_status || taskSummary.git_diff ? (
                  <button
                    type="button"
                    className="quiet-button"
                    onClick={() => void copyText(gitSnapshotText(taskSummary, locale))}
                  >
                    {t(locale, "action.copy")} Git
                  </button>
                ) : null}
              </div>
            </div>
            <strong>
              {taskSummary.changed_files.length} {locale === "zh-CN" ? "个变更文件" : "changed file(s)"}
              {typeof taskSummary.lines_added === "number" || typeof taskSummary.lines_removed === "number" ? (
                <> · +{taskSummary.lines_added ?? 0}/−{taskSummary.lines_removed ?? 0}</>
              ) : null}
            </strong>
            <small>
              {t(locale, "trace.commandsSucceeded", {
                ok: taskSummary.commands_succeeded,
                total: taskSummary.commands_run,
              })}
              {taskSummary.commands_failed
                ? t(locale, "trace.commandsFailed", { n: taskSummary.commands_failed })
                : ""}
            </small>
            {(taskSummary.file_changes?.length ?? 0) > 0 ? (
              <ul className="file-change-list">
                {taskSummary.file_changes!.map((change) => (
                  <li key={change.path}>
                    <span className={`change-kind ${change.kind}`}>{fileChangeLabel(change.kind, locale)}</span>
                    <code>{change.path}</code>
                    <small className="line-delta">+{change.lines_added}/−{change.lines_removed}</small>
                  </li>
                ))}
              </ul>
            ) : taskSummary.changed_files.length > 0 ? (
              <ul>
                {taskSummary.changed_files.map((path) => (
                  <li key={path}><code>{path}</code></li>
                ))}
              </ul>
            ) : null}
            {taskSummary.git_branch ? <small>{t(locale, "trace.branch", { name: taskSummary.git_branch })}</small> : null}
            {taskSummary.git_status ? (
              <details className="summary-details">
                <summary>{t(locale, "trace.gitStatus")}</summary>
                <pre className="summary-pre">{taskSummary.git_status}</pre>
              </details>
            ) : null}
            {taskSummary.git_diff ? (
              <details className="summary-details">
                <summary>{t(locale, "trace.gitDiff")}</summary>
                <pre className="summary-pre">{taskSummary.git_diff}</pre>
              </details>
            ) : null}
          </section>
        ) : null}

        <section className="trace-section">
          <p className="panel-title">
            {t(locale, "trace.activity")}
            {activity.length ? ` · ${activity.length}` : ""}
          </p>
          <div className="activity-list">
            {activity.length === 0 ? <p className="empty-state">{t(locale, "trace.activityEmpty")}</p> : null}
            {activity.map((item) => {
              const badge = activityPolicyBadge(item.policy, locale);
              return (
                <article className={`activity ${item.state} policy-${item.policy}`} key={item.id}>
                  <div className="activity-header">
                    {badge ? <span className={`activity-badge policy-${item.policy}`}>{badge}</span> : null}
                    <strong>{item.label}</strong>
                  </div>
                  <code>{item.detail}</code>
                </article>
              );
            })}
          </div>
        </section>

        <details className="trace-section" open={plan.length > 0}>
          <summary className="panel-title">
            {t(locale, "trace.plan")}
            {plan.length ? ` · ${plan.length}` : ""}
          </summary>
          <ol className="plan-list">
            {plan.length === 0 ? <li className="empty-state">{t(locale, "trace.planEmpty")}</li> : null}
            {plan.map((step) => <li key={step.id}>{step.description}</li>)}
          </ol>
        </details>

        <details className="trace-section" open={restorePoints.length > 0}>
          <summary className="panel-title">
            {t(locale, "trace.restore")}
            {restorePoints.length ? ` · ${restorePoints.length}` : ""}
          </summary>
          <div className="restore-list">
            {restorePoints.length === 0 ? <p className="empty-state">{t(locale, "trace.restoreEmpty")}</p> : null}
            {restorePoints.map((restorePoint) => (
              <div className="restore-point" key={restorePoint.id}>
                <div>
                  <strong>{restorePoint.path}</strong>
                  <small>{new Date(restorePoint.created_at).toLocaleString(locale === "zh-CN" ? "zh-CN" : "en-US")}</small>
                </div>
                <button
                  type="button"
                  className="quiet-button"
                  onClick={() => void rollbackRestorePoint(restorePoint)}
                  disabled={isRunning || !restorePoint.applied_text}
                >
                  {t(locale, "action.rollback")}
                </button>
              </div>
            ))}
          </div>
        </details>

        <details className="trace-section" open={replaySteps.length > 0}>
          <summary className="panel-title">
            {t(locale, "trace.replay")}
            {replaySteps.length ? ` · ${replaySteps.length}` : ""}
          </summary>
          <div className="restore-list">
            {replaySteps.length === 0 ? <p className="empty-state">{t(locale, "trace.replayEmpty")}</p> : null}
            {replaySteps.map((step, index) => (
              <div className="restore-point" key={`${step.kind}-${index}`}>
                <div>
                  <strong>{step.kind}{step.tool_name ? ` · ${step.tool_name}` : ""}</strong>
                  <small>{step.summary}</small>
                </div>
                {typeof step.success === "boolean" ? <code>{step.success ? "ok" : "fail"}</code> : null}
              </div>
            ))}
          </div>
          <button
            type="button"
            className="quiet-button"
            onClick={() => void loadReplay()}
            disabled={!activeSessionId || isRunning}
          >
            {t(locale, "action.replaySteps")}
          </button>
        </details>
      </aside>
    </main>
  );
}

