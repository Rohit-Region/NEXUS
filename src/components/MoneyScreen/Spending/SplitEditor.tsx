import { useState } from 'react';
import { Plus, X } from 'lucide-react';
import type { Expense } from '../../../lib/nexus-db';
import { inr } from './format';

interface Props {
  expense: Expense;
  onSave: (shares: { person: string; amount: number }[]) => Promise<void>;
}

interface Row {
  person: string;
  amount: string;
}

/** Split a payment with other people. Only your share counts as your spending. */
export function SplitEditor({ expense, onSave }: Props) {
  const [rows, setRows] = useState<Row[]>(
    expense.shares.map((s) => ({ person: s.person, amount: String(s.amountPaise / 100) })),
  );
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [people, setPeople] = useState(String(Math.max(rows.length + 1, 2)));

  const theirs = rows.reduce((sum, r) => sum + Math.round((Number(r.amount) || 0) * 100), 0);
  const mine = expense.amountPaise - theirs;

  // Each other person gets the floor; the odd paisa stays with you, as the
  // backend computes it.
  const equalRows = (next: Row[]) => {
    const each = Math.floor(expense.amountPaise / (next.length + 1)) / 100;
    return next.map((r) => ({ ...r, amount: String(each) }));
  };

  // Split between N people counting you: keeps names already typed and
  // fills the rest with placeholders, and saves, so only your 1/N counts in
  // one click. Names can be corrected afterwards.
  const peopleCount = Math.floor(Number(people));
  const validCount = peopleCount >= 2 && peopleCount <= 50;
  const splitBetween = () => {
    if (!validCount) return;
    const next = Array.from({ length: peopleCount - 1 }, (_, i) => ({
      person: rows[i]?.person.trim() || `Person ${i + 2}`,
      amount: '',
    }));
    void save(equalRows(next));
  };

  const save = async (next: Row[]) => {
    setBusy(true);
    setError(null);
    try {
      await onSave(next.map((r) => ({ person: r.person, amount: Number(r.amount) })));
      setRows(next);
    } catch (err) {
      setError(String(err).replace(/^Error:\s*/, ''));
    } finally {
      setBusy(false);
    }
  };

  const set = (i: number, patch: Partial<Row>) => setRows(rows.map((r, j) => (j === i ? { ...r, ...patch } : r)));

  return (
    <fieldset className="spend-split" disabled={busy}>
      <legend className="spend-split__title">Split with others</legend>
      <div className="spend-split__between">
        <label htmlFor={`split-people-${expense.id}`}>Split between</label>
        <input
          id={`split-people-${expense.id}`}
          className="money__input spend-split__count"
          type="number"
          min="2"
          max="50"
          step="1"
          inputMode="numeric"
          value={people}
          onChange={(e) => setPeople(e.target.value)}
        />
        <span>people, including you</span>
        <button className="spend-link" type="button" onClick={splitBetween} disabled={!validCount}>
          Split
        </button>
        {validCount && (
          <span className="spend-form__hint">
            You pay {inr(expense.amountPaise - (peopleCount - 1) * Math.floor(expense.amountPaise / peopleCount))}
          </span>
        )}
      </div>
      {rows.length === 0 && <p className="spend-form__hint">Not split. All {inr(expense.amountPaise)} counts as yours.</p>}
      {rows.map((r, i) => (
        <div className="spend-split__row" key={i}>
          <input
            className="money__input"
            type="text"
            placeholder="Name"
            value={r.person}
            onChange={(e) => set(i, { person: e.target.value })}
            aria-label={`Person ${i + 1}`}
          />
          <input
            className="money__input spend-split__amt"
            type="number"
            min="0"
            step="0.01"
            inputMode="decimal"
            placeholder="₹ owes"
            value={r.amount}
            onChange={(e) => set(i, { amount: e.target.value })}
            aria-label={`What ${r.person || `person ${i + 1}`} owes`}
          />
          <button
            className="spend-link spend-link--quiet"
            type="button"
            onClick={() => setRows(rows.filter((_, j) => j !== i))}
            aria-label={`Remove ${r.person || `person ${i + 1}`}`}
          >
            <X size={12} strokeWidth={2} aria-hidden="true" />
          </button>
        </div>
      ))}
      <div className="spend-split__actions">
        <button className="spend-link" type="button" onClick={() => setRows([...rows, { person: '', amount: '' }])}>
          <Plus size={12} strokeWidth={2.4} aria-hidden="true" /> Add person
        </button>
        {rows.length > 0 && (
          <button className="spend-link" type="button" onClick={() => setRows(equalRows(rows))}>
            Split equally
          </button>
        )}
        <span className="spend-form__spacer" />
        {rows.length > 0 && (
          <span className={`spend-split__mine${mine < 0 ? ' spend-split__mine--bad' : ''}`}>Your share {inr(mine)}</span>
        )}
        {expense.shares.length > 0 && (
          <button className="spend-link spend-link--danger" type="button" onClick={() => void save([])}>
            Remove split
          </button>
        )}
        {rows.length > 0 && (
          <button className="spend-form__submit" type="button" onClick={() => void save(rows)}>
            Save split
          </button>
        )}
      </div>
      {error && (
        <p className="spend-form__error" role="alert">
          {error}
        </p>
      )}
    </fieldset>
  );
}
