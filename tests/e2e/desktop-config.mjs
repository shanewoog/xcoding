import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

function modeHelpText(mode) {
  switch (mode) {
    case "full-auto":
      return "Full auto approves every request automatically, including high-risk writes and arbitrary commands. Use it only in workspaces you fully trust. Network access and hard-denied destructive commands stay blocked.";
    case "auto-edit":
      return "Auto edit applies ordinary workspace file patches and allowlisted safe commands automatically. High-risk writes and other commands still need approval.";
    default:
      return "Ask auto-applies ordinary workspace file patches. Commands and high-risk writes still need approval.";
  }
}

function formatModeOption(mode) {
  switch (mode) {
    case "full-auto":
      return "Full auto";
    case "auto-edit":
      return "Auto edit";
    default:
      return "Ask";
  }
}

function isValidMode(value) {
  return value === "ask" || value === "auto-edit" || value === "full-auto";
}

function buildDesktopDoctorChecks(input) {
  const rootPath = input.workspaceRoot.trim();
  const model = input.model.trim();
  const provider = (input.provider ?? "openai").trim() || "openai";
  const baseUrl = input.providerStatus?.base_url?.trim() || "";

  return [
    {
      name: "workspace",
      ok: rootPath.length > 0,
      detail: rootPath || "Set an absolute workspace path",
    },
    {
      name: "provider_auth",
      ok: Boolean(input.providerStatus?.ready),
      detail: input.providerStatus?.message || "Checking credentials...",
    },
    {
      name: "base_url",
      ok: baseUrl.length > 0,
      detail: baseUrl || "Cloud base URL is unavailable",
    },
    {
      name: "defaults",
      ok: isValidMode(input.mode) && model.length > 0,
      detail: `${formatModeOption(input.mode)} · ${provider} · ${model || "(no model)"}`,
    },
  ];
}

function desktopDoctorReady(checks) {
  return checks.every((check) => check.ok);
}

