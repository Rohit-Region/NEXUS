import { useState } from 'react';
import type { SpendMonth } from '../../../lib/nexus-db';
import { inr, inrShort } from './format';

const WEEKDAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];

/** How a budget reads: plenty left, close, or over. Shown in words as well as
 *  colour, so the state never depends on telling colours apart. */
function budgetState(spent: number, budget: number): { tone: 'ok' | 'near' | 'over'; text: string } {
  if (spent > budget) return { tone: 'over', text: `${inr(spent - budget, true)} over` };
  const left = budget - spent;
  if (spent >= budget * 0.8) return { tone: 'near', text: `${inr(left, true)} left` };
  return { tone: 'ok', text: `${inr(left, true)} left` };
}

interface Props {
  data: SpendMonth;
  onBudget: (category: string, amount: number | null) => void;
  onCategory: (category: string) => void;
}

/** Where the month's money went: by category (with budgets), by place, by
 *  account, and by day of the week. */
export function SpendBreakdown({ data, onBudget, onCategory }: Props) {
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState('');
  const [adding, setAdding] = useState('');

  const catMax = Math.max(1, ...data.categories.map((c) => c.spendPaise));
  const payMax = Math.max(1, ...data.payees.map((p) => p.spendPaise));
  const wdMax = Math.max(1, ...data.weekdays);
  const unbudgeted = data.allCategories.filter(
    (c) => c !== 'Uncategorised' && !data.categories.some((x) => x.category === c && x.budgetPaise !== null),
  );

  const saveBudget = (category: string) => {
    const n = Number(draft);
    onBudget(category, draft.trim() === '' || !Number.isFinite(n) || n <= 0 ? null : n);
    setEditing(null);
    setDraft('');
  };

  return (
    <div className="spend-break">
      <section className="spend-card spend-card--wide" aria-label="Spending by category">
        <h3 className="spend-card__title">By category</h3>
        {data.categories.length === 0 && <p className="spend-card__empty">No spending this month yet.</p>}
        <ul className="spend-cats">
          {data.categories.map((c) => {
            const b = c.budgetPaise !== null ? budgetState(c.spendPaise, c.budgetPaise) : null;
            const fill =
              c.budgetPaise !== null
                ? Math.min(100, (c.spendPaise / c.budgetPaise) * 100)
                : (c.spendPaise / catMax) * 100;
            return (
              <li className="spend-cats__row" key={c.category}>
                <button className="spend-cats__name" type="button" onClick={() => onCategory(c.category)} title={`Show ${c.category} payments`}>
                  {c.category}
                  <span className="spend-cats__count">{c.count}</span>
                </button>
                <span className="spend-cats__amount">{inr(c.spendPaise, true)}</span>
                <span className="spend-cats__track" aria-hidden="true">
                  <span className={`spend-cats__fill${b ? ` spend-cats__fill--${b.tone}` : ''}`} style={{ width: `${fill}%` }} />
                </span>
                <span className="spend-cats__budget">
                  {editing === c.category ? (
                    <span className="spend-budget-edit">
                      <input
                        className="money__input spend-budget-edit__input"
                        type="number"
                        min="0"
                        inputMode="numeric"
                        autoFocus
                        placeholder="₹ per month"
                        value={draft}
                        onChange={(e) => setDraft(e.target.value)}
                        onKeyDown={(e) => {
                          if (e.key === 'Enter') saveBudget(c.category);
                          if (e.key === 'Escape') setEditing(null);
                        }}
                        aria-label={`Monthly budget for ${c.category}`}
                      />
                      <button className="spend-link" type="button" onClick={() => saveBudget(c.category)}>
                        Save
                      </button>
                    </span>
                  ) : b ? (
                    <button
                      className={`spend-budget spend-budget--${b.tone}`}
                      type="button"
                      onClick={() => {
                        setEditing(c.category);
                        setDraft(String((c.budgetPaise ?? 0) / 100));
                      }}
                      title={`Budget ${inr(c.budgetPaise ?? 0, true)} a month. Click to change; clear it to remove.`}
                    >
                      {b.text} of {inrShort(c.budgetPaise ?? 0)}
                    </button>
                  ) : (
                    <button
                      className="spend-link spend-link--quiet"
                      type="button"
                      onClick={() => {
                        setEditing(c.category);
                        setDraft('');
                      }}
                    >
                      Set budget
                    </button>
                  )}
                </span>
              </li>
            );
          })}
        </ul>
        {unbudgeted.length > 0 && (
          <div className="spend-cats__add">
            <select
              className="money__input"
              value={adding}
              onChange={(e) => {
                setAdding(e.target.value);
                setDraft('');
              }}
              aria-label="Add a budget for a category"
            >
              <option value="">+ Budget for another category…</option>
              {unbudgeted.map((c) => (
                <option key={c} value={c}>
                  {c}
                </option>
              ))}
            </select>
            {adding && (
              <>
                <input
                  className="money__input spend-budget-edit__input"
                  type="number"
                  min="0"
                  inputMode="numeric"
                  autoFocus
                  placeholder="₹ per month"
                  value={draft}
                  onChange={(e) => setDraft(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') {
                      saveBudget(adding);
                      setAdding('');
                    }
                  }}
                  aria-label={`Monthly budget for ${adding}`}
                />
                <button
                  className="spend-link"
                  type="button"
                  onClick={() => {
                    saveBudget(adding);
                    setAdding('');
                  }}
                >
                  Save
                </button>
              </>
            )}
          </div>
        )}
      </section>

      <section className="spend-card" aria-label="Where you spent most">
        <h3 className="spend-card__title">Where you spent most</h3>
        {data.payees.length === 0 && <p className="spend-card__empty">Nothing yet.</p>}
        <ol className="spend-places">
          {data.payees.map((p) => (
            <li className="spend-places__row" key={p.payee}>
              <span className="spend-places__name">
                {p.payee}
                <span className="spend-places__meta">
                  {p.category} · {p.count}×
                </span>
              </span>
              <span className="spend-places__amount">{inr(p.spendPaise, true)}</span>
              <span className="spend-places__track" aria-hidden="true">
                <span className="spend-places__fill" style={{ width: `${(p.spendPaise / payMax) * 100}%` }} />
              </span>
            </li>
          ))}
        </ol>
      </section>

      <section className="spend-card" aria-label="By account and by weekday">
        <h3 className="spend-card__title">By account</h3>
        {data.accounts.length === 0 && <p className="spend-card__empty">Nothing yet.</p>}
        <ul className="spend-accounts">
          {data.accounts.map((a) => (
            <li key={a.account} className="spend-accounts__row">
              <span>{a.account}</span>
              <span className="spend-accounts__amount">
                {inr(a.spendPaise, true)}
                <span className="spend-accounts__share">
                  {data.spendPaise > 0 ? Math.round((a.spendPaise / data.spendPaise) * 100) : 0}%
                </span>
              </span>
            </li>
          ))}
        </ul>

        <h3 className="spend-card__title spend-card__title--gap">By day of the week</h3>
        <div className="spend-week" role="list">
          {data.weekdays.map((w, i) => (
            <div
              className="spend-week__col"
              key={WEEKDAYS[i]}
              role="listitem"
              title={`${WEEKDAYS[i]}: ${inr(w)}`}
              aria-label={`${WEEKDAYS[i]}: ${inr(w)}`}
            >
              <span className="spend-week__bar">
                <span className={`spend-week__fill${w === wdMax && w > 0 ? ' spend-week__fill--top' : ''}`} style={{ height: `${(w / wdMax) * 100}%` }} />
              </span>
              <span className="spend-week__label">{WEEKDAYS[i].slice(0, 1)}</span>
            </div>
          ))}
        </div>
      </section>
    </div>
  );
}
