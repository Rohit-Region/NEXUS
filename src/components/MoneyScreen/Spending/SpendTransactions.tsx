import { useMemo } from 'react';
import { Search } from 'lucide-react';
import type { Expense, SpendKind } from '../../../lib/nexus-db';
import { dayLabel, inr, KIND_LABEL } from './format';

export interface TxFilter {
  text: string;
  category: string;
  kind: SpendKind | '';
}

interface Props {
  rows: Expense[];
  categories: string[];
  filter: TxFilter;
  onFilter: (f: TxFilter) => void;
  onEdit: (e: Expense) => void;
  onRecategorise: (e: Expense, category: string) => void;
}

/** Every payment in the month, newest first, grouped by day. Changing a
 *  category here teaches NEXUS that payee for good. */
export function SpendTransactions({ rows, categories, filter, onFilter, onEdit, onRecategorise }: Props) {
  const shown = useMemo(() => {
    const q = filter.text.trim().toLowerCase();
    return rows.filter(
      (t) =>
        (!filter.category || t.category === filter.category) &&
        (!filter.kind || t.kind === filter.kind) &&
        (!q || t.payee.toLowerCase().includes(q) || t.note.toLowerCase().includes(q) || t.account.toLowerCase().includes(q)),
    );
  }, [rows, filter]);

  const groups = useMemo(() => {
    const out: { date: string; rows: Expense[] }[] = [];
    for (const t of shown) {
      const last = out[out.length - 1];
      if (last && last.date === t.occurredOn) last.rows.push(t);
      else out.push({ date: t.occurredOn, rows: [t] });
    }
    return out;
  }, [shown]);

  const total = shown.filter((t) => t.kind === 'spend').reduce((s, t) => s + t.amountPaise, 0);
  const filtered = filter.text || filter.category || filter.kind;

  return (
    <section className="spend-card spend-card--wide spend-tx" aria-label="All payments">
      <div className="spend-tx__head">
        <h3 className="spend-card__title">All payments</h3>
        <span className="spend-tx__summary">
          {shown.length} {shown.length === 1 ? 'entry' : 'entries'} · {inr(total)} spent
        </span>
      </div>

      <div className="spend-tx__filters">
        <label className="spend-tx__search">
          <Search size={13} strokeWidth={2} aria-hidden="true" />
          <input
            type="search"
            placeholder="Search payee, note or account"
            value={filter.text}
            onChange={(e) => onFilter({ ...filter, text: e.target.value })}
            aria-label="Search payments"
          />
        </label>
        <select
          className="money__input"
          value={filter.category}
          onChange={(e) => onFilter({ ...filter, category: e.target.value })}
          aria-label="Filter by category"
        >
          <option value="">All categories</option>
          {categories.map((c) => (
            <option key={c} value={c}>
              {c}
            </option>
          ))}
        </select>
        <select
          className="money__input"
          value={filter.kind}
          onChange={(e) => onFilter({ ...filter, kind: e.target.value as SpendKind | '' })}
          aria-label="Filter by type"
        >
          <option value="">Everything</option>
          {(Object.keys(KIND_LABEL) as SpendKind[]).map((k) => (
            <option key={k} value={k}>
              {KIND_LABEL[k]}
            </option>
          ))}
        </select>
        {filtered && (
          <button className="spend-link" type="button" onClick={() => onFilter({ text: '', category: '', kind: '' })}>
            Clear
          </button>
        )}
      </div>

      {groups.length === 0 && (
        <p className="spend-card__empty">{filtered ? 'Nothing matches.' : 'No payments this month yet.'}</p>
      )}

      {groups.map((g) => (
        <div className="spend-tx__group" key={g.date}>
          <div className="spend-tx__date">{dayLabel(g.date)}</div>
          <ul className="spend-tx__list">
            {g.rows.map((t) => (
              <li className="spend-tx__row" key={t.id}>
                <button className="spend-tx__main" type="button" onClick={() => onEdit(t)} title="Edit">
                  <span className="spend-tx__payee">{t.payee}</span>
                  <span className="spend-tx__meta">
                    {t.account}
                    {t.channel && ` · ${t.channel}`}
                    {t.source === 'manual' && ' · added by you'}
                    {t.note && ` · ${t.note}`}
                  </span>
                </button>
                <select
                  className={`spend-tx__cat${t.category === 'Uncategorised' ? ' spend-tx__cat--todo' : ''}`}
                  value={t.category}
                  onChange={(e) => onRecategorise(t, e.target.value)}
                  aria-label={`Category for ${t.payee}`}
                  title="Change it once and NEXUS remembers it for this payee"
                >
                  {categories.map((c) => (
                    <option key={c} value={c}>
                      {c}
                    </option>
                  ))}
                </select>
                <span className={`spend-amt spend-amt--${t.kind}`}>
                  {t.kind === 'income' || t.kind === 'refund' ? '+' : ''}
                  {inr(t.amountPaise)}
                  {t.kind !== 'spend' && <span className="spend-amt__kind">{KIND_LABEL[t.kind]}</span>}
                </span>
              </li>
            ))}
          </ul>
        </div>
      ))}
    </section>
  );
}
