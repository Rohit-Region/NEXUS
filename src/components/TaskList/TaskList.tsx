import { useCallback, useEffect, useState } from 'react';
import { ListChecks, Plus } from 'lucide-react';
import {
  assignTaskAgent,
  createTask,
  deleteTask,
  listAgents,
  listTasks,
  updateTask,
  updateTaskStatus,
} from '../../lib/nexus-db';
import type { TaskFormValues } from '../../types';
import type { RegistryEntry, Task, TaskStatus } from '../../types/db';
import {
  StatusPill,
  TaskCard,
  TASK_STATUS_ORDER,
  formatStatus,
} from '../TaskCard/TaskCard';
import { TaskForm } from '../TaskForm/TaskForm';
import './TaskList.css';

interface TaskListProps {
  projectId: number;
  onCountChange: (count: number) => void;
}

/** Empty strings become undefined so optional columns stay NULL in SQLite. */
function optional(value: string): string | undefined {
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : undefined;
}

function toFormValues(task: Task): TaskFormValues {
  return {
    title: task.title,
    description: task.description ?? '',
    status: task.status,
  };
}

/**
 * The only task component that talks to the database.
 * TaskCard and TaskForm are presentational and receive callbacks.
 */
export function TaskList({ projectId, onCountChange }: TaskListProps) {
  const [tasks, setTasks] = useState<Task[]>([]);
  // enabledOnly = false: a disabled agent that is still assigned must resolve
  // to a name on the card and stay selectable (F-15).
  const [agents, setAgents] = useState<RegistryEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showCreateForm, setShowCreateForm] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [editingId, setEditingId] = useState<number | null>(null);
  const [confirmDeleteId, setConfirmDeleteId] = useState<number | null>(null);
  const [busyId, setBusyId] = useState<number | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [rows, agentRows] = await Promise.all([
        listTasks(projectId),
        listAgents(false),
      ]);
      setTasks(rows);
      setAgents(agentRows);
      onCountChange(rows.length);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, [projectId, onCountChange]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // ── Actions ──────────────────────────────────────────────────────────────

  async function handleCreate(values: TaskFormValues) {
    setSubmitting(true);
    setError(null);
    try {
      await createTask({
        projectId,
        title: values.title.trim(),
        description: optional(values.description),
        status: values.status,
      });
      setShowCreateForm(false);
      await refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setSubmitting(false);
    }
  }

  async function handleSave(id: number, values: TaskFormValues) {
    setSubmitting(true);
    setError(null);
    try {
      await updateTask({
        id,
        title: values.title.trim(),
        description: optional(values.description),
        status: values.status,
      });
      setEditingId(null);
      await refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setSubmitting(false);
    }
  }

  async function handleStatusChange(id: number, status: TaskStatus) {
    setBusyId(id);
    setError(null);
    try {
      await updateTaskStatus({ id, status });
      await refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusyId(null);
    }
  }

  async function handleAgentChange(id: number, agentId: number | null) {
    setBusyId(id);
    setError(null);
    try {
      await assignTaskAgent({ id, agentId });
      await refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusyId(null);
    }
  }

  async function handleDelete(id: number) {
    setBusyId(id);
    setError(null);
    try {
      await deleteTask(id);
      setConfirmDeleteId(null);
      await refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusyId(null);
    }
  }

  // Only one task may be editing or awaiting confirmation at a time.
  function openEdit(id: number) {
    setConfirmDeleteId(null);
    setEditingId((prev) => (prev === id ? null : id));
  }

  function requestDelete(id: number) {
    setEditingId(null);
    setConfirmDeleteId(id);
  }

  // ── Render ───────────────────────────────────────────────────────────────

  const breakdown = TASK_STATUS_ORDER.map((status) => ({
    status,
    count: tasks.filter((t) => t.status === status).length,
  })).filter((entry) => entry.count > 0);

  const isEmpty = !loading && tasks.length === 0;

  return (
    <section className="task-list" aria-label="Tasks">
      <div className="task-list__header">
        <div className="task-list__title-group">
          <h3 className="task-list__title">Tasks</h3>
          {!loading && <span className="task-list__count">{tasks.length}</span>}
          {breakdown.length > 0 && (
            <div className="task-list__breakdown" aria-label="Status breakdown">
              {breakdown.map(({ status, count }) => (
                <span key={status} className="task-list__breakdown-item">
                  <StatusPill status={status} />
                  <span className="task-list__breakdown-count">
                    {count}
                    <span className="task-list__sr-only">
                      {` ${formatStatus(status)}`}
                    </span>
                  </span>
                </span>
              ))}
            </div>
          )}
        </div>

        <button
          className="nexus-btn nexus-btn--primary"
          type="button"
          onClick={() => {
            setEditingId(null);
            setConfirmDeleteId(null);
            setShowCreateForm((prev) => !prev);
          }}
          aria-expanded={showCreateForm}
        >
          <Plus size={12} strokeWidth={2.5} aria-hidden="true" />
          New Task
        </button>
      </div>

      {showCreateForm && (
        <TaskForm
          mode="create"
          onSubmit={handleCreate}
          onCancel={() => setShowCreateForm(false)}
          submitting={submitting}
        />
      )}

      {error && (
        <p className="task-list__error" role="alert">
          {error}
        </p>
      )}

      {loading && <p className="task-list__loading">Loading tasks...</p>}

      {isEmpty && !showCreateForm && (
        <div className="task-list__empty">
          <ListChecks size={22} strokeWidth={1.5} aria-hidden="true" />
          <span className="task-list__empty-title">No tasks yet</span>
          <span className="task-list__empty-text">
            Add the first task for this project to start tracking work.
          </span>
          <button
            className="nexus-btn nexus-btn--primary"
            type="button"
            onClick={() => setShowCreateForm(true)}
          >
            <Plus size={12} strokeWidth={2.5} aria-hidden="true" />
            Create Task
          </button>
        </div>
      )}

      {tasks.length > 0 && (
        <div className="task-list__items">
          {tasks.map((task) => (
            <TaskCard
              key={task.id}
              task={task}
              agents={agents}
              onAgentChange={(agentId) => void handleAgentChange(task.id, agentId)}
              isEditing={editingId === task.id}
              isConfirmingDelete={confirmDeleteId === task.id}
              busy={busyId === task.id}
              onStatusChange={(status) => void handleStatusChange(task.id, status)}
              onEditToggle={() => openEdit(task.id)}
              onDeleteRequest={() => requestDelete(task.id)}
              onDeleteCancel={() => setConfirmDeleteId(null)}
              onDeleteConfirm={() => void handleDelete(task.id)}
            >
              <TaskForm
                mode="edit"
                initialValues={toFormValues(task)}
                onSubmit={(values) => handleSave(task.id, values)}
                onCancel={() => setEditingId(null)}
                submitting={submitting}
              />
            </TaskCard>
          ))}
        </div>
      )}
    </section>
  );
}
