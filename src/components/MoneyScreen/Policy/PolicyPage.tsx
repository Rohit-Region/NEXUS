import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { ArrowLeft, ArrowDown, ArrowUp, Plus, RefreshCw, X } from 'lucide-react';
import {
  ulipFundHistory,
  ulipSwitchReport,
  type PortfolioGroup,
  type UlipFundKind,
  type UlipFundStats,
  type UlipSwitchReport,
  type UlipVerdict,
} from '../../../lib/nexus-db';
import { LineChart, type Point, type Series } from './LineChart';
import { BestForYou, Comparison, Glide, Pct, PlanFacts, Projection, rupees, SLOTS } from './PlanSections';
import { IS_PHONE } from '../../../lib/platform';
import './PolicyPage.css';

const PERIODS: { id: string; label: string; days: number | null }[] = [
  { id: '1m', label: '1M', days: 30 },
  { id: '3m', label: '3M', days: 91 },
  { id: '6m', label: '6M', days: 182 },
  { id: '1y', label: '1Y', days: 365 },
  { id: '3y', label: '3Y', days: 3 * 365 },
  { id: '5y', label: '5Y', days: null },
];

const VERDICT: Record<UlipVerdict, string> = {
  keep: 'Keep',
  'too-new': 'Too new to judge',
  watch: 'Watch',
  switch: 'Consider switching',
};

const KIND_LABEL: Record<UlipFundKind, string> = { equity: 'Equity', balanced: 'Balanced', debt: 'Debt' };

type SortKey = 'label' | 'r1m' | 'r6m' | 'r1y' | 'bench1y' | 'cagr3y' | 'cagr5y' | 'volatility' | 'worstFall' | 'fmc';

const COLUMNS: { key: SortKey; label: string; title?: string }[] = [
  { key: 'label', label: 'Fund' },
  { key: 'r1m', label: '1M' },
  { key: 'r6m', label: '6M' },
  { key: 'r1y', label: '1Y' },
  { key: 'bench1y', label: 'Index 1Y', title: 'Its benchmark over the same year' },
  { key: 'cagr3y', label: '3Y p.a.', title: 'A year, compounded' },
  { key: 'cagr5y', label: '5Y p.a.', title: 'A year, compounded' },
  { key: 'volatility', label: 'Volatility', title: 'How much the NAV swings, a year (past year)' },
  { key: 'worstFall', label: 'Worst fall', title: 'Largest drop from a high in the past year' },
  { key: 'fmc', label: 'Charge', title: 'Fund management charge a year; returns are already after it' },
];

const START = 10_000;
const MAX_LINES = SLOTS.length;

const dayMs = (iso: string) => Date.parse(`${iso}T00:00:00Z`);

/** The points within `days` of the latest, or all of them. */
function windowed(points: Point[], days: number | null, end: number): Point[] {
  if (days === null) return points;
  const from = end - days * 86_400_000;
  return points.filter((p) => p.t >= from);
}

/** A ULIP policy's own page: the verdict, how its funds and every
 *  alternative have performed, ideas, and each fund's detail. */
