import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useState } from "react";
import type { FormEvent } from "react";
import type {
  ChatParams,
  ChatResult,
  Mode,
  PatchPreview,
  PendingAction,
  PlanStep,
  ResolveActionResult,
  Session,
  SessionEvent,
} from "@xcoding/protocol";

type Activity = {
  id: string;
  kind: "tool" | "error";
  label: string;
  detail: string;
  state: "running" | "done" | "failed";
};

const defaultModel = "gpt-4.1";
const isTauriRuntime = "__TAURI_INTERNALS__" in window;

function sessionTitle(session: Session): string {
  return session.title?.trim() || `${session.workspace_root.split(/[\\/]/).pop() || "Workspace"} session`;
}

function eventActivity(event: SessionEvent): Activity | null {
  if (event.type === "tool_start") return { id: event.tool_call.id, kind: "tool", label: event.summary, detail: JSON.stringify(event.tool_call.arguments), state: "running" };
  if (event.type === "tool_end") return { id: event.tool_call.id, kind: "tool", label: event.summary, detail: JSON.stringify(event.tool_call.arguments), state: event.success ? "done" : "failed" };
  if (event.type === "error") return { id: `error-${Date.now()}`, kind: "error", label: "Agent error", detail: event.message, state: "failed" };
  return null;
}

