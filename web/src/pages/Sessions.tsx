import { useState, useCallback, useEffect } from 'react';
import { useAuth } from '../hooks/useAuth';
import styles from './Sessions.module.css';

const API_BASE = '/api/v1';

interface Session {
  id: string;
  source: string;
  mode: string;
  created_at: string;
  last_activity: string;
}

export function Sessions() {
  const { apiFetch } = useAuth();
  const [sessions, setSessions] = useState<Session[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const fetchSessions = useCallback(async () => {
    try {
      const res = await apiFetch(`${API_BASE}/sessions`);
      if (!res.ok) throw new Error(`Failed to fetch sessions (${res.status})`);
      const data = await res.json() as { sessions: Session[] };
      setSessions(data.sessions ?? []);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to fetch sessions');
    } finally {
      setLoading(false);
    }
  }, [apiFetch]);

  useEffect(() => {
    void fetchSessions();
  }, [fetchSessions]);

  const createSession = useCallback(async () => {
    try {
      const res = await apiFetch(`${API_BASE}/sessions`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ source: 'web', mode: 'general' }),
      });
      if (!res.ok) throw new Error(`Failed to create session (${res.status})`);
      void fetchSessions();
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to create session');
    }
  }, [apiFetch, fetchSessions]);

  const deleteSession = useCallback(async (id: string) => {
    try {
      const res = await apiFetch(`${API_BASE}/sessions/${id}`, { method: 'DELETE' });
      if (!res.ok) throw new Error(`Failed to delete session (${res.status})`);
      setSessions((prev) => prev.filter((s) => s.id !== id));
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to delete session');
    }
  }, [apiFetch]);

  const switchSession = useCallback(async (id: string) => {
    try {
      const res = await apiFetch(`${API_BASE}/sessions/${id}/switch`, { method: 'POST' });
      if (!res.ok) throw new Error(`Failed to switch session (${res.status})`);
      void fetchSessions();
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to switch session');
    }
  }, [apiFetch, fetchSessions]);

  function formatTimestamp(iso: string): string {
    const date = new Date(iso);
    return date.toLocaleString(undefined, {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    });
  }

  if (loading) {
    return (
      <div className={styles.container}>
        <h1 className={styles.title}>Sessions</h1>
        <p className={styles.loading}>Loading sessions...</p>
      </div>
    );
  }

  return (
    <div className={styles.container}>
      <div className={styles.header}>
        <h1 className={styles.title}>Sessions</h1>
        <button className={styles.createButton} onClick={createSession}>
          + New Session
        </button>
      </div>

      {error && (
        <div className={styles.errorBanner} role="alert">
          <span className={styles.errorIcon}>⚠</span>
          {error}
        </div>
      )}

      {sessions.length === 0 ? (
        <p className={styles.emptyState}>No active sessions</p>
      ) : (
        <div className={styles.list}>
          {sessions.map((session) => (
            <div key={session.id} className={styles.sessionCard}>
              <div className={styles.sessionInfo}>
                <span className={styles.sessionId}>{session.id.slice(0, 8)}...</span>
                <div className={styles.sessionMeta}>
                  <span className={styles.badge}>{session.source}</span>
                  <span className={styles.badge}>{session.mode}</span>
                </div>
                <div className={styles.sessionTimes}>
                  <span className={styles.timeLabel}>Created: {formatTimestamp(session.created_at)}</span>
                  <span className={styles.timeLabel}>Last active: {formatTimestamp(session.last_activity)}</span>
                </div>
              </div>
              <div className={styles.sessionActions}>
                <button
                  className={styles.actionButton}
                  onClick={() => switchSession(session.id)}
                  title="Switch to this session"
                >
                  Switch
                </button>
                <button
                  className={`${styles.actionButton} ${styles.deleteButton}`}
                  onClick={() => deleteSession(session.id)}
                  title="Delete session"
                >
                  Delete
                </button>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
