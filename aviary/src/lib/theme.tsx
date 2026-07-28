import { createContext, useContext, useEffect, useMemo, useState } from "react";

/**
 * Aviary themes map 1:1 to the color modes in the Figma file.
 * Dark-family themes carry the `dark` class so shadcn's `dark:` variants apply.
 */
export const THEMES = {
  dark: { label: "Dark", classes: ["dark"] },
  light: { label: "Light", classes: [] },
  aurora: { label: "Aurora", classes: ["dark", "theme-aurora"] },
  ember: { label: "Ember", classes: ["dark", "theme-ember"] },
  paper: { label: "Paper", classes: ["theme-paper"] },
} as const;

export type ThemeName = keyof typeof THEMES;

const ALL_CLASSES = Array.from(
  new Set(Object.values(THEMES).flatMap((t) => t.classes)),
);

const STORAGE_KEY = "aviary.theme";

type ThemeContextValue = {
  theme: ThemeName;
  setTheme: (t: ThemeName) => void;
};

const ThemeContext = createContext<ThemeContextValue | null>(null);

function readStoredTheme(): ThemeName {
  if (typeof localStorage === "undefined") return "dark";
  const stored = localStorage.getItem(STORAGE_KEY);
  return stored && stored in THEMES ? (stored as ThemeName) : "dark";
}

export function ThemeProvider({ children }: { children: React.ReactNode }) {
  const [theme, setThemeState] = useState<ThemeName>(readStoredTheme);

  useEffect(() => {
    const root = document.documentElement;
    root.classList.remove(...ALL_CLASSES);
    root.classList.add(...THEMES[theme].classes);
    localStorage.setItem(STORAGE_KEY, theme);
  }, [theme]);

  const value = useMemo(
    () => ({ theme, setTheme: setThemeState }),
    [theme],
  );

  return <ThemeContext value={value}>{children}</ThemeContext>;
}

export function useTheme() {
  const ctx = useContext(ThemeContext);
  if (!ctx) throw new Error("useTheme must be used within a ThemeProvider");
  return ctx;
}
