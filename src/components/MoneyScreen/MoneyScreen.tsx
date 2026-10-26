import { Fragment, useCallback, useEffect, useRef, useState } from 'react';
import { Wallet, RefreshCw, Plus, X, Search } from 'lucide-react';
import {
  addHolding,
  portfolio,
  removeHolding,
  searchFunds,
  type FundMatch,
  type HoldingKind,
  type Portfolio,
} from '../../lib/nexus-db';
import { GoalsPanel } from './GoalsPanel';
import { RetirementPanel } from './RetirementPanel';
import { BillsPanel } from './BillsPanel';
import { SpendingPanel } from './Spending/SpendingPanel';
import { PortfolioChat } from './PortfolioChat';
import './MoneyScreen.css';

/** The three ways to add to the portfolio, each priced differently. */
const MODES: { kind: HoldingKind; label: string }[] = [
  { kind: 'stock', label: 'Stock / ETF' },
  { kind: 'mf', label: 'Mutual fund' },
  { kind: 'manual', label: 'Manual (ULIP, cash)' },
];

type MoneyTab = 'spending' | 'portfolio' | 'goals' | 'retirement' | 'bills';

const TABS: { id: MoneyTab; label: string }[] = [
  { id: 'portfolio', label: 'Portfolio' },
  { id: 'spending', label: 'Spending' },
  { id: 'goals', label: 'Goals' },
  { id: 'retirement', label: 'Retirement' },
  { id: 'bills', label: 'Bills & Tax' },
];

/** Prices move through the day but not by the second, and each refresh is a
 *  handful of network calls, so this re-prices every few minutes. */
const REFRESH_MS = 300_000;

/** INR, grouped the Indian way (lakh/crore), no paise on large sums. */
function inr(n: number): string {
  return new Intl.NumberFormat('en-IN', {
    style: 'currency',
    currency: 'INR',
    maximumFractionDigits: 2,
  }).format(n);
}

function signed(n: number): string {
  return `${n >= 0 ? '+' : ''}${inr(n)}`;
}

/**
 * Money: your portfolio, priced live.
 *
 * Stocks, shares and gold all live here as holdings with a Yahoo symbol: an
 * NSE share is `RELIANCE.NS`, gold is a gold ETF like `GOLDBEES.NS`, both
 * priced in INR through the same path. You enter what you hold; NEXUS fetches
 * the price and totals it.
 *
 * It informs, it never advises. There is no rating, no target, no "buy" here,
 * by design: a tool that pretends to pick winners is how people lose money.
 * This is the first slice of the Money section; savings, goals, retirement and
 * bill reminders follow.
 */
