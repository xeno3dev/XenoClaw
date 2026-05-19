import { createContext, useContext, useState, useCallback, useEffect, type ReactNode } from 'react';

interface AuthState {
  token: string | null;
  username: string | null;
  isAuthenticated: boolean;
}

interface AuthContextValue extends AuthState {
  loginWithPassword: (username: string, password: string) => Promise<boolean>;
  loginWithApiKey: (apiKey: string) => Promise<boolean>;
  logout: () => void;
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

  const persist = useCallback((token: string, username: string) => {
    localStorage.setItem(STORAGE_KEY, JSON.stringify({ token, username }));
    setState({ token, username, isAuthenticated: true });
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
      // Validate the API key by hitting a protected endpoint
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

  const logout = useCallback(() => {
    localStorage.removeItem(STORAGE_KEY);
    setState({ token: null, username: null, isAuthenticated: false });
  }, []);

  // Verify stored token is still valid on mount
  useEffect(() => {
    if (state.token) {
      fetch('/api/v1/health', {
        headers: { 'Authorization': `Bearer ${state.token}` },
      }).then(res => {
        if (!res.ok) {
          logout();
        }
      }).catch(() => {
        // Network error — don't logout, might be offline
      });
    }
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  return (
    <AuthContext.Provider value={{ ...state, loginWithPassword, loginWithApiKey, logout }}>
      {children}
    </AuthContext.Provider>
  );
}

export function useAuth(): AuthContextValue {
  const ctx = useContext(AuthContext);
  if (!ctx) throw new Error('useAuth must be used within AuthProvider');
  return ctx;
}
