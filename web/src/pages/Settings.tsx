import { useState, useCallback, useEffect } from 'react';
import { useAuth } from '../hooks/useAuth';
import styles from './Settings.module.css';

const API_BASE = '/api/v1';

interface ConfigData {
  [section: string]: Record<string, string | number | boolean>;
}

export function Settings() {
  const { token } = useAuth();
  const [config, setConfig] = useState<ConfigData | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [agentMode, setAgentMode] = useState<'general' | 'coding'>('general');

  const headers = useCallback(() => ({
    'Authorization': `Bearer ${token}`,
    'Content-Type': 'application/json',
  }), [token]);

  const fetchConfig = useCallback(async () => {
    try {
      const res = await fetch(`${API_BASE}/config`, { headers: headers() });
      if (!res.ok) throw new Error(`Failed to fetch config (${res.status})`);
      const data = await res.json() as ConfigData;
      setConfig(data);
      // Try to extract agent mode from config
      const mode = data?.agent?.mode;
      if (mode === 'coding' || mode === 'general') {
        setAgentMode(mode);
      }
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to fetch config');
    } finally {
      setLoading(false);
    }
  }, [headers]);

  useEffect(() => {
    void fetchConfig();
  }, [fetchConfig]);

  const toggleMode = useCallback(() => {
    setAgentMode((prev) => (prev === 'general' ? 'coding' : 'general'));
    // Placeholder — would POST to API to change mode
  }, []);

  if (loading) {
    return (
      <div className={styles.container}>
        <h1 className={styles.title}>Settings</h1>
        <p className={styles.loading}>Loading configuration...</p>
      </div>
    );
  }

  return (
    <div className={styles.container}>
      <h1 className={styles.title}>Settings</h1>

      {error && (
        <div className={styles.errorBanner} role="alert">
          <span className={styles.errorIcon}>⚠</span>
          {error}
        </div>
      )}

      {/* Agent Mode Toggle */}
      <section className={styles.section}>
        <h2 className={styles.sectionTitle}>Agent Mode</h2>
        <div className={styles.modeToggle}>
          <button
            className={`${styles.modeButton} ${agentMode === 'general' ? styles.modeActive : ''}`}
            onClick={toggleMode}
          >
            General
          </button>
          <button
            className={`${styles.modeButton} ${agentMode === 'coding' ? styles.modeActive : ''}`}
            onClick={toggleMode}
          >
            Coding
          </button>
        </div>
        <p className={styles.modeHint}>
          {agentMode === 'general'
            ? 'General mode: broad assistant capabilities'
            : 'Coding mode: focused on code generation and editing'}
        </p>
      </section>

      {/* API Key Management */}
      <section className={styles.section}>
        <h2 className={styles.sectionTitle}>API Key Management</h2>
        <p className={styles.placeholder}>
          API key rotation and management will be available in a future update.
        </p>
      </section>

      {/* Configuration Display */}
      <section className={styles.section}>
        <h2 className={styles.sectionTitle}>Current Configuration</h2>
        {config ? (
          <div className={styles.configGrid}>
            {Object.entries(config).map(([section, values]) => (
              <div key={section} className={styles.configSection}>
                <h3 className={styles.configSectionName}>{section}</h3>
                <div className={styles.configEntries}>
                  {Object.entries(values).map(([key, value]) => (
                    <div key={key} className={styles.configRow}>
                      <span className={styles.configKey}>{key}</span>
                      <span className={styles.configValue}>{String(value)}</span>
                    </div>
                  ))}
                </div>
              </div>
            ))}
          </div>
        ) : (
          <p className={styles.emptyState}>No configuration data available</p>
        )}
      </section>
    </div>
  );
}
