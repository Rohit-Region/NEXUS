import { useState } from 'react';
import { PiggyBank } from 'lucide-react';
import type { SpendMonth } from '../../../lib/nexus-db';
import { inr, inrShort, monthLabel } from './format';

interface Props {
  data: SpendMonth;
  onIncome: (amount: number | null) => void;
  onMonth: (month: string) => void;
}

/**
 * What you kept this month, and the last six months beside it.
 *
 * Saved = money in (income and refunds) less spending. Investments and card
 * bills are "moved", not spent, so money invested still counts as saved.
 * Until a salary-sized credit arrives, the usual monthly income stands in
 * (plus smaller credits), and the card says so.
 */
export function SavingsCard({ data, onIncome, onMonth }: Props) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState('');
  const s = data.savings;
  const current = data.today.startsWith(data.month);
  const monthName = monthLabel(data.month).split(' ')[0];
  const saved = s.savedPaise;
  const known = data.history.filter((h) => h.savings.savedPaise !== null);
  const scale = Math.max(1, ...known.map((h) => Math.abs(h.savings.savedPaise ?? 0)));

  const save = () => {
    const n = Number(draft);
    onIncome(draft.trim() === '' || !Number.isFinite(n) || n <= 0 ? null : n);
    setEditing(false);
  };

  const incomeEditor = (
    <span className="spend-save__income-edit">
      <input
        className="money__input"
        type="number"
        min="0"
        inputMode="numeric"
        autoFocus
        placeholder="₹ take-home per month"
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') save();
          if (e.key === 'Escape') setEditing(false);
        }}
        aria-label="Usual monthly income"
      />
      <button className="spend-link" type="button" onClick={save}>
        Save
      </button>
    </span>
  );

  return (
    <section className="spend-card spend-save" aria-label="Savings">
      <div className="spend-save__main">
        <h3 className="spend-card__title">
          <PiggyBank size={12} strokeWidth={2} aria-hidden="true" /> Saved in {monthName}
          {current && ' so far'}
        </h3>

        {saved === null ? (
          <div className="spend-save__unknown">
            <p>
              Your salary SMS sets this month's income when it arrives in ICICI; savings show from then. To see an
              estimate before that, set your usual take-home pay:
            </p>
            {incomeEditor}
          </div>
        ) : (
          <>
            <span className={`spend-save__value spend-save__value--${saved >= 0 ? 'up' : 'down'}`}>
              {saved >= 0 ? inr(saved, true) : `${inr(-saved, true)} overspent`}
            </span>
            {s.rate !== null && (
              <span className="spend-save__rate">
                {saved >= 0 ? `${s.rate}% of income kept` : `spent ${Math.abs(s.rate)}% more than came in`}
              </span>
            )}
            <dl className="spend-save__sum">
              <div>
                <dt>Money in{s.estimated && <span className="spend-save__est"> usual income</span>}</dt>
                <dd>{inr(s.incomePaise ?? 0, true)}</dd>
              </div>
              {data.refundPaise > 0 && (
                <div>
                  <dt>+ Refunds</dt>
                  <dd>{inr(data.refundPaise, true)}</dd>
                </div>
              )}
              <div>
                <dt>− Spent</dt>
                <dd>{inr(data.spendPaise, true)}</dd>
              </div>
            </dl>
            {data.transferPaise > 0 && (
              <p className="spend-save__note">
                {inr(data.transferPaise, true)} moved (investments, card bills, your own accounts) is not subtracted.
              </p>
            )}
            <p className="spend-save__note">
              {s.estimated
                ? 'Salary not in yet this month, so your usual income stands in, plus any smaller credits. '
                : 'Using the money that came in this month, salary included. '}
              {editing ? (
                incomeEditor
              ) : (
                <button
                  className="spend-link"
                  type="button"
                  onClick={() => {
                    setDraft(data.usualIncomePaise ? String(data.usualIncomePaise / 100) : '');
                    setEditing(true);
                  }}
                >
                  {data.usualIncomePaise ? `Usual income ${inr(data.usualIncomePaise, true)}, change` : 'Set usual income'}
                </button>
              )}
            </p>
          </>
        )}
      </div>

      <div className="spend-save__trend" aria-label="Saved per month, last six months">
        <span className="spend-save__trend-title">Last 6 months</span>
        <div className="spend-save__bars">
          {data.history.map((h) => {
            const v = h.savings.savedPaise;
            const pct = v === null ? 0 : (Math.abs(v) / scale) * 50;
            const name = monthLabel(h.month).split(' ')[0].slice(0, 3);
            const label =
              v === null
                ? `${monthLabel(h.month)}: income unknown`
                : `${monthLabel(h.month)}: ${v >= 0 ? `saved ${inr(v, true)}` : `overspent ${inr(-v, true)}`}${h.savings.estimated ? ' (usual income)' : ''}`;
            return (
              <button
                key={h.month}
                type="button"
                className={`spend-save__col${h.month === data.month ? ' spend-save__col--on' : ''}`}
                onClick={() => onMonth(h.month)}
                title={label}
                aria-label={label}
              >
                <span className="spend-save__amt">{v === null ? '?' : `${v < 0 ? '−' : ''}${inrShort(Math.abs(v))}`}</span>
                <span className="spend-save__plot">
                  <span className="spend-save__zero" />
                  {v !== null && (
                    <span
                      className={`spend-save__bar spend-save__bar--${v >= 0 ? 'up' : 'down'}${h.savings.estimated ? ' spend-save__bar--est' : ''}`}
                      style={v >= 0 ? { bottom: '50%', height: `${pct}%` } : { top: '50%', height: `${pct}%` }}
                    />
                  )}
                </span>
                <span className="spend-save__month">{name}</span>
              </button>
            );
          })}
        </div>
        <span className="spend-save__legend">Above the line: saved. Below: overspent. Striped: from usual income.</span>
      </div>
    </section>
  );
}
