import { useEffect, useState } from 'react';
import {
  pickTrackRecord,
  type KindRecord,
  type PickGroupRecord,
  type PickKind,
  type TrackRecord,
} from '../../lib/nexus-db';
import { errText, inr, signed, tone, verdictClass } from './Picks';

const KINDS: { kind: PickKind; label: string; picks: string; where: string }[] = [
  { kind: 'mine', label: 'Your buys', picks: 'Your buys', where: 'Money' },
  { kind: 'stock', label: 'Nifty 50', picks: 'Buy + Accumulate', where: 'Suggestions' },
  { kind: 'steady', label: 'Steady', picks: 'Buy + Accumulate', where: 'Suits for you' },
  { kind: 'balanced', label: 'Balanced', picks: 'Buy + Accumulate', where: 'Suits for you' },
  { kind: 'volatile', label: 'High-volatility', picks: 'Buy + Accumulate', where: 'Suits for you' },
  { kind: 'news', label: 'News radar', picks: 'Bullish calls', where: 'News radar' },
  { kind: 'fund', label: 'Mutual funds', picks: 'Top picks', where: 'Suits for you' },
  { kind: 'growth', label: 'Growth (old list)', picks: 'Buy + Accumulate', where: 'Suits for you' },
];

/** A journal day number (days since 1970-01-01) as "26 Oct". */
function dayLabel(day: number): string {
  return new Date(day * 86_400_000).toLocaleDateString('en-IN', { day: 'numeric', month: 'short', timeZone: 'UTC' });
}

/** The longest horizon with any settled outcome, or the shortest. */
function headlineHorizon(rec: KindRecord): number {
  const picks = rec.groups.slice(0, 2);
  for (let h = 2; h >= 0; h--) {
    if (picks.some((g) => g.outcomes[h].count > 0)) return h;
  }
  return 0;
}

/** Pooled outcome of the first `n` verdict groups (the positive calls). */
function pooled(groups: PickGroupRecord[], n: number, h: number) {
  let count = 0;
  let excess = 0;
  let hits = 0;
  for (const g of groups.slice(0, n)) {
    const o = g.outcomes[h];
    count += o.count;
    excess += (o.avgExcess ?? 0) * o.count;
    hits += (o.hitRate ?? 0) * o.count;
  }
  return count === 0 ? null : { count, avgExcess: excess / count, hitRate: hits / count };
}

