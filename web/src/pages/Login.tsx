import { useState, useCallback, type FormEvent } from 'react';
import { Navigate, useLocation, useNavigate } from 'react-router-dom';
import { useAuth } from '../hooks/useAuth';
import { isTauri } from '../lib/tauri';
import { ServerSettings, LocalBackendSettings } from '../components/DesktopSettings/DesktopSettings';
import { useServers, useConnection } from '../hooks/useServer';
import styles from './Login.module.css';

type AuthMode = 'password' | 'apikey';

interface LocationState {
  from?: { pathname: string };
}

export function Login() {
  const { isAuthenticated, loginWithPassword, loginWithApiKey } = useAuth();
  const navigate = useNavigate();
  const location = useLocation();
  // Bounce back to wherever ProtectedRoute kicked the user from, else home.
  const from = (location.state as LocationState | null)?.from?.pathname ?? '/';

  const [mode, setMode] = useState<AuthMode>('password');
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [apiKey, setApiKey] = useState('');
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);

  const handleSubmit = useCallback(async (e: FormEvent) => {
    e.preventDefault();
    setError('');
    setLoading(true);

    try {
      let success = false;
      if (mode === 'password') {
        if (!username.trim() || !password) {
          setError('Username and password are required');
          return;
        }
        success = await loginWithPassword(username.trim(), password);
        if (!success) setError('Invalid username or password');
      } else {
        if (!apiKey.trim()) {
          setError('API key is required');
          return;
        }
        success = await loginWithApiKey(apiKey.trim());
        if (!success) setError('Invalid API key');
      }

      if (success) {
        navigate(from, { replace: true });
      }
    } finally {
      setLoading(false);
    }
  }, [mode, username, password, apiKey, loginWithPassword, loginWithApiKey, navigate, from]);

  // Already signed in (e.g. page refresh with a valid stored token) — bounce out.
  if (isAuthenticated) {
    return <Navigate to={from} replace />;
  }

  return (
    <div className={styles.container}>
      <div className={styles.stack}>
      <div className={styles.card}>
        <div className={styles.header}>
          <h1 className={styles.brand}>XenoClaw</h1>
          <p className={styles.subtitle}>Agent Runtime</p>
        </div>

        <div className={styles.tabs}>
          <button
            className={`${styles.tab} ${mode === 'password' ? styles.tabActive : ''}`}
            onClick={() => { setMode('password'); setError(''); }}
            type="button"
          >
            Password
          </button>
          <button
            className={`${styles.tab} ${mode === 'apikey' ? styles.tabActive : ''}`}
            onClick={() => { setMode('apikey'); setError(''); }}
            type="button"
          >
            API Key
          </button>
        </div>

        <form className={styles.form} onSubmit={handleSubmit}>
          {mode === 'password' ? (
            <>
              <div className={styles.field}>
                <label className={styles.label} htmlFor="username">Username</label>
                <input
                  id="username"
                  className={styles.input}
                  type="text"
                  value={username}
                  onChange={(e) => setUsername(e.target.value)}
                  placeholder="admin"
                  autoComplete="username"
                  autoFocus
                />
              </div>
              <div className={styles.field}>
                <label className={styles.label} htmlFor="password">Password</label>
                <input
                  id="password"
                  className={styles.input}
                  type="password"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  placeholder="••••••••"
                  autoComplete="current-password"
                />
              </div>
            </>
          ) : (
            <div className={styles.field}>
              <label className={styles.label} htmlFor="apikey">API Key</label>
              <input
                id="apikey"
                className={styles.input}
                type="password"
                value={apiKey}
                onChange={(e) => setApiKey(e.target.value)}
                placeholder="xc_..."
                autoComplete="off"
                autoFocus
              />
            </div>
          )}

          {error && (
            <div className={styles.error} role="alert">{error}</div>
          )}

          <button
            className={styles.submit}
            type="submit"
            disabled={loading}
          >
            {loading ? 'Signing in...' : 'Sign in'}
          </button>
        </form>
      </div>
      {isTauri() && <DesktopLoginPanel />}
      </div>
    </div>
  );
}

/**
 * Desktop-only pre-login panel: pick/add the backend to connect to (and start
 * the local sidecar) before authenticating. On the web build this is omitted —
 * the page is served by the backend it talks to.
 */
function DesktopLoginPanel() {
  const { active } = useServers();
  const status = useConnection();
  return (
    <div className={styles.desktopPanel}>
      <div className={styles.connBar}>
        <span className={`${styles.connDot} ${styles[`conn_${status}`]}`} aria-hidden="true" />
        <span className={styles.connText}>
          {active ? active.name : 'No server selected'} · {status}
        </span>
      </div>
      <ServerSettings />
      <LocalBackendSettings />
    </div>
  );
}
