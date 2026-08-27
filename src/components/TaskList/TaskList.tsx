import { useCallback, useEffect, useState } from 'react';
import { ListChecks, Plus } from 'lucide-react';
import {
  assignTaskAgent,
  createTask,
  listAgents,
  listTasks,
  updateTask,
  updateTaskStatus,
} from '../../lib/nexus-db';
import {
  matchesQuery,
  normalizeQuery,
  sortWithIdTiebreak,
  taskComparator,
  TASK_SORT_OPTIONS,
} from '../../lib/list-filters';
import type { NexusIntent, TaskFormValues, TaskSortMode } from '../../types';
import type { RegistryEntry, Settings, Task, TaskStatus } from '../../types/db';
import { ListControls } from '../ListControls/ListControls';
import {
  StatusPill,
  TaskCard,
  TASK_STATUS_ORDER,
  formatStatus,
} from '../TaskCard/TaskCard';
import { TaskForm } from '../TaskForm/TaskForm';
import {
  cancelApproval,
  describeActionError,
  isActionError,
  isNeedsApproval,
  runAction,
} from '../../lib/assistant';
import type { NeedsApproval } from '../../types/assistant';
import './TaskList.css';

interface TaskListProps {
  projectId: number;
  /**
   * Optional since NEXUS-012: the deletion prompt is rendered by the action
   * layer, which counts the cascade itself, so ProjectDetail no longer needs
   * the number. Kept for callers that do.
   */
  onCountChange?: (count: number) => void;
  settings: Settings;
  /** NEXUS-009: one-shot, consumed as mount state below. */
  intent?: NexusIntent;
}

