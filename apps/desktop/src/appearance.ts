export const DEFAULT_UI_FONT_SIZE = 14;
export const MIN_UI_FONT_SIZE = 14;
export const MAX_UI_FONT_SIZE = 20;

const UI_FONT_SIZE_STORAGE_KEY = "xcoding.uiFontSize";

export function clampUiFontSize(value: number): number {
  if (!Number.isFinite(value)) return DEFAULT_UI_FONT_SIZE;
  return Math.round(Math.min(MAX_UI_FONT_SIZE, Math.max(MIN_UI_FONT_SIZE, value)));
}

export function loadUiFontSize(): number {
  try {
    const value = localStorage.getItem(UI_FONT_SIZE_STORAGE_KEY);
    return value == null ? DEFAULT_UI_FONT_SIZE : clampUiFontSize(Number(value));
  } catch {
    return DEFAULT_UI_FONT_SIZE;
  }
}

export function saveUiFontSize(value: number): void {
  try {
    localStorage.setItem(UI_FONT_SIZE_STORAGE_KEY, String(clampUiFontSize(value)));
  } catch {
    // Ignore storage failures; the active session still applies the chosen size.
  }
}

export function applyUiFontSize(value: number): void {
  if (typeof document === "undefined") return;
  document.documentElement.style.fontSize = `${clampUiFontSize(value)}px`;
}
