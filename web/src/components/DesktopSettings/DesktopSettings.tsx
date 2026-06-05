import { useCallback, useEffect, useState } from 'react';
import { useServers, useLocalBackend } from '../../hooks/useServer';
import { useTheme } from '../../hooks/useTheme';
import { isTauri, invoke, tryInvoke } from '../../lib/tauri';
import { saveAuth, clearAuth, loadAuth, LOCAL_PROFILE_ID, type ThemePref } from '../../lib/backend';
import styles from './DesktopSettings.module.css';

const THEMES: { id: ThemePref; label: string }[] = [
  { id: 'light', label: 'Light' },
  { id: 'dark', label: 'Dark' },
  { id: 'system', label: 'System' },
];

/** Appearance (theme) — available on both web and desktop. */
export function AppearanceSettings() {
  const { theme, setTheme } = useTheme();
  return (
    <section className={styles.section}>
      <h2 className={styles.sectionTitle}>Appearance</h2>
      <p className={styles.hint}>Theme preference. "System" follows your OS setting.</p>
      <div className={styles.toggle}>
        {THEMES.map((t) => (
          <button
            key={t.id}
            className={`${styles.toggleBtn} ${theme === t.id ? styles.toggleActive : ''}`}
            onClick={() => setTheme(t.id)}
            type="button"
          >
            {t.label}
          </button>
        ))}
      </div>
    </section>
  );
}

interface DraftServer {
  id: string | null;
  name: string;
  url: string;
  token: string;
}

const EMPTY_DRAFT: DraftServer = { id: null, name: '', url: '', token: '' };

/** Server profiles manager — desktop only. */
export function ServerSettings() {
  const { profiles, activeId, active, setActive, upsert, remove, newId } = useServers();
  const [draft, setDraft] = useState<DraftServer>(EMPTY_DRAFT);
  const [editing, setEditing] = useState(false);

  const startAdd = () => {
    setDraft({ ...EMPTY_DRAFT });
    setEditing(true);
  };

  const startEdit = (id: string) => {
    const p = profiles.find((x) => x.id === id);
    if (!p) return;
    const auth = loadAuth(id);
    setDraft({ id: p.id, name: p.name, url: p.url, token: auth?.token ?? '' });
    setEditing(true);
  };

  const cancel = () => {
    setDraft(EMPTY_DRAFT);
    setEditing(false);
  };

  const submit = () => {
    if (!draft.name.trim() || !draft.url.trim()) return;
    const id = draft.id ?? newId();
    upsert({ id, name: draft.name, url: draft.url, kind: 'remote' });
    if (draft.token.trim()) {
      saveAuth({ token: draft.token.trim(), username: 'api-key' }, id);
    } else if (draft.id) {
      // Token cleared on an existing profile.
      clearAuth(id);
    }
    cancel();
  };

  return (
    <section className={styles.section}>
      <h2 className={styles.sectionTitle}>Servers</h2>
      <p className={styles.hint}>
        Saved XenoClaw backends. The active server (highlighted) is used for all
        requests. Add a token here to authenticate automatically, or sign in from
        the login screen.
      </p>

      <div className={styles.serverList}>
        {profiles.map((p) => (
          <div
            key={p.id}
            className={`${styles.serverRow} ${p.id === activeId ? styles.serverRowActive : ''}`}
          >
            <button
              className={styles.serverSelect}
              onClick={() => setActive(p.id)}
              type="button"
              title="Make active"
            >
              <span className={styles.serverName}>
                {p.name}
                {p.id === activeId && <span className={styles.activeBadge}>active</span>}
                {p.kind === 'local' && <span className={styles.localBadge}>local</span>}
              </span>
              <span className={styles.serverUrl}>
                {p.kind === 'local' ? p.url || 'started on demand' : p.url || '—'}
              </span>
            </button>
            {p.id !== LOCAL_PROFILE_ID && (
              <div className={styles.serverActions}>
                <button className={styles.smallBtn} onClick={() => startEdit(p.id)} type="button">
                  Edit
                </button>
                <button
                  className={`${styles.smallBtn} ${styles.dangerBtn}`}
                  onClick={() => remove(p.id)}
                  type="button"
                >
                  Remove
                </button>
              </div>
            )}
          </div>
        ))}
      </div>

      {editing ? (
        <div className={styles.form}>
          <label className={styles.field}>
            <span className={styles.label}>Name</span>
            <input
              className={styles.input}
              value={draft.name}
              onChange={(e) => setDraft({ ...draft, name: e.target.value })}
              placeholder="Production VPS"
            />
          </label>
          <label className={styles.field}>
            <span className={styles.label}>URL</span>
            <input
              className={styles.input}
              value={draft.url}
              onChange={(e) => setDraft({ ...draft, url: e.target.value })}
              placeholder="https://xenoclaw.example.com"
            />
          </label>
          <label className={styles.field}>
            <span className={styles.label}>API key / token (optional)</span>
            <input
              className={styles.input}
              type="password"
              value={draft.token}
              onChange={(e) => setDraft({ ...draft, token: e.target.value })}
              placeholder="xc_…"
              autoComplete="off"
            />
          </label>
          <div className={styles.formActions}>
            <button className={styles.primaryBtn} onClick={submit} type="button">
              {draft.id ? 'Save' : 'Add server'}
            </button>
            <button className={styles.secondaryBtn} onClick={cancel} type="button">
              Cancel
            </button>
          </div>
        </div>
      ) : (
        <button className={styles.primaryBtn} onClick={startAdd} type="button">
          + Add server
        </button>
      )}
      {active?.kind === 'local' && (
        <p className={styles.note}>
          The local profile is active. Start the bundled backend below to connect.
        </p>
      )}
    </section>
  );
}

