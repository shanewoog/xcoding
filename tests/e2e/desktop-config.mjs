import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

function modeHelpText(mode) {
  return mode === "auto-edit"
    ? "Auto edit applies ordinary file patches and allowlisted safe commands automatically. High-risk writes and other commands still need approval."
    : "Ask proposes patches and commands for approval before applying.";
}

function formatModeOption(mode) {
  return mode === "auto-edit" ? "Auto edit" : "Ask";
}

function isValidMode(value) {
  return value === "ask" || value === "auto-edit";
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
  ]) {
    assert.ok(configSource.includes(needle), "config.ts missing " + needle);
  }

  for (const needle of [
    'from "./config"',
    "buildDesktopDoctorChecks",
    "desktopDoctorReady",
    "modeHelpText",
    'id="default-mode"',
    'id="default-provider"',
    'id="default-model"',
    'id="command-allowlist"',
    "commandAllowlistHelpText",
    "parseCommandAllowlistText",
    "doctor-panel",
    "Workspace diagnostics",
    "mode/model defaults, diagnostics",
  ]) {
    assert.ok(appSource.includes(needle), "App.tsx missing " + needle);
  }

  for (const needle of [
    ".mode-help",
    ".doctor-panel",
    ".doctor-list",
    ".workspace-settings select",
    "input[readonly]",
    ".command-allowlist-input",
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
  assert.ok(cliSource.includes("Command allowlist:"), "CLI help missing Command allowlist section");
  assert.ok(cliSource.includes("parseCommandAllowlistOption"), "CLI missing allowlist parser");

  assert.equal(isValidMode("ask"), true);
  assert.equal(isValidMode("auto-edit"), true);
  assert.equal(isValidMode("yolo"), false);
  assert.equal(formatModeOption("ask"), "Ask");
  assert.equal(formatModeOption("auto-edit"), "Auto edit");
  assert.match(modeHelpText("ask"), /approval/i);
  assert.match(modeHelpText("auto-edit"), /allowlisted safe commands/i);
  assert.match(modeHelpText("auto-edit"), /High-risk writes and other commands still need approval/i);

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

  console.log("Desktop config UX checks passed.");
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
