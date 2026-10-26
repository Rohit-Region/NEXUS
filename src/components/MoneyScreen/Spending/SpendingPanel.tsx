import { useCallback, useEffect, useMemo, useState } from 'react';
import { ChevronLeft, ChevronRight, ClipboardPaste, Fingerprint, Lock, Plus, RefreshCw, MessageSquareWarning } from 'lucide-react';
import {
  isSpendLocked,
  spendAccept,
  spendLock,
  spendLockState,
  spendUnlock,
  spendAdd,
  spendConnect,
  spendDelete,
  spendIgnore,
  spendMonth,
  spendPaste,
  spendReview,
  spendSetBalance,
  spendSetBudget,
  spendSetIncome,
  spendStatus,
  spendSync,
  spendUpdate,
  type Expense,
  type ExpenseInput,
  type ReviewItem,
  type SpendMonth,
  type SpendStatus,
} from '../../../lib/nexus-db';
import { SpendCalendar } from './SpendCalendar';
import { SavingsCard } from './SavingsCard';
import { BalanceStrip } from './BalanceStrip';
import { SpendBreakdown } from './SpendBreakdown';
import { SpendTransactions, type TxFilter } from './SpendTransactions';
import { ExpenseForm } from './ExpenseForm';
import { ago, dayLabel, errorText, inr, monthLabel, shiftMonth, toInput } from './format';
import './Spending.css';

/** The tab re-reads while open, so payments synced in the background appear
 *  without a press of Sync. */
const REFRESH_MS = 60_000;
const DEFAULT_ACCOUNTS = ['ICICI XX225', 'HDFC card', 'Cash'];

type Editing =
  | { mode: 'add'; initial: ExpenseInput }
  | { mode: 'edit'; expense: Expense }
  | { mode: 'review'; item: ReviewItem };

/**
 * Spending: every payment from the bank SMS your phone forwards, plus what
 * you add by hand, laid out by day, category, place and account.
 *
 * Only spending counts toward the totals. Income, refunds and money moved
 * (card bills, investments, your own accounts) are listed but kept out, so a
 * card purchase is never counted twice.
 */
