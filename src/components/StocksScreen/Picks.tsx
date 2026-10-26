import { useCallback, useEffect, useRef, useState } from 'react';
import { ExternalLink } from 'lucide-react';
import {
  fundPicks,
  openUrl,
  pickTrackRecord,
  stockPicks,
  type FundPick,
  type FundPicks,
  type ScoreFactor,
  type StockPick,
  type StockPicks,
  type PickGroupRecord,
  type TrackRecord,
} from '../../lib/nexus-db';

const inr = new Intl.NumberFormat('en-IN', { maximumFractionDigits: 2, minimumFractionDigits: 2 });
const whole = new Intl.NumberFormat('en-IN', { maximumFractionDigits: 0 });

function signed(n: number | null, digits = 1, unit = '%'): string {
  if (n === null) return '–';
  return `${n > 0 ? '+' : ''}${n.toFixed(digits)}${unit}`;
}

function tone(n: number | null): string {
  if (n === null || n === 0) return '';
  return n > 0 ? 'stocks__up' : 'stocks__down';
}

function errText(err: unknown): string {
  return String(err).replace(/^Error:\s*/, '');
}

/**
 * Load from cache on mount, and bypass the cache only when the screen's
 * refresh counter moves while this view is showing. Comparing against the
 * counter at mount keeps a tab switch from refetching everything.
 */
function useRefresh(refreshSeq: number, load: (refresh: boolean) => Promise<void>) {
  const seen = useRef(refreshSeq);
  useEffect(() => {
    const refresh = refreshSeq !== seen.current;
    seen.current = refreshSeq;
    void load(refresh);
  }, [load, refreshSeq]);
}

/** Factor bars: what the score is made of, weight beside each name. */
function Factors({ factors }: { factors: ScoreFactor[] }) {
  return (
    <ul className="pick-factors">
      {factors.map((f) => (
        <li key={f.name} className="pick-factors__row">
          <span className="pick-factors__name">
            {f.name} <span className="stocks__dim">{f.weight}</span>
          </span>
          <span className="pick-factors__track" aria-hidden="true">
            {f.score !== null && <span className="pick-factors__fill" style={{ width: `${f.score}%` }} />}
          </span>
          <span className="pick-factors__val">{f.score === null ? 'n/a' : Math.round(f.score)}</span>
        </li>
      ))}
    </ul>
  );
}

function Notes({ reasons, risks }: { reasons: string[]; risks: string[] }) {
  if (reasons.length === 0 && risks.length === 0) return null;
  return (
    <ul className="pick-notes">
      {reasons.map((r) => (
        <li key={r} className="pick-notes__for">
          {r}
        </li>
      ))}
      {risks.map((r) => (
        <li key={r} className="pick-notes__against">
          {r}
        </li>
      ))}
    </ul>
  );
}

/** A journal day number (days since 1970-01-01) as "26 Oct". */
function dayLabel(day: number): string {
  return new Date(day * 86_400_000).toLocaleDateString('en-IN', { day: 'numeric', month: 'short', timeZone: 'UTC' });
}

