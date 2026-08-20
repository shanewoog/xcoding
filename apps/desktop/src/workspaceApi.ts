import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type GitEnvironment = {
  is_repo: boolean;
  branch: string | null;
  upstream: string | null;
  insertions: number;
  deletions: number;
  changed_files: number;
  status_lines: string[];
  local_branches: string[];
  root: string;
};

export type DirEntryInfo = {
  name: string;
  path: string;
  is_dir: boolean;
};

export type TerminalCommandResult = {
  command: string;
  cwd: string;
  exit_code: number | null;
  stdout: string;
  stderr: string;
};

export type GitNexusStatus = {
  available: boolean;
  indexed: boolean;
  up_to_date: boolean;
  detail: string;
  root: string;
};

export type GitNexusCommandResult = {
  command: string;
  exit_code: number | null;
  stdout: string;
  stderr: string;
};

export type GitNexusSymbol = {
  id?: string;
  uid?: string;
  name: string;
  filePath?: string;
  startLine?: number;
  endLine?: number;
};
export type BrowserBounds = {
  x: number;
  y: number;
  width: number;
  height: number;
};

export type BrowserNavigatedEvent = {
  session: string;
  url: string;
  title: string;
};

export type BrowserEnsureResult = {
  created: boolean;
  url: string | null;
};

export async function fetchGitEnvironment(
  workspaceRoot: string,
  includeBranches = false,
): Promise<GitEnvironment> {
  return invoke<GitEnvironment>("git_environment", {
    workspaceRoot,
    includeBranches,
  });
}

export async function listWorkspaceEntries(
  workspaceRoot: string,
  relativePath?: string,
): Promise<DirEntryInfo[]> {
  return invoke<DirEntryInfo[]>("list_workspace_entries", {
    workspaceRoot,
    relativePath: relativePath || null,
  });
}

export async function runTerminalCommand(
  workspaceRoot: string,
  command: string,
): Promise<TerminalCommandResult> {
  return invoke<TerminalCommandResult>("run_terminal_command", {
    workspaceRoot,
    command,
  });
}

export async function fetchGitNexusStatus(workspaceRoot: string): Promise<GitNexusStatus> {
  return invoke<GitNexusStatus>("gitnexus_status", { workspaceRoot });
}

export async function analyzeGitNexus(workspaceRoot: string): Promise<GitNexusCommandResult> {
  return invoke<GitNexusCommandResult>("gitnexus_analyze", { workspaceRoot });
}

export async function queryGitNexus(workspaceRoot: string, searchQuery: string): Promise<GitNexusCommandResult> {
  return invoke<GitNexusCommandResult>("gitnexus_query", { workspaceRoot, searchQuery });
}

export async function contextGitNexus(
  workspaceRoot: string,
  symbol: string,
  symbolUid?: string,
  filePath?: string,
): Promise<GitNexusCommandResult> {
  return invoke<GitNexusCommandResult>("gitnexus_context", { workspaceRoot, symbol, symbolUid: symbolUid ?? null, filePath: filePath ?? null });
}

export async function impactGitNexus(
  workspaceRoot: string,
  symbol: string,
  symbolUid?: string,
  filePath?: string,
): Promise<GitNexusCommandResult> {
  return invoke<GitNexusCommandResult>("gitnexus_impact", { workspaceRoot, symbol, symbolUid: symbolUid ?? null, filePath: filePath ?? null });
}
export async function openPath(path: string): Promise<void> {
  await invoke("open_path", { path });
}

export async function openExternalUrl(url: string): Promise<void> {
  await invoke("open_external_url", { url });
}

export async function browserEnsure(
  session: string,
  bounds: BrowserBounds,
  url?: string | null,
  userAgent?: string | null,
): Promise<BrowserEnsureResult> {
  return invoke<BrowserEnsureResult>("browser_ensure", {
    session,
    x: bounds.x,
    y: bounds.y,
    width: bounds.width,
    height: bounds.height,
    url: url ?? null,
    userAgent: userAgent ?? null,
  });
}

export async function browserSetBounds(session: string, bounds: BrowserBounds): Promise<void> {
  await invoke("browser_set_bounds", {
    session,
    x: bounds.x,
    y: bounds.y,
    width: bounds.width,
    height: bounds.height,
  });
}

