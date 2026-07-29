import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

async function main() {
  const [appSource, panelsSource, cssSource, workspaceApiSource, browserSource, mainSource] = await Promise.all([
    readFile(resolve(repositoryRoot, "apps/desktop/src/App.tsx"), "utf8"),
    readFile(resolve(repositoryRoot, "apps/desktop/src/panels.tsx"), "utf8"),
    readFile(resolve(repositoryRoot, "apps/desktop/src/styles.css"), "utf8"),
    readFile(resolve(repositoryRoot, "apps/desktop/src/workspaceApi.ts"), "utf8"),
    readFile(resolve(repositoryRoot, "apps/desktop/src-tauri/src/browser.rs"), "utf8"),
    readFile(resolve(repositoryRoot, "apps/desktop/src-tauri/src/main.rs"), "utf8"),
  ]);

  assert.ok(appSource.includes("const MESSAGE_LINK_PATTERN"), "assistant messages must recognize Markdown and bare HTTP(S) links");
  assert.ok(appSource.includes("function AssistantMessageBody"), "assistant messages must render link-aware content");
  assert.ok(appSource.includes("\\u2018\\u2019\\u201C\\u201D"), "assistant links must exclude trailing quotation marks from URLs");
  assert.ok(appSource.includes("trailingBareLinkBoldDelimiter"), "assistant links must exclude trailing Markdown bold delimiters from URLs");
  assert.ok(appSource.includes('className="assistant-message-link"'), "assistant links must use the link visual treatment");
  assert.ok(appSource.includes("onOpenLink(url);"), "assistant link clicks must be handled by the application");
  assert.ok(appSource.includes('openRightPanel("browser");'), "assistant link clicks must reveal the built-in browser");
  assert.ok(appSource.includes("browserNavigation={browserNavigation}"), "assistant link navigation must be passed to the right tool panel");
  assert.ok(appSource.includes("<AssistantMessageBody content={streamedText}"), "streaming assistant messages must render links before completion");

  assert.ok(panelsSource.includes("export type BrowserNavigationRequest"), "right tool panel must accept browser navigation requests");
  assert.ok(panelsSource.includes("forceEmbedded?: boolean"), "browser navigation must support an embedded override");
  assert.ok(panelsSource.includes("openUrl(navigation.url, { forceEmbedded: true })"), "assistant links must always open in the built-in browser");
  assert.ok(panelsSource.includes("navigation={browserNavigation}"), "browser panel must receive requested URLs");
  assert.ok(panelsSource.includes("!settingsOpen && !menuOpen"), "opening the browser menu must hide the native webview so the menu remains visible");
  assert.ok(
    panelsSource.includes('label: "Surface Pro 7"') &&
      panelsSource.includes('label: "iPhone 15 Pro Max"') &&
      panelsSource.includes('label: "Samsung Galaxy S24 Ultra"'),
    "browser device presets must include the classic Codex device profiles",
  );
  assert.ok(panelsSource.includes("userAgent?: string"), "browser device presets must define optional user-agent emulation");
  assert.ok(panelsSource.includes("await browserSetUserAgent(preset.userAgent)"), "selecting a device preset must update the webview user agent");
  assert.ok(panelsSource.includes("browserEnsure(bounds, options?.url || activeTab?.url || \"about:blank\", selectedDevicePreset.userAgent)"), "initial browser creation must use the selected device user agent");
  assert.ok(workspaceApiSource.includes('invoke("browser_set_user_agent"'), "desktop bridge must expose user-agent updates");
  assert.ok(browserSource.includes("builder.user_agent(value)"), "native browser creation must pass the emulated user agent to Wry");
  assert.ok(browserSource.includes("pub async fn browser_set_user_agent"), "native browser must recreate the webview when its user agent changes");
  assert.ok(mainSource.includes("browser::browser_set_user_agent"), "Tauri must register the user-agent command");

  assert.match(cssSource, /\.assistant-message-link\s*\{[\s\S]*text-decoration:\s*underline;/, "assistant links must be visibly underlined");
  console.log("Desktop assistant link checks passed.");
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
