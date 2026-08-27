import { Pencil, Trash2 } from 'lucide-react';
import type { RegistryEntry, Task, TaskStatus } from '../../types/db';
import { formatStamp } from '../ProjectCard/ProjectCard';
import { RegistrySelect } from '../RegistryPanel/RegistrySelect';
import './TaskCard.css';

/** Display order and cycle order. Mirrors TASK_STATUSES in db/tasks.rs. */
export const TASK_STATUS_ORDER: TaskStatus[] = [
  'open',
  'in_progress',
  'blocked',
  'done',
];

/** 'in_progress' reads as 'IN PROGRESS'. Unknown values pass through as-is. */
export function formatStatus(status: string): string {
  return status.replace(/_/g, ' ');
}

/** Advance through the vocabulary, wrapping from done back to open. */
export function nextStatus(status: string): TaskStatus {
  const index = TASK_STATUS_ORDER.indexOf(status as TaskStatus);
  return TASK_STATUS_ORDER[(index + 1) % TASK_STATUS_ORDER.length];
}

interface StatusPillProps {
  status: string;
  as?: 'span' | 'button';
  selected?: boolean;
  disabled?: boolean;
  onClick?: () => void;
  ariaLabel?: string;
  ariaPressed?: boolean;
}

/**
 * Shared status chip. A status written outside the app (via sqlite3) has no
 * modifier class and falls back to the muted base colour rather than crashing.
 */
export function StatusPill({
  status,
  as = 'span',
  selected = false,
  disabled = false,
  onClick,
  ariaLabel,
  ariaPressed,
}: StatusPillProps) {
  const known = (TASK_STATUS_ORDER as string[]).includes(status);
  const className = [
    'nexus-status-pill',
    known ? `nexus-status-pill--${status}` : '',
    selected ? 'nexus-status-pill--selected' : '',
  ]
    .filter(Boolean)
    .join(' ');

  const body = <span className="nexus-status-pill__text">{formatStatus(status)}</span>;

  if (as === 'button') {
    return (
      <button
        className={className}
        type="button"
        onClick={onClick}
        disabled={disabled}
        aria-label={ariaLabel}
        aria-pressed={ariaPressed}
      >
        {body}
      </button>
    );
  }

  return <span className={className}>{body}</span>;
}

interface TaskCardProps {
  task: Task;
  /** Supplied by TaskList; the card itself never calls a command. */
  agents: RegistryEntry[];
  onAgentChange: (agentId: number | null) => void;
  isEditing: boolean;
  isConfirmingDelete: boolean;
  /**
   * NEXUS-012: the sentence the action gate rendered for this deletion, and
   * the one the approval token is bound to. Falls back to the generic wording
   * when a caller confirms without going through the gate.
   */
  confirmSummary?: string;
  busy: boolean;
  onStatusChange: (status: TaskStatus) => void;
  onEditToggle: () => void;
  onDeleteRequest: () => void;
  onDeleteCancel: () => void;
  onDeleteConfirm: () => void;
  children?: React.ReactNode;
}

/**
 * Presentational task row. The root is a div, not a button: the card contains
 * buttons, and there is no task detail screen to navigate to, so it needs no
 * click target of its own.
 */
export function TaskCard({
  task,
  agents,
  onAgentChange,
  isEditing,
  isConfirmingDelete,
  confirmSummary,
  busy,
  onStatusChange,
  onEditToggle,
  onDeleteRequest,
  onDeleteCancel,
  onDeleteConfirm,
  children,
}: TaskCardProps) {
  const upcoming = nextStatus(task.status);
  const assigned = agents.find((a) => a.id === task.assignedAgent);

  return (
    <div className="task-card">
      <div className="task-card__row">
        <StatusPill
          status={task.status}
          as="button"
          disabled={busy}
          onClick={() => onStatusChange(upcoming)}
          ariaLabel={`Status ${formatStatus(task.status)}. Change to ${formatStatus(upcoming)}`}
        />

        <div className="task-card__body">
          <span className="task-card__title">{task.title}</span>
          {task.description && (
            <span className="task-card__description">{task.description}</span>
          )}
          {task.assignedAgent !== null && (
            <span className="task-card__agent">
              Agent: {assigned ? assigned.name : `Unknown (id ${task.assignedAgent})`}
            </span>
          )}
        </div>

        <RegistrySelect
          entries={agents}
          value={task.assignedAgent}
          onChange={onAgentChange}
          emptyLabel="No agent"
          ariaLabel={`Assigned agent for ${task.title}`}
          disabled={busy}
        />

        <span className="task-card__date">{formatStamp(task.createdAt)}</span>

        <div className="task-card__actions">
          <button
            className="nexus-btn nexus-btn--secondary task-card__icon-btn"
            type="button"
            onClick={onEditToggle}
            disabled={busy}
            aria-pressed={isEditing}
            aria-label={isEditing ? `Cancel editing ${task.title}` : `Edit ${task.title}`}
          >
            <Pencil size={12} strokeWidth={2} aria-hidden="true" />
          </button>
          <button
            className="nexus-btn nexus-btn--danger task-card__icon-btn"
            type="button"
            onClick={onDeleteRequest}
            disabled={busy || isConfirmingDelete}
            aria-label={`Delete ${task.title}`}
          >
            <Trash2 size={12} strokeWidth={2} aria-hidden="true" />
          </button>
        </div>
      </div>

      {isConfirmingDelete && (
        <div
          className="task-card__confirm"
          role="alertdialog"
          aria-label={`Confirm deletion of ${task.title}`}
        >
          <span className="task-card__confirm-text">
            {confirmSummary
              ? `${confirmSummary}? This cannot be undone.`
              : 'Delete this task? This cannot be undone.'}
          </span>
          <div className="task-card__confirm-actions">
            <button
              className="nexus-btn nexus-btn--secondary"
              type="button"
              onClick={onDeleteCancel}
              disabled={busy}
            >
              Cancel
            </button>
            <button
              className="nexus-btn nexus-btn--primary"
              type="button"
              onClick={onDeleteConfirm}
              disabled={busy}
            >
              {busy ? 'Deleting...' : 'Confirm Delete'}
            </button>
          </div>
        </div>
      )}

      {isEditing && <div className="task-card__edit">{children}</div>}
    </div>
  );
}
