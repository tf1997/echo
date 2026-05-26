export type ThemeId = "midnight" | "wechat" | "daylight" | "aurora" | "carbon" | "plum";

export interface ThemeOption {
  id: ThemeId;
  name: string;
  preview: [string, string, string];
}

export const THEMES: ThemeOption[] = [
  { id: "midnight", name: "星夜", preview: ["#101624", "#1f2937", "#6366f1"] },
  { id: "wechat", name: "微信", preview: ["#f7f7f7", "#d6d6d6", "#95ec69"] },
  { id: "daylight", name: "清昼", preview: ["#e6edf2", "#dce7ec", "#4e8396"] },
  { id: "aurora", name: "极光", preview: ["#0b1719", "#192d31", "#5d918b"] },
  { id: "carbon", name: "曜石", preview: ["#0b0d10", "#20242a", "#9b7a4d"] },
  { id: "plum", name: "绛莓", preview: ["#1e171d", "#362933", "#9b5b70"] },
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
