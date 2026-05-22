import { createContext, useContext, useState, useCallback, useEffect, useRef, type ReactNode } from 'react';

interface AuthState {
  token: string | null;
  username: string | null;
  isAuthenticated: boolean;
}

interface AuthContextValue extends AuthState {
  loginWithPassword: (username: string, password: string) => Promise<boolean>;
  loginWithApiKey: (apiKey: string) => Promise<boolean>;
  logout: () => void;
  /**
   * Authenticated fetch — injects the current Bearer token and auto-logs out
   * on 401/403. Use this for every API call from a page that requires login.
   */
  apiFetch: (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;
}

const AuthContext = createContext<AuthContextValue | null>(null);

const STORAGE_KEY = 'xenoclaw_auth';

interface StoredAuth {
  token: string;
  username: string;
}

export function AuthProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<AuthState>(() => {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored) {
      try {
        const parsed: StoredAuth = JSON.parse(stored);
        return { token: parsed.token, username: parsed.username, isAuthenticated: true };
      } catch {
        localStorage.removeItem(STORAGE_KEY);
      }
    }
    return { token: null, username: null, isAuthenticated: false };
  });

  // Mirror state.token into a ref so apiFetch can be a stable, dep-free
  // callback. Without this every page's poll loop would tear down and
  // recreate its setInterval each render.
  const tokenRef = useRef(state.token);
  useEffect(() => {
    tokenRef.current = state.token;
  }, [state.token]);

  const persist = useCallback((token: string, username: string) => {
    localStorage.setItem(STORAGE_KEY, JSON.stringify({ token, username }));
    setState({ token, username, isAuthenticated: true });
  }, []);

  const logout = useCallback(() => {
    localStorage.removeItem(STORAGE_KEY);
    setState({ token: null, username: null, isAuthenticated: false });
  }, []);

  const loginWithPassword = useCallback(async (username: string, password: string): Promise<boolean> => {
    try {
      const res = await fetch('/api/v1/auth/login', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ username, password }),
      });
      if (!res.ok) return false;
      const data = await res.json() as { token: string };
      persist(data.token, username);
      return true;
    } catch {
      return false;
    }
  }, [persist]);

  const loginWithApiKey = useCallback(async (apiKey: string): Promise<boolean> => {
    try {
      // Validate by hitting a protected endpoint. /status sits behind the auth
      // middleware, so a 401 here means the key is bad.
      const res = await fetch('/api/v1/status', {
        headers: { 'Authorization': `Bearer ${apiKey}` },
      });
      if (!res.ok) return false;
      persist(apiKey, 'api-key');
      return true;
    } catch {
      return false;
    }
  }, [persist]);

  const apiFetch = useCallback(async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const headers = new Headers(init?.headers);
    const token = tokenRef.current;
    if (token && !headers.has('Authorization')) {
      headers.set('Authorization', `Bearer ${token}`);
    }
    const res = await fetch(input, { ...init, headers });
    // 401/403 means the stored token is no longer accepted — server restart
    // wiped login_tokens, key was revoked, etc. Clear auth so the router
    // bounces to /login on the next render.
    if (res.status === 401 || res.status === 403) {
      logout();
    }
    return res;
  }, [logout]);

  // Verify stored token is still valid on mount. /api/v1/status is behind the
  // auth middleware, unlike /health which is unauthenticated — so this actually
  // detects stale or revoked tokens (e.g. after a server restart wipes the
  // in-memory login_tokens set).
  useEffect(() => {
    if (state.token) {
      fetch('/api/v1/status', {
        headers: { 'Authorization': `Bearer ${state.token}` },
      }).then(res => {
        if (res.status === 401 || res.status === 403) {
          logout();
        }
      }).catch(() => {
        // Network error — don't logout, might be offline
      });
    }
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  return (
    <AuthContext.Provider value={{ ...state, loginWithPassword, loginWithApiKey, logout, apiFetch }}>
      {children}
    </AuthContext.Provider>
  );
}

export function useAuth(): AuthContextValue {
  const ctx = useContext(AuthContext);
  if (!ctx) throw new Error('useAuth must be used within AuthProvider');
  return ctx;
}
