import { useState, useCallback, useEffect } from 'react';
import { useAuth } from '../hooks/useAuth';
import styles from './Plugins.module.css';

const API_BASE = '/api/v1';

interface Plugin {
  name: string;
  version: string;
  status: 'loaded' | 'failed';
  enabled: boolean;
}

export function Plugins() {
  const { token } = useAuth();
  const [plugins, setPlugins] = useState<Plugin[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [reloading, setReloading] = useState(false);

  const headers = useCallback(() => ({
    'Authorization': `Bearer ${token}`,
    'Content-Type': 'application/json',
  }), [token]);

  const fetchPlugins = useCallback(async () => {
    try {
      const res = await fetch(`${API_BASE}/plugins`, { headers: headers() });
      if (!res.ok) throw new Error(`Failed to fetch plugins (${res.status})`);
      const data = await res.json() as { plugins: Plugin[] };
      setPlugins(data.plugins ?? []);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to fetch plugins');
    } finally {
      setLoading(false);
    }
  }, [headers]);

  useEffect(() => {
    void fetchPlugins();
  }, [fetchPlugins]);

  const reloadPlugins = useCallback(async () => {
    setReloading(true);
    try {
      const res = await fetch(`${API_BASE}/plugins/reload`, {
        method: 'POST',
        headers: headers(),
      });
      if (!res.ok) throw new Error(`Failed to reload plugins (${res.status})`);
      await fetchPlugins();
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to reload plugins');
    } finally {
      setReloading(false);
    }
  }, [headers, fetchPlugins]);

  const togglePlugin = useCallback((_name: string) => {
    // Placeholder — would POST to API to enable/disable
  }, []);

  if (loading) {
    return (
      <div className={styles.container}>
        <h1 className={styles.title}>Plugins</h1>
        <p className={styles.loading}>Loading plugins...</p>
      </div>
    );
  }

  return (
    <div className={styles.container}>
      <div className={styles.header}>
        <h1 className={styles.title}>Plugins</h1>
        <button
          className={styles.reloadButton}
          onClick={reloadPlugins}
          disabled={reloading}
        >
          {reloading ? 'Reloading...' : 'Reload All'}
        </button>
      </div>

      {error && (
        <div className={styles.errorBanner} role="alert">
          <span className={styles.errorIcon}>⚠</span>
          {error}
        </div>
      )}

      {plugins.length === 0 ? (
        <p className={styles.emptyState}>No plugins installed</p>
      ) : (
        <div className={styles.list}>
          {plugins.map((plugin) => (
            <div key={plugin.name} className={styles.pluginCard}>
              <div className={styles.pluginInfo}>
                <div className={styles.pluginHeader}>
                  <span className={styles.pluginName}>{plugin.name}</span>
                  <span className={styles.pluginVersion}>v{plugin.version}</span>
                </div>
                <span
                  className={`${styles.statusBadge} ${
                    plugin.status === 'loaded' ? styles.statusLoaded : styles.statusFailed
                  }`}
                >
                  {plugin.status}
                </span>
              </div>
              <div className={styles.pluginActions}>
                <label className={styles.toggleLabel}>
                  <input
                    type="checkbox"
                    className={styles.toggleInput}
                    checked={plugin.enabled}
                    onChange={() => togglePlugin(plugin.name)}
                  />
                  <span className={styles.toggleSwitch} />
                </label>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
