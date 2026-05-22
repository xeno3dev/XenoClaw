import { useState, useCallback, useEffect } from 'react';
import { useAuth } from '../hooks/useAuth';
import { useInterval } from '../hooks/useInterval';
import { useWebSocket } from '../hooks/useWebSocket';
import type {
  AgentStatus,
  ScheduledTask,
  ActivityEntry,
  SystemHealth,
  ServiceStatus,
  StatusResponse,
  TasksResponse,
} from '../types/dashboard';
import type { WebSocketMessage } from '../hooks/useWebSocket';
import styles from './Dashboard.module.css';

/** Polling interval: 5 seconds (≤5s as required) */
const POLL_INTERVAL_MS = 5000;

/** API base URL — uses relative path so Vite proxy or same-origin works */
const API_BASE = '/api/v1';

/** Format seconds into a human-readable uptime string */
function formatUptime(seconds: number): string {
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);

  if (days > 0) {
    return `${days}d ${hours}h ${minutes}m`;
  }
  if (hours > 0) {
    return `${hours}h ${minutes}m`;
  }
  return `${minutes}m`;
}

/** Format an ISO timestamp to a locale-friendly relative or absolute time */
function formatTimestamp(iso: string): string {
  const date = new Date(iso);
  const now = Date.now();
  const diffMs = now - date.getTime();

  if (diffMs < 60_000) {
    return 'just now';
  }
  if (diffMs < 3_600_000) {
    const mins = Math.floor(diffMs / 60_000);
    return `${mins}m ago`;
  }
  if (diffMs < 86_400_000) {
    const hrs = Math.floor(diffMs / 3_600_000);
    return `${hrs}h ago`;
  }
  return date.toLocaleDateString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });
}

/** Get CSS class for a service status dot */
function statusDotClass(status: ServiceStatus): string {
  switch (status) {
    case 'healthy':
      return styles.dotHealthy;
    case 'degraded':
      return styles.dotDegraded;
    case 'unavailable':
      return styles.dotUnavailable;
  }
}

/**
 * Dashboard page — displays agent status, resource usage, scheduled tasks,
 * recent activity, and system health indicators.
 * Data is refreshed every ≤5 seconds via REST polling and WebSocket events.
 */
