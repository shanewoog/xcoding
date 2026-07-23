import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useState } from "react";
import type { FormEvent } from "react";
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
  ProviderAuthStatus,
  WorkspaceConfig,
} from "@xcoding/protocol";
import { buildReviewPresentation, latestApprovalSummary } from "./review";

type Activity = {
  id: string;
  label: string;
  detail: string;
  state: "running" | "done" | "failed";
};

const defaultModel = "gpt-5.5";
const defaultProvider = "openai";
const isTauriRuntime = "__TAURI_INTERNALS__" in window;

function sessionTitle(session: Session): string {
  return session.title?.trim() || `${session.workspace_root.split(/[\\/]/).pop() || "Workspace"} session`;
}

function eventActivity(event: SessionEvent, sequence: string): Activity | null {
  if (event.type === "tool_start") {
    return { id: event.tool_call.id, label: event.summary, detail: JSON.stringify(event.tool_call.arguments), state: "running" };
  }
  if (event.type === "tool_end") {
    return { id: event.tool_call.id, label: event.summary, detail: JSON.stringify(event.tool_call.arguments), state: event.success ? "done" : "failed" };
  }
  if (event.type === "restore_point_rolled_back") {
    return { id: sequence, label: event.summary, detail: event.restore_point.path, state: "done" };
  }
  if (event.type === "session_cancelled") {
    return { id: sequence, label: "Session cancelled", detail: event.message, state: "failed" };
  }
  if (event.type === "error") {
    return { id: sequence, label: "Agent error", detail: event.message, state: "failed" };
  }
  return null;
}

