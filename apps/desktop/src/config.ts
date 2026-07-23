import type { Mode, ProviderAuthStatus } from "@xcoding/protocol";

export type DesktopDoctorCheck = {
  name: string;
  ok: boolean;
  detail: string;
};

export function modeHelpText(mode: Mode): string {
  return mode === "auto-edit"
    ? "Auto edit applies ordinary file patches and allowlisted safe commands automatically. High-risk writes and other commands still need approval."
    : "Ask proposes patches and commands for approval before applying.";
}

export function formatModeOption(mode: Mode): string {
  return mode === "auto-edit" ? "Auto edit" : "Ask";
}

export function isValidMode(value: string): value is Mode {
  return value === "ask" || value === "auto-edit";
}

export function buildDesktopDoctorChecks(input: {
  workspaceRoot: string;
  providerStatus: ProviderAuthStatus | null;
  mode: Mode;
  model: string;
  provider?: string;
}): DesktopDoctorCheck[] {
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

export function desktopDoctorReady(checks: DesktopDoctorCheck[]): boolean {
  return checks.every((check) => check.ok);
}
