import { useState, useCallback, useEffect } from 'react';
import type { FormEvent } from 'react';
import { useAuth } from '../hooks/useAuth';
import styles from './Tasks.module.css';

const API_BASE = '/api/v1';

interface Task {
  id: string;
  name: string;
  trigger_type: string;
  expression: string;
  command: string;
  next_run: string | null;
  status: string;
}

export function Tasks() {
  const { apiFetch } = useAuth();
  const [tasks, setTasks] = useState<Task[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showForm, setShowForm] = useState(false);

  // Form state
  const [formName, setFormName] = useState('');
  const [formTrigger, setFormTrigger] = useState('cron');
  const [formExpression, setFormExpression] = useState('');
  const [formCommand, setFormCommand] = useState('');

  const fetchTasks = useCallback(async () => {
    try {
      const res = await apiFetch(`${API_BASE}/tasks`);
      if (!res.ok) throw new Error(`Failed to fetch tasks (${res.status})`);
      const data = await res.json() as { tasks: Task[] };
      setTasks(data.tasks ?? []);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to fetch tasks');
    } finally {
      setLoading(false);
    }
  }, [apiFetch]);

  useEffect(() => {
    void fetchTasks();
  }, [fetchTasks]);

  const createTask = useCallback(async (e: FormEvent) => {
    e.preventDefault();
    if (!formName.trim() || !formExpression.trim() || !formCommand.trim()) return;

    try {
      const res = await apiFetch(`${API_BASE}/tasks`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          name: formName.trim(),
          trigger_type: formTrigger,
          expression: formExpression.trim(),
          command: formCommand.trim(),
        }),
      });
      if (!res.ok) throw new Error(`Failed to create task (${res.status})`);
      setFormName('');
      setFormExpression('');
      setFormCommand('');
      setShowForm(false);
      await fetchTasks();
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to create task');
    }
  }, [apiFetch, fetchTasks, formName, formTrigger, formExpression, formCommand]);

  const deleteTask = useCallback(async (id: string) => {
    try {
      const res = await apiFetch(`${API_BASE}/tasks/${id}`, { method: 'DELETE' });
      if (!res.ok) throw new Error(`Failed to delete task (${res.status})`);
      setTasks((prev) => prev.filter((t) => t.id !== id));
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to delete task');
    }
  }, [apiFetch]);

  function formatNextRun(iso: string | null): string {
    if (!iso) return 'Not scheduled';
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
        <h1 className={styles.title}>Tasks</h1>
        <p className={styles.loading}>Loading tasks...</p>
      </div>
    );
  }

  return (
    <div className={styles.container}>
      <div className={styles.header}>
        <h1 className={styles.title}>Tasks</h1>
        <button
          className={styles.createButton}
          onClick={() => setShowForm((prev) => !prev)}
        >
          {showForm ? 'Cancel' : '+ New Task'}
        </button>
      </div>

      {error && (
        <div className={styles.errorBanner} role="alert">
          <span className={styles.errorIcon}>⚠</span>
          {error}
        </div>
      )}

      {/* Create Task Form */}
      {showForm && (
        <form className={styles.form} onSubmit={createTask}>
          <div className={styles.formRow}>
            <div className={styles.field}>
              <label className={styles.fieldLabel} htmlFor="task-name">Name</label>
              <input
                id="task-name"
                className={styles.input}
                type="text"
                value={formName}
                onChange={(e) => setFormName(e.target.value)}
                placeholder="My task"
                required
              />
            </div>
            <div className={styles.field}>
              <label className={styles.fieldLabel} htmlFor="task-trigger">Trigger Type</label>
              <select
                id="task-trigger"
                className={styles.select}
                value={formTrigger}
                onChange={(e) => setFormTrigger(e.target.value)}
              >
                <option value="cron">Cron</option>
                <option value="interval">Interval</option>
                <option value="event">Event</option>
              </select>
            </div>
          </div>
          <div className={styles.formRow}>
            <div className={styles.field}>
              <label className={styles.fieldLabel} htmlFor="task-expression">Expression</label>
              <input
                id="task-expression"
                className={styles.input}
                type="text"
                value={formExpression}
                onChange={(e) => setFormExpression(e.target.value)}
                placeholder="*/5 * * * *"
                required
              />
            </div>
            <div className={styles.field}>
              <label className={styles.fieldLabel} htmlFor="task-command">Command</label>
              <input
                id="task-command"
                className={styles.input}
                type="text"
                value={formCommand}
                onChange={(e) => setFormCommand(e.target.value)}
                placeholder="echo hello"
                required
              />
            </div>
          </div>
          <button className={styles.submitButton} type="submit">
            Create Task
          </button>
        </form>
      )}

      {/* Task List */}
      {tasks.length === 0 ? (
        <p className={styles.emptyState}>No scheduled tasks</p>
      ) : (
        <div className={styles.list}>
          {tasks.map((task) => (
            <div key={task.id} className={styles.taskCard}>
              <div className={styles.taskInfo}>
                <span className={styles.taskName}>{task.name}</span>
                <div className={styles.taskMeta}>
                  <span className={styles.badge}>{task.trigger_type}</span>
                  <span className={styles.taskNext}>Next: {formatNextRun(task.next_run)}</span>
                </div>
              </div>
              <div className={styles.taskRight}>
                <span className={`${styles.statusBadge} ${styles[`status_${task.status}`]}`}>
                  {task.status}
                </span>
                <button
                  className={styles.deleteButton}
                  onClick={() => deleteTask(task.id)}
                  title="Delete task"
                >
                  ✕
                </button>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
