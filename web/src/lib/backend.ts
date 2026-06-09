/**
 * Backend connection store.
 *
 * Owns the desktop app's server profiles, the active server, theme preference,
 * and per-server auth tokens — persisted to localStorage so they survive
 * restarts (the Tauri webview keeps localStorage in the app data dir).
 *
 * It is a framework-agnostic singleton with a tiny pub/sub so both plain
 * modules and React (`useSyncExternalStore`) can observe it.
 *
 * Web build: there are no profiles. `getApiBase()` returns '' so every existing
 * relative `/api/...` URL resolves same-origin (and the Vite dev proxy still
 * applies) — i.e. behaviour is identical to before this store existed.
 */

import { isTauri } from './tauri';

export type ThemePref = 'light' | 'dark' | 'system';
export type ServerKind = 'remote' | 'local';

export interface ServerProfile {
  id: string;
  name: string;
  /** Base origin, normalized, no trailing slash. '' when unset. */
  url: string;
  kind: ServerKind;
}

interface DesktopState {
  profiles: ServerProfile[];
  activeId: string | null;
  theme: ThemePref;
}

interface AuthRecord {
  token: string;
  username: string;
}

const STATE_KEY = 'xc.desktop.v1';
const AUTH_PREFIX = 'xc.auth.';
const LEGACY_AUTH_KEY = 'xenoclaw_auth';
const WEB_SERVER_ID = '__web__';
export const LOCAL_PROFILE_ID = 'local';

function localProfile(): ServerProfile {
  return { id: LOCAL_PROFILE_ID, name: 'Local backend', url: '', kind: 'local' };
}

function defaultState(): DesktopState {
  return {
    profiles: [localProfile()],
    activeId: isTauri() ? LOCAL_PROFILE_ID : WEB_SERVER_ID,
    theme: 'system',
  };
}

function load(): DesktopState {
  try {
    const raw = localStorage.getItem(STATE_KEY);
    if (raw) {
      const parsed = JSON.parse(raw) as DesktopState;
      if (!Array.isArray(parsed.profiles)) parsed.profiles = [];
      // The built-in local profile must always exist.
      if (!parsed.profiles.some((p) => p.id === LOCAL_PROFILE_ID)) {
        parsed.profiles.unshift(localProfile());
      }
      if (!parsed.theme) parsed.theme = 'system';
      return parsed;
    }
  } catch {
    /* fall through to defaults */
  }
  return defaultState();
}

let state: DesktopState = load();
const listeners = new Set<() => void>();

function save(): void {
  try {
    localStorage.setItem(STATE_KEY, JSON.stringify(state));
  } catch {
    /* storage may be unavailable; ignore */
  }
}

function emit(): void {
  for (const l of listeners) l();
}

export function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

// ---- Profiles ---------------------------------------------------------------

export function getProfiles(): ServerProfile[] {
  return state.profiles;
}

export function getActiveId(): string {
  return state.activeId ?? (isTauri() ? LOCAL_PROFILE_ID : WEB_SERVER_ID);
}

export function getActiveProfile(): ServerProfile | null {
  return state.profiles.find((p) => p.id === getActiveId()) ?? null;
}

export function setActiveId(id: string): void {
  if (id === state.activeId) return;
  state = { ...state, activeId: id };
  save();
  emit();
}