export function MoneyScreen() {
  const [tab, setTab] = useState<MoneyTab>('portfolio');
  const [data, setData] = useState<Portfolio | null>(null);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [mode, setMode] = useState<HoldingKind>('stock');
  const [symbol, setSymbol] = useState('');
  const [qty, setQty] = useState('');
  const [label, setLabel] = useState('');
  // Fund search (mode 'mf'): the query, the matches, and the picked scheme.
  const [fundQuery, setFundQuery] = useState('');
  const [fundResults, setFundResults] = useState<FundMatch[]>([]);
  const [picked, setPicked] = useState<FundMatch | null>(null);
  const [searching, setSearching] = useState(false);

  const load = useCallback(async () => {
    setBusy(true);
    try {
      setData(await portfolio());
    } catch (err) {
      setStatus(`Could not price the portfolio: ${String(err).replace(/^Error:\s*/, '')}`);
    } finally {
      setBusy(false);
    }
  }, []);

  // A ref so the interval reads the latest loader without restarting.
  const loadRef = useRef(load);
  loadRef.current = load;
  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void loadRef.current(), REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [load]);

  const runFundSearch = useCallback(async () => {
    const q = fundQuery.trim();
    if (q.length < 3) {
      setStatus('Type at least three letters to search funds.');
      return;
    }
    setSearching(true);
    try {
      setFundResults(await searchFunds(q));
      setStatus(null);
    } catch (err) {
      setStatus(`Fund search failed: ${String(err).replace(/^Error:\s*/, '')}`);
    } finally {
      setSearching(false);
    }
  }, [fundQuery]);

  const resetAddForm = useCallback(() => {
    setSymbol('');
    setQty('');
    setLabel('');
    setFundQuery('');
    setFundResults([]);
    setPicked(null);
  }, []);

  const add = useCallback(async () => {
    const quantity = Number(qty);
    try {
      if (mode === 'stock') {
        if (!symbol.trim() || !(quantity > 0)) {
          setStatus('Enter a symbol and a positive quantity.');
          return;
        }
        await addHolding(symbol.trim(), label.trim(), quantity, 'stock');
      } else if (mode === 'mf') {
        if (!picked || !(quantity > 0)) {
          setStatus('Search and pick a fund, then enter units.');
          return;
        }
        await addHolding(picked.code, picked.name, quantity, 'mf');
      } else {
        // manual: quantity field holds the rupee value; label names it.
        if (!label.trim() || !(quantity > 0)) {
          setStatus('Enter a label and its value in rupees.');
          return;
        }
        await addHolding('', label.trim(), quantity, 'manual');
      }
      resetAddForm();
      setStatus(null);
      await load();
    } catch (err) {
      setStatus(`Could not add: ${String(err).replace(/^Error:\s*/, '')}`);
    }
  }, [mode, symbol, qty, label, picked, resetAddForm, load]);

  const remove = useCallback(
    async (id: number) => {
      try {
        await removeHolding(id);
        await load();
      } catch {
        // A failed delete leaves the row; the next refresh still shows it.
      }
    },
    [load],
  );

  const holdings = data?.holdings ?? [];
  const dayUp = (data?.totalDayChange ?? 0) >= 0;

  return (
    <div className="money">
      <header className="money__head">
        <Wallet size={18} strokeWidth={1.8} aria-hidden="true" />
        <div className="money__headings">
          <h1 className="money__title">Money</h1>
          <p className="money__subtitle">
            Track, plan, and project. Information, not advice.
          </p>
        </div>
        {tab === 'portfolio' && (
          <button
            className={`money__refresh${busy ? ' money__refresh--busy' : ''}`}
            type="button"
            onClick={() => void load()}
            disabled={busy}
            aria-label="Refresh prices"
          >
            <RefreshCw size={14} strokeWidth={2} aria-hidden="true" />
          </button>
        )}
      </header>

      {/* Sub-sections of Money: what you hold, what you are saving toward, and
          where you land in retirement. */}
      <div className="money__tabs" role="group" aria-label="Money section">
        {TABS.map((t) => (
          <button
            key={t.id}
            type="button"
            className={`money__tab${tab === t.id ? ' money__tab--on' : ''}`}
            aria-pressed={tab === t.id}
            onClick={() => setTab(t.id)}
          >
            {t.label}
          </button>
        ))}
      </div>

      {tab === 'spending' && <SpendingPanel />}
      {tab === 'goals' && <GoalsPanel />}
      {tab === 'retirement' && <RetirementPanel />}
      {tab === 'bills' && <BillsPanel />}

      {tab === 'portfolio' && (
      <>
      <PortfolioChat onApplied={() => void load()} />

      {/* Totals. The one number you glance at, plus the day's move. */}
      <div className="money__totals">
        <div className="money__total">
          <span className="money__total-label">Total value</span>
          <span className="money__total-value">
            {data ? inr(data.totalValue) : '...'}
          </span>
        </div>
        <div className="money__total">
          <span className="money__total-label">Today</span>
          <span className={`money__total-change money__total-change--${dayUp ? 'up' : 'down'}`}>
            {data ? signed(data.totalDayChange) : '...'}
          </span>
        </div>
      </div>

      {status && <p className="money__status">{status}</p>}

      {/* Add a holding. The mode decides how it is priced. */}
      <div className="money__adder">
        <div className="money__modes" role="group" aria-label="What to add">
          {MODES.map((m) => (
            <button
              key={m.kind}
              type="button"
              className={`money__mode${mode === m.kind ? ' money__mode--on' : ''}`}
              aria-pressed={mode === m.kind}
              onClick={() => {
                setMode(m.kind);
                resetAddForm();
                setStatus(null);
              }}
            >
              {m.label}
            </button>
          ))}
        </div>

        {mode === 'stock' && (
          <div className="money__add">
            <input
              className="money__input money__input--sym"
              type="text"
              placeholder="Symbol (e.g. RELIANCE.NS, GOLDBEES.NS)"
              value={symbol}
              onChange={(e) => setSymbol(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') void add();
              }}
              aria-label="Market symbol"
            />
            <input
              className="money__input money__input--qty"
              type="number"
              min="0"
              step="any"
              placeholder="Units"
              value={qty}
              onChange={(e) => setQty(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') void add();
              }}
              aria-label="Units"
            />
            <input
              className="money__input money__input--label"
              type="text"
              placeholder="Label (optional)"
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              aria-label="Label"
            />
            <button className="money__add-btn" type="button" onClick={() => void add()} aria-label="Add holding">
              <Plus size={15} strokeWidth={2.5} aria-hidden="true" />
            </button>
          </div>
        )}

        {mode === 'mf' && (
          <div className="money__mf">
            <div className="money__add">
              <input
                className="money__input money__input--sym"
                type="text"
                placeholder="Search a fund, e.g. Tata Digital India"
                value={fundQuery}
                onChange={(e) => setFundQuery(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') void runFundSearch();
                }}
                aria-label="Fund name"
              />
              <button
                className="money__add-btn"
                type="button"
                onClick={() => void runFundSearch()}
                disabled={searching}
                aria-label="Search funds"
              >
                <Search size={14} strokeWidth={2.5} aria-hidden="true" />
              </button>
            </div>

            {picked ? (
              <div className="money__picked">
                <span className="money__picked-name">{picked.name}</span>
                <div className="money__add">
                  <input
                    className="money__input money__input--qty"
                    type="number"
                    min="0"
                    step="any"
                    placeholder="Units"
                    value={qty}
                    onChange={(e) => setQty(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter') void add();
                    }}
                    aria-label="Units held"
                  />
                  <button className="money__add-btn" type="button" onClick={() => void add()} aria-label="Add fund">
                    <Plus size={15} strokeWidth={2.5} aria-hidden="true" />
                  </button>
                  <button className="money__link" type="button" onClick={() => setPicked(null)}>
                    change
                  </button>
                </div>
              </div>
            ) : (
              fundResults.length > 0 && (
                <ul className="money__results">
                  {fundResults.map((f) => (
                    <li key={f.code}>
                      <button className="money__result" type="button" onClick={() => setPicked(f)}>
                        {f.name}
                      </button>
                    </li>
                  ))}
                </ul>
              )
            )}
          </div>
        )}

        {mode === 'manual' && (
          <div className="money__add">
            <input
              className="money__input money__input--label"
              type="text"
              placeholder="Name (e.g. Tata AIA ULIP, Cash)"
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              aria-label="Name"
            />
            <input
              className="money__input money__input--qty"
              type="number"
              min="0"
              step="any"
              placeholder="Value ₹"
              value={qty}
              onChange={(e) => setQty(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') void add();
              }}
              aria-label="Value in rupees"
            />
            <button className="money__add-btn" type="button" onClick={() => void add()} aria-label="Add manual holding">
              <Plus size={15} strokeWidth={2.5} aria-hidden="true" />
            </button>
          </div>
        )}
      </div>

      {holdings.length === 0 && !busy && (
        <p className="money__empty">
          No holdings yet. Add a stock (like <code>RELIANCE.NS</code>), a gold ETF
          (<code>GOLDBEES.NS</code>), or any Yahoo symbol, with how many units you hold.
        </p>
      )}

      {holdings.length > 0 && (
        <ul className="money__list">
          <li className="money__row money__row--head" aria-hidden="true">
            <span className="money__cell money__cell--name">Holding</span>
            <span className="money__cell money__cell--num">Qty</span>
            <span className="money__cell money__cell--num">Price</span>
            <span className="money__cell money__cell--num">Value</span>
            <span className="money__cell money__cell--num">Today</span>
            <span className="money__cell money__cell--x" />
          </li>
          {(data?.groups ?? []).map((g) => (
            <Fragment key={g.name}>
              <li className="money__group">
                <span className="money__group-name">{g.name}</span>
                <span className="money__group-figs">
                  <span>{inr(g.value)}</span>
                  {g.invested !== null && <span className="money__group-sub">invested {inr(g.invested)}</span>}
                  {g.gain !== null && (
                    <span className={`money__chg money__chg--${g.gain >= 0 ? 'up' : 'down'}`}>
                      {signed(g.gain)}
                      {g.invested ? ` (${((g.gain / g.invested) * 100).toFixed(1)}%)` : ''}
                    </span>
                  )}
                </span>
              </li>
          {holdings.filter((h) => (h.broker || 'Other') === g.name).map((h) => {
            const up = (h.dayChange ?? 0) >= 0;
            const manual = h.kind === 'manual';
            const kindTag = h.kind === 'mf' ? 'FUND' : h.kind === 'manual' ? 'MANUAL' : h.kind === 'ulip' ? 'ULIP' : 'STOCK';
            return (
              <li className="money__row" key={h.id}>
                <span className="money__cell money__cell--name">
                  <span className="money__name">{h.label || h.symbol}</span>
                  <span className="money__sub">
                    <span className={`money__kind money__kind--${h.kind}`}>{kindTag}</span>
                    {h.symbol && <span className="money__sym">{h.symbol}</span>}
                  </span>
                </span>
                <span className="money__cell money__cell--num">{manual ? '-' : h.quantity}</span>
                <span className="money__cell money__cell--num">
                  {h.price !== null ? (
                    <>
                      {inr(h.price)}
                      {h.priceAsOf && <span className="money__asof">NAV {h.priceAsOf.slice(8)}/{h.priceAsOf.slice(5, 7)}</span>}
                    </>
                  ) : manual ? '-' : (
                    <span className="money__err" title={h.error ?? ''}>n/a</span>
                  )}
                </span>
                <span className="money__cell money__cell--num">
                  {h.value !== null ? inr(h.value) : '-'}
                </span>
                <span className={`money__cell money__cell--num money__chg money__chg--${up ? 'up' : 'down'}`}>
                  {h.dayChange !== null ? signed(h.dayChange) : '-'}
                </span>
                <span className="money__cell money__cell--x">
                  <button
                    className="money__remove"
                    type="button"
                    onClick={() => void remove(h.id)}
                    aria-label={`Remove ${h.label || h.symbol}`}
                  >
                    <X size={12} strokeWidth={2.5} aria-hidden="true" />
                  </button>
                </span>
              </li>
            );
          })}
            </Fragment>
          ))}
        </ul>
      )}
      </>
      )}
    </div>
  );
}