export function PolicyPage({ group, onBack }: { group: PortfolioGroup; onBack: () => void }) {
  const [report, setReport] = useState<UlipSwitchReport | null>(null);
  const [navs, setNavs] = useState<Map<string, Point[]>>(new Map());
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [period, setPeriod] = useState('1y');
  // Funds on the comparison chart, each with its colour slot.
  const [lines, setLines] = useState<{ label: string; slot: number }[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [sort, setSort] = useState<{ key: SortKey; desc: boolean }>({ key: 'cagr3y', desc: true });
  const detailRef = useRef<HTMLElement>(null);

  const load = useCallback(async (refresh: boolean) => {
    setBusy(true);
    setError(null);
    try {
      const r = await ulipSwitchReport(group.name, refresh);
      const histories = await ulipFundHistory(group.name, r.funds.map((f) => f.label));
      setNavs(new Map(histories.map((h) => [h.label, h.navs.map(([d, v]) => ({ t: dayMs(d), v }))])));
      setReport(r);
      setLines((current) => {
        if (current.length > 0) return current;
        const first = [...r.held.map((h) => h.label), ...r.alternatives];
        return first.slice(0, MAX_LINES).map((label, slot) => ({ label, slot }));
      });
      setSelected((s) => s ?? r.held[0]?.label ?? r.funds[0]?.label ?? null);
    } catch (err) {
      setError(String(err).replace(/^Error:\s*/, ''));
    } finally {
      setBusy(false);
    }
  }, [group.name]);

  useEffect(() => {
    void load(false);
  }, [load]);

  const days = PERIODS.find((p) => p.id === period)?.days ?? null;
  const end = report ? dayMs(report.asOf) : 0;
  const held = useMemo(() => new Set(report?.held.map((h) => h.label)), [report]);
  const alternatives = useMemo(() => new Set(report?.alternatives), [report]);
  const fund = (label: string) => report?.funds.find((f) => f.label === label);

  const growth: Series[] = lines.map(({ label, slot }) => {
    const pts = windowed(navs.get(label) ?? [], days, end);
    const base = pts[0]?.v ?? 1;
    return {
      key: label,
      label,
      color: SLOTS[slot],
      emphasis: held.has(label),
      points: pts.map((p) => ({ t: p.t, v: (p.v / base) * START })),
    };
  });

  const addLine = (label: string) =>
    setLines((ls) => {
      if (ls.some((l) => l.label === label)) return ls;
      const free = SLOTS.findIndex((_, i) => !ls.some((l) => l.slot === i));
      if (free === -1) return ls;
      return [...ls, { label, slot: free }];
    });
  const removeLine = (label: string) => setLines((ls) => ls.filter((l) => l.label !== label));
  const showOnChart = (labels: string[]) => labels.forEach(addLine);

  const select = (label: string) => {
    setSelected(label);
    detailRef.current?.scrollIntoView({ behavior: 'smooth', block: 'start' });
  };

  const sorted = useMemo(() => {
    const funds = [...(report?.funds ?? [])];
    const { key, desc } = sort;
    return funds.sort((a, b) => {
      if (key === 'label') return desc ? b.label.localeCompare(a.label) : a.label.localeCompare(b.label);
      const av = a[key] ?? -Infinity;
      const bv = b[key] ?? -Infinity;
      return desc ? bv - av : av - bv;
    });
  }, [report, sort]);

  const tone = report?.held.some((h) => h.verdict === 'switch')
    ? 'switch'
    : report?.held.some((h) => h.verdict === 'watch')
      ? 'watch'
      : 'keep';
  const hasBench = report?.funds.some((f) => f.bench1y !== null) ?? false;
  const columns = COLUMNS.filter((c) => c.key !== 'bench1y' || hasBench);
  const factsheetDate = report?.funds.map((f) => f.returnsAsOf).find((d) => d !== null) ?? null;
  const detail = selected ? fund(selected) : undefined;
  const detailNavs = selected ? windowed(navs.get(selected) ?? [], days, end) : [];
  const gainPct = group.gain !== null && group.invested ? (group.gain / group.invested) * 100 : null;
  const periodLabel = PERIODS.find((p) => p.id === period)?.label ?? '';

  return (
    <div className="policy">
      <div className="policy__bar">
        <button className="policy__back" type="button" onClick={onBack}>
          <ArrowLeft size={14} strokeWidth={2.2} aria-hidden="true" /> Portfolio
        </button>
        <span className="policy__spacer" />
        {report && <span className="policy__asof">NAVs to {report.asOf}</span>}
        <button
          className={`money__refresh${busy ? ' money__refresh--busy' : ''}`}
          type="button"
          onClick={() => void load(true)}
          disabled={busy}
          aria-label="Refresh fund NAVs"
        >
          <RefreshCw size={14} strokeWidth={2} aria-hidden="true" />
        </button>
      </div>

      <h2 className="policy__title">{group.name}</h2>

      <div className="policy__tiles">
        <div className="policy__tile">
          <span className="policy__tile-label">Value</span>
          <span className="policy__tile-value">{rupees(group.value)}</span>
        </div>
        <div className="policy__tile">
          <span className="policy__tile-label">Paid in</span>
          <span className="policy__tile-value">{group.invested !== null ? rupees(group.invested) : '-'}</span>
        </div>
        <div className="policy__tile">
          <span className="policy__tile-label">Gain</span>
          <span className="policy__tile-value">
            {group.gain !== null ? (
              <span className={`money__chg money__chg--${group.gain >= 0 ? 'up' : 'down'}`}>
                {group.gain >= 0 ? '+' : ''}
                {rupees(group.gain)}
                {gainPct !== null && ` (${gainPct.toFixed(1)}%)`}
              </span>
            ) : (
              '-'
            )}
          </span>
        </div>
        <div className="policy__tile">
          <span className="policy__tile-label">Next review</span>
          <span className="policy__tile-value">{report?.reviewOn ?? '-'}</span>
        </div>
      </div>

      {busy && !report && <p className="policy__note">Fetching every fund's record…</p>}
      {error && (
        <p className="policy__error" role="alert">
          {error}
        </p>
      )}

      {report && (
        <>
          <PlanFacts key={report.plan ? 'plan' : 'none'} group={group.name} advice={report.plan} onSaved={() => void load(false)} />

          <p className={`policy__headline policy__headline--${tone}`}>{report.headline}</p>

          <section className="policy__section" aria-labelledby="policy-yours">
            <h3 className="policy__h" id="policy-yours">
              Your funds
            </h3>
            <div className="policy__cards">
              {report.held.map((h) => {
                const f = fund(h.label);
                return (
                  <button className="policy__card" type="button" key={h.label} onClick={() => select(h.label)}>
                    <span className="policy__card-top">
                      <span className="policy__fund">{h.label}</span>
                      <span className={`policy__verdict policy__verdict--${h.verdict}`}>{VERDICT[h.verdict]}</span>
                    </span>
                    <span className="policy__figs">
                      <span>{rupees(h.value)}</span>
                      <span className="policy__muted">
                        {h.units.toFixed(2)} units · {h.weight.toFixed(0)}% of the policy
                      </span>
                    </span>
                    {f && (
                      <span className="policy__figs">
                        <span className="policy__muted">1M</span> <Pct value={f.r1m} />
                        <span className="policy__muted">3M</span> <Pct value={f.r3m} />
                        <span className="policy__muted">6M</span> <Pct value={f.r6m} />
                        <span className="policy__muted">1Y</span> <Pct value={f.r1y} />
                      </span>
                    )}
                    <span className="policy__reason">{h.reason}</span>
                  </button>
                );
              })}
            </div>
          </section>

          {report.plan && <BestForYou advice={report.plan} report={report} />}
          {report.plan && <Comparison advice={report.plan} report={report} />}

          <section className="policy__section" aria-labelledby="policy-growth">
            <div className="policy__h-row">
              <h3 className="policy__h" id="policy-growth">
                Growth of {rupees(START, 0)} over {periodLabel}
              </h3>
              <div className="policy__periods" role="group" aria-label="Period">
                {PERIODS.map((p) => (
                  <button
                    key={p.id}
                    type="button"
                    className={`policy__period${period === p.id ? ' policy__period--on' : ''}`}
                    aria-pressed={period === p.id}
                    onClick={() => setPeriod(p.id)}
                  >
                    {p.label}
                  </button>
                ))}
              </div>
            </div>
            <div className="policy__legend">
              {growth.map((s) => {
                const last = s.points[s.points.length - 1];
                return (
                  <span className="policy__chip" key={s.key}>
                    <span className="lchart__swatch" style={{ background: s.color }} aria-hidden="true" />
                    <span>{s.label}</span>
                    {held.has(s.label) && <span className="policy__tag">yours</span>}
                    <span className="policy__chip-val">{last ? rupees(last.v, 0) : 'no prices'}</span>
                    <button
                      className="policy__chip-x"
                      type="button"
                      onClick={() => removeLine(s.label)}
                      aria-label={`Remove ${s.label} from the chart`}
                    >
                      <X size={11} strokeWidth={2.5} aria-hidden="true" />
                    </button>
                  </span>
                );
              })}
              {lines.length < MAX_LINES && (
                <label className="policy__add">
                  <Plus size={12} strokeWidth={2.4} aria-hidden="true" />
                  <select value="" onChange={(e) => e.target.value && addLine(e.target.value)} aria-label="Add a fund to the chart">
                    <option value="">Compare with…</option>
                    {report.funds
                      .filter((f) => !lines.some((l) => l.label === f.label))
                      .map((f) => (
                        <option key={f.label} value={f.label}>
                          {f.label} ({KIND_LABEL[f.kind]})
                        </option>
                      ))}
                  </select>
                </label>
              )}
            </div>
            <LineChart
              series={growth}
              format={(v) => rupees(v, 0)}
              baseline={START}
              label={`Growth of ${rupees(START, 0)} over ${periodLabel} for ${growth.map((s) => s.label).join(', ')}`}
            />
            <p className="policy__note">
              Each line starts at {rupees(START, 0)} at the start of the period, or at launch for a younger fund.{' '}
              {IS_PHONE ? 'Drag along it' : 'Hover'} for values on a day.
            </p>
          </section>

          {report.plan && <Projection advice={report.plan} report={report} />}
          {report.plan && <Glide advice={report.plan} />}

          <section className="policy__section" aria-labelledby="policy-ideas">
            <h3 className="policy__h" id="policy-ideas">
              Suggestions and ideas
            </h3>
            <div className="policy__ideas">
              {report.ideas.map((idea) => (
                <article className="policy__idea" key={idea.title}>
                  <h4 className="policy__idea-title">{idea.title}</h4>
                  <p className="policy__idea-body">{idea.body}</p>
                  {idea.funds.length > 0 && (
                    <button className="policy__link" type="button" onClick={() => showOnChart(idea.funds)}>
                      Show on chart
                    </button>
                  )}
                </article>
              ))}
            </div>
            <ul className="policy__points">
              {report.points.map((p) => (
                <li key={p}>{p}</li>
              ))}
            </ul>
          </section>

          {detail && (
            <section className="policy__section" aria-labelledby="policy-detail" ref={detailRef}>
              <div className="policy__h-row">
                <h3 className="policy__h" id="policy-detail">
                  {detail.label}
                  {held.has(detail.label) && <span className="policy__tag">yours</span>}
                </h3>
                <select
                  className="policy__pick"
                  value={detail.label}
                  onChange={(e) => setSelected(e.target.value)}
                  aria-label="Fund to show"
                >
                  {report.funds.map((f) => (
                    <option key={f.label} value={f.label}>
                      {f.label}
                    </option>
                  ))}
                </select>
                {!lines.some((l) => l.label === detail.label) && lines.length < MAX_LINES && (
                  <button className="policy__link" type="button" onClick={() => addLine(detail.label)}>
                    Add to comparison
                  </button>
                )}
              </div>
              <FundDetail fund={detail} navs={detailNavs} periodLabel={periodLabel} />
            </section>
          )}

          <section className="policy__section" aria-labelledby="policy-all">
            <h3 className="policy__h" id="policy-all">
              Every fund you can switch to
            </h3>
            {factsheetDate && (
              <p className="policy__note">
                Returns are from {report.insurer}'s factsheet, which runs to {factsheetDate}; NAVs and values are the
                latest. Volatility and worst fall appear once enough daily prices are recorded.
              </p>
            )}
            <div className="policy__table-wrap">
              <table className="policy__table">
                <thead>
                  <tr>
                    {columns.map((c) => (
                      <th
                        scope="col"
                        key={c.key}
                        title={c.title}
                        aria-sort={sort.key === c.key ? (sort.desc ? 'descending' : 'ascending') : 'none'}
                      >
                        <button
                          type="button"
                          className="policy__sort"
                          onClick={() => setSort((s) => ({ key: c.key, desc: s.key === c.key ? !s.desc : c.key !== 'label' }))}
                        >
                          {c.label}
                          {sort.key === c.key &&
                            (sort.desc ? <ArrowDown size={10} aria-hidden="true" /> : <ArrowUp size={10} aria-hidden="true" />)}
                        </button>
                      </th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {sorted.map((f) => (
                    <tr
                      key={f.label}
                      className={`${held.has(f.label) ? 'policy__row--held ' : ''}${selected === f.label ? 'policy__row--on' : ''}`}
                    >
                      <th scope="row">
                        <button type="button" className="policy__row-btn" onClick={() => select(f.label)}>
                          {f.label}
                        </button>
                        <span className="policy__kind">{KIND_LABEL[f.kind]}</span>
                        {held.has(f.label) && <span className="policy__tag">yours</span>}
                        {alternatives.has(f.label) && <span className="policy__tag policy__tag--alt">strong record</span>}
                        {f.launched && <span className="policy__since">since {f.launched}</span>}
                      </th>
                      <td><Pct value={f.r1m} /></td>
                      <td><Pct value={f.r6m} /></td>
                      <td><Pct value={f.r1y} /></td>
                      {hasBench && <td><Pct value={f.bench1y} /></td>}
                      <td><Pct value={f.cagr3y} /></td>
                      <td><Pct value={f.cagr5y} /></td>
                      <td>{f.volatility === null ? <span className="policy__na">-</span> : `${f.volatility.toFixed(1)}%`}</td>
                      <td><Pct value={f.worstFall} /></td>
                      <td>{f.fmc === null ? <span className="policy__na">-</span> : `${f.fmc.toFixed(2)}%`}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </section>

          {report.failed.length > 0 && <p className="policy__note">Not fetched: {report.failed.join('; ')}</p>}
          <p className="policy__note">
            From {report.insurer}'s published figures, which are after fund charges. Past returns do not predict future
            ones; the verdict is a written rule over the record, not a forecast. Switches are made on the{' '}
            {report.insurer} policy page.
          </p>
        </>
      )}
    </div>
  );
}

function FundDetail({ fund: f, navs, periodLabel }: { fund: UlipFundStats; navs: Point[]; periodLabel: string }) {
  const maxYear = Math.max(1, ...f.years.map((y) => Math.abs(y.ret)));
  const stats: [string, ReactNode][] = [
    ['NAV', `${rupees(f.nav, 4)} on ${f.navDate}`],
    ['Type', KIND_LABEL[f.kind]],
    ['Launched', f.launched ?? 'over 5 years ago'],
    ['1 month', <Pct value={f.r1m} />],
    ['3 months', <Pct value={f.r3m} />],
    ['6 months', <Pct value={f.r6m} />],
    ['1 year', <Pct value={f.r1y} />],
    ['3 years, a year', <Pct value={f.cagr3y} />],
    ['5 years, a year', <Pct value={f.cagr5y} />],
    ['Volatility', f.volatility === null ? '-' : `${f.volatility.toFixed(1)}% a year`],
    ['Worst fall, past year', <Pct value={f.worstFall} />],
    ['Moves with yours', f.correlation === null ? '-' : f.correlation.toFixed(2)],
    ['Charge', f.fmc === null ? 'not published' : `${f.fmc.toFixed(2)}% a year`],
    ...(f.returnsAsOf ? ([['Returns run to', f.returnsAsOf]] as [string, ReactNode][]) : []),
    ...(f.benchmark
      ? ([
          ['Benchmark', f.benchmark],
          ['Benchmark, 1 year', <Pct value={f.bench1y} />],
          ['Benchmark, 3 years a year', <Pct value={f.bench3y} />],
        ] as [string, ReactNode][])
      : []),
  ];

  return (
    <div className="policy__detail">
      <div className="policy__detail-chart">
        <LineChart
          series={[{ key: f.label, label: f.label, color: SLOTS[0], points: navs }]}
          format={(v) => rupees(v, 2)}
          height={220}
          label={`${f.label} NAV over ${periodLabel}`}
        />
        <p className="policy__note">NAV over {periodLabel}.</p>
      </div>

      <dl className="policy__stats">
        {stats.map(([k, v]) => (
          <div className="policy__stat" key={k}>
            <dt>{k}</dt>
            <dd>{v}</dd>
          </div>
        ))}
      </dl>

      <div className="policy__detail-cols">
        <div>
          <h4 className="policy__sub">Year by year</h4>
          {f.years.length === 0 && <p className="policy__note">No full year yet.</p>}
          <ul className="policy__years">
            {f.years.map((y) => (
              <li key={y.year} className="policy__year">
                <span className="policy__year-label">
                  {y.year}
                  {y.partial && <span className="policy__muted"> part</span>}
                </span>
                <span className="policy__year-track">
                  <span
                    className={`policy__year-bar policy__year-bar--${y.ret >= 0 ? 'up' : 'down'}`}
                    style={{ width: `${(Math.abs(y.ret) / maxYear) * 100}%` }}
                  />
                </span>
                <Pct value={y.ret} />
              </li>
            ))}
          </ul>
        </div>
        <div>
          <h4 className="policy__sub">Consistency</h4>
          {f.rolling ? (
            <p className="policy__body">
              Of {f.rolling.holds} one-year holds, a week apart, <strong>{f.rolling.positive.toFixed(0)}%</strong> made money.
              The typical one returned <Pct value={f.rolling.median} />, the best <Pct value={f.rolling.best} /> and the
              worst <Pct value={f.rolling.worst} />.
            </p>
          ) : (
            <p className="policy__note">Needs a little over a year of prices.</p>
          )}
        </div>
      </div>
    </div>
  );
}
