import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { FormEvent, KeyboardEvent as ReactKeyboardEvent, MouseEvent as ReactMouseEvent } from "react";
import { t, type Locale } from "./i18n";
import {
  analyzeGitNexus,
  browserBack,
  contextGitNexus,
  browserClearData,
  browserDownloadDir,
  browserEval,
  browserEnsure,
  browserFind,
  browserForward,
  browserHide,
  browserNavigate,
  browserPrint,
  browserReload,
  browserScreenshot,
  browserSetBounds,
  browserSetUserAgent,
  browserSetZoom,
  browserShow,
  fetchGitEnvironment,
  fetchGitNexusStatus,
  formatDiffStat,
  listWorkspaceEntries,
  normalizeBrowserUrl,
  onBrowserNavigated,
  openExternalUrl,
  openPath,
  queryGitNexus,
  impactGitNexus,
  runTerminalCommand,
  type DirEntryInfo,
  type GitEnvironment,
  type GitNexusCommandResult,
  type GitNexusStatus,
  type GitNexusSymbol,
} from "./workspaceApi";

export type ToolPanelTab = "review" | "browser" | "files" | "code";
export type PanelTarget = "terminal" | ToolPanelTab;
export type BrowserNavigationRequest = { url: string; id: number };

export type SourceItem = {
  id: string;
  name: string;
  kind: "image" | "file" | "link";
};

type EmptyQuickActionsProps = {
  locale: Locale;
  onOpen: (tab: PanelTarget) => void;
};

type EnvironmentPopoverProps = {
  locale: Locale;
  open: boolean;
  onClose: () => void;
  workspaceRoot: string;
  sources: SourceItem[];
  onOpenTerminal: (seedCommand?: string) => void;
  anchorRef: React.RefObject<HTMLElement | null>;
};

type TerminalBottomPanelProps = {
  locale: Locale;
  open: boolean;
  onClose: () => void;
  workspaceRoot: string;
};

type RightToolsPanelProps = {
  locale: Locale;
  open: boolean;
  tab: ToolPanelTab;
  browserNavigation: BrowserNavigationRequest | null;
  onTabChange: (tab: ToolPanelTab) => void;
  onClose: () => void;
  workspaceRoot: string;
  reviewContent: React.ReactNode;
  width: number;
  onWidthChange: (width: number) => void;
};

function joinWorkspacePath(root: string, relative: string): string {
  const base = root.replace(/[\\/]+$/, "");
  const rel = relative.replace(/^[\\/]+/, "");
  if (!rel) return base;
  return `${base}\\${rel.replace(/\//g, "\\")}`;
}

function parentRelative(path: string): string {
  const parts = path.replace(/\//g, "\\").split("\\").filter(Boolean);
  parts.pop();
  return parts.join("\\");
}

export function EmptyQuickActions({ locale, onOpen }: EmptyQuickActionsProps) {
  const actions: Array<{ tab: PanelTarget; labelKey: "panel.review" | "panel.terminal" | "panel.browser" | "panel.files" | "panel.code"; shortcut?: string }> = [
    { tab: "review", labelKey: "panel.review", shortcut: "Ctrl+Shift+G" },
    { tab: "terminal", labelKey: "panel.terminal" },
    { tab: "browser", labelKey: "panel.browser", shortcut: "Ctrl+T" },
    { tab: "files", labelKey: "panel.files", shortcut: "Ctrl+P" },
    { tab: "code", labelKey: "panel.code" },
  ];

  return (
    <div className="empty-quick-actions" role="list">
      {actions.map((action) => (
        <button
          key={action.tab}
          type="button"
          className="empty-quick-action"
          role="listitem"
          onClick={() => onOpen(action.tab)}
        >
          <span className="empty-quick-action-label">
            <span className="empty-quick-action-icon" aria-hidden="true">
              {action.tab === "review" ? "▣" : action.tab === "terminal" ? ">_ " : action.tab === "browser" ? "◎" : action.tab === "code" ? "⌘" : "📁"}
            </span>
            {t(locale, action.labelKey)}
          </span>
          {action.shortcut ? <kbd>{action.shortcut}</kbd> : <span />}
        </button>
      ))}
    </div>
  );
}

export function EnvironmentPopover({
  locale,
  open,
  onClose,
  workspaceRoot,
  sources,
  onOpenTerminal,
  anchorRef,
}: EnvironmentPopoverProps) {
  const [env, setEnv] = useState<GitEnvironment | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [changesOpen, setChangesOpen] = useState(false);
  const [branchesOpen, setBranchesOpen] = useState(false);
  const panelRef = useRef<HTMLDivElement | null>(null);

  const refresh = useCallback(async () => {
    if (!workspaceRoot.trim()) {
      setEnv(null);
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const next = await fetchGitEnvironment(workspaceRoot.trim(), true);
      setEnv(next);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoading(false);
    }
  }, [workspaceRoot]);

  useEffect(() => {
    if (!open) return;
    void refresh();
  }, [open, refresh]);

  useEffect(() => {
    if (!open) return;
    const onDoc = (event: MouseEvent) => {
      const target = event.target as Node | null;
      if (!target) return;
      if (panelRef.current?.contains(target)) return;
      if (anchorRef.current?.contains(target)) return;
      onClose();
    };
    const onKey = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("mousedown", onDoc);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDoc);
      window.removeEventListener("keydown", onKey);
    };
  }, [open, onClose, anchorRef]);

  if (!open) return null;

  const branch = env?.branch || t(locale, "env.noBranch");
  const diff = env ? formatDiffStat(env.insertions, env.deletions) : "+0/-0";

  return (
    <div className="env-popover" ref={panelRef} role="dialog" aria-label={t(locale, "env.title")}>
      <div className="env-popover-header">
        <strong>{t(locale, "env.title")}</strong>
        <button type="button" className="quiet-button env-icon-button" onClick={() => void refresh()} title={t(locale, "action.refresh")}>
          ↻
        </button>
      </div>
      {loading ? <p className="env-muted">{t(locale, "env.loading")}</p> : null}
      {error ? <p className="error-message">{error}</p> : null}
      {!workspaceRoot.trim() ? <p className="env-muted">{t(locale, "env.needProject")}</p> : null}
      {workspaceRoot.trim() && env && !env.is_repo ? <p className="env-muted">{t(locale, "env.notRepo")}</p> : null}

      <button type="button" className="env-row" onClick={() => setChangesOpen((value) => !value)}>
        <span className="env-row-main">
          <span className="env-row-icon" aria-hidden="true">▦</span>
          {t(locale, "env.changes")}
        </span>
        <span className={`env-diff${env && (env.insertions > 0 || env.deletions > 0) ? " has-changes" : ""}`}>{diff}</span>
      </button>
      {changesOpen ? (
        <div className="env-sublist">
          {(env?.status_lines || []).length === 0 ? (
            <p className="env-muted">{t(locale, "env.noChanges")}</p>
          ) : (
            (env?.status_lines || []).slice(0, 40).map((line) => (
              <code key={line} className="env-status-line">{line}</code>
            ))
          )}
        </div>
      ) : null}

      <div className="env-row static">
        <span className="env-row-main">
          <span className="env-row-icon" aria-hidden="true">⌂</span>
          {t(locale, "env.local")}
        </span>
        <span className="env-muted">{env?.changed_files ?? 0}</span>
      </div>

      <button type="button" className="env-row" onClick={() => setBranchesOpen((value) => !value)}>
        <span className="env-row-main">
          <span className="env-row-icon" aria-hidden="true">⌥</span>
          {branch}
        </span>
        <span className="env-chevron">{branchesOpen ? "▾" : "▸"}</span>
      </button>
      {branchesOpen ? (
        <div className="env-sublist">
          {(env?.local_branches || []).length === 0 ? (
            <p className="env-muted">{t(locale, "env.noBranches")}</p>
          ) : (
            (env?.local_branches || []).map((name) => (
              <button
                key={name}
                type="button"
                className={`env-branch${name === env?.branch ? " current" : ""}`}
                onClick={() => onOpenTerminal(`git switch ${name}`)}
              >
                {name}
              </button>
            ))
          )}
        </div>
      ) : null}

      <button
        type="button"
        className="env-row"
        onClick={() => onOpenTerminal("git status; git log -5 --oneline")}
      >
        <span className="env-row-main">
          <span className="env-row-icon" aria-hidden="true">↻</span>
          {t(locale, "env.commitPush")}
        </span>
      </button>
      <button
        type="button"
        className="env-row"
        onClick={() => onOpenTerminal(env?.upstream ? `git log --oneline HEAD...${env.upstream}` : "git branch -vv")}
      >
        <span className="env-row-main">
          <span className="env-row-icon" aria-hidden="true">⇄</span>
          {t(locale, "env.compare")}
        </span>
        <span className="env-chevron">↗</span>
      </button>

      <div className="env-section-title">
        <span>{t(locale, "env.sources")}</span>
      </div>
      <div className="env-sublist sources">
        {sources.length === 0 ? (
          <p className="env-muted">{t(locale, "env.noSources")}</p>
        ) : (
          sources.slice(0, 8).map((source) => (
            <div key={source.id} className="env-source-item" title={source.name}>
              <span className="env-row-icon" aria-hidden="true">{source.kind === "image" ? "🖼" : source.kind === "link" ? "🔗" : "📄"}</span>
              <span className="env-source-name">{source.name}</span>
            </div>
          ))
        )}
      </div>
    </div>
  );
}

