import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

async function source(path) {
  return readFile(resolve(repositoryRoot, path), "utf8");
}

async function main() {
  const [panels, api, workspaceTools, mainRs, i18n, css] = await Promise.all([
    source("apps/desktop/src/panels.tsx"),
    source("apps/desktop/src/workspaceApi.ts"),
    source("apps/desktop/src-tauri/src/workspace_tools.rs"),
    source("apps/desktop/src-tauri/src/main.rs"),
    source("apps/desktop/src/i18n.ts"),
    source("apps/desktop/src/styles.css"),
  ]);

  for (const needle of ["fetchGitHistory", "type GitCommitEntry", "type GitHistory", "env-history", "gitCommitDate", "gitCommitRefs"]) {
    assert.ok(panels.includes(needle), `panels.tsx missing ${needle}`);
  }
  for (const needle of ["GitCommitEntry", "GitHistory", "fetchGitHistory", '"git_history"']) {
    assert.ok(api.includes(needle), `workspaceApi.ts missing ${needle}`);
  }
  for (const needle of ["pub struct GitCommitEntry", "pub struct GitHistory", "parse_git_history_output", "git_history_sync", "pub async fn git_history", "%H%x1f%h%x1f%P", "--all", "--max-count="]) {
    assert.ok(workspaceTools.includes(needle), `workspace_tools.rs missing ${needle}`);
  }
  assert.ok(mainRs.includes("workspace_tools::git_history"), "main.rs must register git_history");
  assert.ok(i18n.includes('"env.history": "History"'), "English history translation missing");
  assert.ok(i18n.includes('"env.history": "版本记录"'), "Chinese history translation missing");
  assert.ok(css.includes(".env-history") && css.includes(".env-history-item"), "history list styles missing");
  console.log("desktop git history integration source checks passed");
}

await main();