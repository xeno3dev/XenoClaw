import { useState, useCallback, useEffect } from 'react';
import { useAuth } from '../hooks/useAuth';
import styles from './Settings.module.css';

const API_BASE = '/api/v1';

interface ConfigData {
  [section: string]: Record<string, string | number | boolean>;
}

interface MessagingProviderStatus {
  configured: boolean;
}

interface MessagingStatusResponse {
  telegram: MessagingProviderStatus;
  discord: MessagingProviderStatus;
  whatsapp: MessagingProviderStatus;
}

export function Settings() {
  const { apiFetch } = useAuth();
  const [config, setConfig] = useState<ConfigData | null>(null);
  const [messaging, setMessaging] = useState<MessagingStatusResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [agentMode, setAgentMode] = useState<'general' | 'coding'>('general');

  const fetchConfig = useCallback(async () => {
    try {
      const [cfgRes, msgRes] = await Promise.all([
        apiFetch(`${API_BASE}/config`),
        apiFetch(`${API_BASE}/messaging`),
      ]);
      if (!cfgRes.ok) throw new Error(`Failed to fetch config (${cfgRes.status})`);
      const data = await cfgRes.json() as ConfigData;
      setConfig(data);
      const mode = data?.agent?.mode;
      if (mode === 'coding' || mode === 'general') {
        setAgentMode(mode);
      }
      if (msgRes.ok) {
        setMessaging(await msgRes.json() as MessagingStatusResponse);
      }
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to fetch config');
    } finally {
      setLoading(false);
    }
  }, [apiFetch]);

  useEffect(() => {
    void fetchConfig();
  }, [fetchConfig]);

  const setMode = useCallback(async (newMode: 'general' | 'coding') => {
    if (newMode === agentMode) return;
    setAgentMode(newMode); // optimistic update
    try {
      const res = await apiFetch(`${API_BASE}/config`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ settings: { mode: newMode } }),
      });
      if (!res.ok) setAgentMode(agentMode); // revert on error
    } catch {
      setAgentMode(agentMode); // revert on network failure
    }
  }, [agentMode, apiFetch]);

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
            onClick={() => void setMode('general')}
          >
            General
          </button>
          <button
            className={`${styles.modeButton} ${agentMode === 'coding' ? styles.modeActive : ''}`}
            onClick={() => void setMode('coding')}
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
          Rotate the admin API key from the command line:
          {' '}
          <code className={styles.code}>sudo xenoclaw set-api-key</code>
          {' '}
          — restart the agent after for it to take effect.
        </p>
      </section>

      {/* Messaging Bridges */}
      <section className={styles.section}>
        <h2 className={styles.sectionTitle}>Messaging Bridges</h2>
        <p className={styles.sectionHint}>
          Third-party chat integrations. Configure via the setup wizard
          (<code className={styles.code}>xenoclaw -s</code>) — tokens are never
          exposed through the web UI.
        </p>
        <div className={styles.providerGrid}>
          {(['telegram', 'discord', 'whatsapp'] as const).map((provider) => {
            const configured = messaging?.[provider].configured ?? false;
            return (
              <div key={provider} className={styles.providerCard}>
                <span className={styles.providerName}>
                  {provider.charAt(0).toUpperCase() + provider.slice(1)}
                </span>
                <span
                  className={`${styles.providerBadge} ${
                    configured ? styles.providerBadgeOn : styles.providerBadgeOff
                  }`}
                >
                  {configured ? 'Configured' : 'Not configured'}
                </span>
              </div>
            );
          })}
        </div>
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
