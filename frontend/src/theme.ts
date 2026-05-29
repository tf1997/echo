export type ThemeId = "midnight" | "wechat" | "carbon";

export interface ThemeOption {
  id: ThemeId;
  name: string;
  preview: [string, string, string];
}

export const THEMES: ThemeOption[] = [
  { id: "midnight", name: "星夜", preview: ["#101624", "#1f2937", "#6366f1"] },
  { id: "wechat", name: "微信", preview: ["#f8f9f8", "#d3d8d4", "#95e66a"] },
  { id: "carbon", name: "曜石", preview: ["#0b0d10", "#20242a", "#9b7a4d"] },
];

const STORAGE_KEY = "echo.theme";
const DEFAULT_THEME_ID: ThemeId = "midnight";

export function isThemeId(value: string | null): value is ThemeId {
  return THEMES.some((theme) => theme.id === value);
}

export function getInitialTheme(): ThemeId {
  try {
    const stored = window.localStorage.getItem(STORAGE_KEY);
    if (isThemeId(stored)) return stored;
  } catch {
    // Keep default theme when storage is unavailable.
  }
  return DEFAULT_THEME_ID;
}

export function applyTheme(themeId: ThemeId) {
  document.documentElement.dataset.theme = themeId;
  try {
    window.localStorage.setItem(STORAGE_KEY, themeId);
  } catch {
    // Theme persistence is nice to have, not critical.
  }
}

export function initializeTheme() {
  applyTheme(getInitialTheme());
}
