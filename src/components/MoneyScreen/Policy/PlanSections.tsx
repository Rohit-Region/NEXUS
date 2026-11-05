import { useState, type ReactNode } from 'react';
import { Pencil } from 'lucide-react';
import { policyPlanSave, type PlanAdvice, type PolicyPlan, type UlipFundStats, type UlipSwitchReport } from '../../../lib/nexus-db';
import { LineChart, type Series } from './LineChart';

export const SLOTS = ['#3987e5', '#d95926', '#199e70', '#c98500', '#d55181'];

/** Inflation used to state a future sum in today's money. */
const INFLATION = 6;

export function rupees(n: number, digits = 2): string {
  return new Intl.NumberFormat('en-IN', { style: 'currency', currency: 'INR', maximumFractionDigits: digits }).format(n);
}

/** ₹12.3 L / ₹1.09 Cr, the way large sums are read in India. */
export function lakhs(n: number): string {
  if (Math.abs(n) >= 1e7) return `₹${(n / 1e7).toFixed(2)} Cr`;
  if (Math.abs(n) >= 1e5) return `₹${(n / 1e5).toFixed(1)} L`;
  return rupees(n, 0);
}

export function Pct({ value, digits = 1 }: { value: number | null | undefined; digits?: number }) {
  if (value === null || value === undefined) return <span className="policy__na">-</span>;
  return (
    <span className={`money__chg money__chg--${value >= 0 ? 'up' : 'down'}`}>
      {value >= 0 ? '+' : ''}
      {value.toFixed(digits)}%
    </span>
  );
}

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="policy__stat">
      <dt>{label}</dt>
      <dd>{children}</dd>
    </div>
  );
}

const BLANK: PolicyPlan = {
  started: new Date().toISOString().slice(0, 10),
  monthlyPremium: 5000,
  payYears: 10,
  termYears: 30,
  birthYear: new Date().getFullYear() - 30,
  retireAge: 60,
};

/** The policy's terms and the holder's retirement, editable. */
export function PlanFacts({ group, advice, onSaved }: { group: string; advice: PlanAdvice | null; onSaved: () => void }) {
  const [editing, setEditing] = useState(advice === null);
  const [v, setV] = useState<Record<keyof PolicyPlan, string>>(() => {
    const p = advice?.plan ?? BLANK;
    return {
      started: p.started,
      monthlyPremium: String(p.monthlyPremium),
      payYears: String(p.payYears),
      termYears: String(p.termYears),
      birthYear: String(p.birthYear),
      retireAge: String(p.retireAge),
    };
  });
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      await policyPlanSave(group, {
        started: v.started,
        monthlyPremium: Number(v.monthlyPremium),
        payYears: Math.round(Number(v.payYears)),
        termYears: Math.round(Number(v.termYears)),
        birthYear: Math.round(Number(v.birthYear)),
        retireAge: Math.round(Number(v.retireAge)),
      });
      setEditing(false);
      onSaved();
    } catch (err) {
      setError(String(err).replace(/^Error:\s*/, ''));
    } finally {
      setBusy(false);
    }
  };

  const field = (key: keyof PolicyPlan, label: string, type = 'number') => (
    <label className="policy__field">
      <span>{label}</span>
      <input className="money__input" type={type} value={v[key]} onChange={(e) => setV({ ...v, [key]: e.target.value })} />
    </label>
  );

  return (
    <section className="policy__section" aria-labelledby="policy-plan">
      <div className="policy__h-row">
        <h3 className="policy__h" id="policy-plan">
          Your plan
        </h3>
        {!editing && (
          <button className="policy__link" type="button" onClick={() => setEditing(true)}>
            <Pencil size={11} aria-hidden="true" /> Edit
          </button>
        )}
      </div>

      {editing ? (
        <form
          className="policy__form"
          onSubmit={(e) => {
            e.preventDefault();
            void save();
          }}
        >
          <div className="policy__form-grid">
            {field('birthYear', 'Year you were born')}
            {field('retireAge', 'Retire at age')}
            {field('monthlyPremium', 'Premium a month (₹)')}
            {field('payYears', 'Payment term (years)')}
            {field('termYears', 'Policy term (years)')}
            {field('started', 'First premium', 'date')}
          </div>
          {error && (
            <p className="policy__error" role="alert">
              {error}
            </p>
          )}
          <div className="policy__form-actions">
            {advice && (
              <button className="policy__link" type="button" onClick={() => setEditing(false)}>
                Cancel
              </button>
            )}
            <button className="policy__save" type="submit" disabled={busy}>
              {busy ? 'Saving…' : 'Save'}
            </button>
          </div>
        </form>
      ) : (
        advice && (
          <>
            <dl className="policy__stats">
              <Fact label="Your age">{advice.age}</Fact>
              <Fact label="Premium">{rupees(advice.plan.monthlyPremium, 0)} a month</Fact>
              <Fact label="Paying until">
                {advice.payUntil} ({advice.premiumsLeft} left)
              </Fact>
              <Fact label="Paid / still to pay">
                {lakhs(advice.paid)} / {lakhs(advice.toPay)}
              </Fact>
              <Fact label="Retire">
                {advice.retireYear}, at {advice.plan.retireAge} ({advice.yearsToRetire} years)
              </Fact>
              <Fact label="Policy matures">{advice.maturityYear}</Fact>
            </dl>
            <p className="policy__stage">
              <span className="policy__stage-tag">{advice.stage} stage</span> {advice.stageNote}
            </p>
          </>
        )
      )}
    </section>
  );
}