/** Normalize a user-entered URL: add scheme if missing, strip trailing slash. */
export function normalizeUrl(raw: string): string {
  let u = (raw ?? '').trim();
  if (!u) return '';
  if (!/^https?:\/\//i.test(u)) u = `http://${u}`;
  return u.replace(/\/+$/, '');
}

export function upsertProfile(input: {
  id: string;
  name: string;
  url: string;
  kind?: ServerKind;
}): void {
  const profile: ServerProfile = {
    id: input.id,
    name: input.name.trim() || input.id,
    url: input.kind === 'local' ? input.url.replace(/\/+$/, '') : normalizeUrl(input.url),
    kind: input.kind ?? 'remote',
  };
  const idx = state.profiles.findIndex((p) => p.id === profile.id);
  const profiles = [...state.profiles];
  if (idx >= 0) profiles[idx] = profile;
  else profiles.push(profile);
  state = { ...state, profiles };
  save();
  emit();
}

export function removeProfile(id: string): void {
  if (id === LOCAL_PROFILE_ID) return; // keep the built-in local profile
  const profiles = state.profiles.filter((p) => p.id !== id);
  let activeId = state.activeId;
  if (activeId === id) activeId = profiles[0]?.id ?? LOCAL_PROFILE_ID;
  state = { ...state, profiles, activeId };
  clearAuth(id);
  save();
  emit();
}

/** Set the local profile's URL (called once the sidecar reports its port). */
export function setLocalUrl(url: string): void {
  upsertProfile({ id: LOCAL_PROFILE_ID, name: 'Local backend', url, kind: 'local' });
}

/** Generate a stable-ish unique id for a new profile. */
export function newProfileId(): string {
  if (typeof crypto !== 'undefined' && crypto.randomUUID) return crypto.randomUUID();
  return `srv-${Date.now()}-${Math.floor(Math.random() * 1e6)}`;
}

// ---- Theme ------------------------------------------------------------------

export function getTheme(): ThemePref {
  return state.theme;
}

export function setTheme(theme: ThemePref): void {
  if (theme === state.theme) return;
  state = { ...state, theme };
  save();
  emit();
}

// ---- URL resolution ---------------------------------------------------------

/** Active API base origin. '' means "same origin" (web build / Vite proxy). */
export function getApiBase(): string {
  if (!isTauri()) return '';
  return getActiveProfile()?.url ?? '';
}

/** Resolve a possibly-relative API path against the active server. */
export function resolveApiUrl(input: string): string {
  if (/^https?:\/\//i.test(input)) return input; // already absolute
  const base = getApiBase();
  if (!base) return input; // relative — web/same-origin
  return base + (input.startsWith('/') ? input : `/${input}`);
}

/** WebSocket origin for the active server (ws:// or wss://). */
export function getWsBase(): string {
  const base = getApiBase();
  if (!base) {
    const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    return `${proto}//${window.location.host}`;
  }
  // http -> ws, https -> wss
  return base.replace(/^http/i, 'ws');
}

// ---- Per-server auth --------------------------------------------------------

export function getActiveServerId(): string {
  return isTauri() ? getActiveId() : WEB_SERVER_ID;
}

function authKey(serverId: string): string {
  return AUTH_PREFIX + serverId;
}

export function loadAuth(serverId = getActiveServerId()): AuthRecord | null {
  try {
    const raw = localStorage.getItem(authKey(serverId));
    if (raw) return JSON.parse(raw) as AuthRecord;
  } catch {
    /* ignore */
  }
  // Migrate the pre-profiles single-key auth (web installs).
  if (serverId === WEB_SERVER_ID) {
    try {
      const legacy = localStorage.getItem(LEGACY_AUTH_KEY);
      if (legacy) {
        localStorage.setItem(authKey(serverId), legacy);
        return JSON.parse(legacy) as AuthRecord;
      }
    } catch {
      /* ignore */
    }
  }
  return null;
}

export function saveAuth(rec: AuthRecord, serverId = getActiveServerId()): void {
  try {
    localStorage.setItem(authKey(serverId), JSON.stringify(rec));
    if (serverId === WEB_SERVER_ID) {
      localStorage.setItem(LEGACY_AUTH_KEY, JSON.stringify(rec));
    }
  } catch {
    /* ignore */
  }
  emit();
}

export function clearAuth(serverId = getActiveServerId()): void {
  try {
    localStorage.removeItem(authKey(serverId));
    if (serverId === WEB_SERVER_ID) localStorage.removeItem(LEGACY_AUTH_KEY);
  } catch {
    /* ignore */
  }
  emit();
}