function OutcomeTable({ groups, horizons, against }: { groups: PickGroupRecord[]; horizons: number[]; against: string }) {
  return (
    <table className="track-table">
      <thead>
        <tr>
          <th scope="col">Call</th>
          {horizons.map((h) => (
            <th scope="col" key={h} title={`${h} trading sessions later`}>
              After {h} days
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {groups.map((g) => (
          <tr key={g.label}>
            <th scope="row">
              <span className={verdictClass(g.label)}>{g.label}</span>
            </th>
            {g.outcomes.map((o, i) => (
              <td key={horizons[i]}>
                {o.count === 0 ? (
                  <span className="stocks__dim">pending</span>
                ) : (
                  <>
                    <span className={tone(o.avgExcess)}>{signed(o.avgExcess)} vs {against}</span>
                    <span className="track-table__sub">
                      {Math.round((o.hitRate ?? 0) * 100)}% beat it · {o.count} call{o.count === 1 ? '' : 's'}
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

function KindReport({
  rec,
  horizons,
  picksLabel,
  where,
}: {
  rec: KindRecord;
  horizons: number[];
  picksLabel: string;
  where: string;
}) {
  if (rec.days === 0) {
    return (
      <p className="stocks__note">
        {rec.kind === 'mine'
          ? 'No buys recorded yet. When you add shares to a holding in the Money section, NEXUS notes the buy, what it said about that stock that day, and grades the buy here 5, 20 and 60 trading days later.'
          : `Nothing recorded yet. Open ${where} on a trading day and today’s calls are written down; their first results arrive 5 trading days later.`}
      </p>
    );
  }

  const h = headlineHorizon(rec);
  const positive = rec.kind === 'fund' || rec.kind === 'news' ? 1 : rec.kind === 'mine' ? rec.groups.length : 2;
  const head = pooled(rec.groups, positive, h);
  // The call that should trail: Avoid, a fund's Weak, a bearish news call.
  const avoidLabel = rec.kind === 'fund' ? 'Weak' : rec.kind === 'news' ? 'Down' : 'Avoid';
  const avoidGroup = rec.groups.find((g) => g.label === avoidLabel) ?? rec.groups[rec.groups.length - 1];
  const avoid = avoidGroup.outcomes[h];
  const l = rec.learning;

  return (
    <div className="report">
      <div className="report-cards">
        <div className="report-card">
          <span className="report-card__label">Days recorded</span>
          <span className="report-card__value">{rec.days}</span>
          <span className="stocks__dim">since {rec.firstDay !== null ? dayLabel(rec.firstDay) : '–'}</span>
        </div>
        <div className="report-card">
          <span className="report-card__label">
            {picksLabel} vs {rec.benchmark}, {horizons[h]} days
          </span>
          <span className={`report-card__value ${tone(head?.avgExcess ?? null)}`}>
            {head ? signed(head.avgExcess) : 'pending'}
          </span>
          <span className="stocks__dim">
            {head ? `${Math.round(head.hitRate * 100)}% of ${head.count} calls beat it` : 'first results after 5 trading days'}
          </span>
        </div>
        <div className="report-card">
          <span className="report-card__label">
            {rec.kind === 'mine' ? 'Buys NEXUS said Avoid' : `${avoidGroup.label} calls`}, {horizons[h]} days
          </span>
          <span className={`report-card__value ${tone(avoid.avgExcess)}`}>
            {avoid.count ? signed(avoid.avgExcess) : 'pending'}
          </span>
          <span className="stocks__dim">
            {rec.kind === 'news'
              ? 'positive means the bearish calls came true'
              : rec.kind === 'mine'
                ? 'should trail if NEXUS was right to warn you'
                : 'should trail the benchmark if the score works'}
          </span>
        </div>
        <div className="report-card">
          <span className="report-card__label">Learning</span>
          <span className="report-card__value">
            {l ? (l.active ? `${Math.round(l.confidence * 100)}%` : `${l.days}/${l.needed}`) : 'fixed'}
          </span>
          <span className="stocks__dim">
            {l
              ? l.active
                ? 'weights tuned from outcomes'
                : 'days of 20-day outcomes before weights move'
              : 'fund weights do not learn'}
          </span>
        </div>
      </div>

      <h3 className="track__sub">{rec.kind === 'mine' ? 'Your buys, by what NEXUS said that day' : 'Suggestion vs result, by call'}</h3>
      <p className="stocks__rule">
        {rec.kind === 'news'
          ? `Return beyond ${rec.benchmark} in the direction called (a bearish call that fell more than the index counts as right), and how often it was right.`
          : rec.kind === 'mine'
            ? `How your own buys did against ${rec.benchmark}, grouped by NEXUS's view of that stock on the day you bought. If buys NEXUS rated Buy beat the ones it said Avoid, following it would have helped.`
            : `Average return beyond ${rec.benchmark} after each kind of call, and how often it beat it.`}
      </p>
      <OutcomeTable groups={rec.groups} horizons={horizons} against={rec.kind === 'fund' ? 'peers' : 'index'} />

      <h3 className="track__sub">Day by day</h3>
      <div className="stocks-table__wrap">
        <table className="stocks-table report-days">
          <thead>
            <tr>
              <th scope="col" className="stocks-table__name">
                Day
              </th>
              <th scope="col">Picked</th>
              {horizons.map((x) => (
                <th scope="col" key={x}>
                  {x} days: picks / {rec.kind === 'fund' ? 'avg fund' : 'index'} / avoid
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rec.daily.map((d) => (
              <tr key={d.day}>
                <td className="stocks-table__name">{dayLabel(d.day)}</td>
                <td>{d.picked}</td>
                {horizons.map((x, i) => {
                  const p = d.picksRet[i];
                  const b = d.benchRet[i];
                  if (p === null && b === null) {
                    return (
                      <td key={x}>
                        <span className="stocks__dim">pending</span>
                      </td>
                    );
                  }
                  const won = p !== null && b !== null && p > b;
                  return (
                    <td key={x} className={won ? 'report-days__won' : undefined}>
                      <span className={tone(p)}>{signed(p)}</span>
                      <span className="stocks__dim"> / {signed(b)} / </span>
                      <span className={tone(d.avoidRet[i])}>{signed(d.avoidRet[i])}</span>
                    </td>
                  );
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {l && (
        <>
          <h3 className="track__sub">What each factor has been worth</h3>
          <p className="stocks__rule">
            Skill is how well a factor’s scores ranked stocks by their next 20 days’ return beyond {rec.benchmark}, from
            -1 to 1 (0 is no better than chance). Weights move once {l.needed} days have results.
          </p>
          <ul className="track-skill">
            {l.factors.map((f) => (
              <li key={f.name}>
                <span>{f.name}</span>
                <span className={tone(f.skill)}>skill {f.skill === null ? 'n/a' : f.skill.toFixed(2)}</span>
                <span className="stocks__dim">
                  weight {f.baseWeight.toFixed(0)}
                  {Math.abs(f.weight - f.baseWeight) >= 0.05 && ` → ${f.weight.toFixed(1)}`}
                </span>
              </li>
            ))}
          </ul>
        </>
      )}

      {rec.recent.length > 0 && (
        <>
          <h3 className="track__sub">{rec.kind === 'mine' ? 'Every buy' : rec.kind === 'news' ? 'Every call' : 'Every positive call'}</h3>
          <div className="stocks-table__wrap">
            <table className="stocks-table">
              <thead>
                <tr>
                  <th scope="col" className="stocks-table__name">
                    Day
                  </th>
                  <th scope="col" className="stocks-table__name">
                    {rec.kind === 'fund' ? 'Fund code' : 'Stock'}
                  </th>
                  <th scope="col">Score</th>
                  <th scope="col">{rec.kind === 'fund' ? 'NAV then' : 'Price then'}</th>
                  {horizons.map((x) => (
                    <th scope="col" key={x}>
                      {x} days
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {rec.recent.map((p) => (
                  <tr key={`${p.day}-${p.symbol}`}>
                    <td className="stocks-table__name">{dayLabel(p.day)}</td>
                    <td className="stocks-table__name">
                      <span className="stocks-table__symbol">{p.symbol}</span>
                      <span className="stocks-table__long">{p.verdict}</span>
                    </td>
                    <td>{Math.round(p.score)}</td>
                    <td>₹{inr.format(p.price)}</td>
                    {horizons.map((x, i) => (
                      <td key={x}>
                        {p.ret[i] === null ? (
                          <span className="stocks__dim">pending</span>
                        ) : (
                          <>
                            <span className={tone(p.ret[i])}>{signed(p.ret[i])}</span>
                            {p.excess[i] !== null && (
                              <span className="track-table__sub">
                                {rec.kind === 'news'
                                  ? `${p.excess[i]! > 0 ? 'right' : 'wrong'} by ${Math.abs(p.excess[i]!).toFixed(1)} pts`
                                  : `${p.excess[i]! > 0 ? 'beat' : 'trailed'} by ${Math.abs(p.excess[i]!).toFixed(1)} pts`}
                              </span>
                            )}
                          </>
                        )}
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      )}
    </div>
  );
}

/**
 * Report: every suggestion NEXUS has made against what actually happened
 * (Nifty 50 calls, the three divisions, funds, news-radar calls), the
 * investor's own buys graded the same way, and what each stock score has
 * learned from it.
 */
export function ReportView({ refreshSeq }: { refreshSeq: number }) {
  const [data, setData] = useState<TrackRecord | null>(null);
  const [status, setStatus] = useState<string | null>('Checking how past suggestions turned out…');
  const [kind, setKind] = useState<PickKind>('mine');

  useEffect(() => {
    let alive = true;
    setStatus('Checking how past suggestions turned out…');
    pickTrackRecord()
      .then((r) => {
        if (!alive) return;
        setData(r);
        setStatus(null);
      })
      .catch((err) => alive && setStatus(`Could not build the report: ${errText(err)}`));
    return () => {
      alive = false;
    };
  }, [refreshSeq]);

  if (!data) return <p className="stocks__note">{status}</p>;
  const rec = data.kinds.find((k) => k.kind === kind);
  const meta = KINDS.find((k) => k.kind === kind)!;

  return (
    <div className="picks">
      {status && <p className="stocks__status">{status}</p>}
      <nav className="stocks-tabs" aria-label="Report sections">
        {KINDS.filter((k) => data.kinds.some((x) => x.kind === k.kind)).map((k) => {
          const r = data.kinds.find((x) => x.kind === k.kind);
          return (
            <button
              key={k.kind}
              type="button"
              className={`stocks-tab${k.kind === kind ? ' stocks-tab--on' : ''}`}
              aria-pressed={k.kind === kind}
              onClick={() => setKind(k.kind)}
            >
              {k.label}
              <span className="stocks-tab__count">{r?.days ?? 0}d</span>
            </button>
          );
        })}
      </nav>
      {rec && <KindReport rec={rec} horizons={data.horizons} picksLabel={meta.picks} where={meta.where} />}
    </div>
  );
}