/** The best fund for this horizon, the ranking, and the premium split. */
export function BestForYou({ advice, report }: { advice: PlanAdvice; report: UlipSwitchReport }) {
  const [all, setAll] = useState(false);
  const top = advice.fits[0]?.score || 1;
  const unscored = report.held.filter((h) => !advice.fits.some((f) => f.label === h.label));
  return (
    <section className="policy__section" aria-labelledby="policy-best">
      <h3 className="policy__h" id="policy-best">
        Best fund for you
      </h3>
      {advice.best && (
        <div className="policy__best">
          <span className="policy__best-name">{advice.best}</span>
          <p className="policy__body">{advice.bestWhy}</p>
        </div>
      )}

      <ol className="policy__fits">
        {(all ? advice.fits : advice.fits.slice(0, 5)).map((f) => (
          <li key={f.label} className={`policy__fit${report.held.some((h) => h.label === f.label) ? ' policy__fit--held' : ''}`}>
            <span className="policy__fit-rank">{f.rank}</span>
            <span className="policy__fit-main">
              <span className="policy__fit-name">{f.label}</span>
              <span className="policy__fit-why">{f.why}</span>
            </span>
            <span className="policy__fit-bar" aria-hidden="true">
              <span style={{ width: `${(f.score / top) * 100}%` }} />
            </span>
            <span className="policy__fit-score">{f.score.toFixed(0)}</span>
          </li>
        ))}
        {unscored.map((h) => (
          <li key={h.label} className="policy__fit policy__fit--held">
            <span className="policy__fit-rank">-</span>
            <span className="policy__fit-main">
              <span className="policy__fit-name">
                {h.label} <span className="policy__tag">yours</span>
              </span>
              <span className="policy__fit-why">
                Not scored yet: needs three years of prices and two years of 1-year holds to compare fairly.
              </span>
            </span>
            <span className="policy__fit-bar" aria-hidden="true" />
            <span className="policy__fit-score policy__muted">new</span>
          </li>
        ))}
      </ol>
      {advice.fits.length > 5 && (
        <button className="policy__link" type="button" onClick={() => setAll(!all)}>
          {all ? 'Show the top 5' : `Show all ${advice.fits.length} scored funds`}
        </button>
      )}

      {advice.mix.length > 0 && (
        <>
          <h4 className="policy__sub">Suggested split for new premiums</h4>
          <ul className="policy__mix">
            {advice.mix.map((m) => (
              <li key={m.label} className={`policy__mix-row${m.held ? ' policy__mix-row--held' : ''}`}>
                <span className="policy__mix-share">{m.share.toFixed(0)}%</span>
                <span className="policy__mix-track" aria-hidden="true">
                  <span style={{ width: `${m.share}%` }} />
                </span>
                <span className="policy__fit-main">
                  <span className="policy__fit-name">
                    {m.label} {m.held && <span className="policy__tag">yours</span>}
                  </span>
                  <span className="policy__fit-why">{m.why}</span>
                </span>
                <span className="policy__mix-amt">{rupees((advice.plan.monthlyPremium * m.share) / 100, 0)}/mo</span>
              </li>
            ))}
          </ul>
          <p className="policy__note">
            Change it on the {report.insurer} policy page, where premium allocation is set. That moves only new premiums;
            the money already in stays where it is.
          </p>
        </>
      )}
    </section>
  );
}

