import { useCallback, useEffect, useRef, useState } from 'react';
import { ExternalLink } from 'lucide-react';
import {
  fundPicks,
  growthPicks,
  investRisk,
  openUrl,
  setInvestRisk,
  stockPicks,
  type FundPick,
  type FundPicks,
  type ScoreFactor,
  type StockPick,
  type StockPicks,
  type DivisionKind,
  type GrowthPick,
  type GrowthPicks,
  type Prediction,
  type RiskLevel,
} from '../../lib/nexus-db';

export const inr = new Intl.NumberFormat('en-IN', { maximumFractionDigits: 2, minimumFractionDigits: 2 });
export const whole = new Intl.NumberFormat('en-IN', { maximumFractionDigits: 0 });

export function signed(n: number | null, digits = 1, unit = '%'): string {
  if (n === null) return '–';
  return `${n > 0 ? '+' : ''}${n.toFixed(digits)}${unit}`;
}

export function tone(n: number | null): string {
  if (n === null || n === 0) return '';
  return n > 0 ? 'stocks__up' : 'stocks__down';
}

export function errText(err: unknown): string {
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

export function verdictClass(v: string): string {
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

/** Expected move, range, odds, target and stop for one pick. */
export function PredictionBox({ p, price }: { p: Prediction; price: number }) {
  const days = p.sessions === 5 ? 'a week' : p.sessions === 20 ? 'a month' : 'three months';
  return (
    <div className="pick-predict" title={`Basis: ${p.basis}`}>
      <span className="pick-predict__head">Prediction, next {days}</span>
      <span>
        <strong className={tone(p.expected)}>{signed(p.expected)}</strong>{' '}
        <span className="stocks__dim">
          likely {signed(p.low)} to {signed(p.high)} · {Math.round(p.probUp * 100)}% chance up
        </span>
      </span>
      <span>
        Target <strong>₹{inr.format(p.target)}</strong> · stop-loss <strong>₹{inr.format(p.stop)}</strong>
        <span className="stocks__dim"> from ₹{inr.format(price)}</span>
      </span>
      <span className="stocks__dim pick-predict__basis">
        {p.basis === 'model' ? 'Model estimate from its volatility and score' : `From ${p.basis}`}
      </span>
    </div>
  );
}

function GrowthCard({ p }: { p: GrowthPick }) {
  const [open, setOpen] = useState(false);
  const f = p.fundamentals;
  return (
    <article className="pick">
      <header className="pick__head">
        <div>
          <span className="pick__symbol">{p.symbol}</span>
          <span className="pick__name">
            {p.name} · {p.industry}
          </span>
        </div>
        <span className={verdictClass(p.verdict)}>{p.verdict}</span>
        <span className="pick__score" title="Score out of 100">
          {Math.round(p.score)}
        </span>
      </header>
      <p className="pick__line">
        ₹{inr.format(p.price)} · 1W <span className={tone(p.ret1w)}>{signed(p.ret1w)}</span> · 3M{' '}
        <span className={tone(p.ret3m)}>{signed(p.ret3m)}</span>
        {p.volatility !== null && <span className="stocks__dim"> · swings {p.volatility.toFixed(0)}%/yr</span>}
      </p>
      <p className="pick__buy">
        {p.shares} share{p.shares === 1 ? '' : 's'} for ₹{whole.format(p.cost)}
        <span className="stocks__dim"> · {p.index}</span>
      </p>
      {p.prediction && <PredictionBox p={p.prediction} price={p.price} />}
      <Notes reasons={p.reasons} risks={p.risks} />
      <button className="pick__more" type="button" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        {open ? 'Hide detail' : 'Score and company'}
      </button>
      {open && (
        <div className="pick__detail">
          <Factors factors={p.factors} />
          <dl className="fund-stats">
            <div>
              <dt>Revenue</dt>
              <dd className={tone(f.revenueGrowth)}>{signed(f.revenueGrowth, 0)}</dd>
            </div>
            <div>
              <dt>Profit</dt>
              <dd className={tone(f.profitGrowth)}>{signed(f.profitGrowth, 0)}</dd>
            </div>
            <div>
              <dt>ROE</dt>
              <dd>{f.roe === null ? '–' : `${f.roe.toFixed(1)}%`}</dd>
            </div>
            <div>
              <dt>P/E</dt>
              <dd>{f.pe === null ? '–' : f.pe.toFixed(1)}</dd>
            </div>
            <div>
              <dt>Debt/Equity</dt>
              <dd>{f.debtEquity === null ? '–' : f.debtEquity.toFixed(2)}</dd>
            </div>
            <div>
              <dt>Market cap</dt>
              <dd>{f.marketCap === null ? '–' : `₹${whole.format(f.marketCap)} Cr`}</dd>
            </div>
          </dl>
          {p.headlines.length > 0 && (
            <ul className="pick-news">
              {p.headlines.map((h) => (
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

const fundName = (name: string) => name.replace(/\s+Direct.*$/i, '');

/** Share of the monthly amount for the fund core and each division. */
const SPLITS: Record<RiskLevel, { fund: number; steady: number; balanced: number; volatile: number; note: string }> = {
  cautious: { fund: 0.8, steady: 0.2, balanced: 0, volatile: 0, note: 'Mostly the fund; a little in the calmest stocks.' },
  balanced: { fund: 0.6, steady: 0.2, balanced: 0.2, volatile: 0, note: 'A fund core, with steady and balanced stocks around it.' },
  aggressive: { fund: 0.4, steady: 0, balanced: 0.3, volatile: 0.3, note: 'A smaller core; more in balanced and short-term stocks.' },
};

interface PlanLine {
  amount: number;
  text: string;
  detail: string;
}

/**
 * Split the amount by risk level, fill each slice with the best thing it can
 * buy, and return whatever no stock fits to the fund.
 */
function buildPlan(budget: number, risk: RiskLevel, funds: FundPick[], growth: GrowthPicks | null): PlanLine[] {
  const split = SPLITS[risk];
  const lines: PlanLine[] = [];
  let toFund = budget * split.fund;
  for (const kind of ['steady', 'balanced', 'volatile'] as DivisionKind[]) {
    const slice = budget * split[kind];
    if (slice <= 0) continue;
    const div = growth?.divisions.find((d) => d.kind === kind);
    const pick = div?.picks.find((p) => (p.verdict === 'Buy' || p.verdict === 'Accumulate') && p.price <= slice);
    if (!div || !pick) {
      toFund += slice;
      continue;
    }
    const shares = Math.floor(slice / pick.price);
    const cost = shares * pick.price;
    toFund += slice - cost;
    const pr = pick.prediction;
    lines.push({
      amount: cost,
      text: `${shares} × ${pick.symbol} (${div.label})`,
      detail: pr
        ? `score ${Math.round(pick.score)}, expects ${signed(pr.expected)} over ${pr.sessions} sessions; stop-loss ₹${inr.format(pr.stop)}`
        : `score ${Math.round(pick.score)}`,
    });
  }
  const fund = funds.find((f) => f.minSip === null || f.minSip <= toFund);
  if (fund && toFund >= 1) {
    lines.unshift({
      amount: toFund,
      text: `SIP in ${fundName(fund.name)}`,
      detail: `the core: spreads it across a hundred-odd small companies, score ${Math.round(fund.score)}`,
    });
  }
  return lines;
}

const RISK_LABEL: Record<RiskLevel, string> = { cautious: 'Cautious', balanced: 'Balanced', aggressive: 'Aggressive' };

/**
 * Suits for you: what a small monthly amount can buy, sized to the
 * investor's risk level. Mid and small caps in three divisions by how much
 * they swing, each with a score and a prediction, and small-cap funds as the
 * core. Stocks already held are left out.
 */
export function SuitsView({ refreshSeq }: { refreshSeq: number }) {
  const [funds, setFunds] = useState<FundPicks | null>(null);
  const [growth, setGrowth] = useState<GrowthPicks | null>(null);
  const [fundStatus, setFundStatus] = useState<string | null>(null);
  const [growthStatus, setGrowthStatus] = useState<string | null>(null);
  const [budget, setBudget] = useState(readBudget);
  const [risk, setRisk] = useState<RiskLevel>('balanced');
  const [division, setDivision] = useState<DivisionKind>('steady');

  useEffect(() => {
    investRisk()
      .then(setRisk)
      .catch(() => undefined);
  }, []);

  const loadFunds = useCallback(async (refresh: boolean) => {
    setFundStatus('Reading every small-cap fund’s record, risk, cost and portfolio…');
    try {
      setFunds(await fundPicks(refresh));
      setFundStatus(null);
    } catch (err) {
      setFundStatus(`Could not rank funds: ${errText(err)}`);
    }
  }, []);
  useRefresh(refreshSeq, loadFunds);

  // Stocks depend on the amount; wait for typing to settle before rescoring.
  const budgetRef = useRef(budget);
  budgetRef.current = budget;
  const loadGrowth = useCallback(async (refresh: boolean) => {
    const amount = budgetRef.current;
    if (amount < 1) return;
    setGrowthStatus('Scoring the mid and small caps within your amount (the first time after launch can take a minute)…');
    try {
      const g = await growthPicks(amount, refresh);
      if (g.budget === budgetRef.current) setGrowth(g);
      setGrowthStatus(null);
    } catch (err) {
      setGrowthStatus(`Could not rank stocks: ${errText(err)}`);
    }
  }, []);
  useRefresh(refreshSeq, loadGrowth);
  const firstBudget = useRef(true);
  useEffect(() => {
    if (firstBudget.current) {
      firstBudget.current = false;
      return;
    }
    const t = window.setTimeout(() => void loadGrowth(false), 700);
    return () => window.clearTimeout(t);
  }, [budget, loadGrowth]);

  const changeBudget = (v: number) => {
    setBudget(v);
    try {
      localStorage.setItem(BUDGET_KEY, String(v));
    } catch {
      // Remembering the amount is a convenience; the screen works without it.
    }
  };

  const changeRisk = (r: RiskLevel) => {
    setRisk(r);
    void setInvestRisk(r).catch(() => undefined);
  };

  const fits = funds ? funds.funds.filter((f) => f.minSip === null || f.minSip <= budget) : [];
  const hidden = funds ? funds.funds.length - fits.length : 0;
  const plan = funds ? buildPlan(budget, risk, fits, growth) : [];
  const shown = growth?.divisions.find((d) => d.kind === division);
  const good = shown ? shown.picks.filter((p) => p.verdict === 'Buy' || p.verdict === 'Accumulate') : [];

  return (
    <div className="picks">
      <div className="fund-budget">
        <label htmlFor="sip-budget">Monthly amount</label>
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
        <span className="risk-picker" role="radiogroup" aria-label="Risk level">
          {(Object.keys(RISK_LABEL) as RiskLevel[]).map((r) => (
            <button
              key={r}
              type="button"
              role="radio"
              aria-checked={risk === r}
              className={`stocks-tab${risk === r ? ' stocks-tab--on' : ''}`}
              onClick={() => changeRisk(r)}
            >
              {RISK_LABEL[r]}
            </button>
          ))}
        </span>
        <div className="fund-budget__plan">
          <p>
            <strong>Your plan for ₹{whole.format(budget)} this month</strong>{' '}
            <span className="stocks__dim">({SPLITS[risk].note})</span>
          </p>
          {plan.length === 0 ? (
            <p className="stocks__dim">Loading the plan…</p>
          ) : (
            <ol className="plan-lines">
              {plan.map((l) => (
                <li key={l.text}>
                  <strong>₹{whole.format(l.amount)}</strong> · {l.text}
                  <span className="stocks__dim"> · {l.detail}</span>
                </li>
              ))}
            </ol>
          )}
          <p className="stocks__dim">
            Stay at least 5 to 7 years in the fund. Stock picks are held for their division’s period; sell at the
            stop-loss rather than hoping.
          </p>
          {growth && (growth.ownedSkipped.length > 0 || growth.heavySectors.length > 0) && (
            <p className="stocks__dim">
              {growth.ownedSkipped.length > 0 && `Left out because you own them: ${growth.ownedSkipped.join(', ')}. `}
              {growth.heavySectors.length > 0 &&
                `Marked down because you already hold a lot of: ${growth.heavySectors.join(', ')}.`}
            </p>
          )}
        </div>
      </div>

      <h2 className="picks__title">Stocks within ₹{whole.format(budget)}</h2>
      {growthStatus && <p className="stocks__status">{growthStatus}</p>}
      {growth && (
        <>
          <nav className="stocks-tabs" aria-label="Divisions">
            {growth.divisions.map((d) => (
              <button
                key={d.kind}
                type="button"
                className={`stocks-tab${d.kind === division ? ' stocks-tab--on' : ''}`}
                aria-pressed={d.kind === division}
                onClick={() => setDivision(d.kind)}
              >
                {d.label}
                <span className="stocks-tab__count">
                  {d.picks.filter((p) => p.verdict === 'Buy' || p.verdict === 'Accumulate').length}
                </span>
              </button>
            ))}
          </nav>
          {shown && (
            <p className="stocks__rule">
              {shown.blurb} Swings {shown.volLow.toFixed(0)} to {shown.volHigh.toFixed(0)}% a year; {shown.picks.length}{' '}
              stocks scored out of 100 (weights:{' '}
              {shown.picks[0]?.factors.map((f) => `${f.name.toLowerCase()} ${f.weight.toFixed(0)}`).join(', ')}). From
              the Midcap 150 and Smallcap 250: {growth.affordable} of {growth.universe} trade at or under ₹
              {whole.format(growth.budget)} with at least ₹2 Cr a day changing hands.
              {growth.gaps.length > 0 && ` Note: ${growth.gaps.join('; ')}.`}
            </p>
          )}
          {good.length === 0 ? (
            <p className="stocks__note">Nothing in this division clears the bar today. Its share goes to the fund.</p>
          ) : (
            <div className="picks__grid">
              {good.slice(0, 9).map((p) => (
                <GrowthCard key={p.symbol} p={p} />
              ))}
            </div>
          )}
        </>
      )}

      <h2 className="picks__title">Small-cap mutual funds</h2>
      {fundStatus && <p className="stocks__status">{fundStatus}</p>}
      {funds && (
        <>
          <p className="stocks__rule">
            Actively managed small-cap funds with 3+ years of history, scored out of 100: track record vs peers 25,
            risk-adjusted return 20, money flow and size 15, steadiness 10, cost 10, portfolio 10, manager tenure 5,
            small-ticket 5.
            {funds.gaps.length > 0 && ` Note: ${funds.gaps.join('; ')}.`}
            {hidden > 0 && ` ${hidden} more need a bigger minimum SIP than ₹${whole.format(budget)}.`}
          </p>
          {fits.length === 0 ? (
            <p className="stocks__note">No fund starts at this amount. Most begin at ₹100 to ₹500.</p>
          ) : (
            <div className="picks__grid">
              {fits.map((f) => (
                <FundCard key={f.code} f={f} rank={funds.funds.indexOf(f) + 1} />
              ))}
            </div>
          )}
          {funds.tooNew.length > 0 && (
            <p className="stocks__foot">Not ranked, under 3 years old: {funds.tooNew.map(fundName).join(', ')}.</p>
          )}
        </>
      )}
    </div>
  );
}
