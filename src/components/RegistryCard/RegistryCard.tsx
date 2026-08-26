import { Pencil, Power, Trash2 } from 'lucide-react';
import type { RegistryEntry } from '../../types/db';
import { formatStamp } from '../ProjectCard/ProjectCard';
import './RegistryCard.css';

interface RegistryCardProps {
  entry: RegistryEntry;
  singular: string;
  /** Projects using this entry as a default; drives the delete warning. */
  projectUsage: number;
  isEditing: boolean;
  isConfirmingDelete: boolean;
  busy: boolean;
  onToggleEnabled: () => void;
  onEditToggle: () => void;
  onDeleteRequest: () => void;
  onDeleteCancel: () => void;
  onDeleteConfirm: () => void;
  children?: React.ReactNode;
}

function usageWarning(singular: string, projectUsage: number): string {
  const lower = singular.toLowerCase();
  const projects =
    projectUsage === 0
      ? 'No project uses it as a default'
      : projectUsage === 1
        ? '1 project uses it as a default and will be cleared'
        : `${projectUsage} projects use it as a default and will be cleared`;
  return `Delete this ${lower}? ${projects}. Any task assigned to it will be unassigned. Nothing else is deleted.`;
}

/**
 * Presentational registry row. Root is a div, not a button: the card contains
 * buttons, and there is no registry detail screen to navigate to.
 */
export function RegistryCard({
  entry,
  singular,
  projectUsage,
  isEditing,
  isConfirmingDelete,
  busy,
  onToggleEnabled,
  onEditToggle,
  onDeleteRequest,
  onDeleteCancel,
  onDeleteConfirm,
  children,
}: RegistryCardProps) {
  return (
    <div
      className={`registry-card${entry.enabled ? '' : ' registry-card--disabled'}`}
    >
      <div className="registry-card__row">
        <div className="registry-card__body">
          <div className="registry-card__headline">
            <span className="registry-card__name">{entry.name}</span>
            <span className="nexus-chip">{entry.entryType}</span>
            {!entry.enabled && (
              <span className="nexus-chip nexus-chip--muted">Disabled</span>
            )}
          </div>
          {entry.executablePath && (
            <span className="registry-card__path" title={entry.executablePath}>
              {entry.executablePath}
            </span>
          )}
        </div>

        <span className="registry-card__date">{formatStamp(entry.createdAt)}</span>

        <div className="registry-card__actions">
          <button
            className="nexus-btn nexus-btn--secondary registry-card__icon-btn"
            type="button"
            onClick={onToggleEnabled}
            disabled={busy}
            aria-pressed={entry.enabled}
            aria-label={
              entry.enabled ? `Disable ${entry.name}` : `Enable ${entry.name}`
            }
            title={entry.enabled ? 'Disable' : 'Enable'}
          >
            <Power size={12} strokeWidth={2} aria-hidden="true" />
          </button>
          <button
            className="nexus-btn nexus-btn--secondary registry-card__icon-btn"
            type="button"
            onClick={onEditToggle}
            disabled={busy}
            aria-pressed={isEditing}
            aria-label={isEditing ? `Cancel editing ${entry.name}` : `Edit ${entry.name}`}
          >
            <Pencil size={12} strokeWidth={2} aria-hidden="true" />
          </button>
          <button
            className="nexus-btn nexus-btn--danger registry-card__icon-btn"
            type="button"
            onClick={onDeleteRequest}
            disabled={busy || isConfirmingDelete}
            aria-label={`Delete ${entry.name}`}
          >
            <Trash2 size={12} strokeWidth={2} aria-hidden="true" />
          </button>
        </div>
      </div>

      {isConfirmingDelete && (
        <div
          className="registry-card__confirm"
          role="alertdialog"
          aria-label={`Confirm deletion of ${entry.name}`}
        >
          <span className="registry-card__confirm-text">
            {usageWarning(singular, projectUsage)}
          </span>
          <div className="registry-card__confirm-actions">
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

      {isEditing && <div className="registry-card__edit">{children}</div>}
    </div>
  );
}
