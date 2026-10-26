import { useCallback, useEffect, useMemo, useState } from 'react';
import { RefreshCw, TrendingUp } from 'lucide-react';
import { stockBoard, type StockBoard, type StockRow } from '../../lib/nexus-db';
import { FundsView, SuggestionsView } from './Picks';
import './StocksScreen.css';

type SortKey = 'changePct' | 'ret1w' | 'ret1m' | 'ret3m' | 'fromHigh';

interface List {
  id: string;
  label: string;
  /** The rule, shown under the tabs so every list says how it was made. */
  rule: string;
  keep: (r: StockRow) => boolean;
  sort: SortKey;
  desc: boolean;
}

const pos = (n: number | null): n is number => n !== null && n > 0;
const neg = (n: number | null): n is number => n !== null && n < 0;

/**
 * A rising trend is price above its 20-day average, the 20-day above the
 * 50-day, and up over the month. `vsSma20 < vsSma50` is how "20-day above
 * 50-day" reads from two price-relative percentages.
 */
const rising = (r: StockRow) =>
  pos(r.vsSma20) && pos(r.vsSma50) && r.vsSma20 < r.vsSma50 && pos(r.ret1m);
const falling = (r: StockRow) =>
  neg(r.vsSma20) && neg(r.vsSma50) && r.vsSma20 > r.vsSma50 && neg(r.ret1m);

const LISTS: List[] = [
  {
    id: 'gainers',
    label: 'Top gainers',
    rule: 'Up today, biggest rise first.',
    keep: (r) => pos(r.changePct),
    sort: 'changePct',
    desc: true,
  },
  {
    id: 'losers',
    label: 'Top losers',
    rule: 'Down today, biggest fall first.',
    keep: (r) => neg(r.changePct),
    sort: 'changePct',
    desc: false,
  },
  {
    id: 'rising',
    label: 'Rising trend',
    rule: 'Above the 20- and 50-day averages, 20-day above 50-day, up over the month. Strongest 3-month return first.',
    keep: rising,
    sort: 'ret3m',
    desc: true,
  },
  {
    id: 'falling',
    label: 'Falling trend',
    rule: 'Below the 20- and 50-day averages, 20-day below 50-day, down over the month. Weakest month first.',
    keep: falling,
    sort: 'ret1m',
    desc: false,
  },
  {
    id: 'all',
    label: 'All',
    rule: 'Every Nifty 50 stock. Click a column to sort.',
    keep: () => true,
    sort: 'changePct',
    desc: true,
  },
];

/** Tabs that are advice views rather than filters over the table. */
const VIEWS = [
  { id: 'suggestions', label: 'Suggestions' },
  { id: 'funds', label: 'Suits for you' },
] as const;

const COLUMNS: { key: SortKey; label: string; title: string }[] = [
  { key: 'changePct', label: 'Today', title: "Change since yesterday's close" },
  { key: 'ret1w', label: '1W', title: 'Return over 5 sessions' },
  { key: 'ret1m', label: '1M', title: 'Return over 21 sessions' },
  { key: 'ret3m', label: '3M', title: 'Return over 63 sessions' },
  { key: 'fromHigh', label: 'vs 52W high', title: 'How far below the 52-week high' },
];

const inr = new Intl.NumberFormat('en-IN', { maximumFractionDigits: 2, minimumFractionDigits: 2 });

function Pct({ value }: { value: number | null }) {
  if (value === null) return <span className="stocks__dim">–</span>;
  const cls = value > 0 ? 'stocks__up' : value < 0 ? 'stocks__down' : '';
  return (
    <span className={cls}>
      {value > 0 ? '+' : ''}
      {value.toFixed(2)}%
    </span>
  );
}

