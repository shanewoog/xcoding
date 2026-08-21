import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

async function main() {
  const [panelsSource, appSource, cssSource, workspaceApiSource, workspaceToolsSource, mainSource, i18nSource] =
    await Promise.all([
      readFile(resolve(repositoryRoot, "apps/desktop/src/panels.tsx"), "utf8"),
      readFile(resolve(repositoryRoot, "apps/desktop/src/App.tsx"), "utf8"),
      readFile(resolve(repositoryRoot, "apps/desktop/src/styles.css"), "utf8"),
      readFile(resolve(repositoryRoot, "apps/desktop/src/workspaceApi.ts"), "utf8"),
      readFile(resolve(repositoryRoot, "apps/desktop/src-tauri/src/workspace_tools.rs"), "utf8"),
      readFile(resolve(repositoryRoot, "apps/desktop/src-tauri/src/main.rs"), "utf8"),
      readFile(resolve(repositoryRoot, "apps/desktop/src/i18n.ts"), "utf8"),
    ]);

  assert.ok(
    workspaceToolsSource.includes("pub fn read_workspace_file"),
    "the desktop backend must expose a workspace file reader",
  );
  assert.ok(
    workspaceToolsSource.includes("path escapes workspace root"),
    "the file reader must refuse paths outside the workspace root",
  );
  assert.ok(
    workspaceToolsSource.includes("MAX_VIEWABLE_FILE_BYTES") && workspaceToolsSource.includes("fn looks_binary"),
    "the file reader must cap file size and detect binary content",
  );
  assert.ok(
    mainSource.includes("workspace_tools::read_workspace_file"),
    "Tauri must register the workspace file reader command",
  );
  assert.ok(
    workspaceApiSource.includes('invoke<WorkspaceFileContent>("read_workspace_file"'),
    "the desktop bridge must call the workspace file reader",
  );

  assert.ok(
    workspaceToolsSource.includes("pub async fn workspace_changes")
      && workspaceToolsSource.includes("pub async fn workspace_file_diff"),
    "the desktop backend must expose working tree changes and per-file diffs",
  );
  assert.ok(
    workspaceToolsSource.includes("fn parse_status_line") && workspaceToolsSource.includes("fn untracked_diff"),
    "the changes reader must parse git status lines and synthesize diffs for untracked files",
  );
  assert.ok(
    mainSource.includes("workspace_tools::workspace_changes")
      && mainSource.includes("workspace_tools::workspace_file_diff"),
    "Tauri must register the workspace changes and diff commands",
  );
  assert.ok(
    workspaceApiSource.includes('invoke<WorkspaceChanges>("workspace_changes"')
      && workspaceApiSource.includes('invoke<WorkspaceFileDiff>("workspace_file_diff"'),
    "the desktop bridge must call the workspace changes and diff commands",
  );

  assert.ok(
    panelsSource.includes("const openFileInPanel") && panelsSource.includes("void openFileInPanel(entry)"),
    "clicking a file in the files tab must open it inside the panel",
  );
  assert.ok(
    panelsSource.includes('<pre className="file-viewer-body">'),
    "the files tab must render text file content in the panel",
  );
  assert.ok(
    panelsSource.includes("content.binary || content.too_large"),
    "binary and oversized files must fall back to the system application",
  );
  assert.ok(
    panelsSource.includes('t(locale, "panel.backToFiles")') && panelsSource.includes('t(locale, "panel.openExternally")'),
    "the file viewer must offer a way back to the list and a system-app fallback",
  );
  assert.ok(
    panelsSource.includes('className="bottom-files-filter"') && panelsSource.includes('className="bottom-files-crumbs"'),
    "the files tab must offer a filter box and breadcrumbs",
  );
  assert.ok(
    panelsSource.includes('className="file-viewer-gutter"'),
    "the file viewer must render line numbers",
  );
  assert.ok(
    panelsSource.includes("function ReviewChangesPanel") && panelsSource.includes("<ReviewChangesPanel"),
    "the review tab must render the working tree changes panel",
  );
  assert.ok(
    panelsSource.includes("function buildUnifiedDiffLines") && panelsSource.includes('className="diff-preview review-diff"'),
    "the review tab must render unified diffs for the selected file",
  );

  assert.ok(
    appSource.includes('<div className="review-preview">'),
    "the review tab must render the pending action instead of a single line of text",
  );
  assert.match(
    appSource,
    /reviewContent=\{[\s\S]*buildReviewPresentation\(pendingAction, approvalSummary, Boolean\(patchPreview\), locale\)/,
    "the review tab must reuse the shared review presentation",
  );
  assert.match(
    appSource,
    /<div className="review-preview">[\s\S]*buildPatchDiffLines\(patchPreview, locale\)/,
    "the review tab must show the patch diff when one is pending",
  );

  assert.match(cssSource, /\.file-viewer-body\s*\{[\s\S]*overflow:\s*auto;/, "the file viewer must scroll its content");
  assert.ok(
    !/\.file-viewer-body\s*\{[^}]*#[0-9a-fA-F]{3}/.test(cssSource),
    "the file viewer must use theme tokens instead of hard-coded colors",
  );

  for (const key of [
    "panel.backToFiles",
    "panel.openExternally",
    "panel.fileBinary",
    "panel.fileTooLarge",
    "panel.rootCrumb",
    "panel.filterFiles",
    "panel.fileMeta",
    "review.changesTitle",
    "review.backToChanges",
    "review.diffEmpty",
    "review.diffBinary",
    "review.diffTruncated",
  ]) {
    const occurrences = i18nSource.split(`"${key}"`).length - 1;
    assert.equal(occurrences, 2, `${key} must be translated in both English and Chinese`);
  }

  console.log("Desktop file viewer checks passed.");
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
