import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const appSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/App.tsx"), "utf8");
const componentSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/ProjectCleanup.tsx"), "utf8");
const i18nSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/i18n.ts"), "utf8");
const cssSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/styles.css"), "utf8");
const mainSource = await readFile(resolve(repositoryRoot, "apps/desktop/src-tauri/src/main.rs"), "utf8");
const cleanupSource = await readFile(resolve(repositoryRoot, "apps/desktop/src-tauri/src/project_cleanup.rs"), "utf8");
const packageSource = await readFile(resolve(repositoryRoot, "package.json"), "utf8");
const releaseScriptSource = await readFile(resolve(repositoryRoot, "scripts/package-desktop-portable.ps1"), "utf8");

assert.match(
  appSource,
  /type SettingsTab = [^\n]*"cleanup"/,
  "settings must expose a project-cleanup tab",
);
assert.ok(
  appSource.includes("ProjectCleanup") && appSource.includes("settings.tab.cleanup"),
  "settings must render the project-cleanup panel",
);
assert.ok(
  componentSource.includes('invoke<ProjectDiskUsageReport>("scan_project_usage"') &&
    componentSource.includes('invoke<CleanupProjectResult>("cleanup_project"') &&
    componentSource.includes("workspaceHome") &&
    componentSource.includes("settings.cleanup.confirm"),
  "project cleanup must scan usage and clean a selected project through Tauri",
);
assert.ok(
  componentSource.includes("formatCleanupBytes") &&
    componentSource.includes("report.total_bytes") &&
    componentSource.includes("usage.cleanable_bytes") &&
    componentSource.includes("runningProjectRoots") &&
    componentSource.includes("isProjectRunning"),
  "project cleanup must show usage and block only projects with active sessions",
);
assert.ok(
  appSource.includes("const runningProjectRoots = useMemo") &&
    appSource.includes("runningProjectRoots={runningProjectRoots}") &&
    !appSource.includes("workspaceHome={workspaceHome}\n                disabled={anySessionRunning}"),
  "project cleanup must derive per-project running-session blocking instead of disabling globally",
);
assert.ok(
  cleanupSource.includes("node_modules") &&
    cleanupSource.includes("target") &&
    cleanupSource.includes(".xcoding/logs") &&
    cleanupSource.includes("remove_dir_all") &&
    !cleanupSource.includes('"dist"') &&
    !cleanupSource.includes('"build"'),
  "backend cleanup must restrict deletion to the approved regenerable paths",
);
assert.ok(
  cleanupSource.includes("fn nested_project_roots") &&
    cleanupSource.includes("nested_projects_are_not_counted_in_their_parent_usage"),
  "nested projects must be excluded from their parent usage to avoid double counting",
);
assert.ok(
  mainSource.includes("async fn scan_project_usage") &&
    mainSource.includes("async fn cleanup_project") &&
    mainSource.includes("scan_project_usage,") &&
    mainSource.includes("cleanup_project,"),
  "project cleanup Tauri commands must be registered",
);
assert.ok(
  i18nSource.includes('"settings.tab.cleanup"') &&
    i18nSource.includes('"settings.cleanup.cleanable"') &&
    i18nSource.includes('"settings.cleanup.running"') &&
    (i18nSource.match(/"settings.tab.cleanup"/g)?.length ?? 0) >= 2,
  "project cleanup copy must be localized in both languages",
);
assert.ok(
  cssSource.includes(".project-cleanup-summary") &&
    cssSource.includes(".project-cleanup-project") &&
    cssSource.includes(".project-cleanup-details"),
  "project cleanup must have summary, project, and detail styles",
);
assert.ok(
  packageSource.includes("desktop-project-cleanup.mjs") &&
    releaseScriptSource.includes("apps/desktop/src/ProjectCleanup.tsx") &&
    releaseScriptSource.includes("apps/desktop/src-tauri/src/project_cleanup.rs") &&
    releaseScriptSource.includes("tests/e2e/desktop-project-cleanup.mjs"),
  "project cleanup source and regression test must be included in fast verification and portable releases",
);

console.log("Desktop project cleanup checks passed.");
