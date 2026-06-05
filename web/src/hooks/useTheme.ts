import { useEffect, useSyncExternalStore } from 'react';
import {
  getTheme,
  setTheme as setStoredTheme,
  subscribe,
  type ThemePref,
} from '../lib/backend';

function systemPrefersDark(): boolean {
  return (
    typeof window !== 'undefined' &&
    typeof window.matchMedia === 'function' &&
    window.matchMedia('(prefers-color-scheme: dark)').matches
  );
}

function resolve(pref: ThemePref): 'light' | 'dark' {
  if (pref === 'system') return systemPrefersDark() ? 'dark' : 'light';
  return pref;
}

/** Apply the resolved theme to <html data-theme>. Safe to call pre-React. */
export function applyTheme(pref: ThemePref = getTheme()): void {
  if (typeof document === 'undefined') return;
  document.documentElement.dataset.theme = resolve(pref);
}

/**
 * Theme hook — reads the persisted preference, applies it to the document, and
 * follows the OS setting live while in "system" mode.
 */
export function useTheme() {
  const theme = useSyncExternalStore(subscribe, getTheme, getTheme);

  useEffect(() => {
    applyTheme(theme);
    if (theme !== 'system') return;
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    const handler = () => applyTheme('system');
    mq.addEventListener('change', handler);
    return () => mq.removeEventListener('change', handler);
  }, [theme]);

  return { theme, resolved: resolve(theme), setTheme: setStoredTheme };
}