export async function browserNavigate(session: string, url: string): Promise<void> {
  await invoke("browser_navigate", { session, url });
}

export async function browserReload(session: string): Promise<void> {
  await invoke("browser_reload", { session });
}

export async function browserForceReload(session: string): Promise<void> {
  await invoke("browser_force_reload", { session });
}

export async function browserBack(session: string): Promise<void> {
  await invoke("browser_back", { session });
}

export async function browserForward(session: string): Promise<void> {
  await invoke("browser_forward", { session });
}

export async function browserShow(session: string): Promise<void> {
  await invoke("browser_show", { session });
}

export async function browserHide(session: string): Promise<void> {
  await invoke("browser_hide", { session });
}

export async function browserClose(session: string): Promise<void> {
  await invoke("browser_close", { session });
}

// Promoting a draft task keeps its live page: the webview is remapped, not rebuilt.
export async function browserAdoptSession(from: string, to: string): Promise<void> {
  await invoke("browser_adopt_session", { from, to });
}

export async function browserSetUserAgent(session: string, userAgent?: string | null): Promise<void> {
  await invoke("browser_set_user_agent", { session, userAgent: userAgent ?? null });
}

export async function browserSetZoom(session: string, scaleFactor: number): Promise<void> {
  await invoke("browser_set_zoom", { session, scaleFactor });
}

export async function browserPrint(session: string): Promise<void> {
  await invoke("browser_print", { session });
}

export async function browserClearData(session: string): Promise<void> {
  await invoke("browser_clear_data", { session });
}

export async function browserCurrentUrl(session: string): Promise<string | null> {
  return invoke<string | null>("browser_current_url", { session });
}

export async function browserEval(session: string, script: string): Promise<void> {
  await invoke("browser_eval", { session, script });
}

export async function browserFind(
  session: string,
  query: string,
  options?: { forward?: boolean; matchCase?: boolean },
): Promise<void> {
  await invoke("browser_find", {
    session,
    query,
    forward: options?.forward ?? true,
    matchCase: options?.matchCase ?? false,
  });
}

export async function browserDownloadDir(): Promise<string> {
  return invoke<string>("browser_download_dir");
}

export type BrowserPasswordEntry = {
  id: string;
  origin: string;
  username: string;
  updatedAt: number;
};

export type BrowserCapturedPassword = {
  origin: string;
  username: string;
};

export async function browserPasswordsList(): Promise<BrowserPasswordEntry[]> {
  return invoke<BrowserPasswordEntry[]>("browser_passwords_list");
}

export async function browserPasswordSave(
  origin: string,
  username: string,
  password: string,
): Promise<BrowserPasswordEntry> {
  return invoke<BrowserPasswordEntry>("browser_password_save", { origin, username, password });
}

export async function browserPasswordDelete(id: string): Promise<boolean> {
  return invoke<boolean>("browser_password_delete", { id });
}

// The plaintext only travels for an explicit reveal, so callers must not store it.
export async function browserPasswordReveal(id: string): Promise<string> {
  return invoke<string>("browser_password_reveal", { id });
}

export async function browserPasswordCapture(session: string): Promise<BrowserCapturedPassword | null> {
  return invoke<BrowserCapturedPassword | null>("browser_password_capture", { session });
}

export async function browserPasswordFill(session: string): Promise<boolean> {
  return invoke<boolean>("browser_password_fill", { session });
}

export async function browserScreenshot(session: string): Promise<string> {
  return invoke<string>("browser_save_snapshot", { session });
}


export async function onBrowserNavigated(
  handler: (event: BrowserNavigatedEvent) => void,
): Promise<UnlistenFn> {
  return listen<BrowserNavigatedEvent>("browser-navigated", (event) => {
    handler(event.payload);
  });
}

export function formatDiffStat(insertions: number, deletions: number): string {
  const add = insertions.toLocaleString();
  const del = deletions.toLocaleString();
  return `+${add}/-${del}`;
}

export function normalizeBrowserUrl(raw: string): string {
  const trimmed = raw.trim();
  if (!trimmed) return "";
  if (/^https?:\/\//i.test(trimmed)) return trimmed;
  if (/^about:/i.test(trimmed)) return trimmed;
  return `https://${trimmed}`;
}