function OutcomeTable({ groups, horizons, against }: { groups: PickGroupRecord[]; horizons: number[]; against: string }) {
  return (
    <table className="track-table">
      <thead>
        <tr>
          <th scope="col" />
          {horizons.map((h) => (
            <th scope="col" key={h} title={`${h} trading sessions later`}>
              After {h}d
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {groups.map((g) => (
          <tr key={g.label}>
            <th scope="row">{g.label}</th>
            {g.outcomes.map((o, i) => (
              <td key={horizons[i]}>
                {o.count === 0 ? (
                  <span className="stocks__dim">pending</span>
                ) : (
                  <>
                    <span className={tone(o.avgExcess)}>{signed(o.avgExcess)}</span>
                    <span className="track-table__sub">
                      {Math.round((o.hitRate ?? 0) * 100)}% beat {against} · {o.count}
                    </span>
                  </>
                )}
              </td>
            ))}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/**
 * How past suggestions turned out. Loaded after the suggestions themselves,
 * so today's record is already written when it is read.
 */
function TrackRecordPanel({ kind, after }: { kind: 'stocks' | 'funds'; after: unknown }) {
  const [rec, setRec] = useState<TrackRecord | null>(null);
  const [open, setOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!after) return;
    let alive = true;
    pickTrackRecord()
      .then((r) => alive && setRec(r))
      .catch((err) => alive && setError(errText(err)));
    return () => {
      alive = false;
    };
  }, [after]);

  if (error) return <p className="stocks__note">Track record unavailable: {error}</p>;
  if (!rec) return null;

  const days = kind === 'stocks' ? rec.stockDays : rec.fundDays;
  const l = rec.learning;
  const since = rec.firstDay !== null ? ` since ${dayLabel(rec.firstDay)}` : '';
  const summary =
    kind === 'stocks'
      ? l.active
        ? `Learning on: weights tuned from ${l.days} days of outcomes (${Math.round(l.confidence * 100)}% confidence).`
        : `Weights learn after ${l.needed} days with 20-day outcomes (${l.days} so far).`
      : 'Fund weights stay fixed; outcomes are tracked against the average fund.';

  return (
    <section className="track">
      <button className="track__head" type="button" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        <span className="track__title">Track record</span>
        <span className="stocks__dim">
          {days} day{days === 1 ? '' : 's'} recorded{since}. {summary}
        </span>
        <span className="track__toggle">{open ? 'Hide' : 'Show'}</span>
      </button>
      {open && (
        <div className="track__body">
          {kind === 'stocks' ? (
            <>
              <p className="stocks__rule">
                Average return beyond the Nifty after each suggestion, and how often it beat the Nifty. Outcomes
                fill in as each window passes.
              </p>
              <OutcomeTable groups={rec.stocks} horizons={rec.horizons} against="Nifty" />
              <h3 className="track__sub">What each factor has been worth</h3>
              <ul className="track-skill">
                {l.factors.map((f) => (
                  <li key={f.name}>
                    <span>{f.name}</span>
                    <span
                      className={tone(f.skill)}
                      title="Rank correlation between the factor and 20-day return beyond the Nifty, averaged over days"
                    >
                      skill {f.skill === null ? 'n/a' : f.skill.toFixed(2)}
                    </span>
                    <span className="stocks__dim">
                      weight {f.baseWeight.toFixed(0)}
                      {Math.abs(f.weight - f.baseWeight) >= 0.05 && ` → ${f.weight.toFixed(1)}`}
                    </span>
                  </li>
                ))}
              </ul>
              {rec.recent.length > 0 && (
                <>
                  <h3 className="track__sub">Recent Buy and Accumulate calls</h3>
                  <table className="track-table">
                    <thead>
                      <tr>
                        <th scope="col">Day</th>
                        <th scope="col">Stock</th>
                        {rec.horizons.map((h) => (
                          <th scope="col" key={h}>
                            vs Nifty {h}d
                          </th>
                        ))}
                      </tr>
                    </thead>
                    <tbody>
                      {rec.recent.map((p) => (
                        <tr key={`${p.day}-${p.symbol}`}>
                          <td>{dayLabel(p.day)}</td>
                          <td>
                            {p.symbol} <span className="stocks__dim">{p.verdict}</span>
                          </td>
                          {p.excess.map((x, i) => (
                            <td key={rec.horizons[i]} className={tone(x)}>
                              {x === null ? <span className="stocks__dim">pending</span> : signed(x)}
                            </td>
                          ))}
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </>
              )}
            </>
          ) : (
            <>
              <p className="stocks__rule">
                Average return beyond the average small-cap fund ranked the same day, and how often it beat it.
              </p>
              <OutcomeTable groups={rec.funds} horizons={rec.horizons} against="peers" />
            </>
          )}
        </div>
      )}
    </section>
  );
}

function verdictClass(v: string): string {
  return `pick-verdict pick-verdict--${v.toLowerCase().replace(/\s+/g, '-')}`;
}

function StockCard({ p }: { p: StockPick }) {
  const [open, setOpen] = useState(false);
  return (
    <article className="pick">
      <header className="pick__head">
        <div>
          <span className="pick__symbol">{p.symbol}</span>
          <span className="pick__name">{p.fundamentals.displayName ?? p.name}</span>
        </div>
        <span className={verdictClass(p.verdict)}>{p.verdict}</span>
        <span className="pick__score" title="Score out of 100">
          {Math.round(p.score)}
        </span>
      </header>
      <p className="pick__line">
        ₹{inr.format(p.price)} · today <span className={tone(p.changePct)}>{signed(p.changePct, 2)}</span> · 3M{' '}
        <span className={tone(p.ret3m)}>{signed(p.ret3m)}</span>
      </p>
      {(p.buyNear !== null || p.support !== null) && (
        <p className="pick__levels">
          {p.buyNear !== null && <>Buy near ₹{inr.format(p.buyNear)} (20-day avg)</>}
          {p.buyNear !== null && p.support !== null && ' · '}
          {p.support !== null && <>exit below ₹{inr.format(p.support)} (50-day avg)</>}
        </p>
      )}
      <Notes reasons={p.reasons} risks={p.risks} />
      <button className="pick__more" type="button" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        {open ? 'Hide detail' : 'Score and headlines'}
      </button>
      {open && (
        <div className="pick__detail">
          <Factors factors={p.factors} />
          {p.headlines.length > 0 && (
            <ul className="pick-news">
              {p.headlines.slice(0, 5).map((h) => (
                <li key={h.link}>
                  <span className={`pick-news__dot pick-news__dot--${h.tone > 0 ? 'up' : h.tone < 0 ? 'down' : 'flat'}`} />
                  <button type="button" className="pick-news__link" onClick={() => void openUrl(h.link)}>
                    {h.title}
                  </button>
                  <span className="stocks__dim"> · {h.source}</span>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </article>
  );
}

/**
 * Suggestions: the Nifty 50 ranked by a five-factor score, best first, with
 * the reasons for and against each and the ones to stay away from.
 */
export function SuggestionsView({ refreshSeq }: { refreshSeq: number }) {
  const [data, setData] = useState<StockPicks | null>(null);
  const [status, setStatus] = useState<string | null>(null);

  const load = useCallback(async (refresh: boolean) => {
    setStatus('Scoring 50 stocks on prices, fundamentals and this week’s news…');
    try {
      setData(await stockPicks(refresh));
      setStatus(null);
    } catch (err) {
      setStatus(`Could not build suggestions: ${errText(err)}`);
    }
  }, []);

  useRefresh(refreshSeq, load);

  if (!data) return <p className="stocks__note">{status ?? 'Loading…'}</p>;

  const top = data.picks.filter((p) => p.verdict === 'Buy' || p.verdict === 'Accumulate').slice(0, 10);
  const avoid = data.picks.filter((p) => p.verdict === 'Avoid').slice(-6).reverse();

  return (
    <div className="picks">
      {status && <p className="stocks__status">{status}</p>}
      <TrackRecordPanel kind="stocks" after={data} />
      <p className="stocks__rule">
        Each stock is scored out of 100 on momentum vs the Nifty, fundamentals, this week’s news, institutional buying
        and valuation vs its industry (weights:{' '}
        {data.picks[0]?.factors.map((f) => `${f.name.toLowerCase()} ${f.weight.toFixed(0)}`).join(', ')}). Penalties
        for chasing a sharp run-up or a stock still falling. Buy ≥ 68, Accumulate ≥ 58.
        {data.gaps.length > 0 && ` Missing: ${data.gaps.join('; ')}.`}
      </p>

      <h2 className="picks__title">Top picks</h2>
      {top.length === 0 ? (
        <p className="stocks__note">No stock clears the bar today. That is an answer too: hold cash or keep your SIPs.</p>
      ) : (
        <div className="picks__grid">
          {top.map((p) => (
            <StockCard key={p.symbol} p={p} />
          ))}
        </div>
      )}

      {avoid.length > 0 && (
        <>
          <h2 className="picks__title">Avoid for now</h2>
          <ul className="picks-avoid">
            {avoid.map((p) => (
              <li key={p.symbol}>
                <span className="pick__symbol">{p.symbol}</span>
                <span className="pick__score pick__score--low">{Math.round(p.score)}</span>
                <span className="stocks__dim">{p.risks[0] ?? 'Weak on most factors'}</span>
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  );
}

function FundCard({ f, rank }: { f: FundPick; rank: number }) {
  const [open, setOpen] = useState(false);
  return (
    <article className="pick">
      <header className="pick__head">
        <div>
          <span className="pick__symbol">
            #{rank} {f.name.replace(/\s+Direct.*$/i, '')}
          </span>
          <span className="pick__name">{f.fundHouse ?? ''}</span>
        </div>
        <span className={verdictClass(f.verdict)}>{f.verdict}</span>
        <span className="pick__score" title="Score out of 100">
          {Math.round(f.score)}
        </span>
      </header>
      <dl className="fund-stats">
        <div>
          <dt>3Y / yr</dt>
          <dd className={tone(f.ret3y)}>{signed(f.ret3y)}</dd>
        </div>
        <div>
          <dt>Category 3Y</dt>
          <dd>{signed(f.catRet3y)}</dd>
        </div>
        <div>
          <dt>5Y / yr</dt>
          <dd className={tone(f.ret5y)}>{signed(f.ret5y)}</dd>
        </div>
        <div>
          <dt>Min SIP</dt>
          <dd>{f.minSip === null ? '–' : `₹${whole.format(f.minSip)}`}</dd>
        </div>
        <div>
          <dt>Expense</dt>
          <dd>{f.expenseRatio === null ? '–' : `${f.expenseRatio.toFixed(2)}%`}</dd>
        </div>
        <div>
          <dt>AUM</dt>
          <dd>{f.aum === null ? '–' : `₹${whole.format(f.aum)} Cr`}</dd>
        </div>
      </dl>
      <Notes reasons={f.reasons} risks={f.risks} />
      <button className="pick__more" type="button" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        {open ? 'Hide detail' : 'Score and portfolio'}
      </button>
      {open && (
        <div className="pick__detail">
          <Factors factors={f.factors} />
          <p className="pick__levels">
            Sharpe {f.sharpe?.toFixed(2) ?? '–'} · volatility {f.stdDev?.toFixed(1) ?? '–'}% · alpha{' '}
            {signed(f.alpha)}
            {f.manager && ` · ${f.manager}, ${f.managerYears?.toFixed(1)} yrs`}
            {f.flowPct !== null && ` · net flow ${signed(f.flowPct)}`}
          </p>
          {f.topSectors.length > 0 && (
            <p className="pick__levels">
              Sectors: {f.topSectors.map((s) => `${s.name} ${s.pct.toFixed(0)}%`).join(', ')}
            </p>
          )}
          {f.topHoldings.length > 0 && (
            <p className="pick__levels">
              Top holdings: {f.topHoldings.map((h) => `${h.name} ${h.pct.toFixed(1)}%`).join(', ')}
            </p>
          )}
          {f.exitLoad && <p className="pick__levels">{f.exitLoad}</p>}
          <button
            type="button"
            className="pick-news__link"
            onClick={() => void openUrl(`https://groww.in/mutual-funds/${f.id}`)}
          >
            Open on Groww <ExternalLink size={11} strokeWidth={2} aria-hidden="true" />
          </button>
        </div>
      )}
    </article>
  );
}

const BUDGET_KEY = 'nexus.stocks.sipBudget';

function readBudget(): number {
  try {
    const v = Number(localStorage.getItem(BUDGET_KEY));
    return v > 0 ? v : 500;
  } catch {
    return 500;
  }
}

/**
 * Suits for you: actively managed small-cap funds ranked for a small monthly
 * SIP, filtered to what the entered amount can start.
 */
export function FundsView({ refreshSeq }: { refreshSeq: number }) {
  const [data, setData] = useState<FundPicks | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [budget, setBudget] = useState(readBudget);

  const load = useCallback(async (refresh: boolean) => {
    setStatus('Reading every small-cap fund’s record, risk, cost and portfolio…');
    try {
      setData(await fundPicks(refresh));
      setStatus(null);
    } catch (err) {
      setStatus(`Could not rank funds: ${errText(err)}`);
    }
  }, []);

  useRefresh(refreshSeq, load);

  const changeBudget = (v: number) => {
    setBudget(v);
    try {
      localStorage.setItem(BUDGET_KEY, String(v));
    } catch {
      // Remembering the amount is a convenience; the screen works without it.
    }
  };

  if (!data) return <p className="stocks__note">{status ?? 'Loading…'}</p>;

  const fits = data.funds.filter((f) => f.minSip === null || f.minSip <= budget);
  const hidden = data.funds.length - fits.length;
  const [first, second] = fits;
  // Two funds only when each still gets at least its minimum: splitting ₹500
  // into two ₹250 SIPs buys nothing but paperwork.
  const split =
    first && second && budget >= 1000 && budget / 2 >= (first.minSip ?? 0) && budget / 2 >= (second.minSip ?? 0);

  return (
    <div className="picks">
      {status && <p className="stocks__status">{status}</p>}

      <div className="fund-budget">
        <label htmlFor="sip-budget">Monthly SIP</label>
        <span className="fund-budget__input">
          ₹
          <input
            id="sip-budget"
            type="number"
            min={100}
            step={100}
            value={budget}
            onChange={(e) => changeBudget(Math.max(0, Number(e.target.value) || 0))}
          />
        </span>
        {first && (
          <p className="fund-budget__plan">
            {split ? (
              <>
                Split it: <strong>₹{whole.format(Math.ceil(budget / 2 / 100) * 100)}</strong> into{' '}
                <strong>{first.name.replace(/\s+Direct.*$/i, '')}</strong> and the rest into{' '}
                <strong>{second.name.replace(/\s+Direct.*$/i, '')}</strong>. Two managers with different styles
                smooth out either one’s bad year.
              </>
            ) : (
              <>
                Start a <strong>₹{whole.format(budget)}</strong> monthly SIP in{' '}
                <strong>{first.name.replace(/\s+Direct.*$/i, '')}</strong>. Stay at least 5 to 7 years: small caps
                can fall 30% or more in a bad year, and the SIP buys more units when they do.
              </>
            )}
          </p>
        )}
      </div>

      <TrackRecordPanel kind="funds" after={data} />
      <p className="stocks__rule">
        Actively managed small-cap funds with 3+ years of history, scored out of 100: track record vs peers 25,
        risk-adjusted return 20, money flow and size 15, steadiness 10, cost 10, portfolio 10, manager tenure 5,
        small-ticket 5.
        {data.gaps.length > 0 && ` Note: ${data.gaps.join('; ')}.`}
        {hidden > 0 && ` ${hidden} more need a bigger minimum SIP than ₹${whole.format(budget)}.`}
      </p>

      {fits.length === 0 ? (
        <p className="stocks__note">No fund starts at this amount. Most begin at ₹100 to ₹500.</p>
      ) : (
        <div className="picks__grid">
          {fits.map((f) => (
            <FundCard key={f.code} f={f} rank={data.funds.indexOf(f) + 1} />
          ))}
        </div>
      )}

      {data.tooNew.length > 0 && (
        <p className="stocks__foot">
          Not ranked, under 3 years old: {data.tooNew.map((n) => n.replace(/\s+Direct.*$/i, '')).join(', ')}.
        </p>
      )}
    </div>
  );
}