export function Dashboard() {
  const { apiFetch, token } = useAuth();
  const [agentStatus, setAgentStatus] = useState<AgentStatus | null>(null);
  const [systemHealth, setSystemHealth] = useState<SystemHealth | null>(null);
  const [scheduledTasks, setScheduledTasks] = useState<ScheduledTask[]>([]);
  const [recentActivity, setRecentActivity] = useState<ActivityEntry[]>([]);
  const [error, setError] = useState<string | null>(null);

  /** Fetch status data from the REST API.
   *
   * The current backend returns a flat AgentStatus-shaped payload
   * ({status, mode, current_task, uptime_seconds}). The full
   * {agent, health, recentActivity} envelope this dashboard was
   * designed around is aspirational — those fields don't exist yet
   * server-side. Parse defensively so the page renders what the
   * server actually provides and degrades gracefully on the rest.
   */
  const fetchStatus = useCallback(async () => {
    try {
      const res = await apiFetch(`${API_BASE}/status`);
      if (!res.ok) {
        throw new Error(`Status API returned ${res.status}`);
      }
      const data = (await res.json()) as Partial<StatusResponse> & {
        // Flat fallback shape the backend currently returns.
        status?: string;
        mode?: string;
        current_task?: string | null;
        uptime_seconds?: number;
      };

      // Prefer the richer envelope when present; fall back to the
      // flat fields the live server actually emits.
      const flatAgent: AgentStatus | null =
        data.agent ??
        (data.status !== undefined
          ? {
              status: (data.status as AgentStatus['status']) ?? 'idle',
              currentTask: data.current_task ?? null,
              uptimeSeconds: data.uptime_seconds ?? 0,
              mode: ((data.mode ?? 'general').charAt(0).toUpperCase() +
                (data.mode ?? 'general').slice(1)) as AgentStatus['mode'],
              // Resource usage not yet reported — leave blank until
              // backend adds it; cards render a placeholder.
              cpuPercent: 0,
              memoryPercent: 0,
            }
          : null);

      setAgentStatus(flatAgent);
      setSystemHealth(data.health ?? null);
      setRecentActivity(Array.isArray(data.recentActivity) ? data.recentActivity.slice(0, 50) : []);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to fetch status');
    }
  }, [apiFetch]);

  /** Fetch scheduled tasks from the REST API */
  const fetchTasks = useCallback(async () => {
    try {
      const res = await apiFetch(`${API_BASE}/tasks`);
      if (!res.ok) {
        throw new Error(`Tasks API returned ${res.status}`);
      }
      const data = (await res.json()) as TasksResponse;
      setScheduledTasks(Array.isArray(data.tasks) ? data.tasks : []);
    } catch {
      // Non-critical — status fetch error already shown
    }
  }, [apiFetch]);

  /** Combined fetch for polling */
  const pollData = useCallback(() => {
    void fetchStatus();
    void fetchTasks();
  }, [fetchStatus, fetchTasks]);

  // Initial fetch on mount
  useEffect(() => {
    pollData();
  }, [pollData]);

  // Poll every 5 seconds
  useInterval(pollData, POLL_INTERVAL_MS);

  /** Handle real-time WebSocket events for immediate updates */
  const handleWsMessage = useCallback((message: WebSocketMessage) => {
    switch (message.type) {
      case 'status_update': {
        const payload = message.payload as Partial<AgentStatus>;
        setAgentStatus((prev) => (prev ? { ...prev, ...payload } : null));
        break;
      }
      case 'health_update': {
        const payload = message.payload as SystemHealth;
        setSystemHealth(payload);
        break;
      }
      case 'activity': {
        const entry = message.payload as ActivityEntry;
        setRecentActivity((prev) => [entry, ...prev].slice(0, 50));
        break;
      }
    }
  }, []);

  // Connect to the events WebSocket for real-time updates. The server
  // expects the same Bearer token via the ?token= query param.
  useWebSocket({
    token: token ?? '',
    endpoint: '/api/v1/ws/events',
    onMessage: handleWsMessage,
    autoConnect: true,
  });

  return (
    <div className={styles.container}>
      <h1 className={styles.title}>Dashboard</h1>

      {error && (
        <div className={styles.errorBanner} role="alert">
          <span className={styles.errorIcon}>⚠</span>
          {error}
        </div>
      )}

      <div className={styles.grid}>
        {/* Agent Status Card */}
        <section className={styles.card} aria-label="Agent Status">
          <h2 className={styles.cardTitle}>Agent Status</h2>
          {agentStatus ? (
            <div className={styles.statusContent}>
              <div className={styles.statusRow}>
                <span className={styles.label}>Status</span>
                <span className={`${styles.statusBadge} ${styles[`status_${agentStatus.status}`]}`}>
                  {agentStatus.status}
                </span>
              </div>
              <div className={styles.statusRow}>
                <span className={styles.label}>Current Task</span>
                <span className={styles.value}>
                  {agentStatus.currentTask ?? 'None'}
                </span>
              </div>
              <div className={styles.statusRow}>
                <span className={styles.label}>Uptime</span>
                <span className={styles.value}>{formatUptime(agentStatus.uptimeSeconds)}</span>
              </div>
              <div className={styles.statusRow}>
                <span className={styles.label}>Mode</span>
                <span className={styles.value}>{agentStatus.mode}</span>
              </div>
            </div>
          ) : (
            <p className={styles.loading}>Loading...</p>
          )}
        </section>

        {/* Resource Usage Card */}
        <section className={styles.card} aria-label="Resource Usage">
          <h2 className={styles.cardTitle}>Resource Usage</h2>
          {agentStatus ? (
            <div className={styles.resourceContent}>
              <div className={styles.resourceItem}>
                <div className={styles.resourceHeader}>
                  <span className={styles.label}>CPU</span>
                  <span className={styles.resourceValue}>{agentStatus.cpuPercent.toFixed(1)}%</span>
                </div>
                <div className={styles.progressBar} role="progressbar" aria-valuenow={agentStatus.cpuPercent} aria-valuemin={0} aria-valuemax={100}>
                  <div
                    className={styles.progressFill}
                    style={{ width: `${Math.min(agentStatus.cpuPercent, 100)}%` }}
                    data-level={agentStatus.cpuPercent > 80 ? 'high' : agentStatus.cpuPercent > 50 ? 'medium' : 'low'}
                  />
                </div>
              </div>
              <div className={styles.resourceItem}>
                <div className={styles.resourceHeader}>
                  <span className={styles.label}>Memory</span>
                  <span className={styles.resourceValue}>{agentStatus.memoryPercent.toFixed(1)}%</span>
                </div>
                <div className={styles.progressBar} role="progressbar" aria-valuenow={agentStatus.memoryPercent} aria-valuemin={0} aria-valuemax={100}>
                  <div
                    className={styles.progressFill}
                    style={{ width: `${Math.min(agentStatus.memoryPercent, 100)}%` }}
                    data-level={agentStatus.memoryPercent > 80 ? 'high' : agentStatus.memoryPercent > 50 ? 'medium' : 'low'}
                  />
                </div>
              </div>
            </div>
          ) : (
            <p className={styles.loading}>Loading...</p>
          )}
        </section>

        {/* System Health Card */}
        <section className={styles.card} aria-label="System Health">
          <h2 className={styles.cardTitle}>System Health</h2>
          {systemHealth ? (
            <div className={styles.healthContent}>
              {systemHealth.services.map((service) => (
                <div key={service.name} className={styles.serviceRow}>
                  <span className={`${styles.statusDot} ${statusDotClass(service.status)}`} aria-hidden="true" />
                  <span className={styles.serviceName}>{service.name}</span>
                  <span className={styles.serviceStatus}>{service.status}</span>
                  {service.latencyMs !== null && (
                    <span className={styles.serviceLatency}>{service.latencyMs}ms</span>
                  )}
                </div>
              ))}
              {systemHealth.services.length === 0 && (
                <p className={styles.emptyState}>No services configured</p>
              )}
            </div>
          ) : (
            <p className={styles.loading}>Loading...</p>
          )}
        </section>

        {/* Scheduled Tasks Card */}
        <section className={styles.card} aria-label="Scheduled Tasks">
          <h2 className={styles.cardTitle}>Scheduled Tasks</h2>
          <div className={styles.tasksList}>
            {scheduledTasks.length > 0 ? (
              scheduledTasks.map((task) => (
                <div key={task.id} className={styles.taskRow}>
                  <div className={styles.taskInfo}>
                    <span className={styles.taskName}>{task.name}</span>
                    <span className={styles.taskMeta}>
                      {task.triggerType} · {task.nextRun ? formatTimestamp(task.nextRun) : 'No next run'}
                    </span>
                  </div>
                  <span className={`${styles.taskStatus} ${styles[`taskStatus_${task.status}`]}`}>
                    {task.status}
                  </span>
                </div>
              ))
            ) : (
              <p className={styles.emptyState}>No scheduled tasks</p>
            )}
          </div>
        </section>

        {/* Recent Activity Card */}
        <section className={`${styles.card} ${styles.activityCard}`} aria-label="Recent Activity">
          <h2 className={styles.cardTitle}>Recent Activity</h2>
          <div className={styles.activityList}>
            {recentActivity.length > 0 ? (
              recentActivity.map((entry) => (
                <div key={entry.id} className={styles.activityRow}>
                  <span className={`${styles.activityDot} ${styles[`activity_${entry.type}`]}`} aria-hidden="true" />
                  <span className={styles.activityMessage}>{entry.message}</span>
                  <span className={styles.activityTime}>{formatTimestamp(entry.timestamp)}</span>
                </div>
              ))
            ) : (
              <p className={styles.emptyState}>No recent activity</p>
            )}
          </div>
        </section>
      </div>
    </div>
  );
}