export function TerminalBottomPanel({
  locale,
  open,
  onClose,
  workspaceRoot,
}: TerminalBottomPanelProps) {
  const [terminalLines, setTerminalLines] = useState<string[]>([]);
  const [terminalInput, setTerminalInput] = useState("");
  const [terminalBusy, setTerminalBusy] = useState(false);
  const terminalEndRef = useRef<HTMLDivElement | null>(null);
  const seedRef = useRef<string | null>(null);

  const cwdLabel = useMemo(
    () => workspaceRoot.trim() || t(locale, "composer.chooseWorkspace"),
    [workspaceRoot, locale],
  );

  const appendTerminal = useCallback((chunk: string) => {
    setTerminalLines((current) => {
      const next = [...current, ...chunk.replace(/\r\n/g, "\n").split("\n")];
      return next.slice(-400);
    });
  }, []);

  const runCommand = useCallback(
    async (command: string) => {
      const trimmed = command.trim();
      if (!trimmed || !workspaceRoot.trim() || terminalBusy) return;
      setTerminalBusy(true);
      appendTerminal(`PS ${cwdLabel}> ${trimmed}`);
      try {
        const result = await runTerminalCommand(workspaceRoot.trim(), trimmed);
        if (result.stdout.trim()) appendTerminal(result.stdout.replace(/\s+$/, ""));
        if (result.stderr.trim()) appendTerminal(result.stderr.replace(/\s+$/, ""));
        if (result.exit_code !== null && result.exit_code !== 0) {
          appendTerminal(t(locale, "terminal.exitCode", { code: String(result.exit_code) }));
        }
      } catch (cause) {
        appendTerminal(cause instanceof Error ? cause.message : String(cause));
      } finally {
        setTerminalBusy(false);
      }
    },
    [appendTerminal, cwdLabel, locale, terminalBusy, workspaceRoot],
  );

  useEffect(() => {
    const handler = (event: Event) => {
      const detail = (event as CustomEvent<string>).detail;
      if (typeof detail === "string" && detail.trim()) {
        seedRef.current = detail.trim();
        setTerminalInput(detail.trim());
      }
    };
    window.addEventListener("xcoding-terminal-seed", handler as EventListener);
    return () => window.removeEventListener("xcoding-terminal-seed", handler as EventListener);
  }, []);

  useEffect(() => {
    if (!open) return;
    if (terminalLines.length === 0 && workspaceRoot.trim()) {
      appendTerminal(t(locale, "terminal.ready", { cwd: workspaceRoot.trim() }));
    }
    const seed = seedRef.current;
    if (seed) {
      seedRef.current = null;
      void runCommand(seed);
    }
  }, [open, terminalLines.length, workspaceRoot, appendTerminal, locale, runCommand]);

  useEffect(() => {
    terminalEndRef.current?.scrollIntoView({ block: "end" });
  }, [terminalLines, terminalBusy]);

  if (!open) return null;

  const onTerminalSubmit = (event: FormEvent) => {
    event.preventDefault();
    const command = terminalInput;
    setTerminalInput("");
    void runCommand(command);
  };

  const onTerminalKeyDown = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter") {
      event.preventDefault();
      const command = terminalInput;
      setTerminalInput("");
      void runCommand(command);
    }
  };

  return (
    <section className="bottom-panel terminal-only" aria-label={t(locale, "panel.terminal")}>
      <div className="bottom-panel-tabs">
        <span className="bottom-panel-tab active">{t(locale, "panel.terminal")}</span>
        <button type="button" className="bottom-panel-close" onClick={onClose} title={t(locale, "panel.toggle")}>
          ×
        </button>
      </div>
      <div className="bottom-panel-body">
        <div className="bottom-terminal">
          <div className="bottom-terminal-meta">
            <span className="bottom-terminal-chip" title={cwdLabel}>
              {cwdLabel}
            </span>
          </div>
          <div className="bottom-terminal-output" aria-live="polite">
            {terminalLines.map((line, index) => (
              <div key={`${index}-${line.slice(0, 24)}`} className="bottom-terminal-line">
                {line || " "}
              </div>
            ))}
            <div ref={terminalEndRef} />
          </div>
          <form className="bottom-terminal-input-row" onSubmit={onTerminalSubmit}>
            <span className="bottom-terminal-prompt">PS&gt;</span>
            <input
              value={terminalInput}
              onChange={(event) => setTerminalInput(event.target.value)}
              onKeyDown={onTerminalKeyDown}
              disabled={!workspaceRoot.trim() || terminalBusy}
              placeholder={t(locale, "terminal.placeholder")}
              spellCheck={false}
              autoComplete="off"
            />
          </form>
        </div>
      </div>
    </section>
  );
}


type BrowserTabState = {
  id: string;
  title: string;
  url: string;
  input: string;
}

type BrowserOpenTarget = "browser" | "system";
type BrowserApprovals = "alwaysAsk" | "autoAllow";
type BrowserAnnotated = "always" | "never";
type BrowserSettingsSection =
  | "general"
  | "autofill"
  | "downloads"
  | "permissions"
  | "site"
  | "developer";

type BrowserSettingsState = {
  openWebTarget: BrowserOpenTarget;
  openLocalTarget: BrowserOpenTarget;
  annotatedScreenshots: BrowserAnnotated;
  askDownloadLocation: boolean;
  approvals: BrowserApprovals;
  cdpAccess: boolean;
};

type DevicePresetId =
  | "responsive"
  | "4k"
  | "laptop-large"
  | "laptop"
  | "surface-pro-7"
  | "ipad-air"
  | "ipad-mini"
  | "surface-duo"
  | "iphone-15-pro-max"
  | "pixel-8"
  | "iphone-15-pro"
  | "samsung-galaxy-s24-ultra";

type DevicePreset = {
  id: DevicePresetId;
  label: string;
  width: number;
  height: number;
  userAgent?: string;
};

const WINDOWS_CHROME_USER_AGENT =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
const IPAD_USER_AGENT =
  "Mozilla/5.0 (iPad; CPU OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";
const IPHONE_USER_AGENT =
  "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";
const SURFACE_DUO_USER_AGENT =
  "Mozilla/5.0 (Linux; Android 12; Surface Duo) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Mobile Safari/537.36";
const PIXEL_8_USER_AGENT =
  "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Mobile Safari/537.36";
const GALAXY_S24_ULTRA_USER_AGENT =
  "Mozilla/5.0 (Linux; Android 14; SM-S928B) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Mobile Safari/537.36";

const BROWSER_SETTINGS_KEY = "xcoding.browserSettings.v1";

const DEFAULT_BROWSER_SETTINGS: BrowserSettingsState = {
  openWebTarget: "browser",
  openLocalTarget: "browser",
  annotatedScreenshots: "always",
  askDownloadLocation: false,
  approvals: "alwaysAsk",
  cdpAccess: false,
};

