import { AlertTriangle, Check, X } from 'lucide-react';
import type { NeedsApproval } from '../../types/assistant';
import './ApprovalPrompt.css';

interface ApprovalPromptProps {
  /**
   * The pending request, exactly as the backend described it. The wording is
   * rendered in Rust from the real target, so the prompt can say "Delete
   * project Atlas and its 6 tasks" rather than "Delete project 41". The UI
   * never composes this sentence itself, because the sentence the user reads
   * must be the one the approval is bound to.
   */
  request: NeedsApproval;
  busy?: boolean;
  onApprove: () => void;
  onCancel: () => void;
}

const PERMISSION_LABEL: Record<NeedsApproval['permission'], string> = {
  read: 'Read',
  interact: 'Interact',
  write: 'Write',
  execute: 'Execute',
  destructive: 'Destructive',
};

/**
 * NEXUS-012: the approval surface.
 *
 * Presentational, like every other form in NEXUS: it calls no command. The
 * caller holds the token and redeems it, which keeps this component reusable
 * for whatever a connector asks for next.
 */
export function ApprovalPrompt({
  request,
  busy = false,
  onApprove,
  onCancel,
}: ApprovalPromptProps) {
  const severe = !request.reversible;

  return (
    <div
      className={`approval-prompt${severe ? ' approval-prompt--severe' : ''}`}
      role="alertdialog"
      aria-label="Confirm action"
    >
      <div className="approval-prompt__head">
        {severe && <AlertTriangle size={13} strokeWidth={2} aria-hidden="true" />}
        <span className="approval-prompt__summary">{request.summary}</span>
        <span className="nexus-chip">{PERMISSION_LABEL[request.permission]}</span>
      </div>

      <p className="approval-prompt__note">
        {severe
          ? 'This cannot be undone.'
          : 'Nothing has happened yet. NEXUS is waiting for you.'}
      </p>

      <div className="approval-prompt__actions">
        <button
          className="nexus-btn nexus-btn--secondary"
          type="button"
          onClick={onCancel}
          disabled={busy}
        >
          <X size={12} strokeWidth={2} aria-hidden="true" />
          Cancel
        </button>
        <button
          className={`nexus-btn ${
            severe ? 'nexus-btn--danger' : 'nexus-btn--primary'
          }`}
          type="button"
          onClick={onApprove}
          disabled={busy}
          autoFocus
        >
          <Check size={12} strokeWidth={2} aria-hidden="true" />
          {busy ? 'Working...' : 'Approve'}
        </button>
      </div>
    </div>
  );
}
