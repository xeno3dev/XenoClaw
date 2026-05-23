import { useState, useCallback, useEffect } from 'react';
import { useAuth } from '../hooks/useAuth';
import styles from './Settings.module.css';

const API_BASE = '/api/v1';

interface ConfigData {
  version: string;
  mode: 'general' | 'coding' | string;
  rate_limit_default: number;
  system_prompt: string | null;
  log_level: string;
}

interface MessagingProviderStatus {
  configured: boolean;
}

interface MessagingStatusResponse {
  telegram: MessagingProviderStatus;
  discord: MessagingProviderStatus;
  whatsapp: MessagingProviderStatus;
}

const LOG_LEVELS = ['error', 'warn', 'info', 'debug', 'trace'] as const;

export function Settings() {
  const { apiFetch } = useAuth();
  const [config, setConfig] = useState<ConfigData | null>(null);
  const [messaging, setMessaging] = useState<MessagingStatusResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [agentMode, setAgentMode] = useState<'general' | 'coding'>('general');
  const [systemPrompt, setSystemPrompt] = useState('');
  const [systemPromptDirty, setSystemPromptDirty] = useState(false);
  const [savingPrompt, setSavingPrompt] = useState(false);
  const [logLevel, setLogLevel] = useState('info');
  const [rateLimit, setRateLimit] = useState<string>('');
  const [rateLimitDirty, setRateLimitDirty] = useState(false);
  const [savingRateLimit, setSavingRateLimit] = useState(false);

  const fetchConfig = useCallback(async () => {
    try {
      const [cfgRes, msgRes] = await Promise.all([
        apiFetch(`${API_BASE}/config`),
        apiFetch(`${API_BASE}/messaging`),
      ]);
      if (!cfgRes.ok) throw new Error(`Failed to fetch config (${cfgRes.status})`);
      const data = await cfgRes.json() as ConfigData;
      setConfig(data);
      if (data.mode === 'coding' || data.mode === 'general') setAgentMode(data.mode);
      if (!systemPromptDirty) setSystemPrompt(data.system_prompt ?? '');
      setLogLevel(data.log_level ?? 'info');
      if (!rateLimitDirty) setRateLimit(String(data.rate_limit_default ?? ''));
      if (msgRes.ok) {
        setMessaging(await msgRes.json() as MessagingStatusResponse);
      }
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to fetch config');
    } finally {
      setLoading(false);
    }
  }, [apiFetch, systemPromptDirty, rateLimitDirty]);

  useEffect(() => {
    void fetchConfig();
  }, [fetchConfig]);

  const setMode = useCallback(async (newMode: 'general' | 'coding') => {
    if (newMode === agentMode) return;
    setAgentMode(newMode);
    try {
      const res = await apiFetch(`${API_BASE}/config`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ settings: { mode: newMode } }),
      });
      if (!res.ok) setAgentMode(agentMode);
    } catch {
      setAgentMode(agentMode);
    }
  }, [agentMode, apiFetch]);

  const saveSystemPrompt = useCallback(async () => {
    setSavingPrompt(true);
    try {
      const res = await apiFetch(`${API_BASE}/config`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          settings: { system_prompt: systemPrompt.trim() === '' ? null : systemPrompt },
        }),
      });
      if (!res.ok) throw new Error(`Save failed (${res.status})`);
      setSystemPromptDirty(false);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to save system prompt');
    } finally {
      setSavingPrompt(false);
    }
  }, [apiFetch, systemPrompt]);

  const saveRateLimit = useCallback(async () => {
    const parsed = Number.parseInt(rateLimit, 10);
    if (!Number.isFinite(parsed) || parsed <= 0) {
      setError('Rate limit must be a positive integer');
      return;
    }
    setSavingRateLimit(true);
    try {
      const res = await apiFetch(`${API_BASE}/config`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ settings: { rate_limit_default: parsed } }),
      });
      if (!res.ok) throw new Error(`Save failed (${res.status})`);
      setRateLimitDirty(false);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to save rate limit');
    } finally {
      setSavingRateLimit(false);
    }
  }, [apiFetch, rateLimit]);

  const changeLogLevel = useCallback(async (level: string) => {
    const previous = logLevel;
    setLogLevel(level);
    try {
      const res = await apiFetch(`${API_BASE}/config`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ settings: { log_level: level } }),
      });
      if (!res.ok) {
        setLogLevel(previous);
      } else {
        const body = await res.json() as { warnings?: string[] };
        if (body.warnings?.length) {
          setError(body.warnings.join('; '));
        }
      }
    } catch {
      setLogLevel(previous);
    }
  }, [apiFetch, logLevel]);

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

      {/* System Prompt Editor */}
      <section className={styles.section}>
        <h2 className={styles.sectionTitle}>System Prompt</h2>
        <p className={styles.sectionHint}>
          Prepended to every conversation. Takes effect on the next message —
          in-flight requests keep the previous prompt.
        </p>
        <textarea
          className={styles.promptEditor}
          value={systemPrompt}
          onChange={(e) => {
            setSystemPrompt(e.target.value);
            setSystemPromptDirty(true);
          }}
          rows={6}
          placeholder="(empty — agent uses no system prompt)"
        />
        <div className={styles.promptActions}>
          <button
            className={styles.primaryButton}
            onClick={() => void saveSystemPrompt()}
            disabled={!systemPromptDirty || savingPrompt}
          >
            {savingPrompt ? 'Saving…' : 'Save'}
          </button>
          {systemPromptDirty && (
            <button
              className={styles.secondaryButton}
              onClick={() => {
                setSystemPrompt(config?.system_prompt ?? '');
                setSystemPromptDirty(false);
              }}
              disabled={savingPrompt}
            >
              Revert
            </button>
          )}
        </div>
      </section>

      {/* Log Level */}
      <section className={styles.section}>
        <h2 className={styles.sectionTitle}>Log Level</h2>
        <p className={styles.sectionHint}>
          Adjusts the tracing filter at runtime — no restart needed.
        </p>
        <div className={styles.modeToggle}>
          {LOG_LEVELS.map((level) => (
            <button
              key={level}
              className={`${styles.modeButton} ${logLevel === level ? styles.modeActive : ''}`}
              onClick={() => void changeLogLevel(level)}
            >
              {level}
            </button>
          ))}
        </div>
      </section>

      {/* API Key Management */}
      <section className={styles.section}>
        <h2 className={styles.sectionTitle}>API Key Management</h2>
        <p className={styles.sectionHint}>
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

      {/* Rate Limit */}
      <section className={styles.section}>
        <h2 className={styles.sectionTitle}>Rate Limit</h2>
        <p className={styles.sectionHint}>
          Default requests per minute per API key. Applies immediately — atomic
          swap inside the live rate limiter.
        </p>
        <div className={styles.promptActions}>
          <input
            type="number"
            min={1}
            className={styles.rateInput}
            value={rateLimit}
            onChange={(e) => {
              setRateLimit(e.target.value);
              setRateLimitDirty(true);
            }}
            placeholder="100"
          />
          <button
            className={styles.primaryButton}
            onClick={() => void saveRateLimit()}
            disabled={!rateLimitDirty || savingRateLimit}
          >
            {savingRateLimit ? 'Saving…' : 'Save'}
          </button>
          {rateLimitDirty && (
            <button
              className={styles.secondaryButton}
              onClick={() => {
                setRateLimit(String(config?.rate_limit_default ?? ''));
                setRateLimitDirty(false);
              }}
              disabled={savingRateLimit}
            >
              Revert
            </button>
          )}
        </div>
      </section>

      {/* Server Info */}
      {config && (
        <section className={styles.section}>
          <h2 className={styles.sectionTitle}>Server Info</h2>
          <div className={styles.configEntries}>
            <div className={styles.configRow}>
              <span className={styles.configKey}>version</span>
              <span className={styles.configValue}>{config.version}</span>
            </div>
          </div>
        </section>
      )}
    </div>
  );
}