const DEVICE_PRESETS: DevicePreset[] = [
  { id: "responsive", label: "Responsive", width: 0, height: 0 },
  { id: "4k", label: "4K", width: 3840, height: 2160, userAgent: WINDOWS_CHROME_USER_AGENT },
  { id: "laptop-large", label: "Laptop L", width: 1440, height: 900, userAgent: WINDOWS_CHROME_USER_AGENT },
  { id: "laptop", label: "笔记本电脑", width: 1280, height: 800, userAgent: WINDOWS_CHROME_USER_AGENT },
  { id: "surface-pro-7", label: "Surface Pro 7", width: 912, height: 1368, userAgent: WINDOWS_CHROME_USER_AGENT },
  { id: "ipad-air", label: "iPad Air", width: 820, height: 1180, userAgent: IPAD_USER_AGENT },
  { id: "ipad-mini", label: "iPad Mini", width: 744, height: 1133, userAgent: IPAD_USER_AGENT },
  { id: "surface-duo", label: "Surface Duo", width: 540, height: 720, userAgent: SURFACE_DUO_USER_AGENT },
  { id: "iphone-15-pro-max", label: "iPhone 15 Pro Max", width: 430, height: 932, userAgent: IPHONE_USER_AGENT },
  { id: "pixel-8", label: "Pixel 8", width: 412, height: 915, userAgent: PIXEL_8_USER_AGENT },
  { id: "iphone-15-pro", label: "iPhone 15 Pro", width: 393, height: 852, userAgent: IPHONE_USER_AGENT },
  { id: "samsung-galaxy-s24-ultra", label: "Samsung Galaxy S24 Ultra", width: 412, height: 915, userAgent: GALAXY_S24_ULTRA_USER_AGENT },
];

function loadBrowserSettings(): BrowserSettingsState {
  try {
    if (typeof localStorage === "undefined") return { ...DEFAULT_BROWSER_SETTINGS };
    const raw = localStorage.getItem(BROWSER_SETTINGS_KEY);
    if (!raw) return { ...DEFAULT_BROWSER_SETTINGS };
    const parsed = JSON.parse(raw) as Partial<BrowserSettingsState>;
    return {
      openWebTarget: parsed.openWebTarget === "system" ? "system" : "browser",
      openLocalTarget: parsed.openLocalTarget === "system" ? "system" : "browser",
      annotatedScreenshots: parsed.annotatedScreenshots === "never" ? "never" : "always",
      askDownloadLocation: Boolean(parsed.askDownloadLocation),
      approvals: parsed.approvals === "autoAllow" ? "autoAllow" : "alwaysAsk",
      cdpAccess: Boolean(parsed.cdpAccess),
    };
  } catch {
    return { ...DEFAULT_BROWSER_SETTINGS };
  }
}

function saveBrowserSettings(next: BrowserSettingsState): void {
  try {
    if (typeof localStorage !== "undefined") {
      localStorage.setItem(BROWSER_SETTINGS_KEY, JSON.stringify(next));
    }
  } catch {
    // ignore storage access failures
  }
}

function createBrowserTab(partial?: Partial<BrowserTabState>): BrowserTabState {
  const id = partial?.id || `tab-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 7)}`;
  return {
    id,
    title: partial?.title || "",
    url: partial?.url || "",
    input: partial?.input || "",
  };
}

function isLocalDevUrl(url: string): boolean {
  try {
    const parsed = new URL(url);
    const host = parsed.hostname.toLowerCase();
    return host === "localhost" || host === "127.0.0.1" || host === "::1" || host.endsWith(".local");
  } catch {
    return /^(localhost|127\.0\.0\.1|\[::1\])/i.test(url.trim());
  }
}

const BROWSER_SCROLLBAR_STYLE = `
  :root,
  body,
  * {
    scrollbar-color: #3a4350 transparent !important;
    scrollbar-width: thin !important;
  }
  *::-webkit-scrollbar {
    width: 10px !important;
    height: 10px !important;
  }
  *::-webkit-scrollbar-track {
    background: transparent !important;
  }
  *::-webkit-scrollbar-thumb {
    background: #3a4350 !important;
    border: 2px solid transparent !important;
    border-radius: 999px !important;
    background-clip: padding-box !important;
  }
  *::-webkit-scrollbar-thumb:hover {
    background: #4a5563 !important;
    background-clip: padding-box !important;
  }
`;