function buildActivity(events: PersistedSessionEvent[]): Activity[] {
  const items = new Map<string, Activity>();
  for (const item of events) {
    const activity = eventActivity(item.event, item.id);
    if (activity) items.set(activity.id, activity);
  }
  return [...items.values()];
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

function buildPatchDiffLines(preview: PatchPreview): Array<{ kind: "add" | "remove" | "meta"; text: string }> {
  const lines: Array<{ kind: "add" | "remove" | "meta"; text: string }> = [];
  if (!preview.old_text) {
    lines.push({ kind: "meta", text: "(new file)" });
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

function gitSnapshotText(summary: TaskSummary): string {
  return [
    summary.git_branch ? `Branch: ${summary.git_branch}` : "",
    summary.git_status ? `Status:\n${summary.git_status}` : "",
    summary.git_diff ? `Diff:\n${summary.git_diff}` : "",
  ]
    .filter(Boolean)
    .join("\n\n");
}

function formatTaskSummaryText(summary: TaskSummary): string {
  const added = summary.lines_added ?? 0;
  const removed = summary.lines_removed ?? 0;
  const lines: string[] = [
    `Task complete: ${summary.changed_files.length} changed file(s), +${added}/-${removed} line(s)`,
    `Commands: ${summary.commands_succeeded}/${summary.commands_run} succeeded` +
      (summary.commands_failed ? `, ${summary.commands_failed} failed` : ""),
  ];
  const fileChanges = summary.file_changes ?? [];
  if (fileChanges.length > 0) {
    lines.push("Files:");
    for (const change of fileChanges) {
      lines.push(`  [${change.kind}] ${change.path} (+${change.lines_added}/-${change.lines_removed})`);
    }
  } else if (summary.changed_files.length > 0) {
    lines.push(`Changed: ${summary.changed_files.join(", ")}`);
  }
  const git = gitSnapshotText(summary);
  if (git) lines.push(git);
  return lines.join("\n");
}

function fileChangeLabel(kind: string): string {
  if (kind === "created") return "created";
  if (kind === "deleted") return "deleted";
  return "modified";
}


function mergeMessage(messages: Message[], message: Message): Message[] {
  return messages.some((current) => current.id === message.id) ? messages : [...messages, message];
}

export function App() {
  const [workspaceRoot, setWorkspaceRoot] = useState("");
  const [prompt, setPrompt] = useState("");
  const [mode, setMode] = useState<Mode>("ask");
  const [model, setModel] = useState(defaultModel);
  const [sessions, setSessions] = useState<Session[]>([]);
  const [activeSessionId, setActiveSessionId] = useState<string | null>(null);
  const [messages, setMessages] = useState<Message[]>([]);
  const [streamedText, setStreamedText] = useState("");
  const [plan, setPlan] = useState<PlanStep[]>([]);
  const [activity, setActivity] = useState<Activity[]>([]);
  const [pendingAction, setPendingAction] = useState<PendingAction | null>(null);
  const [approvalSummary, setApprovalSummary] = useState<string | null>(null);
  const [patchPreview, setPatchPreview] = useState<PatchPreview | null>(null);
  const [restorePoints, setRestorePoints] = useState<RestorePoint[]>([]);
  const [taskSummary, setTaskSummary] = useState<TaskSummary | null>(null);
  const [replaySteps, setReplaySteps] = useState<ReplayStep[]>([]);
  const [isSavingConfig, setIsSavingConfig] = useState(false);
  const [isRunning, setIsRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [providerStatus, setProviderStatus] = useState<ProviderAuthStatus | null>(null);

  const activeSession = useMemo(
    () => sessions.find((session) => session.id === activeSessionId) ?? null,
    [activeSessionId, sessions],
  );

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

  const loadWorkspaceConfig = useCallback(async () => {
    const root = workspaceRoot.trim();
    if (!isTauriRuntime || !root) return;
    try {
      const config = await invoke<WorkspaceConfig>("workspace_config", { workspaceRoot: root });
      setMode(config.mode);
      setModel(config.model);
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
      setActivity(buildActivity(detail.events));
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
  }, []);

  useEffect(() => { void refreshWorkspace(); }, [refreshWorkspace]);
  useEffect(() => {
    if (activeSessionId) void hydrateSession(activeSessionId);
  }, [activeSessionId, hydrateSession]);

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
      }
      if (payload.type === "plan") setPlan(payload.steps);
      if (payload.type === "patch_preview") setPatchPreview(payload.preview);
      if (payload.type === "approval_requested") {
        setPendingAction(payload.action);
        setApprovalSummary(payload.summary);
      }
      if (payload.type === "session_cancelled") {
        setPendingAction(null);
        setApprovalSummary(null);
      }
      if (payload.type === "task_completed") setTaskSummary(payload.summary);
      const nextActivity = eventActivity(payload, `${payload.type}-${Date.now()}`);
      if (nextActivity) {
        setActivity((current) => {
          const index = current.findIndex((item) => item.id === nextActivity.id);
          return index < 0 ? [...current, nextActivity] : current.map((item) => item.id === nextActivity.id ? nextActivity : item);
        });
      }
    }).then((stop) => { unlisten = stop; });
    return () => unlisten?.();
  }, []);

  async function submit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    const root = workspaceRoot.trim();
    const message = prompt.trim();
    if (!root || !message || isRunning) return;
    if (!isTauriRuntime) {
      setError("Open XCoding through Tauri to run a coding task.");
      return;
    }

    setError(null);
    setIsRunning(true);
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
    const params: ChatParams = { workspace_root: root, message, mode, provider: defaultProvider, model };
    try {
      const result = await invoke<ChatResult>("chat", { params });
      setActiveSessionId(result.session.id);
      const completedMessage = result.message;
      if (completedMessage) setMessages((current) => mergeMessage(current, completedMessage));
      setPrompt("");
      await refreshSessions();
      await hydrateSession(result.session.id);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setIsRunning(false);
    }
  }

  async function resolveAction(approved: boolean): Promise<void> {
    if (!pendingAction || !activeSessionId || isRunning) return;
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
    if (!activeSessionId || isRunning) return;
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

  async function saveWorkspaceConfig(): Promise<void> {
    const root = workspaceRoot.trim();
    if (!root || isSavingConfig || isRunning) return;
    if (!isTauriRuntime) {
      setError("Open XCoding through Tauri to save workspace defaults.");
      return;
    }
    setError(null);
    setIsSavingConfig(true);
    try {
      const config = await invoke<WorkspaceConfig>("set_workspace_config", {
        params: { workspace_root: root, mode, provider: defaultProvider, model },
      });
      setMode(config.mode);
      setModel(config.model);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setIsSavingConfig(false);
    }
  }

  return (
    <main className="workbench">
      <aside className="sessions-panel" aria-label="Sessions">
        <div className="brand-row">
          <div><p className="eyebrow">XCoding</p><h1>Sessions</h1></div>
          <button type="button" className="quiet-button" onClick={() => void refreshWorkspace()} aria-label="Refresh workspace">Refresh</button>
        </div>
        <label className="field-label" htmlFor="workspace-root">Workspace</label>
        <input id="workspace-root" value={workspaceRoot} onChange={(event) => setWorkspaceRoot(event.target.value)} placeholder="D:\\work\\project" spellCheck={false} />
        <section className="workspace-settings" aria-label="Workspace defaults">
          <p className="panel-title">Defaults</p>
          <div className={`auth-status ${providerStatus?.ready ? "ready" : "missing"}`} role="status">
            <strong>{providerStatus?.ready ? "Cloud ready" : "API key missing"}</strong>
            <small>{providerStatus?.message || "Checking provider credentials..."}</small>
            <small>Base {providerStatus?.base_url || "https://ai.v58.dev/v1"}{providerStatus?.key_hint ? ` · key ${providerStatus.key_hint}` : ""}</small>
            <button type="button" className="quiet-button" onClick={() => void refreshProviderStatus()} disabled={isRunning}>Refresh auth</button>
          </div>
          <label className="field-label" htmlFor="default-model">Model</label>
          <input id="default-model" value={model} onChange={(event) => setModel(event.target.value)} disabled={isRunning || isSavingConfig} spellCheck={false} />
          <button type="button" className="quiet-button" onClick={() => void saveWorkspaceConfig()} disabled={!workspaceRoot.trim() || isRunning || isSavingConfig}>{isSavingConfig ? "Saving..." : "Save defaults"}</button>
        </section>
        <nav className="session-list" aria-label="Saved sessions">
          {sessions.length === 0 ? <p className="empty-state">No saved sessions in this workspace.</p> : null}
          {sessions.map((session) => (
            <button type="button" className={`session-item ${session.id === activeSessionId ? "is-active" : ""}`} key={session.id} onClick={() => setActiveSessionId(session.id)}>
              <span>{sessionTitle(session)}</span><small>{session.status.replace("_", " ")}</small>
            </button>
          ))}
        </nav>
      </aside>

      <section className="chat-panel" aria-label="Coding conversation">
        <header className="chat-header">
          <div><p className="eyebrow">Cloud model · {activeSession?.model || model}</p><h2>{activeSession ? sessionTitle(activeSession) : "New coding task"}</h2></div>
          <div className="header-controls">
            {activeSession?.status === "need_user" ? <button type="button" className="quiet-button" onClick={() => void cancelSession()} disabled={isRunning}>Cancel</button> : null}
            <label className="mode-control">Mode<select value={mode} onChange={(event) => setMode(event.target.value as Mode)} disabled={isRunning}><option value="ask">Ask</option><option value="auto-edit">Auto edit</option></select></label>
          </div>
        </header>
        <div className="conversation" aria-live="polite">
          {messages.map((message) => <article className={`message message-${message.role}`} key={message.id}><p>{message.role}</p><div>{message.content}</div></article>)}
          {streamedText ? <article className="message message-assistant streaming"><p>assistant</p><div>{streamedText}</div></article> : null}
          {messages.length === 0 && !streamedText && !isRunning ? <p className="empty-state">Describe the repository task you want XCoding to inspect.</p> : null}
          {error ? <p className="error-message">{error}</p> : null}
        </div>
        <form className="composer" onSubmit={submit}>
          <textarea value={prompt} onChange={(event) => setPrompt(event.target.value)} placeholder="Ask about this codebase..." rows={4} disabled={isRunning} />
          <div className="composer-footer"><span>{workspaceRoot.trim() ? workspaceRoot : "Choose a workspace path"}</span><button type="submit" disabled={isRunning || !workspaceRoot.trim() || !prompt.trim()}>{isRunning ? "Working..." : "Send"}</button></div>
        </form>
      </section>

      <aside className="trace-panel" aria-label="Agent trace">
        {pendingAction ? (() => {
          const review = buildReviewPresentation(pendingAction, approvalSummary, Boolean(patchPreview));
          return (
            <section className={`review-panel${review.highRisk ? " high-risk" : ""}`} aria-label={review.title}>
              <p className="panel-title">Review</p>
              <div className="review-header">
                <strong>{review.title}</strong>
                {review.highRisk ? <span className="risk-badge">HIGH-RISK</span> : null}
              </div>
              <p className="review-summary">{review.summary}</p>
              {review.bodyKind === "patch" && patchPreview ? (
                <>
                  <code>{patchPreview.path}</code>
                  <pre className="diff-preview">
                    {buildPatchDiffLines(patchPreview).map((line, index) => (
                      <span key={index} className={`diff-line ${line.kind}`}>
                        {line.kind === "remove" ? `- ${line.text}` : line.kind === "add" ? `+ ${line.text}` : line.text}
                      </span>
                    ))}
                  </pre>
                </>
              ) : null}
              {review.bodyKind === "command" ? (
                <pre className="command-preview" aria-label="Command to approve">{review.commandText ?? JSON.stringify(pendingAction.tool_call.arguments, null, 2)}</pre>
              ) : null}
              {review.bodyKind === "generic" ? <code>{JSON.stringify(pendingAction.tool_call.arguments)}</code> : null}
              {review.highRisk ? (
                <p className="risk-hint">Shell or force-push style commands can change the system or remote git history. Approve only if you trust the exact command.</p>
              ) : null}
              <div className="review-actions">
                <button type="button" className="reject-button" onClick={() => void resolveAction(false)} disabled={isRunning}>Reject</button>
                <button type="button" className={review.highRisk ? "approve-risk-button" : undefined} onClick={() => void resolveAction(true)} disabled={isRunning}>
                  {review.highRisk ? "Approve high-risk" : "Approve"}
                </button>
              </div>
            </section>
          );
        })() : null}
        <section><p className="panel-title">Plan</p><ol className="plan-list">{plan.length === 0 ? <li className="empty-state">The plan appears when a task starts.</li> : null}{plan.map((step) => <li key={step.id}>{step.description}</li>)}</ol></section>
        <section><p className="panel-title">Restore points</p><div className="restore-list">{restorePoints.length === 0 ? <p className="empty-state">Applied patches appear here.</p> : null}{restorePoints.map((restorePoint) => <div className="restore-point" key={restorePoint.id}><div><strong>{restorePoint.path}</strong><small>{new Date(restorePoint.created_at).toLocaleString()}</small></div><button type="button" className="quiet-button" onClick={() => void rollbackRestorePoint(restorePoint)} disabled={isRunning || !restorePoint.applied_text}>Rollback</button></div>)}</div></section>
        <section><p className="panel-title">Replay</p><div className="restore-list">{replaySteps.length === 0 ? <p className="empty-state">Load a finished session to reconstruct major steps.</p> : null}{replaySteps.map((step, index) => <div className="restore-point" key={`${step.kind}-${index}`}><div><strong>{step.kind}{step.tool_name ? ` · ${step.tool_name}` : ""}</strong><small>{step.summary}</small></div>{typeof step.success === "boolean" ? <code>{step.success ? "ok" : "fail"}</code> : null}</div>)}</div><button type="button" className="quiet-button" onClick={() => void loadReplay()} disabled={!activeSessionId || isRunning}>Replay steps</button></section>
        {taskSummary ? <section className="task-summary"><div className="summary-header"><p className="panel-title">Task summary</p><div className="summary-actions"><button type="button" className="quiet-button" onClick={() => void copyText(formatTaskSummaryText(taskSummary))}>Copy summary</button>{taskSummary.git_status || taskSummary.git_diff ? <button type="button" className="quiet-button" onClick={() => void copyText(gitSnapshotText(taskSummary))}>Copy git</button> : null}</div></div><strong>{taskSummary.changed_files.length} changed file(s){typeof taskSummary.lines_added === "number" || typeof taskSummary.lines_removed === "number" ? <> · +{taskSummary.lines_added ?? 0}/−{taskSummary.lines_removed ?? 0}</> : null}</strong><small>{taskSummary.commands_succeeded}/{taskSummary.commands_run} command(s) succeeded{taskSummary.commands_failed ? `, ${taskSummary.commands_failed} failed` : ""}</small>{(taskSummary.file_changes?.length ?? 0) > 0 ? <ul className="file-change-list">{taskSummary.file_changes!.map((change) => <li key={change.path}><span className={`change-kind ${change.kind}`}>{fileChangeLabel(change.kind)}</span><code>{change.path}</code><small className="line-delta">+{change.lines_added}/−{change.lines_removed}</small></li>)}</ul> : taskSummary.changed_files.length > 0 ? <ul>{taskSummary.changed_files.map((path) => <li key={path}><code>{path}</code></li>)}</ul> : null}{taskSummary.git_branch ? <small>Branch {taskSummary.git_branch}</small> : null}{taskSummary.git_status ? <details className="summary-details"><summary>Git status</summary><pre className="summary-pre">{taskSummary.git_status}</pre></details> : null}{taskSummary.git_diff ? <details className="summary-details"><summary>Git diff</summary><pre className="summary-pre">{taskSummary.git_diff}</pre></details> : null}</section> : null}
        <section><p className="panel-title">Activity</p><div className="activity-list">{activity.length === 0 ? <p className="empty-state">Agent activity will be recorded here.</p> : null}{activity.map((item) => <article className={`activity ${item.state}`} key={item.id}><strong>{item.label}</strong><code>{item.detail}</code></article>)}</div></section>
      </aside>
    </main>
  );
}