export function SpendingPanel() {
  const [status, setStatus] = useState<SpendStatus | null>(null);
  const [month, setMonth] = useState<string | null>(null);
  const [data, setData] = useState<SpendMonth | null>(null);
  const [review, setReview] = useState<ReviewItem[]>([]);
  const [showReview, setShowReview] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [filter, setFilter] = useState<TxFilter>({ text: '', category: '', kind: '' });
  const [editing, setEditing] = useState<Editing | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [syncing, setSyncing] = useState(false);
  const [link, setLink] = useState('');
  const [pasting, setPasting] = useState<string | null>(null);
  // null while checking; the lock is enforced by the backend, this mirrors it.
  const [locked, setLocked] = useState<boolean | null>(null);
  const [unlockMethod, setUnlockMethod] = useState<'touchId' | 'password'>('touchId');
  const [unlocking, setUnlocking] = useState(false);
  const [lockError, setLockError] = useState<string | null>(null);

  const load = useCallback(async (m: string | null) => {
    try {
      const [s, d, r] = await Promise.all([spendStatus(), spendMonth(m ?? undefined), spendReview()]);
      setStatus(s);
      setData(d);
      setReview(r);
      setMonth(d.month);
      setSelected((cur) => (cur && cur.startsWith(d.month) ? cur : d.today.startsWith(d.month) ? d.today : null));
    } catch (err) {
      if (isSpendLocked(err)) {
        setLocked(true);
        setData(null);
      } else {
        setMessage(`Could not load spending: ${errorText(err)}`);
      }
    }
  }, []);

  /** A command refused because the session lapsed shows the lock again. */
  const fail = useCallback((err: unknown) => {
    if (isSpendLocked(err)) {
      setLocked(true);
      setData(null);
      setEditing(null);
    } else {
      setMessage(errorText(err));
    }
  }, []);

  const unlock = useCallback(async () => {
    setUnlocking(true);
    setLockError(null);
    try {
      await spendUnlock();
      setLocked(false);
    } catch (err) {
      setLockError(errorText(err));
    } finally {
      setUnlocking(false);
    }
  }, []);

  // Opening the tab asks straight away; the button is there if it is dismissed.
  useEffect(() => {
    void spendLockState().then((s) => {
      setUnlockMethod(s.method);
      setLocked(!s.open);
      if (!s.open) void unlock();
    });
  }, [unlock]);

  useEffect(() => {
    if (locked !== false) return;
    void load(month);
    const t = window.setInterval(() => void load(month), REFRESH_MS);
    return () => window.clearInterval(t);
  }, [load, month, locked]);

  // First visit after connecting: fetch straight away rather than waiting for
  // the background sync.
  const neverSynced = status?.connected === true && status.lastSync === null && !status.lastError;

  const sync = useCallback(async () => {
    setSyncing(true);
    try {
      const r = await spendSync();
      const parts = [
        r.added > 0 ? `${r.added} new payment${r.added === 1 ? '' : 's'}` : null,
        r.review > 0 ? `${r.review} to check` : null,
      ].filter(Boolean);
      setMessage(parts.length ? `Synced: ${parts.join(', ')}.` : 'Synced. Nothing new.');
      if (r.review > 0) setShowReview(true);
    } catch (err) {
      fail(err);
    } finally {
      setSyncing(false);
      await load(month);
    }
  }, [load, month, fail]);

  useEffect(() => {
    if (neverSynced && !syncing) void sync();
    // Runs once per "never synced" state; sync() updates status afterwards.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [neverSynced]);

  const connect = useCallback(async () => {
    try {
      setStatus(await spendConnect(link));
      setLink('');
      setMessage('Connected. Fetching your bank messages…');
      await sync();
    } catch (err) {
      fail(err);
    }
  }, [link, sync, fail]);

  const accounts = useMemo(() => {
    const seen = new Set(DEFAULT_ACCOUNTS);
    data?.transactions.forEach((t) => seen.add(t.account));
    return [...seen];
  }, [data]);

  const after = async (text: string) => {
    setEditing(null);
    setMessage(text);
    await load(month);
  };

  if (locked) {
    return (
      <div className="spend-lock">
        <Lock size={22} strokeWidth={1.8} aria-hidden="true" />
        <h3 className="spend-lock__title">Spending is locked</h3>
        <p className="spend-lock__text">
          Your payments, balance and savings stay hidden until you confirm it is you. It locks again after 5 minutes
          away.
        </p>
        <button className="spend-lock__btn" type="button" onClick={() => void unlock()} disabled={unlocking}>
          <Fingerprint size={16} strokeWidth={2} aria-hidden="true" />
          {unlocking ? 'Waiting for Touch ID…' : unlockMethod === 'touchId' ? 'Unlock with Touch ID' : 'Unlock with your Mac password'}
        </button>
        {lockError && <p className="spend-lock__error">{lockError}</p>}
      </div>
    );
  }

  if (!data) {
    return <p className="money__note">{message ?? 'Loading spending…'}</p>;
  }

  const isCurrent = data.today.startsWith(data.month);
  const todayPaise = data.days.find((d) => d.date === data.today)?.spendPaise ?? 0;
  const daysSoFar = isCurrent ? Number(data.today.slice(8)) : data.daysInMonth;
  const avg = daysSoFar > 0 ? Math.round(data.spendPaise / daysSoFar) : 0;
  const biggest = data.days.reduce((a, b) => (b.spendPaise > a.spendPaise ? b : a), data.days[0]);
  const change = data.previousPaise > 0 ? Math.round(((data.spendPaise - data.previousPaise) / data.previousPaise) * 100) : null;

  return (
    <div className="spend">
      <div className="spend-bar">
        <div className="spend-bar__month">
          <button className="spend-icon" type="button" onClick={() => setMonth(shiftMonth(data.month, -1))} aria-label="Previous month">
            <ChevronLeft size={15} strokeWidth={2} aria-hidden="true" />
          </button>
          <span className="spend-bar__label">{monthLabel(data.month)}</span>
          <button
            className="spend-icon"
            type="button"
            onClick={() => setMonth(shiftMonth(data.month, 1))}
            disabled={isCurrent}
            aria-label="Next month"
          >
            <ChevronRight size={15} strokeWidth={2} aria-hidden="true" />
          </button>
          {!isCurrent && (
            <button className="spend-link" type="button" onClick={() => setMonth(data.today.slice(0, 7))}>
              This month
            </button>
          )}
        </div>

        <span className="spend-bar__sync">
          {status?.connected
            ? status.lastError
              ? <span className="spend-bar__err" title={status.lastError}>Last sync failed</span>
              : status.lastSync
                ? `Synced ${ago(status.lastSync)}`
                : 'Not synced yet'
            : 'Bank SMS not connected'}
        </span>
        {status?.connected && (
          <button
            className={`spend-icon${syncing ? ' spend-icon--busy' : ''}`}
            type="button"
            onClick={() => void sync()}
            disabled={syncing}
            aria-label="Sync bank messages now"
            title="Sync bank messages now"
          >
            <RefreshCw size={14} strokeWidth={2} aria-hidden="true" />
          </button>
        )}
        <button
          className="spend-icon"
          type="button"
          onClick={() => void spendLock().then(() => { setLocked(true); setData(null); })}
          aria-label="Lock Spending now"
          title="Lock Spending now"
        >
          <Lock size={14} strokeWidth={2} aria-hidden="true" />
        </button>
        <button
          className="spend-icon spend-icon--text"
          type="button"
          onClick={() => setPasting('')}
          title="Paste bank messages from before the phone started forwarding, or any it missed"
        >
          <ClipboardPaste size={14} strokeWidth={2} aria-hidden="true" />
          Paste
        </button>
        <button
          className="spend-add"
          type="button"
          onClick={() =>
            setEditing({
              mode: 'add',
              initial: {
                date: selected ?? data.today,
                amount: 0,
                payee: '',
                category: 'Food',
                account: 'Cash',
                kind: 'spend',
                note: '',
              },
            })
          }
        >
          <Plus size={14} strokeWidth={2.4} aria-hidden="true" />
          Add
        </button>
      </div>

      {message && (
        <p className="money__status" role="status">
          {message}
        </p>
      )}
      {status?.connected && status.lastError && <p className="spend-error">{status.lastError}</p>}

      {status && !status.connected && (
        <div className="spend-connect">
          <p>
            Paste your bank SMS link (the Google script link ending in <code>/exec?k=…</code>) to bring in payments
            automatically. It is kept in the macOS Keychain. You can still add payments by hand without it.
          </p>
          <div className="spend-connect__row">
            <input
              className="money__input"
              type="password"
              placeholder="https://script.google.com/macros/s/…/exec?k=…"
              value={link}
              onChange={(e) => setLink(e.target.value)}
              aria-label="Bank SMS link"
            />
            <button className="spend-form__submit" type="button" onClick={() => void connect()} disabled={!link.trim()}>
              Connect
            </button>
          </div>
        </div>
      )}

      {review.length > 0 && (
        <div className="spend-review">
          <button className="spend-review__toggle" type="button" onClick={() => setShowReview((s) => !s)} aria-expanded={showReview}>
            <MessageSquareWarning size={14} strokeWidth={2} aria-hidden="true" />
            {review.length} bank message{review.length === 1 ? '' : 's'} to check
            <span className="spend-review__why">NEXUS read them but has not seen this format before</span>
          </button>
          {showReview && (
            <ul className="spend-review__list">
              {review.map((r) => (
                <li className="spend-review__item" key={r.smsId}>
                  <p className="spend-review__text">{r.text}</p>
                  <p className="spend-review__reason">
                    {r.sender} · {r.reason}
                  </p>
                  <div className="spend-review__actions">
                    <button className="spend-form__submit" type="button" onClick={() => setEditing({ mode: 'review', item: r })}>
                      Check and add
                    </button>
                    <button
                      className="spend-link"
                      type="button"
                      onClick={() => void spendIgnore(r.smsId).then(() => after('Set aside.')).catch(fail)}
                    >
                      Not a payment
                    </button>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}

      <div className="spend-kpis">
        <div className="spend-kpi spend-kpi--hero">
          <span className="spend-kpi__label">Spent in {monthLabel(data.month).split(' ')[0]}</span>
          <span className="spend-kpi__value">{inr(data.spendPaise, true)}</span>
          {change !== null && (
            <span className={`spend-kpi__delta spend-kpi__delta--${change > 0 ? 'up' : 'down'}`}>
              {change > 0 ? '▲' : '▼'} {Math.abs(change)}% vs the same days last month
            </span>
          )}
        </div>
        {isCurrent && (
          <div className="spend-kpi">
            <span className="spend-kpi__label">Today</span>
            <span className="spend-kpi__value">{inr(todayPaise, true)}</span>
          </div>
        )}
        <div className="spend-kpi">
          <span className="spend-kpi__label">Daily average</span>
          <span className="spend-kpi__value">{inr(avg, true)}</span>
        </div>
        <div className="spend-kpi">
          <span className="spend-kpi__label">Biggest day</span>
          <span className="spend-kpi__value">{biggest && biggest.spendPaise > 0 ? inr(biggest.spendPaise, true) : '–'}</span>
          {biggest && biggest.spendPaise > 0 && (
            <button className="spend-link spend-kpi__sub" type="button" onClick={() => setSelected(biggest.date)}>
              {dayLabel(biggest.date)}
            </button>
          )}
        </div>
      </div>
      <BalanceStrip
        balances={data.balances}
        onSet={(account, amount) =>
          void spendSetBalance(account, amount)
            .then(() => after(`${account} balance set to ${inr(Math.round(amount * 100))}.`))
            .catch(fail)
        }
      />

      <SavingsCard
        data={data}
        onMonth={setMonth}
        onIncome={(amount) =>
          void spendSetIncome(amount)
            .then(() => after(amount ? `Usual income set to ${inr(Math.round(amount * 100), true)}.` : 'Usual income cleared.'))
            .catch(fail)
        }
      />

      <SpendCalendar data={data} selected={selected} onSelect={setSelected} onEdit={(e) => setEditing({ mode: 'edit', expense: e })} />

      <SpendBreakdown
        data={data}
        onBudget={(category, amount) =>
          void spendSetBudget(category, amount)
            .then(() => after(amount ? `Budget set for ${category}.` : `Budget removed for ${category}.`))
            .catch(fail)
        }
        onCategory={(category) => setFilter({ text: '', kind: '', category })}
      />

      <SpendTransactions
        rows={data.transactions}
        categories={data.allCategories}
        filter={filter}
        onFilter={setFilter}
        onEdit={(e) => setEditing({ mode: 'edit', expense: e })}
        onRecategorise={(e, category) =>
          void spendUpdate(e.id, { ...toInput(e), category }, true)
            .then(() => after(`${e.payee} is now ${category}, here and from now on.`))
            .catch(fail)
        }
      />

      {pasting !== null && (
        <div className="spend-modal" role="presentation" onMouseDown={(e) => e.target === e.currentTarget && setPasting(null)}>
          <form
            className="spend-form"
            role="dialog"
            aria-modal="true"
            aria-label="Paste bank messages"
            onSubmit={(e) => {
              e.preventDefault();
              void spendPaste(pasting)
                .then((r) => {
                  setPasting(null);
                  const parts = [
                    r.added > 0 ? `${r.added} added` : null,
                    r.review > 0 ? `${r.review} to check` : null,
                    r.ignored > 0 ? `${r.ignored} not payments` : null,
                  ].filter(Boolean);
                  if (r.review > 0) setShowReview(true);
                  return after(`Pasted: ${parts.join(', ')}.`);
                })
                .catch(fail);
            }}
          >
            <h3 className="spend-form__title">Paste bank messages</h3>
            <p className="spend-form__hint">
              For messages from before your phone started forwarding, or any it missed. Copy them from Messages and
              paste here; put a blank line between messages. One already in NEXUS is skipped.
            </p>
            <textarea
              className="money__input spend-paste"
              autoFocus
              rows={9}
              value={pasting}
              onChange={(e) => setPasting(e.target.value)}
              placeholder={'Txn Rs.925.00\nOn HDFC Bank Card 5131\n…\n\nICICI Bank Acct XX225 debited for Rs 1.00 on 24-Oct-26; …'}
              aria-label="Bank messages"
            />
            <div className="spend-form__actions">
              <span className="spend-form__spacer" />
              <button className="spend-link" type="button" onClick={() => setPasting(null)}>
                Cancel
              </button>
              <button className="spend-form__submit" type="submit" disabled={!pasting.trim()}>
                Read and add
              </button>
            </div>
          </form>
        </div>
      )}

      {editing?.mode === 'add' && (
        <ExpenseForm
          title="Add a payment"
          initial={editing.initial}
          categories={data.allCategories}
          accounts={accounts}
          submitLabel="Add"
          offerRemember={false}
          onSubmit={async (input) => {
            await spendAdd(input);
            if (!input.date.startsWith(data.month)) setMonth(input.date.slice(0, 7));
            await after(`Added ${inr(Math.round(input.amount * 100))} at ${input.payee}.`);
          }}
          onClose={() => setEditing(null)}
        />
      )}

      {editing?.mode === 'edit' && (
        <ExpenseForm
          title="Edit payment"
          initial={toInput(editing.expense)}
          categories={data.allCategories}
          accounts={accounts}
          submitLabel="Save"
          offerRemember
          onSubmit={async (input, remember) => {
            await spendUpdate(editing.expense.id, input, remember);
            await after('Saved.');
          }}
          onDelete={async () => {
            await spendDelete(editing.expense.id);
            await after('Deleted.');
          }}
          onClose={() => setEditing(null)}
        />
      )}

      {editing?.mode === 'review' && (
        <ExpenseForm
          title="Check this payment"
          source={editing.item.text}
          initial={{
            date: editing.item.suggestion?.date ?? data.today,
            amount: (editing.item.suggestion?.amountPaise ?? 0) / 100,
            payee: editing.item.suggestion?.payee ?? '',
            category: editing.item.category,
            account: editing.item.suggestion?.account ?? '',
            kind: editing.item.kind,
            note: '',
          }}
          categories={data.allCategories}
          accounts={accounts}
          submitLabel="Add payment"
          offerRemember
          onSubmit={async (input, remember) => {
            await spendAccept(editing.item.smsId, input, remember);
            await after('Added.');
          }}
          onClose={() => setEditing(null)}
        />
      )}
    </div>
  );
}