async function main() {
  const appSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/App.tsx"), "utf8");
  const cssSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/styles.css"), "utf8");
  const configSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/config.ts"), "utf8");
  const i18nSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/i18n.ts"), "utf8");
  const appearanceSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/appearance.ts"), "utf8");
  assert.ok(appSource.includes("const MAX_MAX_TOOL_ROUNDS = 1024;"), "tool-round settings should allow up to 1024 rounds");
  assert.ok(appSource.includes("max={MAX_MAX_TOOL_ROUNDS}"), "tool-round input should use the configured upper bound");
  assert.ok(i18nSource.includes("export type Locale"), "i18n.ts missing Locale");
  assert.ok(i18nSource.includes("zh-CN"), "i18n.ts missing zh-CN");
  assert.ok(i18nSource.includes("export function t"), "i18n.ts missing t()");
  assert.ok(i18nSource.includes("export function loadLocale"), "i18n.ts missing loadLocale");
  assert.ok(i18nSource.includes("export function saveLocale"), "i18n.ts missing saveLocale");
  assert.ok(i18nSource.includes('"lang.label"'), "i18n.ts missing lang.label");
  assert.ok(i18nSource.includes("简体中文"), "i18n.ts missing Chinese labels");
  assert.ok(appSource.includes("const [diagnosticsOpen, setDiagnosticsOpen] = useState(false)"), "diagnostics popover should be closed by default");
  assert.ok(appSource.includes("settings-diagnostics-menu"), "settings header should contain a diagnostics trigger");
  assert.ok(appSource.includes("aria-expanded={diagnosticsOpen}"), "diagnostics trigger should expose its expanded state");
  assert.ok(
    appSource.includes("onClick={() => setDiagnosticsOpen((current) => !current)}"),
    "diagnostics trigger should toggle visibility on click",
  );
  assert.ok(appSource.includes("settings-diagnostics-popover"), "diagnostics should render in a popover");
  assert.ok(!appSource.includes("settings-diagnostics-card"), "diagnostics should not occupy a permanent settings-grid card");
  assert.ok(cssSource.includes(".settings-diagnostics-popover"), "styles should position the diagnostics popover");
  for (const needle of [
    '"settings.title"',
    '"settings.subtitle"',
    '"settings.section.provider"',
    '"field.baseUrl"',
    '"field.apiKey"',
    '"action.settings"',
    '"action.saveSettings"',
    '"composer.needWorkspace"',
    '"composer.needPrompt"',
    '"composer.needProvider"',
    '"error.needProvider"',
  ]) {
    assert.ok(i18nSource.includes(needle), "i18n.ts missing " + needle);
  }
  const messageCompletedStart = appSource.indexOf('if (payload.type === "message_completed") {');
  const messageCompletedEnd = appSource.indexOf('if (payload.type === "plan")', messageCompletedStart);
  assert.ok(messageCompletedStart >= 0 && messageCompletedEnd > messageCompletedStart, "App missing message_completed event branch");
  const messageCompletedSource = appSource.slice(messageCompletedStart, messageCompletedEnd);
  assert.ok(!messageCompletedSource.includes("markRunning(false)"), "message_completed must not unlock the composer before task_completed");
  assert.ok(!messageCompletedSource.includes("setIsRunning(false)"), "message_completed must not unlock the composer before task_completed");
  assert.ok(!messageCompletedSource.includes('status: "done"'), "message_completed must not mark the session done before task_completed");

  const taskCompletedStart = appSource.indexOf('if (payload.type === "task_completed") {');
  const taskCompletedEnd = appSource.indexOf('if (payload.type === "error")', taskCompletedStart);
  assert.ok(taskCompletedStart >= 0 && taskCompletedEnd > taskCompletedStart, "App missing task_completed event branch");
  const taskCompletedSource = appSource.slice(taskCompletedStart, taskCompletedEnd);
  assert.ok(
    taskCompletedSource.includes("markRunning(false)") || taskCompletedSource.includes("setIsRunning(false)"),
    "task_completed must unlock the composer for that session",
  );
  assert.ok(taskCompletedSource.includes('status: "done"'), "task_completed must mark the session done");
  assert.ok(
    appSource.includes("runningSessionIds") && appSource.includes("anySessionRunning"),
    "App should track multi-session running state",
  );
  assert.ok(appSource.includes("onClick={() => startNewTask()}"), "New task button required");
  assert.ok(!/onClick=\{\(\) => startNewTask\(\)\}[^>]*disabled=\{isRunning\}/.test(appSource), "New task must stay available while another session runs");

  const submitStart = appSource.indexOf("async function submit(");
  const submitEnd = appSource.indexOf("async function steerCurrentRun", submitStart);
  assert.ok(submitStart >= 0 && submitEnd > submitStart, "App missing composer submit handler");
  const submitSource = appSource.slice(submitStart, submitEnd);
  assert.ok(submitSource.includes("enqueueFollowUp(activeSessionId, message, images);"), "running sessions must queue follow-up input");
  const conversationStart = appSource.indexOf('className="conversation"');
  const conversationEnd = appSource.indexOf('<div className="chat-bottom">', conversationStart);
  assert.ok(conversationStart >= 0 && conversationEnd > conversationStart, "App missing conversation region");
  const conversationSource = appSource.slice(conversationStart, conversationEnd);
  assert.ok(conversationSource.includes("activeFollowUps.map"), "queued follow-ups must stay visible in the conversation");
  assert.ok(
    appSource.indexOf("activeFollowUps.map") < appSource.indexOf('<div className="chat-bottom">'),
    "queued follow-ups must not be hidden in the composer",
  );
  assert.ok(
    conversationSource.includes("<RunPlanProgress"),
    "the current plan step must be hinted inline in the conversation, not only in the floating popover",
  );
  assert.ok(
    conversationSource.indexOf("activeFollowUps.map") < conversationSource.indexOf("<RunPlanProgress"),
    "the current-step hint must follow the tool activity and queued follow-ups",
  );
  const runPlanProgressStart = appSource.indexOf("function RunPlanProgress(");
  assert.ok(runPlanProgressStart >= 0, "App missing RunPlanProgress component");
  const runPlanProgressSource = appSource.slice(runPlanProgressStart, appSource.indexOf("\n}\n", runPlanProgressStart));
  assert.ok(
    runPlanProgressSource.includes("phase === null") && runPlanProgressSource.includes("return null"),
    "the inline hint must disappear once the turn is no longer running",
  );
  assert.ok(
    !runPlanProgressSource.includes("plan.map("),
    "the inline hint must show only the current step, not the whole plan list",
  );
  assert.ok(
    runPlanProgressSource.includes("runningActivity(activity)"),
    "the inline hint must name the tool call that is currently executing",
  );
  assert.ok(
    /function runningActivity\([\s\S]*?state === "running"/.test(appSource),
    "runningActivity must prefer the in-flight tool call over the last finished one",
  );
  assert.ok(
    appSource.slice(appSource.indexOf('<div className="run-status-popover">')).includes("run-plan-list"),
    "the full plan list must stay available in the run status popover",
  );

  const keyDownStart = appSource.indexOf("function onComposerKeyDown(");
  assert.ok(keyDownStart >= 0, "App missing composer keyboard handler");
  const keyDownSource = appSource.slice(keyDownStart, appSource.indexOf("\n  }\n", keyDownStart));
  assert.ok(
    keyDownSource.includes("void submitComposer()"),
    "Ctrl+Enter must reuse the send-button path so it follows the queue/steer toggle",
  );
  assert.ok(
    keyDownSource.includes("if (sendBlockReason) return;"),
    "Ctrl+Enter must respect the same block reasons that disable the send button",
  );
  assert.ok(
    keyDownSource.includes("event.shiftKey") && keyDownSource.includes("steerCurrentRun()"),
    "keyboard steering must require the explicit Ctrl+Shift+Enter chord",
  );
  assert.ok(
    !/if \(queueMode\) \{\s*void steerCurrentRun\(\);/.test(keyDownSource),
    "Ctrl+Enter must never turn a queued follow-up into an interrupt",
  );
  assert.ok(
    appSource.includes("if (!sent) restoreComposerDraft(message, images);"),
    "a steer that never reaches the model must put the message back in the composer",
  );
  assert.ok(
    appSource.includes("<kbd>Ctrl+Shift</kbd>"),
    "the steer toggle should advertise the Ctrl+Shift+Enter chord",
  );

  const cliSource = await readFile(resolve(repositoryRoot, "apps/cli/src/index.ts"), "utf8");

  for (const needle of [
    "export function modeHelpText",
    "export function formatModeOption",
    "export function isValidMode",
    "export function buildDesktopDoctorChecks",
    "export function desktopDoctorReady",
    "export function commandAllowlistHelpText",
    "export function parseCommandAllowlistText",
    "export function formatCommandAllowlistText",
    "export function commandDenylistHelpText",
    "export function parseCommandDenylistText",
    "export function formatCommandDenylistText",
  ]) {
    assert.ok(configSource.includes(needle), "config.ts missing " + needle);
  }

  for (const needle of [
    'from "./config"',
    "buildDesktopDoctorChecks",
    "desktopDoctorReady",
    "modeHelpText",
    'id="default-mode"',
    'id="stream-idle-timeout"',
    'stream_idle_timeout_secs',

    'id="composer-model"',
    'id="composer-reasoning"',
    "list_provider_models",
    "refreshModels",
    "CloudProviderConfig",
    "activeProviderId",
    "active_provider_id",
    "selectedProviderId",
    'id="provider-list"',
    "provider-list-item",
    "provider-editor",
    "provider-model-list",
    "provider-model-actions",
    "error.needModel",
    "reasoningEffort",
    'id="command-allowlist"',
    'id="command-denylist"',
    'commandDenylistHelpText',
    'parseCommandDenylistText',
    "commandAllowlistHelpText",
    "parseCommandAllowlistText",
    "doctor-panel",
    "aria.diagnostics",
    "chat.hint.left",
    'id="ui-locale"',
    "workspace-missing",
    "field.workspaceHint",
    "loadLocale",
    "saveLocale",
    "loadUiFontSize",
    "applyUiFontSize",
    "saveUiFontSize",
    'id="ui-font-size"',
  ]) {
    assert.ok(appSource.includes(needle), "App.tsx missing " + needle);
  }

  assert.ok(!appSource.includes('id="settings-current-project"'), "Settings must not expose a manual current-project input");
  assert.ok(!i18nSource.includes('"field.currentProject"'), "i18n must not retain the removed current-project label");
  assert.ok(!i18nSource.includes('"field.currentProjectPlaceholder"'), "i18n must not retain the removed current-project placeholder");
  assert.ok(!i18nSource.includes('"field.currentProjectHint"'), "i18n must not retain the removed current-project help text");
  assert.ok(appSource.includes("list_provider_models"), "App should call list_provider_models");
  assert.match(
    appSource,
    /<div className="settings-header-actions">[\s\S]*?id="ui-locale"[\s\S]*?action\.back/,
    "language picker should be in the settings header actions",
  );
  assert.doesNotMatch(
    appSource,
    /<section className="settings-card" aria-label=\{t\(locale, "settings\.section\.language"\)\}>/,
    "language picker should not consume a separate settings card",
  );
  assert.ok(appSource.includes("provider-settings-card"), "provider manager should occupy the expanded settings area");
  assert.ok(appSource.includes("provider-manager-body"), "provider manager should place list and editor side by side");
  const providerListMatch = appSource.match(/<div id="provider-list"[\s\S]*?<\/div>\s*\{selectedProvider \?/);
  assert.ok(providerListMatch, "provider list should be separate from the selected-provider editor");
  assert.match(providerListMatch[0], /\{item\.name\.trim\(\) \|\| "Provider"\}/, "provider list should render only provider names");
  assert.doesNotMatch(providerListMatch[0], /item\.base_url|item\.api_key/, "provider list must not render Base URL or API Key");
  assert.ok(cssSource.includes(".provider-settings-card"), "styles.css missing expanded provider manager styles");
  assert.ok(cssSource.includes(".provider-manager-body"), "styles.css missing provider list/editor layout");
  assert.ok(cssSource.includes(".settings-locale-control"), "styles.css missing header language control styles");
  assert.ok(cssSource.includes(".settings-font-size-control"), "styles.css missing UI font-size control styles");
  assert.ok(appearanceSource.includes("xcoding.uiFontSize"), "appearance.ts must persist the UI font size");
  assert.ok(appearanceSource.includes("document.documentElement.style.fontSize"), "appearance.ts must apply the UI font size at the document root");
  assert.ok(cssSource.includes(".provider-list-item"), "styles.css missing provider list item styles");
  assert.ok(cssSource.includes(".provider-editor"), "styles.css missing selected provider editor styles");
  assert.ok(!cssSource.includes(".provider-entry {"), "legacy expanded provider cards should be removed");
  assert.ok(appSource.includes("xcoding.cachedModels.v1"), "App should cache provider models for fast startup");
  assert.ok(appSource.includes("silent: true"), "startup model refresh should be silent/deferred");
  assert.ok(!appSource.includes('id="provider-model"'), "Settings must not expose a model-binding selector");
  assert.ok(appSource.includes("provider-model-list"), "Settings should show fetched models as a read-only list");
  assert.ok(appSource.includes("provider: selectedProvider"), "Fetch models should target the selected provider");
  assert.ok(appSource.includes("updateComposerModels: selectedProvider.id === activeProvider?.id"), "Fetching another provider must not replace composer models");
  assert.ok(appSource.includes('id="composer-model"'), "Composer must retain the model selector");
  assert.ok(appSource.includes("pendingConversationScrollTopRef"), "App should retain the conversation scroll offset while settings are open");
  assert.ok(appSource.includes("persistWorkspaceMode"), "mode changes should persist to the active workspace config");
  assert.ok(appSource.includes("workspaceRootRef"), "workspace mode saves should reject stale project callbacks");
  assert.ok(appSource.includes("workspaceModeRevisionRef"), "workspace mode saves should reject stale mode callbacks");
  assert.ok(appSource.includes("workspaceModeSaveChainRef"), "workspace mode saves should be serialized");
  assert.ok(appSource.includes("function openSettings(): void"), "Settings navigation should capture the conversation scroll offset");
  assert.ok(
    appSource.includes("pendingConversationScrollToBottomRef.current = conversationAtBottomRef.current"),
    "Settings navigation should preserve bottom-follow mode while the conversation is unmounted",
  );
  assert.ok(appSource.includes("function returnToWorkbench(): void"), "Settings back navigation should restore the workbench view");
  assert.ok(appSource.includes("onClick={openSettings}"), "Sidebar settings entry should capture the conversation scroll offset");
  assert.ok(appSource.includes("node.scrollTop = Math.min(scrollTop, maxScrollTop)"), "Returning from settings should restore the prior conversation scroll offset");
  assert.ok(appSource.includes("hiddenProjectPaths"), "App should track projects removed from the project area");
  assert.ok(appSource.includes("removeProjectFromArea"), "App should support removing projects without deleting folders");
  const saveAllSettingsStart = appSource.indexOf("async function saveAllSettings");
  const doctorChecksStart = appSource.indexOf("  const doctorChecks", saveAllSettingsStart);
  assert.ok(saveAllSettingsStart >= 0 && doctorChecksStart > saveAllSettingsStart, "saveAllSettings source block should be discoverable");
  const saveAllSettingsSource = appSource.slice(saveAllSettingsStart, doctorChecksStart);
  assert.ok(
    saveAllSettingsSource.includes("hidden_project_paths: hiddenProjectPaths"),
    "saving settings must preserve projects removed from the project area",
  );
  assert.ok(appSource.includes("chooseExistingProjectFolder"), "App should support choosing an existing project folder");
  assert.ok(appSource.includes("import_project"), "App should call import_project for external/workspace folders");
  assert.ok(appSource.includes("pick_directory"), "App should open a directory picker for project import");
  assert.ok(appSource.includes("!activeIsChat"), "chat selection should not keep a project highlighted");
  assert.ok(i18nSource.includes("action.removeProject"), "i18n missing action.removeProject");
  assert.ok(i18nSource.includes("action.chooseProjectFolder"), "i18n missing action.chooseProjectFolder");
  assert.ok(i18nSource.includes("project.removeConfirm"), "i18n missing project.removeConfirm");
  const desktopMainSource = await readFile(resolve(repositoryRoot, "apps/desktop/src-tauri/src/main.rs"), "utf8");
  assert.match(
    desktopMainSource,
    /async fn import_project[\s\S]*?tauri::async_runtime::spawn_blocking/,
    "external project copies must run off the desktop event thread",
  );
  assert.ok(desktopMainSource.includes("TrayIconBuilder"), "desktop should create a system tray icon");
  assert.ok(desktopMainSource.includes('MenuItem::with_id(app, "show", "显示 XCoding"'), "tray should provide a show action");
  assert.ok(desktopMainSource.includes('MenuItem::with_id(app, "quit", "退出"'), "tray should provide a quit action");
  assert.ok(desktopMainSource.includes("show_menu_on_left_click(false)"), "tray left click should restore the window instead of opening the menu");
  assert.ok(desktopMainSource.includes("WindowEvent::CloseRequested"), "main window close should be intercepted");
  assert.ok(desktopMainSource.includes("api.prevent_close()"), "main window close should prevent process exit");
  assert.ok(desktopMainSource.includes("restore_main_window"), "tray actions should restore the main window");
  assert.ok(desktopMainSource.includes('"quit" => app.exit(0)'), "tray quit action should exit the process");
  const projectsSource = await readFile(resolve(repositoryRoot, "apps/desktop/src-tauri/src/projects.rs"), "utf8");
  assert.ok(projectsSource.includes("pub fn import_project"), "projects.rs missing import_project");
  assert.ok(projectsSource.includes("copy_dir_recursive"), "projects.rs should copy external folders into workspace");
  const projectsProductionSource = projectsSource.split("#[cfg(test)]", 1)[0];
  assert.ok(!/fs::remove_dir|remove_dir_all/.test(projectsProductionSource), "project removal must not delete workspace folders");
  const protocolSource = await readFile(resolve(repositoryRoot, "packages/protocol/src/index.ts"), "utf8");
  assert.ok(protocolSource.includes("hidden_project_paths"), "protocol missing hidden_project_paths");
  assert.ok(protocolSource.includes("model_context_windows"), "protocol missing model_context_windows");
  assert.ok(protocolSource.includes("context_compaction_threshold_percent"), "protocol missing compaction threshold");
  assert.ok(protocolSource.includes("ImportProjectResult"), "protocol missing ImportProjectResult");
  const apiSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/workspaceApi.ts"), "utf8");
  assert.ok(apiSource.includes("includeBranches"), "git_environment should support includeBranches");
  const workspaceToolsSource = await readFile(resolve(repositoryRoot, "apps/desktop/src-tauri/src/workspace_tools.rs"), "utf8");
  assert.ok(workspaceToolsSource.includes("CREATE_NO_WINDOW"), "Git probes must not show a Windows console at startup");
  assert.ok(workspaceToolsSource.includes("shell.creation_flags(CREATE_NO_WINDOW)"), "embedded terminal commands must not create a Windows console");
  assert.ok(workspaceToolsSource.includes("GIT_TERMINAL_PROMPT"), "Git probes must not wait for interactive prompts");
  assert.ok(workspaceToolsSource.includes("git environment lookup timed out"), "Git environment lookup must time out instead of blocking the UI");
  const toolsSource = await readFile(resolve(repositoryRoot, "crates/xcoding-tools/src/lib.rs"), "utf8");
  assert.ok(toolsSource.includes("fn git_command() -> Command"), "agent Git tools must use a shared hidden-process launcher");
  assert.ok(toolsSource.includes("command.creation_flags(CREATE_NO_WINDOW)"), "agent Git tools must not open a Windows console");
  assert.equal((toolsSource.match(/workspace_command\(\"git\"\)/g) || []).length, 1, "all Git invocations must use the hidden-process launcher");

  assert.ok(appSource.includes("composer-select"), "App should style composer model/reasoning selects");
  assert.ok(cssSource.includes(".composer-select"), "styles.css missing .composer-select");
  assert.ok(appSource.includes("persistComposerPrefs"), "App should persist model/reasoning from composer");
  assert.ok(!appSource.includes('id="default-model"'), "model picker should leave Settings");
  assert.ok(i18nSource.includes("field.reasoning"), "i18n missing field.reasoning");
  assert.ok(i18nSource.includes("models.refresh") && i18nSource.includes("error.needModel"), "i18n missing model list keys");
  assert.ok(!i18nSource.includes("Select a model in Settings"), "needModel copy should not point at Settings");
  assert.ok(!/const defaultModel\s*=\s*"gpt-5.5"/.test(appSource), "UI must not autofill gpt-5.5");
  assert.ok(appSource.includes("context-usage-popover"), "composer should expose a context-usage popover");
  assert.ok(appSource.includes("SYSTEM_CONTEXT_TOKEN_RESERVE"), "context usage should reserve system/tool context");
  assert.ok(appSource.includes("estimateMessageTokens") && appSource.includes("IMAGE_CONTEXT_TOKEN_ESTIMATE"), "context usage should estimate text and image context");
  assert.ok(appSource.includes("contextWindowForModel"), "context usage should choose a model context window");
  assert.ok(cssSource.includes(".context-usage-popover") && cssSource.includes(".context-usage-meter"), "styles.css missing context usage popover styles");
  assert.ok(i18nSource.includes("context.title") && i18nSource.includes("context.estimated"), "i18n missing context usage copy");
  assert.ok(appSource.includes("model_context_windows"), "settings should persist model context window overrides");
  assert.ok(appSource.includes("context_compaction_threshold_percent"), "settings should persist compaction threshold");
  assert.ok(appSource.includes("modelContextWindowEntries"), "settings should track model context window entries");
  assert.ok(appSource.includes("normalizeModelContextWindows") && appSource.includes("contextWindowMapFromEntries"), "settings should normalize model context window values");
  assert.ok(appSource.includes("context-windows-settings-card") && appSource.includes('id="context-window-list"') && appSource.includes("context-window-row"), "settings should render model context window controls");
  assert.ok(cssSource.includes(".context-windows-settings-card") && cssSource.includes(".context-window-list") && cssSource.includes(".context-window-row"), "styles.css missing model context window settings layout");
  assert.ok(appSource.includes("settings-tabs-container") && appSource.includes('role="tablist"') && appSource.includes('role="tabpanel"') && appSource.includes("setSettingsTab"), "settings sections should render as a tabbed interface");
  assert.ok(cssSource.includes(".settings-tabs-container") && cssSource.includes(".settings-tab"), "styles.css missing settings tab layout");
  assert.ok(cssSource.includes("grid-template-columns: minmax(0, 1fr) minmax(0, 1fr) auto;"), "context window rows should align model, tokens, and remove button horizontally");
  assert.ok(i18nSource.includes("settings.contextWindows.title") && i18nSource.includes("field.contextWindowTokens") && i18nSource.includes("action.addContextWindow"), "i18n missing model context window settings keys");

  assert.ok(protocolSource.includes("VisionDelegateConfig") && protocolSource.includes("vision_delegate"), "protocol missing vision delegate config");
  assert.ok(protocolSource.includes("vision_delegate_start") && protocolSource.includes("vision_delegate_success") && protocolSource.includes("vision_delegate_failed"), "protocol missing vision delegate events");
  assert.ok(appSource.includes("vision-settings-card") && appSource.includes('id="vision-delegate-provider"') && appSource.includes('id="vision-delegate-model"') && appSource.includes('id="vision-delegate-timeout"'), "settings should render vision delegate controls");
  assert.ok(appSource.includes("visionDelegateConfigFromForm") && appSource.includes("visionDelegateFormFromConfig"), "settings should map vision delegate config to form state");
  assert.ok(appSource.includes("model_capabilities"), "settings should persist model vision capabilities");
  assert.ok(cssSource.includes(".vision-settings-card"), "styles.css missing vision delegate settings layout");
  assert.ok(i18nSource.includes("settings.vision.title") && i18nSource.includes("field.visionDelegateProvider") && i18nSource.includes("field.visionDelegateModel"), "i18n missing vision delegate settings keys");
  assert.ok(i18nSource.includes("activity.visionDelegate") && i18nSource.includes("activity.visionDelegateFailed"), "i18n missing vision delegate activity copy");

  for (const needle of [
    ".mode-help",
    ".doctor-panel",
    ".doctor-list",
    ".workspace-settings select",
    "input[readonly]",
    ".command-allowlist-input",
    "input.workspace-missing",
  ]) {
    assert.ok(cssSource.includes(needle), "styles.css missing " + needle);
  }

  assert.ok(cliSource.includes("function parseModeOption"), "CLI missing parseModeOption");
  assert.ok(cliSource.includes("invalid mode:"), "CLI missing invalid mode error");
  assert.ok(cliSource.includes("Mode policy:"), "CLI help missing Mode policy");
  assert.ok(
    cliSource.includes("allowlisted safe commands") ||
      cliSource.includes("allowlisted safe command"),
    "CLI Mode policy should describe auto-edit allowlist behavior",
  );
  assert.ok(cliSource.includes("--command-allowlist"), "CLI missing command-allowlist flag");
  assert.ok(cliSource.includes("--command-denylist"), "CLI missing command-denylist flag");
  assert.ok(cliSource.includes("Command denylist:"), "CLI help missing Command denylist section");
  assert.ok(cliSource.includes("parseCommandDenylistOption"), "CLI missing denylist parser");
  assert.ok(cliSource.includes("Command allowlist:"), "CLI help missing Command allowlist section");
  assert.ok(cliSource.includes("parseCommandAllowlistOption"), "CLI missing allowlist parser");

  assert.equal(isValidMode("ask"), true);
  assert.equal(isValidMode("auto-edit"), true);
  assert.equal(isValidMode("full-auto"), true);
  assert.equal(isValidMode("yolo"), false);
  assert.equal(formatModeOption("ask"), "Ask");
  assert.equal(formatModeOption("auto-edit"), "Auto edit");
  assert.equal(formatModeOption("full-auto"), "Full auto");
  assert.match(modeHelpText("ask"), /ordinary workspace file patches/i);
  assert.match(modeHelpText("ask"), /Commands and high-risk writes still need approval/i);
  assert.match(modeHelpText("auto-edit"), /allowlisted safe commands/i);
  assert.match(modeHelpText("auto-edit"), /High-risk writes and other commands still need approval/i);
  assert.match(modeHelpText("full-auto"), /approves every request automatically/i);
  assert.match(modeHelpText("full-auto"), /Network access and hard-denied destructive commands stay blocked/i);

  const blocked = buildDesktopDoctorChecks({
    workspaceRoot: "",
    providerStatus: null,
    mode: "ask",
    model: "",
  });
  assert.equal(desktopDoctorReady(blocked), false);
  assert.equal(blocked.find((c) => c.name === "workspace")?.ok, false);
  assert.equal(blocked.find((c) => c.name === "provider_auth")?.ok, false);
  assert.equal(blocked.find((c) => c.name === "base_url")?.ok, false);
  assert.equal(blocked.find((c) => c.name === "defaults")?.ok, false);

  const ready = buildDesktopDoctorChecks({
    workspaceRoot: "D:\\\\work\\\\demo",
    providerStatus: {
      ready: true,
      message: "OPENAI_API_KEY is set",
      base_url: "https://ai.v58.dev/v1",
      key_hint: "…4730",
    },
    mode: "auto-edit",
    model: "gpt-5.5",
    provider: "openai",
  });
  assert.equal(desktopDoctorReady(ready), true);
  assert.equal(ready.find((c) => c.name === "defaults")?.detail.includes("Auto edit"), true);
  assert.equal(ready.find((c) => c.name === "base_url")?.detail, "https://ai.v58.dev/v1");

  // Epoch barrier prevents background tasks from hijacking a fresh composer after "new task".
  assert.ok(appSource.includes("const composerEpochRef"), "Missing composerEpochRef");
  assert.ok(appSource.includes("const draftEpochRef"), "Missing draftEpochRef");
  assert.ok(appSource.includes("const draftKnownSessionIdsRef"), "Missing draftKnownSessionIdsRef");
  assert.ok(
    appSource.includes("composerEpochRef.current += 1"),
    "resetComposerSession must bump composerEpochRef",
  );
  assert.ok(
    appSource.includes("draftEpochRef.current === composerEpochRef.current"),
    "Event handler or invoke result must check epoch ownership",
  );
  assert.ok(
    appSource.includes("draftEpochRef.current = composerEpochRef.current"),
    "New draft turn must claim the current epoch",
  );
  assert.ok(
    appSource.includes("draftKnownSessionIdsRef.current = new Set(sessionsRef.current.map"),
    "New draft turn must snapshot known sessions",
  );
  assert.ok(
    appSource.includes("draftEpochRef.current = null"),
    "Draft turn cleanup must release epoch ownership",
  );

  const resetComposerStart = appSource.indexOf("function resetComposerSession(");
  assert.ok(resetComposerStart >= 0, "App missing resetComposerSession");
  const resetComposerSource = appSource.slice(
    resetComposerStart,
    appSource.indexOf("\n  }\n", resetComposerStart),
  );
  assert.ok(
    resetComposerSource.includes("composerEpochRef.current += 1;"),
    "resetComposerSession must bump the epoch before clearing the composer",
  );
  assert.ok(
    resetComposerSource.includes("setDraftRunning(false);"),
    "resetComposerSession must clear draftRunning",
  );
  assert.ok(
    !resetComposerSource.includes("if (!draftInFlightRef.current)"),
    "resetComposerSession must clear draftRunning unconditionally, otherwise the new composer stays blocked",
  );

  console.log("Desktop config UX checks passed.");
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
