export const DEFAULT_UI_FONT_SIZE = 14;
export const MIN_UI_FONT_SIZE = 14;
export const MAX_UI_FONT_SIZE = 20;

const UI_FONT_SIZE_STORAGE_KEY = "xcoding.uiFontSize";

export type Theme = "dark" | "light";

export const THEMES: Theme[] = ["dark", "light"];
export const DEFAULT_THEME: Theme = "dark";

// Kept in sync with the inline bootstrap script in index.html, which applies the
// stored theme before the bundle loads so startup never flashes the wrong shade.
export const THEME_STORAGE_KEY = "xcoding.theme";

export function normalizeTheme(value: unknown): Theme {
  return value === "light" ? "light" : DEFAULT_THEME;
}

export function loadTheme(): Theme {
  try {
    return normalizeTheme(localStorage.getItem(THEME_STORAGE_KEY));
  } catch {
    return DEFAULT_THEME;
  }
}

export function saveTheme(value: Theme): void {
  try {
    localStorage.setItem(THEME_STORAGE_KEY, normalizeTheme(value));
  } catch {
    // Ignore storage failures; the active session still applies the chosen theme.
  }
}

export function applyTheme(value: Theme): void {
  if (typeof document === "undefined") return;
  document.documentElement.dataset.theme = normalizeTheme(value);
}

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
