import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

async function source(path) {
  return readFile(resolve(repositoryRoot, path), "utf8");
}

async function main() {
  const [panels, api, rust, mainRs, i18n, css, packageJsonSource, e2eRunner] = await Promise.all([
    source("apps/desktop/src/panels.tsx"),
    source("apps/desktop/src/workspaceApi.ts"),
    source("apps/desktop/src-tauri/src/gitnexus.rs"),
    source("apps/desktop/src-tauri/src/main.rs"),
    source("apps/desktop/src/i18n.ts"),
    source("apps/desktop/src/styles.css"),
    source("package.json"),
    source("tests/e2e/run.mjs"),
  ]);
  const packageJson = JSON.parse(packageJsonSource);

  for (const needle of [
    'ToolPanelTab = "review" | "browser" | "files" | "code"',
    'tab: "code", labelKey: "panel.code"',
    "function GitNexusPanel",
    "queryGitNexus",
    "contextGitNexus",
    "impactGitNexus",
    "gitNexusSymbolId",
    "gitnexus-relationships",
    '["code", "panel.code"]',
    '<GitNexusPanel',
  ]) {
    assert.ok(panels.includes(needle), `panels.tsx missing ${needle}`);
  }

  for (const needle of [
    "fetchGitNexusStatus",
    "analyzeGitNexus",
    "queryGitNexus",
    "contextGitNexus",
    "impactGitNexus",
    '"gitnexus_status"',
    '"gitnexus_analyze"',
    '"gitnexus_query"',
    '"gitnexus_context"',
    '"gitnexus_impact"',
  ]) {
    assert.ok(api.includes(needle), `workspaceApi.ts missing ${needle}`);
  }

  for (const needle of [
    'Command::new("gitnexus.cmd")',
    ".current_dir(root)",
    "gitnexus_registry_path",
    "repo_name_from_registry",
    '"--repo".to_owned()',
    '"--direction".to_owned()',
    '"upstream".to_owned()',
    "COMMAND_TIMEOUT",
    "ANALYZE_TIMEOUT",
  ]) {
    assert.ok(rust.includes(needle), `gitnexus.rs missing ${needle}`);
  }

  for (const needle of [
    "gitnexus::gitnexus_status",
    "gitnexus::gitnexus_analyze",
    "gitnexus::gitnexus_query",
    "gitnexus::gitnexus_context",
    "gitnexus::gitnexus_impact",
  ]) {
    assert.ok(mainRs.includes(needle), `main.rs missing ${needle}`);
  }

  for (const needle of [
    '"panel.code": "Code relations"',
    '"panel.code": "代码关系"',
    '"gitnexus.statusReady"',
    '"gitnexus.analyze"',
    '"gitnexus.impact"',
  ]) {
    assert.ok(i18n.includes(needle), `i18n.ts missing ${needle}`);
  }

  for (const needle of [".gitnexus-panel", ".gitnexus-search", ".gitnexus-result", ".gitnexus-symbol", ".gitnexus-relationships"] ) {
    assert.ok(css.includes(needle), `styles.css missing ${needle}`);
  }

  assert.equal(packageJson.scripts["test:e2e"], "node tests/e2e/run.mjs", "package.json must use the unified e2e runner");
  assert.ok(
    e2eRunner.includes('entry.name.endsWith(".mjs")') && e2eRunner.includes("entry.name !== runnerName"),
    "the e2e runner must automatically discover top-level regression checks",
  );
  console.log("desktop GitNexus integration source checks passed");
}

await main();
