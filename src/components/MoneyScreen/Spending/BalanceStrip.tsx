import { useState } from 'react';
import { Landmark } from 'lucide-react';
import type { AccountBalance } from '../../../lib/nexus-db';
import { ago, inr } from './format';

interface Props {
  balances: AccountBalance[];
  onSet: (account: string, amount: number) => void;
}

/** The account the balance is asked for when none is known yet. */
const DEFAULT_ACCOUNT = 'ICICI XX225';

/**
 * Bank balance: the last one a bank SMS stated ("Avl Bal ..."), or that you
 * set, carried forward through every payment booked since, so it stays
 * current even though a Google Pay SMS does not state it.
 */
export function BalanceStrip({ balances, onSet }: Props) {
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState('');

  const save = (account: string) => {
    const n = Number(draft.replace(/,/g, ''));
    if (draft.trim() !== '' && Number.isFinite(n)) onSet(account, n);
    setEditing(null);
  };

  const editor = (account: string) => (
    <span className="spend-bal__edit">
      <input
        className="money__input"
        type="number"
        inputMode="decimal"
        autoFocus
        placeholder="₹ balance now"
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') save(account);
          if (e.key === 'Escape') setEditing(null);
        }}
        aria-label={`Balance of ${account} now`}
      />
      <button className="spend-link" type="button" onClick={() => save(account)}>
        Save
      </button>
    </span>
  );

  const rows = balances.filter((b) => !b.account.includes('card'));

  return (
    <section className="spend-bal" aria-label="Bank balance">
      <Landmark size={14} strokeWidth={2} aria-hidden="true" className="spend-bal__icon" />
      {rows.length === 0 && (
        <span className="spend-bal__row">
          <span className="spend-bal__label">Bank balance not known yet.</span>
          {editing === DEFAULT_ACCOUNT ? (
            editor(DEFAULT_ACCOUNT)
          ) : (
            <button className="spend-link" type="button" onClick={() => { setDraft(''); setEditing(DEFAULT_ACCOUNT); }}>
              Set it
            </button>
          )}
        </span>
      )}
      {rows.map((b) => (
        <span className="spend-bal__row" key={b.account}>
          <span className="spend-bal__label">{b.account} balance</span>
          <span className={`spend-bal__value${b.balancePaise < 0 ? ' spend-bal__value--neg' : ''}`}>{inr(b.balancePaise)}</span>
          <span className="spend-bal__meta">
            {b.source === 'sms' ? 'from bank SMS' : 'set by you'} {ago(b.asOf)}
            {b.adjusted > 0 && `, with ${b.adjusted} payment${b.adjusted === 1 ? '' : 's'} since applied`}
          </span>
          {editing === b.account ? (
            editor(b.account)
          ) : (
            <button
              className="spend-link spend-link--quiet"
              type="button"
              onClick={() => { setDraft(String(b.balancePaise / 100)); setEditing(b.account); }}
              title="Correct it to what your bank app shows now"
            >
              Correct
            </button>
          )}
        </span>
      ))}
    </section>
  );
}
