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
  url: string;
  title: string;
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
  bounds: BrowserBounds,
  url?: string | null,
  userAgent?: string | null,
): Promise<void> {
  await invoke("browser_ensure", {
    x: bounds.x,
    y: bounds.y,
    width: bounds.width,
    height: bounds.height,
    url: url ?? null,
    userAgent: userAgent ?? null,
  });
}

export async function browserSetBounds(bounds: BrowserBounds): Promise<void> {
  await invoke("browser_set_bounds", {
    x: bounds.x,
    y: bounds.y,
    width: bounds.width,
    height: bounds.height,
  });
}

export async function browserNavigate(url: string): Promise<void> {
  await invoke("browser_navigate", { url });
}

export async function browserReload(): Promise<void> {
  await invoke("browser_reload");
}

export async function browserBack(): Promise<void> {
  await invoke("browser_back");
}

export async function browserForward(): Promise<void> {
  await invoke("browser_forward");
}

export async function browserShow(): Promise<void> {
  await invoke("browser_show");
}

export async function browserHide(): Promise<void> {
  await invoke("browser_hide");
}

export async function browserClose(): Promise<void> {
  await invoke("browser_close");
}

export async function browserSetUserAgent(userAgent?: string | null): Promise<void> {
  await invoke("browser_set_user_agent", { userAgent: userAgent ?? null });
}

export async function browserSetZoom(scaleFactor: number): Promise<void> {
  await invoke("browser_set_zoom", { scaleFactor });
}

export async function browserPrint(): Promise<void> {
  await invoke("browser_print");
}

export async function browserClearData(): Promise<void> {
  await invoke("browser_clear_data");
}

export async function browserCurrentUrl(): Promise<string | null> {
  return invoke<string | null>("browser_current_url");
}

export async function browserEval(script: string): Promise<void> {
  await invoke("browser_eval", { script });
}

export async function browserFind(
  query: string,
  options?: { forward?: boolean; matchCase?: boolean },
): Promise<void> {
  await invoke("browser_find", {
    query,
    forward: options?.forward ?? true,
    matchCase: options?.matchCase ?? false,
  });
}

export async function browserDownloadDir(): Promise<string> {
  return invoke<string>("browser_download_dir");
}

export async function browserScreenshot(): Promise<string> {
  return invoke<string>("browser_screenshot");
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