export function App() {
  const [workspaceRoot, setWorkspaceRoot] = useState("");
  const [prompt, setPrompt] = useState("");
  const [mode, setMode] = useState<Mode>("ask");
  const [sessions, setSessions] = useState<Session[]>([]);
  const [activeSessionId, setActiveSessionId] = useState<string | null>(null);
  const [streamedText, setStreamedText] = useState("");
  const [completedText, setCompletedText] = useState("");
  const [plan, setPlan] = useState<PlanStep[]>([]);
  const [activity, setActivity] = useState<Activity[]>([]);
  const [pendingAction, setPendingAction] = useState<PendingAction | null>(null);
  const [patchPreview, setPatchPreview] = useState<PatchPreview | null>(null);
  const [isRunning, setIsRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const activeSession = useMemo(() => sessions.find((session) => session.id === activeSessionId) ?? null, [activeSessionId, sessions]);

  const refreshSessions = useCallback(async () => {
    if (!isTauriRuntime) return;
    try {
      const nextSessions = await invoke<Session[]>("list_sessions", { workspaceRoot: workspaceRoot.trim() || null });
      setSessions(nextSessions);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }, [workspaceRoot]);

  useEffect(() => { void refreshSessions(); }, [refreshSessions]);

  useEffect(() => {
    if (!isTauriRuntime) return;
    let unlisten: (() => void) | undefined;
    void listen<SessionEvent>("session-event", (event) => {
      const payload = event.payload;
      setActiveSessionId((current) => current ?? payload.session_id);
      if (payload.type === "text_delta") setStreamedText((current) => current + payload.delta);
      if (payload.type === "message_completed") { setCompletedText(payload.message.content); setStreamedText(""); }
      if (payload.type === "plan") setPlan(payload.steps);
      if (payload.type === "patch_preview") setPatchPreview(payload.preview);
      if (payload.type === "approval_requested") setPendingAction(payload.action);
      const nextActivity = eventActivity(payload);
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
    if (!isTauriRuntime) { setError("Open XCoding through Tauri to run a coding task."); return; }

    setError(null); setIsRunning(true); setActiveSessionId(null); setStreamedText(""); setCompletedText(""); setPlan([]); setActivity([]); setPendingAction(null); setPatchPreview(null);
    const params: ChatParams = { workspace_root: root, message, mode, provider: "openai", model: defaultModel };
    try {
      const result = await invoke<ChatResult>("chat", { params });
      setActiveSessionId(result.session.id);
      setCompletedText(result.message?.content ?? "");
      setPrompt("");
      await refreshSessions();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setIsRunning(false);
    }
  }

  async function resolveAction(approved: boolean): Promise<void> {
    if (!pendingAction || !activeSessionId || isRunning) return;
    setError(null); setIsRunning(true);
    try {
      const result = await invoke<ResolveActionResult>("resolve_action", {
        params: { session_id: activeSessionId, action_id: pendingAction.id, approved },
      });
      setCompletedText(result.message?.content ?? "");
      setPendingAction(null); setPatchPreview(null);
      await refreshSessions();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setIsRunning(false);
    }
  }

  return (
    <main className="workbench">
      <aside className="sessions-panel" aria-label="Sessions">
        <div className="brand-row"><div><p className="eyebrow">XCoding</p><h1>Sessions</h1></div><button type="button" className="quiet-button" onClick={() => void refreshSessions()} aria-label="Refresh sessions">Refresh</button></div>
        <label className="field-label" htmlFor="workspace-root">Workspace</label>
        <input id="workspace-root" value={workspaceRoot} onChange={(event) => setWorkspaceRoot(event.target.value)} placeholder="D:\work\project" spellCheck={false} />
        <nav className="session-list" aria-label="Saved sessions">
          {sessions.length === 0 ? <p className="empty-state">No saved sessions in this workspace.</p> : null}
          {sessions.map((session) => <button type="button" className={`session-item ${session.id === activeSessionId ? "is-active" : ""}`} key={session.id} onClick={() => setActiveSessionId(session.id)}><span>{sessionTitle(session)}</span><small>{session.status.replace("_", " ")}</small></button>)}
        </nav>
      </aside>

      <section className="chat-panel" aria-label="Coding conversation">
        <header className="chat-header"><div><p className="eyebrow">Cloud model</p><h2>{activeSession ? sessionTitle(activeSession) : "New coding task"}</h2></div><label className="mode-control">Mode<select value={mode} onChange={(event) => setMode(event.target.value as Mode)} disabled={isRunning}><option value="ask">Ask</option><option value="auto-edit">Auto edit</option></select></label></header>
        <div className="conversation" aria-live="polite">
          {completedText ? <article className="assistant-message">{completedText}</article> : null}
          {streamedText ? <article className="assistant-message streaming">{streamedText}</article> : null}
          {!completedText && !streamedText && !isRunning ? <p className="empty-state">Describe the repository task you want XCoding to inspect.</p> : null}
          {error ? <p className="error-message">{error}</p> : null}
        </div>
        <form className="composer" onSubmit={submit}><textarea value={prompt} onChange={(event) => setPrompt(event.target.value)} placeholder="Ask about this codebase..." rows={4} disabled={isRunning} /><div className="composer-footer"><span>{workspaceRoot.trim() ? workspaceRoot : "Choose a workspace path"}</span><button type="submit" disabled={isRunning || !workspaceRoot.trim() || !prompt.trim()}>{isRunning ? "Working..." : "Send"}</button></div></form>
      </section>

      <aside className="trace-panel" aria-label="Agent trace">
        {pendingAction ? <section className="review-panel"><p className="panel-title">Review</p><strong>{pendingAction.tool_call.name === "apply_patch" ? "Patch approval" : "Command approval"}</strong>{patchPreview ? <><code>{patchPreview.path}</code><pre className="diff-preview"><span>- {patchPreview.old_text || "(new file)"}</span><span>+ {patchPreview.new_text}</span></pre></> : <code>{JSON.stringify(pendingAction.tool_call.arguments)}</code>}<div className="review-actions"><button type="button" className="reject-button" onClick={() => void resolveAction(false)} disabled={isRunning}>Reject</button><button type="button" onClick={() => void resolveAction(true)} disabled={isRunning}>Approve</button></div></section> : null}
        <section><p className="panel-title">Plan</p><ol className="plan-list">{plan.length === 0 ? <li className="empty-state">The plan appears when a task starts.</li> : null}{plan.map((step) => <li key={step.id}>{step.description}</li>)}</ol></section>
        <section><p className="panel-title">Activity</p><div className="activity-list">{activity.length === 0 ? <p className="empty-state">Agent activity will be recorded here.</p> : null}{activity.map((item) => <article className={`activity ${item.state}`} key={item.id}><strong>{item.label}</strong><code>{item.detail}</code></article>)}</div></section>
      </aside>
    </main>
  );
}