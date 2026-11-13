import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import type {
  CleanupProjectResult,
  ProjectDiskUsage,
  ProjectDiskUsageReport,
} from "@xcoding/protocol";
import { t, type Locale } from "./i18n";

function formatCleanupBytes(bytes: number): string {
  const safe = Number.isFinite(bytes) ? Math.max(0, bytes) : 0;
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = safe;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  const digits = unitIndex === 0 ? 0 : value >= 100 ? 0 : value >= 10 ? 1 : 2;
  return `${value.toFixed(digits)} ${units[unitIndex]}`;
}

type ProjectCleanupProps = {
  locale: Locale;
  workspaceHome: string;
  disabled: boolean;
};

export function ProjectCleanup({ locale, workspaceHome, disabled }: ProjectCleanupProps) {
  const [report, setReport] = useState<ProjectDiskUsageReport | null>(null);
  const [loading, setLoading] = useState(false);
  const [scanError, setScanError] = useState<string | null>(null);
  const [cleanupError, setCleanupError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [cleaningPath, setCleaningPath] = useState<string | null>(null);
  const scanRequestRef = useRef(0);

  const scan = useCallback(async (): Promise<void> => {
    const home = workspaceHome.trim();
    if (!home) {
      scanRequestRef.current += 1;
      setReport(null);
      setLoading(false);
      setScanError(null);
      return;
    }
    const requestId = ++scanRequestRef.current;
    setLoading(true);
    setScanError(null);
    try {
      const next = await invoke<ProjectDiskUsageReport>("scan_project_usage", { workspaceHome: home });
      if (scanRequestRef.current === requestId) setReport(next);
    } catch (cause) {
      if (scanRequestRef.current === requestId) {
        setScanError(cause instanceof Error ? cause.message : String(cause));
      }
    } finally {
      if (scanRequestRef.current === requestId) setLoading(false);
    }
  }, [workspaceHome]);

  useEffect(() => {
    setCleanupError(null);
    setNotice(null);
    void scan();
    return () => {
      scanRequestRef.current += 1;
    };
  }, [scan]);

  async function cleanProject(usage: ProjectDiskUsage): Promise<void> {
    if (disabled || cleaningPath || usage.cleanable_bytes === 0) return;
    const projectName = usage.project.title.trim() || usage.project.dir_name;
    if (!window.confirm(t(locale, "settings.cleanup.confirm", {
      name: projectName,
      size: formatCleanupBytes(usage.cleanable_bytes),
    }))) {
      return;
    }
    setCleaningPath(usage.project.path);
    setCleanupError(null);
    setNotice(null);
    try {
      const result = await invoke<CleanupProjectResult>("cleanup_project", {
        params: {
          workspace_home: workspaceHome.trim(),
          project_path: usage.project.path,
        },
      });
      if (result.removed_items.length > 0) {
        setNotice(t(locale, "settings.cleanup.removed", {
          name: projectName,
          size: formatCleanupBytes(result.removed_bytes),
        }));
      }
      if (result.failed_items.length > 0) {
        setCleanupError(t(locale, "settings.cleanup.partial", { count: result.failed_items.length }));
      }
      await scan();
    } catch (cause) {
      setCleanupError(cause instanceof Error ? cause.message : t(locale, "settings.cleanup.failed"));
    } finally {
      setCleaningPath(null);
    }
  }

  const canClean = !disabled && !loading && !cleaningPath;

  return (
    <div className="project-cleanup" aria-busy={loading}>
      <div className="project-cleanup-header">
        <div>
          <p className="panel-title">{t(locale, "settings.cleanup.title")}</p>
          <p className="mode-help">{t(locale, "settings.cleanup.subtitle")}</p>
        </div>
        <button
          type="button"
          className="quiet-button"
          onClick={() => void scan()}
          disabled={loading || Boolean(cleaningPath) || !workspaceHome.trim()}
        >
          {loading ? t(locale, "settings.cleanup.scanning") : t(locale, "settings.cleanup.refresh")}
        </button>
      </div>

      {!workspaceHome.trim() ? (
        <p className="mode-help">{t(locale, "settings.cleanup.needWorkspace")}</p>
      ) : null}
      {disabled ? <p className="mode-help">{t(locale, "settings.cleanup.running")}</p> : null}
      {scanError ? <p className="error-message">{scanError}</p> : null}
      {cleanupError ? <p className="error-message">{cleanupError}</p> : null}
      {notice ? <p className="project-cleanup-notice" role="status">{notice}</p> : null}

      {report ? (
        <>
          <div className="project-cleanup-summary">
            <div>
              <span>{t(locale, "settings.cleanup.totalUsage")}</span>
              <strong>{formatCleanupBytes(report.total_bytes)}</strong>
            </div>
            <div className="cleanable">
              <span>{t(locale, "settings.cleanup.cleanable")}</span>
              <strong>{formatCleanupBytes(report.cleanable_bytes)}</strong>
            </div>
          </div>

          {report.projects.length === 0 && !loading ? (
            <p className="project-cleanup-empty">{t(locale, "settings.cleanup.empty")}</p>
          ) : (
            <div className="project-cleanup-list" role="list">
              {report.projects.map((usage) => {
                const projectName = usage.project.title.trim() || usage.project.dir_name;
                const isCleaning = cleaningPath === usage.project.path;
                return (
                  <article className="project-cleanup-project" key={usage.project.path} role="listitem">
                    <header>
                      <div className="project-cleanup-project-title">
                        <strong>{projectName}</strong>
                        <span>{usage.project.path}</span>
                      </div>
                      <button
                        type="button"
                        className="quiet-button project-cleanup-button"
                        onClick={() => void cleanProject(usage)}
                        disabled={!canClean || usage.cleanable_bytes === 0}
                        title={disabled ? t(locale, "settings.cleanup.running") : undefined}
                      >
                        {isCleaning
                          ? t(locale, "settings.cleanup.cleaning")
                          : t(locale, "settings.cleanup.cleanProject")}
                      </button>
                    </header>
                    <div className="project-cleanup-metrics">
                      <span>
                        {t(locale, "settings.cleanup.projectUsage")}
                        <strong>{formatCleanupBytes(usage.total_bytes)}</strong>
                      </span>
                      <span className="cleanable">
                        {t(locale, "settings.cleanup.projectCleanable")}
                        <strong>{formatCleanupBytes(usage.cleanable_bytes)}</strong>
                      </span>
                    </div>
                    {usage.skipped_entries > 0 ? (
                      <p className="project-cleanup-warning">
                        {t(locale, "settings.cleanup.skipped", { count: usage.skipped_entries })}
                      </p>
                    ) : null}
                    {usage.cleanable_items.length > 0 ? (
                      <details className="project-cleanup-details">
                        <summary>
                          {t(locale, "settings.cleanup.items", { count: usage.cleanable_items.length })}
                        </summary>
                        <div>
                          {usage.cleanable_items.map((item) => (
                            <span key={item.relative_path}>
                              <code>{item.relative_path}</code>
                              <em>{formatCleanupBytes(item.bytes)}</em>
                            </span>
                          ))}
                        </div>
                      </details>
                    ) : null}
                  </article>
                );
              })}
            </div>
          )}
        </>
      ) : null}
    </div>
  );
}
