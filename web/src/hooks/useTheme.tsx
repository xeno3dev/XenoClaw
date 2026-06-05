import {
  createContext,
  useContext,
  useCallback,
  useEffect,
  useState,
  type ReactNode,
} from 'react';

/** User preference. 'system' follows the OS setting live. */
export type ThemePref = 'light' | 'dark' | 'system';
/** The concrete theme actually applied to the document. */
export type ResolvedTheme = 'light' | 'dark';

const STORAGE_KEY = 'xenoclaw_theme';

interface ThemeContextValue {
  /** The stored preference (may be 'system'). */
  theme: ThemePref;
  /** The concrete theme currently applied. */
  resolvedTheme: ResolvedTheme;
  setTheme: (pref: ThemePref) => void;
  /** Flip between the two concrete themes (drops 'system'). */
  toggleTheme: () => void;
}

const ThemeContext = createContext<ThemeContextValue | null>(null);

function prefersDark(): boolean {
  return (
    typeof window !== 'undefined' &&
    !!window.matchMedia &&
    window.matchMedia('(prefers-color-scheme: dark)').matches
  );
}

function readStoredPref(): ThemePref {
  try {
    const v = localStorage.getItem(STORAGE_KEY);
    if (v === 'light' || v === 'dark' || v === 'system') return v;
  } catch {
    /* localStorage unavailable — fall through to default */
  }
  return 'system';
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [theme, setThemeState] = useState<ThemePref>(() => readStoredPref());
  const [systemDark, setSystemDark] = useState<boolean>(() => prefersDark());

  // Derived during render — no effect-driven setState, so the toggle icon and
  // the applied theme stay in sync without cascading renders.
  const resolvedTheme: ResolvedTheme =
    theme === 'system' ? (systemDark ? 'dark' : 'light') : theme;

  // Sync the external system (the <html> data-theme attribute) with state.
  useEffect(() => {
    document.documentElement.dataset.theme = resolvedTheme;
  }, [resolvedTheme]);

  // Track OS preference changes; setState only fires from the subscription.
  useEffect(() => {
    if (!window.matchMedia) return;
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    const onChange = () => setSystemDark(mq.matches);
    mq.addEventListener('change', onChange);
    return () => mq.removeEventListener('change', onChange);
  }, []);

  const setTheme = useCallback((pref: ThemePref) => {
    setThemeState(pref);
    try {
      localStorage.setItem(STORAGE_KEY, pref);
    } catch {
      /* ignore persistence failures */
    }
  }, []);

  const toggleTheme = useCallback(() => {
    setTheme(resolvedTheme === 'dark' ? 'light' : 'dark');
  }, [resolvedTheme, setTheme]);

  return (
    <ThemeContext.Provider value={{ theme, resolvedTheme, setTheme, toggleTheme }}>
      {children}
    </ThemeContext.Provider>
  );
}

export function useTheme(): ThemeContextValue {
  const ctx = useContext(ThemeContext);
  if (!ctx) throw new Error('useTheme must be used within ThemeProvider');
  return ctx;
}
