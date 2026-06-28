export const THEME_IDS = ["midnight", "daylight", "wechat", "aurora", "carbon", "plum"] as const;
export type ThemeId = (typeof THEME_IDS)[number];

export interface ThemeOption {
  id: ThemeId;
  name: string;
  preview: [string, string, string];
}

export const THEMES: ThemeOption[] = [
  { id: "midnight", name: "星夜", preview: ["#111827", "#182033", "#4f46e5"] },
  { id: "daylight", name: "晨光", preview: ["#eef3f6", "#ffffff", "#416f78"] },
  { id: "wechat", name: "微信", preview: ["#edf1ee", "#ffffff", "#4f6f58"] },
  { id: "aurora", name: "极光", preview: ["#071113", "#122224", "#4d807a"] },
  { id: "carbon", name: "曜石", preview: ["#090b0d", "#161a20", "#8d7048"] },
  { id: "plum", name: "梅影", preview: ["#101011", "#1f1f22", "#965f6c"] },
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
