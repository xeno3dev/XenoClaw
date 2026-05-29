/** TypeScript interfaces for Dashboard API response shapes */

export type AgentStatusState = 'idle' | 'working' | 'error';
export type AgentMode = 'General' | 'Plan' | 'Code' | 'Coding';

export interface AgentStatus {
  /** Current agent state */
  status: AgentStatusState;
  /** Name of the current task (if working) */
  currentTask: string | null;
  /** Agent uptime in seconds */
  uptimeSeconds: number;
  /** Current operating mode */
  mode: AgentMode;
  /** CPU usage percentage (0-100) */
  cpuPercent: number;
  /** Memory usage percentage (0-100) */
  memoryPercent: number;
}

export type TaskTriggerType = 'cron' | 'file_change' | 'webhook' | 'time_condition' | 'task_completion';
export type ScheduledTaskStatus = 'active' | 'paused' | 'failed';

export interface ScheduledTask {
  /** Unique task identifier */
  id: string;
  /** Human-readable task name */
  name: string;
  /** Next scheduled run time (ISO 8601) */
  nextRun: string | null;
  /** Type of trigger */
  triggerType: TaskTriggerType;
  /** Current task status */
  status: ScheduledTaskStatus;
}

export type ActivityType = 'task_completed' | 'task_failed' | 'status_change' | 'error' | 'info';

export interface ActivityEntry {
  /** Unique entry identifier */
  id: string;
  /** Type of activity */
  type: ActivityType;
  /** Human-readable description */
  message: string;
  /** When the activity occurred (ISO 8601) */
  timestamp: string;
}

export type ServiceStatus = 'healthy' | 'degraded' | 'unavailable';

export interface ServiceHealth {
  /** Service name */
  name: string;
  /** Current status */
  status: ServiceStatus;
  /** Optional latency in milliseconds */
  latencyMs: number | null;
}

export interface SystemHealth {
  /** CPU usage percentage (0-100) */
  cpuPercent: number;
  /** Memory usage percentage (0-100) */
  memoryPercent: number;
  /** Individual service health indicators */
  services: ServiceHealth[];
}

/** Combined status response from GET /api/v1/status */
export interface StatusResponse {
  agent: AgentStatus;
  health: SystemHealth;
  recentActivity: ActivityEntry[];
}

/** Tasks list response from GET /api/v1/tasks */
export interface TasksResponse {
  tasks: ScheduledTask[];
}