function BuiltInBrowserPanel({
  locale,
  active,
  navigation,
}: {
  locale: Locale;
  active: boolean;
  navigation: BrowserNavigationRequest | null;
}) {
  const [tabs, setTabs] = useState<BrowserTabState[]>(() => [createBrowserTab()]);
  const [activeTabId, setActiveTabId] = useState(() => tabs[0]?.id || "");
  const [menuOpen, setMenuOpen] = useState(false);
  const [zoom, setZoom] = useState(1);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [findOpen, setFindOpen] = useState(false);
  const [findQuery, setFindQuery] = useState("");
  const [deviceOpen, setDeviceOpen] = useState(false);
  const [devicePreset, setDevicePreset] = useState<DevicePresetId>("responsive");
  const [deviceWidth, setDeviceWidth] = useState(390);
  const [deviceHeight, setDeviceHeight] = useState(844);
  const [deviceScale, setDeviceScale] = useState(100);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsSection, setSettingsSection] = useState<BrowserSettingsSection>("general");
  const [settings, setSettings] = useState<BrowserSettingsState>(() => loadBrowserSettings());
  const [downloadDir, setDownloadDir] = useState("");
  const contentRef = useRef<HTMLDivElement | null>(null);
  const surfaceRef = useRef<HTMLDivElement | null>(null);
  const menuRef = useRef<HTMLDivElement | null>(null);
  const findInputRef = useRef<HTMLInputElement | null>(null);
  const lastUrlRef = useRef<string>("");
  const readyRef = useRef(false);
  const activeTabIdRef = useRef(activeTabId);
  const handledNavigationIdRef = useRef<number | null>(null);
  activeTabIdRef.current = activeTabId;

  const activeTab = tabs.find((tab) => tab.id === activeTabId) || tabs[0];
  const hasPage = Boolean(activeTab?.url);
  // The native child webview is always above the desktop DOM on Windows, so hide it while a DOM menu is open.
  const showWebview = active && hasPage && !settingsOpen && !menuOpen;

  const updateSettings = useCallback((patch: Partial<BrowserSettingsState>) => {
    setSettings((current) => {
      const next = { ...current, ...patch };
      saveBrowserSettings(next);
      return next;
    });
  }, []);

  const readBounds = useCallback(() => {
    // A Tauri child webview sits above the desktop DOM on Windows. Measure the
    // dedicated, inset host so it cannot cover the browser controls or splitter.
    const node = surfaceRef.current;
    if (!node) return null;
    const rect = node.getBoundingClientRect();
    if (rect.width < 2 || rect.height < 2) return null;
    if (!deviceOpen || devicePreset === "responsive") {
      return { x: rect.left, y: rect.top, width: rect.width, height: rect.height };
    }
    const scale = Math.max(0.25, Math.min(deviceScale, 200) / 100);
    const targetW = Math.max(120, deviceWidth * scale);
    const targetH = Math.max(120, deviceHeight * scale);
    const width = Math.min(rect.width, targetW);
    const height = Math.min(rect.height, targetH);
    return {
      x: rect.left + (rect.width - width) / 2,
      y: rect.top + (rect.height - height) / 2,
      width,
      height,
    };
  }, [deviceHeight, deviceOpen, devicePreset, deviceScale, deviceWidth]);

  const syncSurface = useCallback(
    async (options?: { url?: string | null; show?: boolean; hide?: boolean }) => {
      const bounds = readBounds();
      if (!bounds) return;
      const selectedDevicePreset = DEVICE_PRESETS.find((preset) => preset.id === devicePreset) || DEVICE_PRESETS[0];
      try {
        if (!readyRef.current) {
          await browserEnsure(bounds, options?.url || activeTab?.url || "about:blank", selectedDevicePreset.userAgent);
          readyRef.current = true;
        } else {
          await browserSetBounds(bounds);
          if (options?.url) {
            await browserNavigate(options.url);
            lastUrlRef.current = options.url;
          }
        }
        if (options?.hide || !showWebview) {
          await browserHide();
        } else if (options?.show || showWebview) {
          await browserShow();
        }
        setError(null);
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause));
      }
    },
    [activeTab?.url, devicePreset, readBounds, showWebview],
  );

  const applyBrowserScrollbarStyle = useCallback(async () => {
    const css = JSON.stringify(BROWSER_SCROLLBAR_STYLE);
    await browserEval(`
      (() => {
        const id = "xcoding-browser-scrollbar-style";
        let style = document.getElementById(id);
        if (!(style instanceof HTMLStyleElement)) {
          style?.remove();
          style = document.createElement("style");
          style.id = id;
          (document.head || document.documentElement).appendChild(style);
        }
        style.textContent = ${css};
      })();
    `);
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void onBrowserNavigated((payload) => {
      const nextUrl = payload.url || "";
      if (!nextUrl || nextUrl === "about:blank") return;
      const currentId = activeTabIdRef.current;
      setTabs((current) =>
        current.map((tab) =>
          tab.id === currentId
            ? {
                ...tab,
                url: nextUrl,
                input: nextUrl,
                title: payload.title?.trim() || tab.title || nextUrl,
              }
            : tab,
        ),
      );
      lastUrlRef.current = nextUrl;
      void applyBrowserScrollbarStyle().catch(() => undefined);
    }).then((fn) => {
      unlisten = fn;
    });
    return () => {
      unlisten?.();
    };
  }, [applyBrowserScrollbarStyle]);

  useEffect(() => {
    // RightToolsPanel removes this component when the side panel closes.
    // Hide the native child webview too, otherwise it stays above the desktop DOM.
    return () => {
      void browserHide().catch(() => undefined);
    };
  }, []);

  useEffect(() => {
    void browserDownloadDir()
      .then((dir) => setDownloadDir(dir))
      .catch(() => setDownloadDir(""));
  }, []);

  useEffect(() => {
    if (!menuOpen) return;
    const onPointerDown = (event: MouseEvent) => {
      if (!menuRef.current?.contains(event.target as Node)) setMenuOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setMenuOpen(false);
    };
    window.addEventListener("mousedown", onPointerDown);
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("mousedown", onPointerDown);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [menuOpen]);

  useEffect(() => {
    if (!active) {
      void browserHide().catch(() => undefined);
      return;
    }
    void syncSurface({ show: showWebview, hide: !showWebview });
  }, [active, showWebview, syncSurface]);

  useEffect(() => {
    if (!active || !showWebview) return;
    const onResize = () => {
      void syncSurface();
    };
    window.addEventListener("resize", onResize);
    const observer =
      typeof ResizeObserver !== "undefined" && surfaceRef.current
        ? new ResizeObserver(() => {
            void syncSurface();
          })
        : null;
    if (surfaceRef.current && observer) observer.observe(surfaceRef.current);
    return () => {
      window.removeEventListener("resize", onResize);
      observer?.disconnect();
    };
  }, [active, deviceHeight, deviceOpen, devicePreset, deviceScale, deviceWidth, showWebview, syncSurface]);

  useEffect(() => {
    if (!findOpen) return;
    findInputRef.current?.focus();
    findInputRef.current?.select();
  }, [findOpen]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!active) return;
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f") {
        event.preventDefault();
        setFindOpen(true);
        setSettingsOpen(false);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [active]);

  const updateActive = useCallback((patch: Partial<BrowserTabState>) => {
    setTabs((current) =>
      current.map((tab) => (tab.id === activeTabIdRef.current ? { ...tab, ...patch } : tab)),
    );
  }, []);

  const openUrl = useCallback(
    async (raw: string, options?: { forceSystem?: boolean; forceEmbedded?: boolean }) => {
      const next = normalizeBrowserUrl(raw);
      if (!next) return;
      const useSystem =
        options?.forceSystem ||
        (!options?.forceEmbedded && (isLocalDevUrl(next) ? settings.openLocalTarget === "system" : settings.openWebTarget === "system"));
      if (useSystem) {
        try {
          await openExternalUrl(next);
          setError(null);
        } catch (cause) {
          setError(cause instanceof Error ? cause.message : String(cause));
        }
        return;
      }
      updateActive({ input: next, url: next, title: next });
      lastUrlRef.current = next;
      setSettingsOpen(false);
      await syncSurface({ url: next, show: true });
    },
    [settings.openLocalTarget, settings.openWebTarget, syncSurface, updateActive],
  );

  useEffect(() => {
    if (!active || !navigation || handledNavigationIdRef.current === navigation.id) return;
    handledNavigationIdRef.current = navigation.id;
    void openUrl(navigation.url, { forceEmbedded: true });
  }, [active, navigation, openUrl]);

  const onSubmit = useCallback(
    (event: FormEvent) => {
      event.preventDefault();
      void openUrl(activeTab?.input || "");
    },
    [activeTab?.input, openUrl],
  );

  const addTab = useCallback(() => {
    const tab = createBrowserTab({ title: t(locale, "browser.newTab") });
    setTabs((current) => [...current, tab]);
    setActiveTabId(tab.id);
    setSettingsOpen(false);
    setFindOpen(false);
    void browserHide().catch(() => undefined);
  }, [locale]);

  const closeTab = useCallback(
    (id: string) => {
      setTabs((current) => {
        if (current.length <= 1) {
          const fresh = createBrowserTab({ title: t(locale, "browser.newTab") });
          setActiveTabId(fresh.id);
          lastUrlRef.current = "";
          void browserHide().catch(() => undefined);
          return [fresh];
        }
        const index = current.findIndex((tab) => tab.id === id);
        const next = current.filter((tab) => tab.id !== id);
        if (id === activeTabIdRef.current) {
          const fallback = next[Math.max(0, index - 1)] || next[0];
          setActiveTabId(fallback.id);
          if (fallback.url) void syncSurface({ url: fallback.url, show: true });
          else void browserHide().catch(() => undefined);
        }
        return next;
      });
    },
    [locale, syncSurface],
  );

  const selectTab = useCallback(
    (id: string) => {
      setActiveTabId(id);
      setSettingsOpen(false);
      const tab = tabs.find((item) => item.id === id);
      if (tab?.url) void syncSurface({ url: tab.url, show: true });
      else void browserHide().catch(() => undefined);
    },
    [syncSurface, tabs],
  );

  const changeZoom = useCallback(async (next: number) => {
    const clamped = Math.min(3, Math.max(0.5, Math.round(next * 10) / 10));
    setZoom(clamped);
    try {
      await browserSetZoom(clamped);
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }, []);

  const runFind = useCallback(
    async (forward = true) => {
      const query = findQuery.trim();
      if (!query || !hasPage) return;
      try {
        await browserFind(query, { forward });
        setError(null);
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause));
      }
    },
    [findQuery, hasPage],
  );

  const onFindKeyDown = useCallback(
    (event: ReactKeyboardEvent<HTMLInputElement>) => {
      if (event.key === "Enter") {
        event.preventDefault();
        void runFind(!event.shiftKey);
      } else if (event.key === "Escape") {
        event.preventDefault();
        setFindOpen(false);
      }
    },
    [runFind],
  );

  const takeScreenshot = useCallback(async () => {
    if (!hasPage) return;
    setMenuOpen(false);
    try {
      await syncSurface({ show: true });
      const saved = await browserScreenshot();
      setStatus(t(locale, "browser.screenshotSaved", { path: saved }));
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }, [hasPage, locale, syncSurface]);

  const openDownloads = useCallback(async () => {
    setMenuOpen(false);
    try {
      const dir = downloadDir || (await browserDownloadDir());
      if (dir) {
        setDownloadDir(dir);
        await openPath(dir);
      }
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }, [downloadDir]);

  const clearBrowsingData = useCallback(async () => {
    setMenuOpen(false);
    try {
      await browserClearData();
      setStatus(t(locale, "browser.clearData"));
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }, [locale]);

  const openSettings = useCallback((section: BrowserSettingsSection = "general") => {
    setMenuOpen(false);
    setSettingsSection(section);
    setSettingsOpen(true);
    setFindOpen(false);
    void browserHide().catch(() => undefined);
  }, []);

  const applyDevicePreset = useCallback(async (presetId: DevicePresetId) => {
    const preset = DEVICE_PRESETS.find((item) => item.id === presetId) || DEVICE_PRESETS[0];
    try {
      // Wry applies a user agent at webview creation, so switching profiles recreates the current page.
      await browserSetUserAgent(preset.userAgent);
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
    setDevicePreset(preset.id);
    if (preset.width > 0 && preset.height > 0) {
      setDeviceWidth(preset.width);
      setDeviceHeight(preset.height);
    }
  }, []);

  const markUnavailable = useCallback(() => {
    setMenuOpen(false);
    setStatus(t(locale, "browser.unavailable"));
  }, [locale]);

  return (
    <div className="builtin-browser">
      <div className="browser-tabbar">
        <div className="browser-tabs">
          {tabs.map((tab) => {
            const selected = tab.id === activeTabId;
            return (
              <div key={tab.id} className={`browser-tab${selected ? " active" : ""}`}>
                <button type="button" className="browser-tab-main" onClick={() => selectTab(tab.id)}>
                  <span className="browser-tab-icon" aria-hidden="true">
                    ◎
                  </span>
                  <span className="browser-tab-title">{tab.title || tab.url || t(locale, "browser.newTab")}</span>
                </button>
                <button
                  type="button"
                  className="browser-tab-close"
                  aria-label={t(locale, "browser.closeTab")}
                  onClick={() => closeTab(tab.id)}
                >
                  ×
                </button>
              </div>
            );
          })}
          <button type="button" className="browser-tab-add" aria-label={t(locale, "browser.newTab")} onClick={addTab}>
            +
          </button>
        </div>
      </div>

      <div className="browser-toolbar">
        <div className="browser-nav">
          <button
            type="button"
            className="browser-icon-button"
            title={t(locale, "browser.back")}
            aria-label={t(locale, "browser.back")}
            disabled={!hasPage || settingsOpen}
            onClick={() => void browserBack().catch((cause) => setError(String(cause)))}
          >
            ←
          </button>
          <button
            type="button"
            className="browser-icon-button"
            title={t(locale, "browser.forward")}
            aria-label={t(locale, "browser.forward")}
            disabled={!hasPage || settingsOpen}
            onClick={() => void browserForward().catch((cause) => setError(String(cause)))}
          >
            →
          </button>
          <button
            type="button"
            className="browser-icon-button"
            title={t(locale, "browser.reload")}
            aria-label={t(locale, "browser.reload")}
            disabled={!hasPage || settingsOpen}
            onClick={() => void browserReload().catch((cause) => setError(String(cause)))}
          >
            ↻
          </button>
        </div>

        <form className="browser-url-form" onSubmit={onSubmit}>
          <input
            value={activeTab?.input || ""}
            onChange={(event) => updateActive({ input: event.target.value })}
            placeholder={t(locale, "browser.urlPlaceholder")}
            spellCheck={false}
            disabled={settingsOpen}
          />
        </form>

        <div className="browser-menu-wrap" ref={menuRef}>
          <button
            type="button"
            className="browser-icon-button"
            aria-label={t(locale, "browser.menu")}
            aria-expanded={menuOpen}
            onClick={() => setMenuOpen((open) => !open)}
          >
            ⋮
          </button>
          {menuOpen ? (
            <div className="browser-menu" role="menu">
              <button
                type="button"
                role="menuitem"
                disabled={!hasPage || settingsOpen}
                onClick={() => {
                  setMenuOpen(false);
                  setFindOpen(true);
                }}
              >
                {t(locale, "browser.find")}
              </button>
              <button
                type="button"
                role="menuitem"
                disabled={!hasPage || settingsOpen}
                onClick={() => {
                  setMenuOpen(false);
                  void browserPrint().catch((cause) => setError(String(cause)));
                }}
              >
                {t(locale, "browser.print")}
              </button>
              <div className="browser-menu-zoom">
                <span>{t(locale, "browser.zoom")}</span>
                <div className="browser-zoom-controls">
                  <button type="button" onClick={() => void changeZoom(zoom - 0.1)}>
                    −
                  </button>
                  <span>{Math.round(zoom * 100)}%</span>
                  <button type="button" onClick={() => void changeZoom(zoom + 0.1)}>
                    +
                  </button>
                  <button type="button" onClick={() => void changeZoom(1)}>
                    ↻
                  </button>
                </div>
              </div>
              <button
                type="button"
                role="menuitem"
                onClick={() => {
                  setMenuOpen(false);
                  setDeviceOpen((open) => !open);
                  setSettingsOpen(false);
                }}
              >
                {deviceOpen ? t(locale, "browser.hideDeviceToolbar") : t(locale, "browser.deviceToolbar")}
              </button>
              <button type="button" role="menuitem" disabled={!hasPage || settingsOpen} onClick={() => void takeScreenshot()}>
                {t(locale, "browser.screenshot")}
              </button>
              <div className="browser-menu-divider" role="separator" />
              <button type="button" role="menuitem" onClick={markUnavailable}>
                {t(locale, "browser.importCookies")}
              </button>
              <button type="button" role="menuitem" className="browser-menu-advance" onClick={() => openSettings("autofill")}>
                <span>{t(locale, "browser.passwords")}</span>
                <span aria-hidden="true">›</span>
              </button>
              <button type="button" role="menuitem" onClick={() => void openDownloads()}>
                {t(locale, "browser.downloads")}
              </button>
              <button type="button" role="menuitem" className="browser-menu-advance" onClick={() => void clearBrowsingData()}>
                <span>{t(locale, "browser.clearDataMenu")}</span>
                <span aria-hidden="true">›</span>
              </button>
              <div className="browser-menu-divider" role="separator" />
              <button type="button" role="menuitem" onClick={() => openSettings("general")}>
                {t(locale, "browser.settings")}
              </button>
              <button
                type="button"
                role="menuitem"
                disabled={!hasPage}
                onClick={() => {
                  setMenuOpen(false);
                  if (!activeTab?.url) return;
                  void openExternalUrl(activeTab.url).catch((cause) => setError(String(cause)));
                }}
              >
                {t(locale, "browser.openExternal")}
              </button>
            </div>
          ) : null}
        </div>
      </div>

      {findOpen && !settingsOpen ? (
        <div className="browser-findbar">
          <input
            ref={findInputRef}
            value={findQuery}
            onChange={(event) => setFindQuery(event.target.value)}
            onKeyDown={onFindKeyDown}
            placeholder={t(locale, "browser.findPlaceholder")}
            spellCheck={false}
          />
          <button type="button" className="browser-icon-button" onClick={() => void runFind(false)} title={t(locale, "browser.findPrev")}>
            ↑
          </button>
          <button type="button" className="browser-icon-button" onClick={() => void runFind(true)} title={t(locale, "browser.findNext")}>
            ↓
          </button>
          <button
            type="button"
            className="browser-icon-button"
            onClick={() => setFindOpen(false)}
            title={t(locale, "browser.findClose")}
            aria-label={t(locale, "browser.findClose")}
          >
            ×
          </button>
        </div>
      ) : null}

      {deviceOpen && !settingsOpen ? (
        <div className="browser-devicebar">
          <label className="browser-device-field">
            <span>{t(locale, "browser.deviceSize")}</span>
            <select value={devicePreset} onChange={(event) => void applyDevicePreset(event.target.value as DevicePresetId)}>
              {DEVICE_PRESETS.map((preset) => (
                <option key={preset.id} value={preset.id}>
                  {preset.id === "responsive" ? t(locale, "browser.deviceResponsive") : preset.label}
                </option>
              ))}
            </select>
          </label>
          <label className="browser-device-field">
            <span className="sr-only">{t(locale, "browser.deviceWidth")}</span>
            <input
              type="number"
              min={120}
              value={deviceWidth}
              onChange={(event) => {
                const width = Number(event.target.value) || 120;
                if (devicePreset !== "responsive") {
                  void browserSetUserAgent(null).catch((cause) => {
                    setError(cause instanceof Error ? cause.message : String(cause));
                  });
                  setDevicePreset("responsive");
                }
                setDeviceWidth(width);
              }}
            />
          </label>
          <span className="browser-device-x" aria-hidden="true">
            ×
          </span>
          <label className="browser-device-field">
            <span className="sr-only">{t(locale, "browser.deviceHeight")}</span>
            <input
              type="number"
              min={120}
              value={deviceHeight}
              onChange={(event) => {
                const height = Number(event.target.value) || 120;
                if (devicePreset !== "responsive") {
                  void browserSetUserAgent(null).catch((cause) => {
                    setError(cause instanceof Error ? cause.message : String(cause));
                  });
                  setDevicePreset("responsive");
                }
                setDeviceHeight(height);
              }}
            />
          </label>
          <label className="browser-device-field">
            <span className="sr-only">{t(locale, "browser.deviceScale")}</span>
            <select value={deviceScale} onChange={(event) => setDeviceScale(Number(event.target.value) || 100)}>
              {[50, 75, 100, 125, 150].map((value) => (
                <option key={value} value={value}>
                  {value}%
                </option>
              ))}
            </select>
          </label>
          <button
            type="button"
            className="browser-icon-button"
            aria-label={t(locale, "browser.hideDeviceToolbar")}
            onClick={() => setDeviceOpen(false)}
          >
            ×
          </button>
        </div>
      ) : null}

      {error ? <p className="error-message browser-error">{error}</p> : null}
      {status ? <p className="browser-status">{status}</p> : null}

      <div className={`browser-content${deviceOpen && !settingsOpen ? " device-mode" : ""}`} ref={contentRef}>
        {settingsOpen ? (
          <div className="browser-settings">
            <div className="browser-settings-header">
              <div>
                <div className="browser-settings-title">{t(locale, "browser.settingsTitle")}</div>
                <div className="browser-settings-sub">{t(locale, "browser.settingsSub")}</div>
              </div>
              <button type="button" className="quiet-button" onClick={() => setSettingsOpen(false)}>
                {t(locale, "browser.backToBrowser")}
              </button>
            </div>

            <div className="browser-settings-nav">
              {(
                [
                  ["general", "browser.sectionGeneral"],
                  ["autofill", "browser.sectionAutofill"],
                  ["downloads", "browser.sectionDownloads"],
                  ["permissions", "browser.sectionPermissions"],
                  ["site", "browser.sectionSitePermissions"],
                  ["developer", "browser.sectionDeveloper"],
                ] as const
              ).map(([id, labelKey]) => (
                <button
                  key={id}
                  type="button"
                  className={`browser-settings-nav-item${settingsSection === id ? " active" : ""}`}
                  onClick={() => setSettingsSection(id)}
                >
                  {t(locale, labelKey)}
                </button>
              ))}
            </div>

            <div className="browser-settings-body">
              {settingsSection === "general" ? (
                <div className="browser-settings-section">
                  <div className="browser-settings-card">
                    <div className="browser-settings-row">
                      <div>
                        <div className="browser-settings-label">{t(locale, "browser.openWebTarget")}</div>
                        <div className="browser-settings-help">{t(locale, "browser.openWebTargetHelp")}</div>
                      </div>
                      <select
                        value={settings.openWebTarget}
                        onChange={(event) => updateSettings({ openWebTarget: event.target.value as BrowserOpenTarget })}
                      >
                        <option value="browser">{t(locale, "browser.targetBrowser")}</option>
                        <option value="system">{t(locale, "browser.targetSystem")}</option>
                      </select>
                    </div>
                    <div className="browser-settings-row">
                      <div>
                        <div className="browser-settings-label">{t(locale, "browser.openLocalTarget")}</div>
                        <div className="browser-settings-help">{t(locale, "browser.openLocalTargetHelp")}</div>
                      </div>
                      <select
                        value={settings.openLocalTarget}
                        onChange={(event) => updateSettings({ openLocalTarget: event.target.value as BrowserOpenTarget })}
                      >
                        <option value="browser">{t(locale, "browser.targetBrowser")}</option>
                        <option value="system">{t(locale, "browser.targetSystem")}</option>
                      </select>
                    </div>
                    <div className="browser-settings-row">
                      <div>
                        <div className="browser-settings-label">{t(locale, "browser.browsingData")}</div>
                        <div className="browser-settings-help">{t(locale, "browser.browsingDataHelp")}</div>
                      </div>
                      <button type="button" className="quiet-button" onClick={() => void clearBrowsingData()}>
                        {t(locale, "browser.clearAllData")}
                      </button>
                    </div>
                    <div className="browser-settings-row">
                      <div>
                        <div className="browser-settings-label">{t(locale, "browser.annotatedScreenshots")}</div>
                        <div className="browser-settings-help">{t(locale, "browser.annotatedScreenshotsHelp")}</div>
                      </div>
                      <select
                        value={settings.annotatedScreenshots}
                        onChange={(event) => updateSettings({ annotatedScreenshots: event.target.value as BrowserAnnotated })}
                      >
                        <option value="always">{t(locale, "browser.always")}</option>
                        <option value="never">{t(locale, "browser.never")}</option>
                      </select>
                    </div>
                  </div>
                </div>
              ) : null}

              {settingsSection === "autofill" ? (
                <div className="browser-settings-section">
                  <div className="browser-settings-card">
                    <div className="browser-settings-row">
                      <div>
                        <div className="browser-settings-label">{t(locale, "browser.passwordManager")}</div>
                        <div className="browser-settings-help">{t(locale, "browser.passwordManagerHelp")}</div>
                      </div>
                      <button type="button" className="quiet-button" disabled title={t(locale, "browser.unavailable")}>
                        {t(locale, "browser.manage")}
                      </button>
                    </div>
                    <div className="browser-settings-row">
                      <div>
                        <div className="browser-settings-label">{t(locale, "browser.contactInfo")}</div>
                        <div className="browser-settings-help">{t(locale, "browser.contactInfoHelp")}</div>
                      </div>
                      <button type="button" className="quiet-button" disabled title={t(locale, "browser.unavailable")}>
                        {t(locale, "browser.manage")}
                      </button>
                    </div>
                  </div>
                </div>
              ) : null}

              {settingsSection === "downloads" ? (
                <div className="browser-settings-section">
                  <div className="browser-settings-card">
                    <div className="browser-settings-row">
                      <div>
                        <div className="browser-settings-label">{t(locale, "browser.downloadLocation")}</div>
                        <div className="browser-settings-help">{downloadDir || t(locale, "browser.downloadLocationHelp")}</div>
                      </div>
                      <button type="button" className="quiet-button" onClick={() => void openDownloads()}>
                        {t(locale, "browser.openFolder")}
                      </button>
                    </div>
                    <div className="browser-settings-row">
                      <div>
                        <div className="browser-settings-label">{t(locale, "browser.askDownloadLocation")}</div>
                        <div className="browser-settings-help">{t(locale, "browser.askDownloadLocationHelp")}</div>
                      </div>
                      <button
                        type="button"
                        className={`browser-toggle${settings.askDownloadLocation ? " on" : ""}`}
                        aria-pressed={settings.askDownloadLocation}
                        onClick={() => updateSettings({ askDownloadLocation: !settings.askDownloadLocation })}
                      >
                        <span className="browser-toggle-knob" />
                      </button>
                    </div>
                    <div className="browser-settings-row">
                      <div>
                        <div className="browser-settings-label">{t(locale, "browser.downloadHistory")}</div>
                        <div className="browser-settings-help">{t(locale, "browser.downloadHistoryHelp")}</div>
                      </div>
                      <button type="button" className="quiet-button" onClick={() => void openDownloads()}>
                        {t(locale, "browser.manage")}
                      </button>
                    </div>
                  </div>
                </div>
              ) : null}

              {settingsSection === "permissions" ? (
                <div className="browser-settings-section">
                  <div className="browser-settings-card">
                    <div className="browser-settings-row">
                      <div>
                        <div className="browser-settings-label">{t(locale, "browser.siteSettings")}</div>
                        <div className="browser-settings-help">{t(locale, "browser.siteSettingsHelp")}</div>
                      </div>
                      <button type="button" className="quiet-button" disabled title={t(locale, "browser.unavailable")}>
                        {t(locale, "browser.manage")}
                      </button>
                    </div>
                    <div className="browser-settings-row">
                      <div>
                        <div className="browser-settings-label">{t(locale, "browser.approvals")}</div>
                        <div className="browser-settings-help">{t(locale, "browser.approvalsHelp")}</div>
                      </div>
                      <select
                        value={settings.approvals}
                        onChange={(event) => updateSettings({ approvals: event.target.value as BrowserApprovals })}
                      >
                        <option value="alwaysAsk">{t(locale, "browser.alwaysAsk")}</option>
                        <option value="autoAllow">{t(locale, "browser.autoAllow")}</option>
                      </select>
                    </div>
                  </div>
                </div>
              ) : null}

              {settingsSection === "site" ? (
                <div className="browser-settings-section">
                  <div className="browser-settings-card">
                    <div className="browser-settings-empty">{t(locale, "browser.noSitePermissions")}</div>
                  </div>
                </div>
              ) : null}

              {settingsSection === "developer" ? (
                <div className="browser-settings-section">
                  <div className="browser-settings-card risk">
                    <div className="browser-settings-row">
                      <div>
                        <div className="browser-settings-risk">{t(locale, "browser.riskElevated")}</div>
                        <div className="browser-settings-label">{t(locale, "browser.cdpAccess")}</div>
                        <div className="browser-settings-help">{t(locale, "browser.cdpAccessHelp")}</div>
                      </div>
                      <button
                        type="button"
                        className={`browser-toggle${settings.cdpAccess ? " on" : ""}`}
                        aria-pressed={settings.cdpAccess}
                        onClick={() => updateSettings({ cdpAccess: !settings.cdpAccess })}
                      >
                        <span className="browser-toggle-knob" />
                      </button>
                    </div>
                  </div>
                </div>
              ) : null}
            </div>
          </div>
        ) : !hasPage ? (
          <div className="browser-empty">
            <div className="browser-empty-icon" aria-hidden="true">
              ◎
            </div>
            <div className="browser-empty-title">{t(locale, "browser.emptyTitle")}</div>
            <div className="browser-empty-sub">{t(locale, "browser.emptySub")}</div>
          </div>
        ) : (
          <div className="browser-surface-host" ref={surfaceRef} aria-hidden="true" />
        )}
      </div>
    </div>
  );
}

type GitNexusPayload = {
  processes?: Array<{ id?: string; summary?: string; symbol_count?: number; step_count?: number }>;
  process_symbols?: GitNexusSymbol[];
  definitions?: GitNexusSymbol[];
  symbol?: GitNexusSymbol;
  target?: GitNexusSymbol;
  incoming?: { calls?: GitNexusSymbol[] };
  outgoing?: { calls?: GitNexusSymbol[] };
  risk?: string;
  byDepth?: Record<string, GitNexusSymbol[]>;
};

type GitNexusViewResult = {
  title: string;
  result: GitNexusCommandResult;
};

function parseGitNexusPayload(value: string): GitNexusPayload | null {
  try {
    const parsed: unknown = JSON.parse(value);
    return parsed && typeof parsed === "object" ? (parsed as GitNexusPayload) : null;
  } catch {
    return null;
  }
}

function gitNexusSymbolId(symbol: GitNexusSymbol): string | undefined {
  return symbol.id ?? symbol.uid;
}

function gitNexusSymbols(payload: GitNexusPayload | null): GitNexusSymbol[] {
  if (!payload) return [];
  const candidates = [
    ...(payload.definitions ?? []),
    ...(payload.process_symbols ?? []),
    ...(payload.symbol ? [payload.symbol] : []),
    ...(payload.target ? [payload.target] : []),
    ...Object.values(payload.byDepth ?? {}).flat(),
  ];
  const seen = new Set<string>();
  return candidates.filter((symbol) => {
    if (!symbol?.name) return false;
    const key = gitNexusSymbolId(symbol) ?? `${symbol.filePath ?? ""}:${symbol.name}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

function GitNexusPanel({ locale, active, workspaceRoot }: { locale: Locale; active: boolean; workspaceRoot: string }) {
  const [status, setStatus] = useState<GitNexusStatus | null>(null);
  const [query, setQuery] = useState("");
  const [loading, setLoading] = useState(false);
  const [result, setResult] = useState<GitNexusViewResult | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!workspaceRoot.trim()) {
      setStatus(null);
      setResult(null);
      return;
    }
    setLoading(true);
    setError(null);
    try {
      setStatus(await fetchGitNexusStatus(workspaceRoot.trim()));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoading(false);
    }
  }, [workspaceRoot]);

  useEffect(() => {
    if (!active) return;
    void refresh();
  }, [active, refresh]);

  const run = useCallback(
    async (operation: "query" | "context" | "impact", symbol?: GitNexusSymbol) => {
      const value = symbol?.name ?? query.trim();
      if (!workspaceRoot.trim()) {
        setError(t(locale, "env.needProject"));
        return;
      }
      if (!value) {
        setError(t(locale, "gitnexus.needQuery"));
        return;
      }
      setLoading(true);
      setError(null);
      try {
        const root = workspaceRoot.trim();
        const next =
          operation === "query"
            ? await queryGitNexus(root, value)
            : operation === "context"
              ? await contextGitNexus(root, value, symbol ? gitNexusSymbolId(symbol) : undefined, symbol?.filePath)
              : await impactGitNexus(root, value, symbol ? gitNexusSymbolId(symbol) : undefined, symbol?.filePath);
        setResult({ title: t(locale, `gitnexus.${operation}`), result: next });
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause));
      } finally {
        setLoading(false);
      }
    },
    [locale, query, workspaceRoot],
  );

  const analyze = useCallback(async () => {
    if (!workspaceRoot.trim()) {
      setError(t(locale, "env.needProject"));
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const next = await analyzeGitNexus(workspaceRoot.trim());
      setResult({ title: t(locale, "gitnexus.analyze"), result: next });
      setStatus(await fetchGitNexusStatus(workspaceRoot.trim()));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoading(false);
    }
  }, [locale, workspaceRoot]);

  const payload = useMemo(() => parseGitNexusPayload(result?.result.stdout ?? ""), [result]);
  const symbols = useMemo(() => gitNexusSymbols(payload), [payload]);
  const incoming = payload?.incoming?.calls ?? [];
  const outgoing = payload?.outgoing?.calls ?? [];
  const ready = Boolean(status?.available && status.indexed);
  const statusKey = !status?.available
    ? "gitnexus.statusMissing"
    : status.indexed
      ? status.up_to_date
        ? "gitnexus.statusReady"
        : "gitnexus.statusStale"
      : "gitnexus.statusUnindexed";

  return (
    <section className="gitnexus-panel" aria-label={t(locale, "panel.code")}>
      <div className="gitnexus-toolbar">
        <div>
          <p className="panel-title">{t(locale, "panel.code")}</p>
          <p className={`gitnexus-status${status?.up_to_date ? " ready" : ""}`}>{t(locale, statusKey)}</p>
        </div>
        <div className="gitnexus-actions">
          <button type="button" className="quiet-button" onClick={() => void refresh()} disabled={loading}>
            {t(locale, "action.refresh")}
          </button>
          <button type="button" className="quiet-button" onClick={() => void analyze()} disabled={loading || status?.available === false}>
            {t(locale, "gitnexus.analyze")}
          </button>
        </div>
      </div>
      <p className="gitnexus-help">{status?.detail ?? t(locale, "gitnexus.help")}</p>
      {!workspaceRoot.trim() ? <p className="env-muted">{t(locale, "env.needProject")}</p> : null}
      {error ? <p className="error-message">{error}</p> : null}

      <form
        className="gitnexus-search"
        onSubmit={(event) => {
          event.preventDefault();
          void run("query");
        }}
      >
        <input
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder={t(locale, "gitnexus.placeholder")}
          aria-label={t(locale, "gitnexus.placeholder")}
          disabled={!ready || loading}
        />
        <button type="submit" className="quiet-button" disabled={!ready || loading}>
          {t(locale, "gitnexus.query")}
        </button>
      </form>
      <div className="gitnexus-actions gitnexus-query-actions">
        <button type="button" className="quiet-button" disabled={!ready || loading} onClick={() => void run("context")}>
          {t(locale, "gitnexus.context")}
        </button>
        <button type="button" className="quiet-button" disabled={!ready || loading} onClick={() => void run("impact")}>
          {t(locale, "gitnexus.impact")}
        </button>
      </div>

      {loading ? <p className="env-muted">{t(locale, "env.loading")}</p> : null}
      {result ? (
        <div className="gitnexus-result">
          <p className="gitnexus-result-title">{result.title}</p>
          {payload?.processes?.length ? (
            <div className="gitnexus-processes">
              {payload.processes.map((process) => (
                <div key={process.id ?? process.summary} className="gitnexus-process">
                  <strong>{process.summary ?? process.id}</strong>
                  {process.symbol_count || process.step_count ? (
                    <span>{t(locale, "gitnexus.processMeta", { symbols: process.symbol_count ?? 0, steps: process.step_count ?? 0 })}</span>
                  ) : null}
                </div>
              ))}
            </div>
          ) : null}
          {payload?.risk ? <p className="gitnexus-risk">{payload.risk}</p> : null}
          {incoming.length || outgoing.length ? (
            <div className="gitnexus-relationships" aria-label={t(locale, "gitnexus.context")}>
              {incoming.map((symbol) => (
                <button
                  key={`incoming:${gitNexusSymbolId(symbol) ?? `${symbol.filePath}:${symbol.name}`}`}
                  type="button"
                  className="gitnexus-relationship"
                  onClick={() => {
                    setQuery(symbol.name);
                    void run("context", symbol);
                  }}
                  disabled={loading}
                >
                  ← {symbol.name}
                </button>
              ))}
              {outgoing.map((symbol) => (
                <button
                  key={`outgoing:${gitNexusSymbolId(symbol) ?? `${symbol.filePath}:${symbol.name}`}`}
                  type="button"
                  className="gitnexus-relationship"
                  onClick={() => {
                    setQuery(symbol.name);
                    void run("context", symbol);
                  }}
                  disabled={loading}
                >
                  → {symbol.name}
                </button>
              ))}
            </div>
          ) : null}
          {symbols.length ? (
            <div className="gitnexus-symbols">
              {symbols.map((symbol) => (
                <div key={gitNexusSymbolId(symbol) ?? `${symbol.filePath}:${symbol.name}`} className="gitnexus-symbol">
                  <button
                    type="button"
                    className="gitnexus-symbol-main"
                    onClick={() => {
                      setQuery(symbol.name);
                      void run("context", symbol);
                    }}
                    disabled={loading}
                    title={t(locale, "gitnexus.context")}
                  >
                    <strong>{symbol.name}</strong>
                    {symbol.filePath ? <span>{symbol.filePath}{symbol.startLine ? `:${symbol.startLine}` : ""}</span> : null}
                  </button>
                  <button type="button" className="quiet-button" onClick={() => void run("impact", symbol)} disabled={loading}>
                    {t(locale, "gitnexus.impact")}
                  </button>
                </div>
              ))}
            </div>
          ) : null}
          <details className="gitnexus-raw">
            <summary>{t(locale, "gitnexus.raw")}</summary>
            <pre>{result.result.stdout || result.result.stderr || t(locale, "gitnexus.empty")}</pre>
          </details>
        </div>
      ) : null}
    </section>
  );
}

export function RightToolsPanel({
  locale,
  open,
  tab,
  browserNavigation,
  onTabChange,
  onClose,
  workspaceRoot,
  reviewContent,
  width,
  onWidthChange,
}: RightToolsPanelProps) {
  const [filePath, setFilePath] = useState("");
  const [entries, setEntries] = useState<DirEntryInfo[]>([]);
  const [filesError, setFilesError] = useState<string | null>(null);
  const [filesLoading, setFilesLoading] = useState(false);
  const resizeRef = useRef<{ startX: number; startWidth: number } | null>(null);

  const loadFiles = useCallback(
    async (relative?: string) => {
      if (!workspaceRoot.trim()) {
        setEntries([]);
        setFilesError(t(locale, "env.needProject"));
        return;
      }
      setFilesLoading(true);
      setFilesError(null);
      try {
        const next = await listWorkspaceEntries(workspaceRoot.trim(), relative || "");
        setFilePath(relative || "");
        setEntries(next);
      } catch (cause) {
        setFilesError(cause instanceof Error ? cause.message : String(cause));
      } finally {
        setFilesLoading(false);
      }
    },
    [locale, workspaceRoot],
  );

  useEffect(() => {
    if (!open || tab !== "files") return;
    void loadFiles(filePath);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, tab, workspaceRoot]);

  const onRightPanelResizeStart = (event: ReactMouseEvent<HTMLDivElement>) => {
    event.preventDefault();
    event.stopPropagation();
    resizeRef.current = { startX: event.clientX, startWidth: width };
    document.body.classList.add("is-resizing-right-panel");

    const onMove = (moveEvent: MouseEvent) => {
      if (!resizeRef.current) return;
      const delta = resizeRef.current.startX - moveEvent.clientX;
      onWidthChange(resizeRef.current.startWidth + delta);
    };
    const onUp = () => {
      resizeRef.current = null;
      document.body.classList.remove("is-resizing-right-panel");
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };

  if (!open) return null;

  const openEntry = (entry: DirEntryInfo) => {
    if (entry.is_dir) {
      void loadFiles(entry.path);
      return;
    }
    const absolute = joinWorkspacePath(workspaceRoot, entry.path);
    void openPath(absolute).catch((cause) => {
      setFilesError(cause instanceof Error ? cause.message : String(cause));
    });
  };

  return (
    <aside className="right-tools-panel" aria-label={t(locale, "panel.sideTitle")}>
      <div
        className="right-panel-resizer"
        role="separator"
        aria-orientation="vertical"
        aria-label={t(locale, "panel.resize")}
        title={t(locale, "panel.resize")}
        onMouseDown={onRightPanelResizeStart}
      />
      <div className="right-tools-tabs">
        {(
          [
            ["review", "panel.review"],
            ["browser", "panel.browser"],
            ["files", "panel.files"],
            ["code", "panel.code"],
          ] as const
        ).map(([id, key]) => (
          <button
            key={id}
            type="button"
            className={`right-tools-tab${tab === id ? " active" : ""}`}
            onClick={() => onTabChange(id)}
          >
            {t(locale, key)}
          </button>
        ))}
        <button type="button" className="bottom-panel-close" onClick={onClose} title={t(locale, "action.cancel")}>
          ×
        </button>
      </div>
      <div className={`right-tools-body${tab === "browser" ? " is-browser" : ""}`}>
        {tab === "review" ? (
          <div className="right-tools-review">
            {reviewContent || <p className="env-muted">{t(locale, "panel.reviewEmpty")}</p>}
          </div>
        ) : null}

        {tab === "browser" ? <BuiltInBrowserPanel locale={locale} active={open && tab === "browser"} navigation={browserNavigation} /> : null}

        {tab === "code" ? <GitNexusPanel locale={locale} active={open && tab === "code"} workspaceRoot={workspaceRoot} /> : null}

        {tab === "files" ? (
          <div className="bottom-files">
            <div className="bottom-files-toolbar">
              <button
                type="button"
                className="quiet-button"
                disabled={!filePath}
                onClick={() => void loadFiles(parentRelative(filePath))}
              >
                {t(locale, "panel.up")}
              </button>
              <code className="bottom-files-path">{filePath || "."}</code>
              <button type="button" className="quiet-button" onClick={() => void loadFiles(filePath)} disabled={filesLoading}>
                {t(locale, "action.refresh")}
              </button>
            </div>
            {filesError ? <p className="error-message">{filesError}</p> : null}
            {filesLoading ? <p className="env-muted">{t(locale, "env.loading")}</p> : null}
            <div className="bottom-files-list">
              {entries.map((entry) => (
                <button key={entry.path} type="button" className="bottom-file-item" onClick={() => openEntry(entry)}>
                  <span aria-hidden="true">{entry.is_dir ? "📁" : "📄"}</span>
                  <span>{entry.name}</span>
                </button>
              ))}
              {!filesLoading && entries.length === 0 && !filesError ? (
                <p className="env-muted">{t(locale, "panel.filesEmpty")}</p>
              ) : null}
            </div>
          </div>
        ) : null}
      </div>
    </aside>
  );
}

export function HeaderPanelToggle({
  locale,
  open,
  onToggle,
}: {
  locale: Locale;
  open: boolean;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      className={`header-icon-button${open ? " active" : ""}`}
      onClick={onToggle}
      title={`${t(locale, "panel.toggle")} Ctrl+J`}
      aria-label={t(locale, "panel.toggle")}
      aria-pressed={open}
    >
      <span className="header-panel-icon" aria-hidden="true" />
    </button>
  );
}

export function HeaderRightPanelToggle({
  locale,
  open,
  onToggle,
}: {
  locale: Locale;
  open: boolean;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      className={`header-icon-button${open ? " active" : ""}`}
      onClick={onToggle}
      title={`${t(locale, "panel.toggleRight")} Ctrl+Shift+G`}
      aria-label={t(locale, "panel.toggleRight")}
      aria-pressed={open}
    >
      <span className="header-right-panel-icon" aria-hidden="true" />
    </button>
  );
}

export function HeaderEnvButton({
  locale,
  open,
  onToggle,
  buttonRef,
  summary,
}: {
  locale: Locale;
  open: boolean;
  onToggle: (event: ReactMouseEvent<HTMLButtonElement>) => void;
  buttonRef: React.RefObject<HTMLButtonElement | null>;
  summary: string;
}) {
  return (
    <button
      type="button"
      className={`header-icon-button env-trigger${open ? " active" : ""}`}
      onClick={onToggle}
      ref={buttonRef as React.RefObject<HTMLButtonElement>}
      title={t(locale, "env.title")}
      aria-label={t(locale, "env.title")}
      aria-expanded={open}
    >
      <span className="header-env-label">{summary}</span>
    </button>
  );
}

export function seedTerminalCommand(command: string): void {
  window.dispatchEvent(new CustomEvent("xcoding-terminal-seed", { detail: command }));
}
