import { useEffect, useRef, useState } from 'react';
import { X } from 'lucide-react';
import type { ExpenseInput, SpendKind } from '../../../lib/nexus-db';
import { KIND_HINT, KIND_LABEL } from './format';

interface Props {
  title: string;
  initial: ExpenseInput;
  categories: string[];
  accounts: string[];
  /** The bank message being confirmed, shown above the fields. */
  source?: string;
  submitLabel: string;
  /** Offer "remember for this payee" (editing and confirming, not adding). */
  offerRemember: boolean;
  onSubmit: (input: ExpenseInput, remember: boolean) => Promise<void>;
  onDelete?: () => Promise<void>;
  onClose: () => void;
}

const KINDS: SpendKind[] = ['spend', 'income', 'refund', 'transfer'];

/** Add, edit, or confirm one payment. */
export function ExpenseForm({
  title,
  initial,
  categories,
  accounts,
  source,
  submitLabel,
  offerRemember,
  onSubmit,
  onDelete,
  onClose,
}: Props) {
  const [v, setV] = useState({ ...initial, amount: initial.amount ? String(initial.amount) : '' });
  const [remember, setRemember] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const first = useRef<HTMLInputElement>(null);

  useEffect(() => {
    first.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  const submit = async () => {
    const amount = Number(v.amount);
    if (!v.amount || !Number.isFinite(amount) || amount <= 0) {
      setError('Enter an amount above zero.');
      return;
    }
    if (!v.payee.trim()) {
      setError('Say where the money went.');
      return;
    }
    setBusy(true);
    try {
      await onSubmit({ ...v, amount }, offerRemember && remember);
    } catch (err) {
      setError(String(err).replace(/^Error:\s*/, ''));
      setBusy(false);
    }
  };

  return (
    <div className="spend-modal" role="presentation" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <form
        className="spend-form"
        role="dialog"
        aria-modal="true"
        aria-label={title}
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <div className="spend-form__head">
          <h3 className="spend-form__title">{title}</h3>
          <button className="spend-form__close" type="button" onClick={onClose} aria-label="Close">
            <X size={14} strokeWidth={2.2} aria-hidden="true" />
          </button>
        </div>

        {source && <p className="spend-form__source">{source}</p>}

        <div className="spend-form__kinds" role="radiogroup" aria-label="Type">
          {KINDS.map((k) => (
            <button
              key={k}
              type="button"
              role="radio"
              aria-checked={v.kind === k}
              className={`spend-form__kind${v.kind === k ? ' spend-form__kind--on' : ''}`}
              onClick={() => setV({ ...v, kind: k })}
            >
              {KIND_LABEL[k]}
            </button>
          ))}
        </div>
        <p className="spend-form__hint">{KIND_HINT[v.kind]}</p>

        <div className="spend-form__grid">
          <label className="spend-form__field">
            <span>Amount (₹)</span>
            <input
              ref={first}
              className="money__input"
              type="number"
              min="0"
              step="0.01"
              inputMode="decimal"
              value={v.amount}
              onChange={(e) => setV({ ...v, amount: e.target.value })}
            />
          </label>
          <label className="spend-form__field">
            <span>Date</span>
            <input className="money__input" type="date" value={v.date} onChange={(e) => setV({ ...v, date: e.target.value })} />
          </label>
          <label className="spend-form__field spend-form__field--wide">
            <span>Where (shop, person or bill)</span>
            <input
              className="money__input"
              type="text"
              placeholder="e.g. Swiggy, Tea stall, Electricity"
              value={v.payee}
              onChange={(e) => setV({ ...v, payee: e.target.value })}
            />
          </label>
          <label className="spend-form__field">
            <span>Category</span>
            <select className="money__input" value={v.category} onChange={(e) => setV({ ...v, category: e.target.value })}>
              {categories.map((c) => (
                <option key={c} value={c}>
                  {c}
                </option>
              ))}
            </select>
          </label>
          <label className="spend-form__field">
            <span>Paid from</span>
            <input
              className="money__input"
              type="text"
              list="spend-accounts"
              value={v.account}
              onChange={(e) => setV({ ...v, account: e.target.value })}
            />
            <datalist id="spend-accounts">
              {accounts.map((a) => (
                <option key={a} value={a} />
              ))}
            </datalist>
          </label>
          <label className="spend-form__field spend-form__field--wide">
            <span>Note (optional)</span>
            <input className="money__input" type="text" value={v.note} onChange={(e) => setV({ ...v, note: e.target.value })} />
          </label>
        </div>

        {offerRemember && (
          <label className="spend-form__remember">
            <input type="checkbox" checked={remember} onChange={(e) => setRemember(e.target.checked)} />
            Use this name, category and type for every payment to this payee, past and future
          </label>
        )}

        {error && (
          <p className="spend-form__error" role="alert">
            {error}
          </p>
        )}

        <div className="spend-form__actions">
          {onDelete &&
            (confirmDelete ? (
              <button className="spend-form__delete" type="button" onClick={() => void onDelete()}>
                Yes, delete it
              </button>
            ) : (
              <button className="spend-link spend-link--danger" type="button" onClick={() => setConfirmDelete(true)}>
                Delete
              </button>
            ))}
          <span className="spend-form__spacer" />
          <button className="spend-link" type="button" onClick={onClose}>
            Cancel
          </button>
          <button className="spend-form__submit" type="submit" disabled={busy}>
            {busy ? 'Saving…' : submitLabel}
          </button>
        </div>
      </form>
    </div>
  );
}
