import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

async function main() {
  const appSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/App.tsx"), "utf8");
  const cssSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/styles.css"), "utf8");
  const i18nSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/i18n.ts"), "utf8");

  assert.ok(appSource.includes("function ProjectChevronIcon"), "missing project chevron icon");
  assert.ok(appSource.includes('className="project-collapse"'), "missing project collapse button");
  assert.ok(appSource.includes('className="project-select"'), "missing project selection button");
  assert.ok(appSource.includes("aria-expanded={!collapsed}"), "collapse button must expose expanded state");
  assert.ok(
    appSource.includes("toggleProjectCollapsed(group.root)"),
    "collapse button must toggle the project session list",
  );

  const collapseIndex = appSource.indexOf('className="project-collapse"');
  const selectIndex = appSource.indexOf('className="project-select"');
  const nameIndex = appSource.indexOf('className="project-name"', selectIndex);
  assert.ok(
    collapseIndex >= 0 && collapseIndex < selectIndex && selectIndex < nameIndex,
    "collapse control must render before the project name",
  );

  assert.ok(cssSource.includes(".project-collapse"), "missing project collapse styles");
  assert.ok(cssSource.includes(".project-select"), "missing project selection styles");
  assert.ok(
    cssSource.includes(".project-chevron.is-collapsed"),
    "missing collapsed chevron rotation style",
  );
  assert.ok(i18nSource.includes('"action.collapseProject"'), "missing collapse localization key");
  assert.ok(i18nSource.includes('"action.expandProject"'), "missing expand localization key");
  assert.ok(i18nSource.includes("Collapse project sessions"), "missing English collapse label");
  assert.ok(i18nSource.includes("折叠项目会话"), "missing Chinese collapse label");

  console.log("Desktop project collapse checks passed.");
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});