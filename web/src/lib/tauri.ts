/**
 * Minimal typed access to the Tauri 2 runtime.
 *
 * The same React frontend serves both the web UI and the desktop shell. When
 * running inside Tauri (`withGlobalTauri: true`), `window.__TAURI__` exposes the
 * core IPC + event APIs. In a plain browser these helpers degrade to no-ops /
 * `false`, so nothing here breaks the web build.
 */

interface TauriCore {
  invoke<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T>;
}

interface TauriEventApi {
  listen<T = unknown>(
    event: string,
    handler: (e: { payload: T }) => void,
  ): Promise<() => void>;
}

interface TauriGlobal {
  core: TauriCore;
  event: TauriEventApi;
}

declare global {
  interface Window {
    __TAURI__?: TauriGlobal;
    __TAURI_INTERNALS__?: unknown;
  }
}

/** True when the frontend is hosted inside the Tauri desktop shell. */
export function isTauri(): boolean {
  return (
    typeof window !== 'undefined' &&
    ('__TAURI_INTERNALS__' in window || '__TAURI__' in window)
  );
}

function core(): TauriCore | null {
  return (typeof window !== 'undefined' && window.__TAURI__?.core) || null;
}

/** Invoke a Rust command. Throws if not running under Tauri. */
export async function invoke<T = unknown>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<T> {
  const c = core();
  if (!c) throw new Error('Tauri runtime not available');
  return c.invoke<T>(cmd, args);
}

/** Invoke a Rust command, returning null instead of throwing. */
export async function tryInvoke<T = unknown>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<T | null> {
  try {
    return await invoke<T>(cmd, args);
  } catch {
    return null;
  }
}

/** Subscribe to a Tauri event. Returns an unsubscribe function (no-op on web). */
export async function listen<T = unknown>(
  event: string,
  handler: (payload: T) => void,
): Promise<() => void> {
  const ev = (typeof window !== 'undefined' && window.__TAURI__?.event) || null;
  if (!ev) return () => {};
  return ev.listen<T>(event, (e) => handler(e.payload));
}
