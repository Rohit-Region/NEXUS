import { useCallback, useEffect, useState } from 'react';
import { Plus, X, Landmark } from 'lucide-react';
import {
  addBill,
  listBills,
  removeBill,
  taxDates,
  type BillView,
  type TaxDate,
} from '../../lib/nexus-db';

function inr(n: number): string {
  return new Intl.NumberFormat('en-IN', {
    style: 'currency',
    currency: 'INR',
    maximumFractionDigits: 0,
  }).format(n);
}

/** How a due date reads and colours: overdue, soon (within a week), or later. */
function urgency(days: number): 'over' | 'soon' | 'later' {
  if (days < 0) return 'over';
  if (days <= 7) return 'soon';
  return 'later';
}

function dueLabel(days: number): string {
  if (days < 0) return `${-days}d overdue`;
  if (days === 0) return 'due today';
  if (days === 1) return 'due tomorrow';
  return `in ${days}d`;
}

const KINDS = ['bill', 'emi', 'insurance', 'subscription'];
const RECURRENCES = ['monthly', 'quarterly', 'yearly', 'once'];

/**
 * Bills & tax: the recurring money you must not miss.
 *
 * User-entered bills, EMIs, insurance premiums and subscriptions, each showing
 * its next due date worked out from today, plus India's fixed statutory tax
 * dates, which are built in and never need entering. A reminder board, not a
 * payment system: it tells you what is coming, it does not pay anything.
 */
export function BillsPanel() {
  const [bills, setBills] = useState<BillView[]>([]);
  const [tax, setTax] = useState<TaxDate[]>([]);
  const [status, setStatus] = useState<string | null>(null);

  const [name, setName] = useState('');
  const [kind, setKind] = useState('bill');
  const [amount, setAmount] = useState('');
  const [recurrence, setRecurrence] = useState('monthly');
  const [date, setDate] = useState('');

  const load = useCallback(async () => {
    try {
      const [b, t] = await Promise.all([listBills(), taxDates()]);
      setBills(b);
      setTax(t);
    } catch (err) {
      setStatus(`Could not load: ${String(err).replace(/^Error:\s*/, '')}`);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const add = useCallback(async () => {
    if (!name.trim() || !date) {
      setStatus('Enter a name and a due date.');
      return;
    }
    const amt = amount.trim() ? Number(amount) : null;
    try {
      await addBill(name.trim(), kind, amt, recurrence, date);
      setName('');
      setAmount('');
      setDate('');
      setStatus(null);
      await load();
    } catch (err) {
      setStatus(`Could not add: ${String(err).replace(/^Error:\s*/, '')}`);
    }
  }, [name, kind, amount, recurrence, date, load]);

  return (
    <div className="bills">
      <p className="money__note">
        Next due dates are worked out from today. India tax dates are built in. A
        reminder board, not a payment system.
      </p>

      {status && <p className="money__status">{status}</p>}

      <div className="bills__add">
        <input
          className="money__input"
          type="text"
          placeholder="Name (e.g. Home loan EMI)"
          value={name}
          onChange={(e) => setName(e.target.value)}
          aria-label="Reminder name"
        />
        <select className="money__input bills__select" value={kind} onChange={(e) => setKind(e.target.value)} aria-label="Kind">
          {KINDS.map((k) => (
            <option key={k} value={k}>
              {k}
            </option>
          ))}
        </select>
        <input
          className="money__input money__input--qty"
          type="number"
          min="0"
          step="any"
          placeholder="Amount ₹"
          value={amount}
          onChange={(e) => setAmount(e.target.value)}
          aria-label="Amount"
        />
        <select
          className="money__input bills__select"
          value={recurrence}
          onChange={(e) => setRecurrence(e.target.value)}
          aria-label="Recurrence"
        >
          {RECURRENCES.map((r) => (
            <option key={r} value={r}>
              {r}
            </option>
          ))}
        </select>
        <input
          className="money__input money__input--date"
          type="date"
          value={date}
          onChange={(e) => setDate(e.target.value)}
          aria-label="Due date"
        />
        <button className="money__add-btn" type="button" onClick={() => void add()} aria-label="Add reminder">
          <Plus size={15} strokeWidth={2.5} aria-hidden="true" />
        </button>
      </div>

      {bills.length === 0 && <p className="money__empty">No bills or EMIs yet. Add one above.</p>}

      {bills.length > 0 && (
        <ul className="bills__list">
          {bills.map((b) => {
            const u = urgency(b.daysUntil);
            return (
              <li className={`bills__row bills__row--${u}`} key={b.id}>
                <span className={`bills__dot bills__dot--${u}`} aria-hidden="true" />
                <span className="bills__name">
                  {b.name}
                  <span className="bills__meta">
                    {b.kind} · {b.recurrence}
                  </span>
                </span>
                <span className="bills__amount">{b.amount !== null ? inr(b.amount) : ''}</span>
                <span className="bills__when">
                  <span className={`bills__due bills__due--${u}`}>{dueLabel(b.daysUntil)}</span>
                  <span className="bills__date">{b.nextDue}</span>
                </span>
                <button
                  className="money__remove"
                  type="button"
                  onClick={() => void removeBill(b.id).then(load)}
                  aria-label={`Remove ${b.name}`}
                >
                  <X size={12} strokeWidth={2.5} aria-hidden="true" />
                </button>
              </li>
            );
          })}
        </ul>
      )}

      {/* India statutory tax dates, built in. */}
      <div className="bills__tax">
        <div className="bills__tax-head">
          <Landmark size={13} strokeWidth={2} aria-hidden="true" />
          <span>India tax dates</span>
        </div>
        <ul className="bills__taxlist">
          {tax.map((t) => {
            const u = urgency(t.daysUntil);
            return (
              <li className="bills__taxrow" key={`${t.name}-${t.date}`}>
                <span className="bills__name">{t.name}</span>
                <span className="bills__date">{t.date}</span>
                <span className={`bills__due bills__due--${u}`}>{dueLabel(t.daysUntil)}</span>
              </li>
            );
          })}
        </ul>
        <p className="bills__disclaimer">
          The common fixed dates, not tax advice. A given year's extended deadline or
          your own situation can differ.
        </p>
      </div>
    </div>
  );
}