/** A 30-session line, coloured by whether it ends above where it began. */
function Spark({ points }: { points: number[] }) {
  if (points.length < 2) return null;
  const w = 90;
  const h = 24;
  const lo = Math.min(...points);
  const span = Math.max(...points) - lo || 1;
  const d = points
    .map((p, i) => `${((i / (points.length - 1)) * w).toFixed(1)},${(h - ((p - lo) / span) * h).toFixed(1)}`)
    .join(' ');
  const up = points[points.length - 1] >= points[0];
  return (
    <svg className="stocks__spark" width={w} height={h} viewBox={`0 0 ${w} ${h}`} aria-hidden="true">
      <polyline points={d} fill="none" className={up ? 'stocks__spark--up' : 'stocks__spark--down'} />
    </svg>
  );
}

function time(unix: number): string {
  if (!unix) return '';
  return new Date(unix * 1000).toLocaleString('en-IN', {
    day: 'numeric',
    month: 'short',
    hour: '2-digit',
    minute: '2-digit',
  });
}

/**
 * Stocks: the Nifty 50 sorted by how it is moving (today's gainers and
 * losers, rising and falling trends, the whole index in one sortable table),
 * plus two advice tabs: scored stock suggestions, and small-cap funds ranked
 * for a small monthly SIP. Every list and score states its rule.
 */