/** Your funds side by side with the best fits. */
export function Comparison({ advice, report }: { advice: PlanAdvice; report: UlipSwitchReport }) {
  const fund = (label: string) => report.funds.find((f) => f.label === label);
  const heldLabels = report.held.map((h) => h.label);
  const cols = [...heldLabels, ...advice.fits.filter((f) => !heldLabels.includes(f.label)).slice(0, 3).map((f) => f.label)]
    .map(fund)
    .filter((f): f is UlipFundStats => f !== undefined);
  const fit = (label: string) => advice.fits.find((f) => f.label === label);
  const hasBench = cols.some((f) => f.bench1y !== null);
  const rows: [string, (f: UlipFundStats) => ReactNode][] = [
    ['Fit for you', (f) => (fit(f.label) ? `#${fit(f.label)?.rank} · ${fit(f.label)?.score.toFixed(0)}` : 'not scored yet')],
    ['Launched', (f) => f.launched ?? 'over 5 years ago'],
    ['1 year', (f) => <Pct value={f.r1y} />],
    ...(hasBench ? ([['Its benchmark, 1 year', (f) => <Pct value={f.bench1y} />]] as [string, (f: UlipFundStats) => ReactNode][]) : []),
    ['3 years, a year', (f) => <Pct value={f.cagr3y} />],
    ['5 years, a year', (f) => <Pct value={f.cagr5y} />],
    ['1-year holds that made money', (f) => (f.rolling ? `${f.rolling.positive.toFixed(0)}%` : '-')],
    ['Typical 1-year hold', (f) => <Pct value={f.rolling?.median} />],
    ['Worst 1-year hold', (f) => <Pct value={f.rolling?.worst} />],
    ['Worst fall, past year', (f) => <Pct value={f.worstFall} />],
    ['Volatility', (f) => (f.volatility === null ? '-' : `${f.volatility.toFixed(1)}%`)],
    ['Charge', (f) => (f.fmc === null ? '-' : `${f.fmc.toFixed(2)}%`)],
  ];
  return (
    <section className="policy__section" aria-labelledby="policy-compare">
      <h3 className="policy__h" id="policy-compare">
        Your funds against the best fits
      </h3>
      <div className="policy__table-wrap">
        <table className="policy__table policy__compare">
          <thead>
            <tr>
              <th scope="col">
                <span className="policy__sr">Measure</span>
              </th>
              {cols.map((f) => (
                <th scope="col" key={f.label} className={heldLabels.includes(f.label) ? 'policy__col--held' : undefined}>
                  {f.label}
                  {heldLabels.includes(f.label) && <span className="policy__tag">yours</span>}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map(([label, cell]) => (
              <tr key={label}>
                <th scope="row">{label}</th>
                {cols.map((f) => (
                  <td key={f.label} className={heldLabels.includes(f.label) ? 'policy__col--held' : undefined}>
                    {cell(f)}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}

/** When the policy moves from growth to safety. */
export function Glide({ advice }: { advice: PlanAdvice }) {
  return (
    <section className="policy__section" aria-labelledby="policy-glide">
      <h3 className="policy__h" id="policy-glide">
        Glide path to retirement
      </h3>
      <ol className="policy__glide">
        {advice.glide.map((g) => (
          <li key={g.year} className="policy__glide-step">
            <span className="policy__glide-year">
              {g.year} <span className="policy__muted">age {g.age}</span>
            </span>
            <span className="policy__glide-bar" aria-hidden="true">
              <span className="policy__glide-eq" style={{ width: `${g.equity}%` }} />
            </span>
            <span className="policy__glide-pct">
              {g.equity.toFixed(0)}% equity · {(100 - g.equity).toFixed(0)}% debt
            </span>
            <span className="policy__fit-why">{g.note}</span>
          </li>
        ))}
      </ol>
      <p className="policy__note">
        Moving to safety gradually, from ten years out, means a crash in any one year can't take a large share of what took
        decades to build.
      </p>
    </section>
  );
}

const yearMs = (y: number) => Date.UTC(y, 11, 31);

/** What the policy could be worth, at fixed rates. */
export function Projection({ advice, report }: { advice: PlanAdvice; report: UlipSwitchReport }) {
  const fiveYear = report.funds.map((f) => f.cagr5y ?? f.cagr3y).filter((v): v is number => v !== null);
  const series: Series[] = [
    ...advice.projection.map((s, i) => ({
      key: s.label,
      label: `${s.label} (${s.rate}% a year)`,
      color: SLOTS[i],
      points: s.points.map(([y, v]) => ({ t: yearMs(y), v })),
    })),
    {
      key: 'paid',
      label: 'Premiums paid',
      color: '#888888',
      dashed: true,
      points: advice.paidLine.map(([y, v]) => ({ t: yearMs(y), v })),
    },
  ];
  const payYear = Number(advice.payUntil.slice(0, 4));
  const at = (points: [number, number][], year: number) => points.find(([y]) => y === year)?.[1] ?? null;
  const thisYear = advice.glide[0]?.year ?? new Date().getFullYear();
  const todays = (v: number, year: number) => v / (1 + INFLATION / 100) ** (year - thisYear);
  return (
    <section className="policy__section" aria-labelledby="policy-proj">
      <h3 className="policy__h" id="policy-proj">
        Where this could get you by {advice.retireYear}
      </h3>
      <div className="policy__legend">
        {series.map((s) => (
          <span className="policy__chip" key={s.key}>
            <span className={`lchart__swatch${s.dashed ? ' lchart__swatch--dashed' : ''}`} style={{ background: s.color }} aria-hidden="true" />
            {s.label}
          </span>
        ))}
      </div>
      <LineChart series={series} format={lakhs} label={`Projected policy value to ${advice.retireYear}`} />
      <div className="policy__table-wrap">
        <table className="policy__table">
          <thead>
            <tr>
              <th scope="col">At</th>
              {advice.projection.map((s) => (
                <th scope="col" key={s.label}>
                  {s.label} {s.rate}%
                </th>
              ))}
              <th scope="col">Paid in</th>
            </tr>
          </thead>
          <tbody>
            {(
              [
                [payYear, `Last premium, ${payYear}`],
                [advice.retireYear, `Retirement, ${advice.retireYear}`],
              ] as [number, string][]
            ).map(([year, label]) => (
              <tr key={label}>
                <th scope="row">{label}</th>
                {advice.projection.map((s) => {
                  const v = at(s.points, year);
                  return (
                    <td key={s.label}>
                      {v === null ? '-' : lakhs(v)}
                      {v !== null && <span className="policy__since">{lakhs(todays(v, year))} today</span>}
                    </td>
                  );
                })}
                <td>{lakhs(at(advice.paidLine, year) ?? 0)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <p className="policy__note">
        Arithmetic at fixed rates, not a forecast.
        {fiveYear.length > 0 &&
          ` The longest records in this plan run from about ${Math.min(...fiveYear).toFixed(0)}% to ${Math.max(...fiveYear).toFixed(0)}% a year.`}{' '}
        Fund charges are already in those returns; the policy's own administration and mortality charges are not, and
        lower the result somewhat. "Today" is the same sum after {INFLATION}% a year of inflation.
      </p>
    </section>
  );
}
