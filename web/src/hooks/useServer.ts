import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react';
import * as backend from '../lib/backend';
import { invoke, isTauri, listen } from '../lib/tauri';

/** React view over the server-profiles store. */
export function useServers() {
  const profiles = useSyncExternalStore(backend.subscribe, backend.getProfiles, backend.getProfiles);
  const activeId = useSyncExternalStore(backend.subscribe, backend.getActiveId, backend.getActiveId);
  const active = profiles.find((p) => p.id === activeId) ?? null;

  return {
    profiles,
    activeId,
    active,
    isTauri: isTauri(),
    setActive: backend.setActiveId,
    upsert: backend.upsertProfile,
    remove: backend.removeProfile,
    newId: backend.newProfileId,
  };
}

export type LiveStatus = 'connected' | 'connecting' | 'offline';

/**
 * Poll the active server's unauthenticated /health endpoint to surface a live
 * connection indicator. Re-runs when the active server changes.
 */
export function useConnection(intervalMs = 6000): LiveStatus {
  const activeId = useSyncExternalStore(backend.subscribe, backend.getActiveId, backend.getActiveId);
  // The active profile's URL can change (e.g. local sidecar boot) without the
  // id changing, so depend on the resolved base too.
  const base = useSyncExternalStore(backend.subscribe, backend.getApiBase, backend.getApiBase);
  const [status, setStatus] = useState<LiveStatus>('connecting');

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;

    const ping = async () => {
      try {
        const res = await fetch(backend.resolveApiUrl('/api/v1/health'), { method: 'GET' });
        if (!cancelled) setStatus(res.ok ? 'connected' : 'offline');
      } catch {
        if (!cancelled) setStatus('offline');
      }
      if (!cancelled) timer = setTimeout(ping, intervalMs);
    };

    setStatus('connecting');
    void ping();
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, [activeId, base, intervalMs]);

  return status;
}

export interface BackendStatus {
  running: boolean;
  port: number | null;
  url: string | null;
}

/** Control the bundled local backend sidecar (desktop only). */
export function useLocalBackend() {
  const [status, setStatus] = useState<BackendStatus>({ running: false, port: null, url: null });
  const [logs, setLogs] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const unlistenRef = useRef<(() => void) | null>(null);

  const refresh = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const s = await invoke<BackendStatus>('local_backend_status');
      setStatus(s);
    } catch {
      /* ignore */
    }
  }, []);

  useEffect(() => {
    if (!isTauri()) return;
    void refresh();
    // Stream sidecar logs.
    let active = true;
    void listen<string>('local-backend-log', (line) => {
      setLogs((prev) => [...prev.slice(-400), line.replace(/\n$/, '')]);
    }).then((un) => {
      if (active) unlistenRef.current = un;
      else un();
    });
    return () => {
      active = false;
      unlistenRef.current?.();
      unlistenRef.current = null;
    };
  }, [refresh]);

  const start = useCallback(async (port?: number) => {
    setBusy(true);
    setError(null);
    try {
      const s = await invoke<BackendStatus>('local_backend_start', { port: port ?? null });
      setStatus(s);
      if (s.url) backend.setLocalUrl(s.url);
      return s;
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      throw e;
    } finally {
      setBusy(false);
    }
  }, []);

  const stop = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const s = await invoke<BackendStatus>('local_backend_stop');
      setStatus(s);
      return s;
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      throw e;
    } finally {
      setBusy(false);
    }
  }, []);

  return { status, logs, busy, error, start, stop, refresh };
}