export function StocksScreen() {
  const [board, setBoard] = useState<StockBoard | null>(null);
  const [listId, setListId] = useState('gainers');
  const [sort, setSort] = useState<{ key: SortKey; desc: boolean } | null>(null);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [refreshSeq, setRefreshSeq] = useState(0);

  const load = useCallback(async (refresh: boolean) => {
    setBusy(true);
    try {
      setBoard(await stockBoard(refresh));
      setStatus(null);
    } catch (err) {
      setStatus(`Could not load prices: ${String(err).replace(/^Error:\s*/, '')}`);
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    void load(false);
  }, [load]);

  const list = LISTS.find((l) => l.id === listId) ?? LISTS[0];
  const view = VIEWS.find((v) => v.id === listId);

  const rows = useMemo(() => {
    if (!board) return [];
    const key = sort?.key ?? list.sort;
    const desc = sort?.desc ?? list.desc;
    // Missing values sink to the bottom whichever way the column is sorted.
    return board.stocks.filter(list.keep).sort((a, b) => {
      const x = a[key];
      const y = b[key];
      if (x === null) return y === null ? 0 : 1;
      if (y === null) return -1;
      return desc ? y - x : x - y;
    });
  }, [board, list, sort]);

  if (!board) {
    return (
      <div className="stocks">
        <p className="stocks__note">{status ?? 'Reading the market…'}</p>
      </div>
    );
  }

  const up = board.stocks.filter((r) => pos(r.changePct)).length;
  const down = board.stocks.filter((r) => neg(r.changePct)).length;
  const flat = board.stocks.length - up - down;
  const activeKey = sort?.key ?? list.sort;
  const activeDesc = sort?.desc ?? list.desc;

  const pickList = (id: string) => {
    setListId(id);
    setSort(null);
  };

  const pickSort = (key: SortKey) =>
    setSort({ key, desc: activeKey === key ? !activeDesc : true });

  return (
    <div className="stocks">
      <header className="stocks__head">
        <TrendingUp size={18} strokeWidth={1.8} aria-hidden="true" />
        <div className="stocks__headings">
          <h1 className="stocks__title">Stocks</h1>
          <p className="stocks__subtitle">
            Nifty 50 · prices as of {time(board.asOf)}
            {board.skipped.length > 0 && ` · ${board.skipped.length} unavailable (${board.skipped.join(', ')})`}
          </p>
        </div>
        <button
          className={`stocks__icon${busy ? ' stocks__icon--busy' : ''}`}
          type="button"
          onClick={() => {
            void load(true);
            if (view) setRefreshSeq((n) => n + 1);
          }}
          disabled={busy}
          aria-label="Refresh"
          title="Refresh"
        >
          <RefreshCw size={14} strokeWidth={2} aria-hidden="true" />
        </button>
      </header>

      {status && <p className="stocks__status">{status}</p>}

      <section className="stocks-indices" aria-label="Indices">
        {board.indices.map((i) => (
          <div className="stocks-index" key={i.symbol}>
            <span className="stocks-index__name">{i.name}</span>
            <span className="stocks-index__price">{inr.format(i.price)}</span>
            <span className="stocks-index__chg">
              <Pct value={i.changePct} /> <span className="stocks__dim">· 1M</span> <Pct value={i.ret1m} />
            </span>
            <Spark points={i.spark} />
          </div>
        ))}
        <div className="stocks-index">
          <span className="stocks-index__name">Breadth today</span>
          <span className="stocks-index__price">
            <span className="stocks__up">{up}</span> / <span className="stocks__down">{down}</span>
          </span>
          <span className="stocks-index__chg stocks__dim">
            up / down{flat > 0 ? ` · ${flat} flat` : ''}
          </span>
          <div className="stocks-breadth" aria-hidden="true">
            <span className="stocks-breadth__up" style={{ flex: up }} />
            <span className="stocks-breadth__down" style={{ flex: down }} />
          </div>
        </div>
      </section>

      <nav className="stocks-tabs" aria-label="Lists">
        {LISTS.map((l) => (
          <button
            key={l.id}
            type="button"
            className={`stocks-tab${l.id === listId ? ' stocks-tab--on' : ''}`}
            aria-pressed={l.id === listId}
            onClick={() => pickList(l.id)}
          >
            {l.label}
            <span className="stocks-tab__count">{board.stocks.filter(l.keep).length}</span>
          </button>
        ))}
        {VIEWS.map((v) => (
          <button
            key={v.id}
            type="button"
            className={`stocks-tab stocks-tab--advice${v.id === listId ? ' stocks-tab--on' : ''}`}
            aria-pressed={v.id === listId}
            onClick={() => pickList(v.id)}
          >
            {v.label}
          </button>
        ))}
      </nav>

      {view?.id === 'suggestions' && <SuggestionsView refreshSeq={refreshSeq} />}
      {view?.id === 'funds' && <FundsView refreshSeq={refreshSeq} />}

      {!view && <p className="stocks__rule">{list.rule}</p>}

      {view ? null : rows.length === 0 ? (
        <p className="stocks__note">Nothing fits this list right now.</p>
      ) : (
        <div className="stocks-table__wrap">
          <table className="stocks-table">
            <thead>
              <tr>
                <th scope="col" className="stocks-table__name">
                  Stock
                </th>
                <th scope="col">Price</th>
                {COLUMNS.map((c) => (
                  <th
                    scope="col"
                    key={c.key}
                    aria-sort={activeKey === c.key ? (activeDesc ? 'descending' : 'ascending') : undefined}
                  >
                    <button type="button" className="stocks-table__sort" title={c.title} onClick={() => pickSort(c.key)}>
                      {c.label}
                      {activeKey === c.key && (activeDesc ? ' ↓' : ' ↑')}
                    </button>
                  </th>
                ))}
                <th scope="col">30 days</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr key={r.symbol}>
                  <td className="stocks-table__name">
                    <span className="stocks-table__symbol">{r.symbol}</span>
                    <span className="stocks-table__long">{r.name}</span>
                  </td>
                  <td>{inr.format(r.price)}</td>
                  <td>
                    <Pct value={r.changePct} />
                    {r.volumeRatio !== null && r.volumeRatio >= 2 && (
                      <span className="stocks-table__vol" title="Traded at least twice its usual volume today">
                        {r.volumeRatio.toFixed(1)}× vol
                      </span>
                    )}
                  </td>
                  <td>
                    <Pct value={r.ret1w} />
                  </td>
                  <td>
                    <Pct value={r.ret1m} />
                  </td>
                  <td>
                    <Pct value={r.ret3m} />
                  </td>
                  <td>
                    <Pct value={r.fromHigh} />
                  </td>
                  <td>
                    <Spark points={r.spark} />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <p className="stocks__foot">
        {view
          ? 'Suggestions come from a rules-based score over public data (Yahoo Finance, Groww, Google News), not from a SEBI-registered adviser. Markets can fall whatever a score says; invest only what you can leave alone for years.'
          : 'Sorted from published prices (Yahoo Finance, may lag a few minutes). Past movement is not a forecast.'}
      </p>
    </div>
  );
}
