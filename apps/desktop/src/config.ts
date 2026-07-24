import type { Mode, ProviderAuthStatus } from "@xcoding/protocol";
import { t, type Locale } from "./i18n";

export type DesktopDoctorCheck = {
  name: string;
  ok: boolean;
  detail: string;
};

export function modeHelpText(mode: Mode, locale: Locale = "en"): string {
  return mode === "auto-edit" ? t(locale, "mode.help.autoEdit") : t(locale, "mode.help.ask");
}

export function commandAllowlistHelpText(locale: Locale = "en"): string {
  return t(locale, "help.allowlist");
}

export function parseCommandAllowlistText(text: string): string[] {
  return text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0 && !line.startsWith("#"));
}

export function formatCommandAllowlistText(patterns: string[] | undefined): string {
  return (patterns ?? []).join("\n");
}

export function commandDenylistHelpText(locale: Locale = "en"): string {
  return t(locale, "help.denylist");
}

export function parseCommandDenylistText(text: string): string[] {
  return parseCommandAllowlistText(text);
}

export function formatCommandDenylistText(patterns: string[] | undefined): string {
  return formatCommandAllowlistText(patterns);
}

export function formatModeOption(mode: Mode, locale: Locale = "en"): string {
  return mode === "auto-edit" ? t(locale, "mode.autoEdit") : t(locale, "mode.ask");
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
  locale?: Locale;
}): DesktopDoctorCheck[] {
  const locale = input.locale ?? "en";
  const rootPath = input.workspaceRoot.trim();
  const model = input.model.trim();
  const provider = (input.provider ?? "openai").trim() || "openai";
  const baseUrl = input.providerStatus?.base_url?.trim() || "";

  return [
    {
      name: t(locale, "doctor.workspace"),
      ok: rootPath.length > 0,
      detail: rootPath || t(locale, "doctor.workspaceEmpty"),
    },
    {
      name: t(locale, "doctor.provider_auth"),
      ok: Boolean(input.providerStatus?.ready),
      detail: input.providerStatus?.message || t(locale, "doctor.checkingCredentials"),
    },
    {
      name: t(locale, "doctor.base_url"),
      ok: baseUrl.length > 0,
      detail: baseUrl || t(locale, "doctor.baseUrlMissing"),
    },
    {
      name: t(locale, "doctor.defaults"),
      ok: isValidMode(input.mode) && model.length > 0,
      detail: `${formatModeOption(input.mode, locale)} · ${provider} · ${model || t(locale, "doctor.noModel")}`,
    },
  ];
}

export function desktopDoctorReady(checks: DesktopDoctorCheck[]): boolean {
  return checks.every((check) => check.ok);
}