/** Local backend (sidecar) control — desktop only. */
export function LocalBackendSettings() {
  const { status, logs, busy, error, start, stop } = useLocalBackend();
  const { setActive } = useServers();
  const [port, setPort] = useState('');

  const onStart = useCallback(async () => {
    const p = port.trim() ? Number.parseInt(port.trim(), 10) : undefined;
    try {
      await start(Number.isFinite(p as number) ? (p as number) : undefined);
      setActive(LOCAL_PROFILE_ID);
    } catch {
      /* surfaced via error */
    }
  }, [port, start, setActive]);

  return (
    <section className={styles.section}>
      <h2 className={styles.sectionTitle}>Local backend</h2>
      <p className={styles.hint}>
        Run the bundled XenoClaw backend as a managed sidecar for fully offline
        use. Requires a configured backend (run <code className={styles.code}>xenoclaw setup</code>
        {' '}once). Leave the port blank to auto-pick a free one.
      </p>

      <div className={styles.localRow}>
        <span className={`${styles.statusPill} ${status.running ? styles.pillOn : styles.pillOff}`}>
          {status.running ? `Running · ${status.url ?? ''}` : 'Stopped'}
        </span>
        <input
          className={styles.portInput}
          value={port}
          onChange={(e) => setPort(e.target.value)}
          placeholder="auto"
          inputMode="numeric"
          disabled={status.running}
        />
        {status.running ? (
          <button className={styles.secondaryBtn} onClick={() => void stop()} disabled={busy} type="button">
            Stop
          </button>
        ) : (
          <button className={styles.primaryBtn} onClick={() => void onStart()} disabled={busy} type="button">
            {busy ? 'Starting…' : 'Start'}
          </button>
        )}
      </div>

      {error && <div className={styles.error}>{error}</div>}

      {logs.length > 0 && (
        <pre className={styles.logs}>
          {logs.slice(-40).join('\n')}
        </pre>
      )}
    </section>
  );
}

/** Auto-update status + manual check — desktop only. */
function UpdateSettings() {
  const [version, setVersion] = useState<string>('');
  const [checking, setChecking] = useState(false);
  const [result, setResult] = useState<string | null>(null);

  useEffect(() => {
    void tryInvoke<string>('app_version').then((v) => v && setVersion(v));
  }, []);

  const check = async () => {
    setChecking(true);
    setResult(null);
    try {
      const v = await invoke<string | null>('check_update');
      setResult(v ? `Update available: v${v}` : 'You are on the latest version.');
    } catch (e) {
      setResult(`Update check failed: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setChecking(false);
    }
  };

  return (
    <section className={styles.section}>
      <h2 className={styles.sectionTitle}>About &amp; updates</h2>
      <p className={styles.hint}>XenoClaw Desktop {version && `v${version}`}</p>
      <div className={styles.localRow}>
        <button className={styles.secondaryBtn} onClick={() => void check()} disabled={checking} type="button">
          {checking ? 'Checking…' : 'Check for updates'}
        </button>
        {result && <span className={styles.updateResult}>{result}</span>}
      </div>
    </section>
  );
}

/**
 * Desktop-only settings: appearance is always shown; server profiles, the local
 * backend, and updater controls appear only inside the Tauri shell.
 */
export function DesktopSettings() {
  return (
    <>
      <AppearanceSettings />
      {isTauri() && (
        <>
          <ServerSettings />
          <LocalBackendSettings />
          <UpdateSettings />
        </>
      )}
    </>
  );
}
