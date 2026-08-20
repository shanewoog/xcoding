import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

async function main() {
  const [appearanceSource, appSource, cssSource, indexSource, i18nSource] = await Promise.all([
    readFile(resolve(repositoryRoot, "apps/desktop/src/appearance.ts"), "utf8"),
    readFile(resolve(repositoryRoot, "apps/desktop/src/App.tsx"), "utf8"),
    readFile(resolve(repositoryRoot, "apps/desktop/src/styles.css"), "utf8"),
    readFile(resolve(repositoryRoot, "apps/desktop/index.html"), "utf8"),
    readFile(resolve(repositoryRoot, "apps/desktop/src/i18n.ts"), "utf8"),
  ]);

  // Two themes, dark stays the default so an existing install looks unchanged.
  assert.ok(appearanceSource.includes('export type Theme = "dark" | "light"'), "the app must offer exactly a dark and a light theme");
  assert.ok(appearanceSource.includes('export const DEFAULT_THEME: Theme = "dark"'), "dark must remain the default theme");
  assert.ok(
    appearanceSource.includes("loadTheme") &&
      appearanceSource.includes("saveTheme") &&
      appearanceSource.includes("applyTheme"),
    "theme selection must be loadable, persistable, and applicable",
  );
  assert.ok(
    appearanceSource.includes("document.documentElement.dataset.theme"),
    "the chosen theme must be applied as a data attribute on the document root",
  );

  // The picker has to be reachable and its choice has to survive a restart.
  assert.ok(appSource.includes('id="ui-theme"'), "settings must expose a theme control");
  assert.ok(
    appSource.includes("applyTheme(theme);") && appSource.includes("saveTheme(theme);"),
    "changing the theme must apply and persist it",
  );
  assert.ok(
    i18nSource.includes('"settings.theme.dark"') && i18nSource.includes('"settings.theme.light"'),
    "both themes need localized labels",
  );

  // Only the token layer may differ between themes.
  assert.ok(cssSource.includes(':root[data-theme="light"]'), "the light theme must be defined as a token override block");
  assert.match(cssSource, /:root\s*\{[\s\S]*?color-scheme:\s*dark;/, "the dark theme must declare its color scheme for native controls");
  assert.match(
    cssSource,
    /:root\[data-theme="light"\]\s*\{[\s\S]*?color-scheme:\s*light;/,
    "the light theme must declare its color scheme for native controls",
  );

  // Every token defined for dark needs a light counterpart, otherwise a surface
  // silently keeps its dark value and turns unreadable on a light background.
  const darkBlock = cssSource.match(/:root\s*\{([\s\S]*?)\n\}/);
  const lightBlock = cssSource.match(/:root\[data-theme="light"\]\s*\{([\s\S]*?)\n\}/);
  assert.ok(darkBlock && lightBlock, "both theme token blocks must be present");
  const tokenNames = (block) => new Set(block.match(/--[\w-]+(?=\s*:)/g) || []);
  const darkTokens = tokenNames(darkBlock[1]);
  const lightTokens = tokenNames(lightBlock[1]);
  const missing = [...darkTokens].filter((token) => !lightTokens.has(token));
  assert.deepEqual(missing, [], `light theme is missing token overrides: ${missing.join(", ")}`);

  // Components must not reintroduce hardcoded surface colors.
  assert.ok(!/\.status-\w+ \.status-badge \{ background: #/.test(cssSource), "status badges must read their colors from theme tokens");
  assert.ok(!/\.activity-badge\.\w+ \{ background: #/.test(cssSource), "activity badges must read their colors from theme tokens");

  // A light-theme launch must not flash the dark boot screen.
  assert.ok(
    indexSource.includes('localStorage.getItem("xcoding.theme")'),
    "the boot document must apply the stored theme before first paint",
  );
  assert.ok(indexSource.includes('html[data-theme="light"]'), "the boot screen must have a light-theme appearance");
  assert.ok(
    appearanceSource.includes('export const THEME_STORAGE_KEY = "xcoding.theme"'),
    "the bootstrap script and the appearance module must agree on the storage key",
  );

  console.log("Desktop theme checks passed.");
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
