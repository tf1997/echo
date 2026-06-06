export const THEME_IDS = ["midnight", "daylight", "wechat", "aurora", "carbon", "plum"] as const;
export type ThemeId = (typeof THEME_IDS)[number];

export interface ThemeOption {
  id: ThemeId;
  name: string;
  preview: [string, string, string];
}

export const THEMES: ThemeOption[] = [
  { id: "midnight", name: "星夜", preview: ["#101624", "#1f2937", "#6366f1"] },
  { id: "daylight", name: "晨光", preview: ["#f3f7f8", "#ffffff", "#2f7f8a"] },
  { id: "wechat", name: "微信", preview: ["#f8f9f8", "#d3d8d4", "#95e66a"] },
  { id: "aurora", name: "极光", preview: ["#071113", "#122224", "#5d918b"] },
  { id: "carbon", name: "曜石", preview: ["#0b0d10", "#20242a", "#9b7a4d"] },
  { id: "plum", name: "梅影", preview: ["#101012", "#202023", "#d46a82"] },
];

const STORAGE_KEY = "echo.theme";
const DEFAULT_THEME_ID: ThemeId = "midnight";

export function isThemeId(value: string | null): value is ThemeId {
  return THEME_IDS.some((themeId) => themeId === value);
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