/** The single predicate used by both the render and the R-02 hidden check. */
function taskMatches(
  task: Task,
  normalized: string,
  statusFilter: TaskStatus[],
): boolean {
  const statusOk =
    statusFilter.length === 0 || statusFilter.includes(task.status);
  return statusOk && matchesQuery(normalized, [task.title, task.description]);
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
export function TaskList({
  projectId,
  onCountChange,
  settings,
  intent,
}: TaskListProps) {
  const [tasks, setTasks] = useState<Task[]>([]);
  // enabledOnly = false: a disabled agent that is still assigned must resolve
  // to a name on the card and stay selectable (F-15).
  const [agents, setAgents] = useState<RegistryEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  // One-shot by construction, as in ProjectList.
  const [showCreateForm, setShowCreateForm] = useState(intent === 'create-task');
  const [submitting, setSubmitting] = useState(false);
  const [editingId, setEditingId] = useState<number | null>(null);
  const [confirmDeleteId, setConfirmDeleteId] = useState<number | null>(null);
  // NEXUS-012: the live approval from the action gate. Holding the token here
  // rather than a boolean is what stops the confirm button from deleting
  // something other than what the prompt described.
  const [pendingDelete, setPendingDelete] = useState<NeedsApproval | null>(null);
  const [busyId, setBusyId] = useState<number | null>(null);

  // NEXUS-007 controls. Session-local: discarded on unmount by design, which
  // is required here so a filter from project A cannot leak into project B.
  const [query, setQuery] = useState('');
  // Seeded from settings on mount; session changes never write back.
  const [sort, setSort] = useState<TaskSortMode>(settings.taskSort);
  const [statusFilter, setStatusFilter] = useState<TaskStatus[]>(
    settings.taskStatusFilter,
  );
  // The mount-time seed defines "unfiltered" for this session and is what
  // Reset returns to, so a stored default never reads as an active filter.
  const [seededSort] = useState<TaskSortMode>(settings.taskSort);
  const [seededStatusFilter] = useState<TaskStatus[]>(settings.taskStatusFilter);
  const [hiddenNotice, setHiddenNotice] = useState<string | null>(null);

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
      onCountChange?.(rows.length);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, [projectId, onCountChange]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // R-02: a save that lands outside the active controls must never look like
  // it failed. Returns the title to announce, or null when it stays visible.
  function noticeFor(task: Task): string | null {
    return taskMatches(task, normalizeQuery(query), statusFilter)
      ? null
      : task.title;
  }

  // ── Actions ──────────────────────────────────────────────────────────────

  async function handleCreate(values: TaskFormValues) {
    setSubmitting(true);
    setError(null);
    try {
      const created = await createTask({
        projectId,
        title: values.title.trim(),
        description: optional(values.description),
        status: values.status,
      });
      setShowCreateForm(false);
      setHiddenNotice(noticeFor(created));
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
      const saved = await updateTask({
        id,
        title: values.title.trim(),
        description: optional(values.description),
        status: values.status,
      });
      setEditingId(null);
      setHiddenNotice(noticeFor(saved));
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
      const changed = await updateTaskStatus({ id, status });
      setHiddenNotice(noticeFor(changed));
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

  /** Ask the gate. It answers with a prompt, never by deleting. */
  async function requestDelete(id: number) {
    setError(null);
    try {
      await runAction({ actionId: 'nexus.delete_task', input: { id } });
      // Unreachable while delete_task is Destructive: the gate must ask
      // before it acts. Reported rather than ignored, because a silent
      // deletion is exactly what this milestone exists to prevent.
      setError('NEXUS deleted the task without asking. Please report this.');
    } catch (err) {
      if (isNeedsApproval(err)) {
        setPendingDelete(err);
      } else if (isActionError(err)) {
        setError(describeActionError(err));
      } else {
        setError(String(err));
      }
    }
  }

  async function confirmDelete(id: number, approval: number) {
    setBusyId(id);
    setError(null);
    try {
      await runAction({ actionId: 'nexus.delete_task', input: { id }, approval });
      setPendingDelete(null);
      setConfirmDeleteId(null);
      await refresh();
    } catch (err) {
      setError(isActionError(err) ? describeActionError(err) : String(err));
      setPendingDelete(null);
    } finally {
      setBusyId(null);
    }
  }

  // Only one task may be editing or awaiting confirmation at a time.
  function openEdit(id: number) {
    setConfirmDeleteId(null);
    setEditingId((prev) => (prev === id ? null : id));
  }

  function openDeleteConfirm(id: number) {
    // Asking about a second task withdraws the first request rather than
    // leaving its token to expire on its own. Only one deletion is ever in
    // flight, so only one approval should ever be outstanding.
    if (pendingDelete) void cancelApproval(pendingDelete.token);
    setPendingDelete(null);
    setEditingId(null);
    setConfirmDeleteId(id);
    void requestDelete(id);
  }

  // ── Controls ─────────────────────────────────────────────────────────────

  const normalized = normalizeQuery(query);
  const isActive =
    normalized.length > 0 ||
    sort !== seededSort ||
    statusFilter.length !== seededStatusFilter.length ||
    statusFilter.some((s) => !seededStatusFilter.includes(s));

  const visible = sortWithIdTiebreak(
    tasks.filter((t) => taskMatches(t, normalized, statusFilter)),
    taskComparator(sort, TASK_STATUS_ORDER),
  );

  function resetControls() {
    setQuery('');
    setSort(seededSort);
    setStatusFilter(seededStatusFilter);
    setHiddenNotice(null);
  }

  function toggleStatus(status: TaskStatus) {
    setHiddenNotice(null);
    setStatusFilter((prev) =>
      prev.includes(status)
        ? prev.filter((s) => s !== status)
        : [...prev, status],
    );
  }

  // ── Render ───────────────────────────────────────────────────────────────

  // Breakdown reads the UNFILTERED array: it reports what the project holds,
  // not what the filter shows (spec 007 7.4).
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
          {!loading && (
            <span className="task-list__count">
              {isActive ? `${visible.length} of ${tasks.length}` : tasks.length}
            </span>
          )}
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

      {tasks.length > 0 && (
        <ListControls
          searchValue={query}
          onSearchChange={(v) => {
            setQuery(v);
            setHiddenNotice(null);
          }}
          searchPlaceholder="Search title or description"
          sortValue={sort}
          sortOptions={TASK_SORT_OPTIONS}
          onSortChange={(v) => {
            setSort(v);
            setHiddenNotice(null);
          }}
          filterSlot={
            <div className="nexus-filter-bar" role="group" aria-label="Filter by status">
              {TASK_STATUS_ORDER.map((status) => (
                <StatusPill
                  key={status}
                  status={status}
                  as="button"
                  selected={statusFilter.includes(status)}
                  onClick={() => toggleStatus(status)}
                  ariaPressed={statusFilter.includes(status)}
                  ariaLabel={`Filter by ${formatStatus(status)}`}
                />
              ))}
            </div>
          }
          isActive={isActive}
          onReset={resetControls}
          disabled={loading}
        />
      )}

      {hiddenNotice && (
        <div className="nexus-notice" role="status">
          <span>
            Saved &quot;{hiddenNotice}&quot;, but it is hidden by the current
            filters.
          </span>
          <div className="nexus-notice__actions">
            <button
              className="nexus-btn nexus-btn--secondary"
              type="button"
              onClick={resetControls}
            >
              Reset Filters
            </button>
            <button
              className="nexus-btn nexus-btn--secondary"
              type="button"
              onClick={() => setHiddenNotice(null)}
            >
              Dismiss
            </button>
          </div>
        </div>
      )}

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

      {!loading && tasks.length > 0 && visible.length === 0 && (
        <div className="nexus-no-results">
          <span className="nexus-no-results__title">No matching tasks</span>
          <span className="nexus-no-results__text">
            {tasks.length} {tasks.length === 1 ? 'task exists' : 'tasks exist'} in this
            project. Relax the filters to see them.
          </span>
          <button
            className="nexus-btn nexus-btn--secondary"
            type="button"
            onClick={resetControls}
          >
            Reset Filters
          </button>
        </div>
      )}

      {visible.length > 0 && (
        <div className="task-list__items">
          {visible.map((task) => (
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
              confirmSummary={
                confirmDeleteId === task.id ? pendingDelete?.summary : undefined
              }
              onDeleteRequest={() => openDeleteConfirm(task.id)}
              onDeleteCancel={() => {
                if (pendingDelete) void cancelApproval(pendingDelete.token);
                setPendingDelete(null);
                setConfirmDeleteId(null);
              }}
              onDeleteConfirm={() => {
                // No token means the gate never issued one, so there is
                // nothing to redeem and nothing should happen.
                if (pendingDelete) {
                  void confirmDelete(task.id, pendingDelete.token);
                }
              }}
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
