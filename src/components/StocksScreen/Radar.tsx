import { useCallback, useEffect, useRef, useState } from 'react';
import { ExternalLink } from 'lucide-react';
import { newsRadar, openUrl, type NewsRadar, type RadarEvent, type SectorFlow } from '../../lib/nexus-db';
import { errText, inr, PredictionBox, signed, tone } from './Picks';

function ago(unix: number): string {
  const s = Math.max(0, Math.floor(Date.now() / 1000) - unix);
  if (s < 3600) return `${Math.max(1, Math.floor(s / 60))} min ago`;
  return `${Math.floor(s / 3600)} h ago`;
}

function Flow({ f }: { f: SectorFlow }) {
  return (
    <li className="radar-flow">
      <span>{f.industry}</span>
      <span className={tone(f.ret1w)}>{signed(f.ret1w)} 1W</span>
      <span className="stocks__dim">
        {Math.round(f.breadth * 100)}% up · {f.volumeRatio.toFixed(1)}x vol
      </span>
    </li>
  );
}

function EventCard({ e, read }: { e: RadarEvent; read: NewsRadar['read'] }) {
  const [openStock, setOpenStock] = useState<string | null>(null);
  return (
    <article className="radar-event">
      <header className="radar-event__head">
        <span className={`pick-verdict pick-verdict--${e.direction === 'up' ? 'buy' : 'avoid'}`}>
          {e.direction === 'up' ? 'Bullish' : 'Bearish'}
        </span>
        <h3 className="radar-event__title">{e.title}</h3>
        <span className="stocks__dim">
          over {e.horizon} · {e.confidence} confidence
        </span>
      </header>
      <p className="radar-event__text">{e.summary}</p>
      <p className="radar-event__text">
        <strong>Why it matters here: </strong>
        {e.whyIndia}
      </p>
      {(e.sectors.length > 0 || e.sources.length > 0) && (
        <p className="stocks__dim radar-event__meta">
          {e.sectors.length > 0 && <>Sectors: {e.sectors.join(', ')}. </>}
          {e.sources.map((i) => (
            <button key={i} type="button" className="pick-news__link" onClick={() => void openUrl(read[i].link)}>
              {read[i].source} <ExternalLink size={10} strokeWidth={2} aria-hidden="true" />{' '}
            </button>
          ))}
        </p>
      )}
      <ul className="radar-stocks">
        {e.stocks.map((s) => {
          const open = openStock === s.symbol;
          return (
            <li key={s.symbol} className="radar-stock">
              <button
                type="button"
                className="radar-stock__row"
                aria-expanded={open}
                onClick={() => setOpenStock(open ? null : s.symbol)}
              >
                <span className={s.direction === 'up' ? 'stocks__up' : 'stocks__down'}>{s.direction === 'up' ? '▲' : '▼'}</span>
                <span className="pick__symbol">{s.symbol}</span>
                <span className="stocks__dim radar-stock__name">
                  {s.name}
                  {s.industry && ` · ${s.industry}`}
                </span>
                {s.held && <span className="pick-verdict pick-verdict--hold">You own</span>}
                <span>₹{inr.format(s.price)}</span>
                <span className={tone(s.ret1w)}>{signed(s.ret1w)} 1W</span>
                {s.prediction && (
                  <span className={tone(s.prediction.expected)}>
                    {signed(s.prediction.expected)} expected
                  </span>
                )}
              </button>
              <p className="radar-stock__why">
                {s.why}
                {!s.named && <span className="stocks__dim"> (added from the sector)</span>}
              </p>
              <p className="radar-stock__plan">{s.plan}</p>
              {open && (
                <div className="pick__detail">
                  {s.prediction && <PredictionBox p={s.prediction} price={s.price} />}
                  {s.fundamentals && (
                    <p className="pick__levels">
                      Revenue {signed(s.fundamentals.revenueGrowth, 0)} · profit {signed(s.fundamentals.profitGrowth, 0)} ·
                      ROE {s.fundamentals.roe?.toFixed(1) ?? '–'}% · P/E {s.fundamentals.pe?.toFixed(1) ?? '–'} vs industry{' '}
                      {s.fundamentals.industryPe?.toFixed(1) ?? '–'}
                    </p>
                  )}
                </div>
              )}
            </li>
          );
        })}
      </ul>
    </article>
  );
}

/**
 * News radar: today's market-moving news turned into the Indian stocks it is
 * likely to move, which way, how soon, and what to do about each. Every
 * call is graded in the Report.
 */
export function RadarView({ refreshSeq }: { refreshSeq: number }) {
  const [data, setData] = useState<NewsRadar | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const seen = useRef(refreshSeq);

  const load = useCallback(async (refresh: boolean) => {
    setBusy(true);
    setStatus(
      refresh
        ? 'Researching today’s news with Claude. This takes one to two minutes…'
        : 'Loading the news radar (a fresh run takes one to two minutes)…',
    );
    try {
      setData(await newsRadar(refresh));
      setStatus(null);
    } catch (err) {
      setStatus(`Could not build the radar: ${errText(err)}`);
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    const refresh = refreshSeq !== seen.current;
    seen.current = refreshSeq;
    void load(refresh);
  }, [load, refreshSeq]);

  return (
    <div className="picks">
      <div className="radar-head">
        <p className="stocks__rule">
          NEXUS reads the latest business, tech and world news, checks where money is moving across sectors, and asks{' '}
          {data?.provider ?? 'Claude'} which Indian stocks each story should move. Every named stock is checked against
          the real NSE lists, gets a plan from its own price action, and is graded later in the Report.
          {data && ` Researched ${ago(data.generatedAt)} from ${data.read.length} headlines.`}
        </p>
        <button className="stocks-tab stocks-tab--advice" type="button" disabled={busy} onClick={() => void load(true)}>
          Research again
        </button>
      </div>
      {status && <p className="stocks__status">{status}</p>}

      {data && (
        <>
          <div className="radar-flows">
            <div>
              <h3 className="track__sub">Money flowing in this week</h3>
              <ul>
                {data.inflows.map((f) => (
                  <Flow key={f.industry} f={f} />
                ))}
              </ul>
            </div>
            <div>
              <h3 className="track__sub">Flowing out</h3>
              <ul>
                {data.outflows.map((f) => (
                  <Flow key={f.industry} f={f} />
                ))}
              </ul>
            </div>
          </div>

          {data.events.length === 0 ? (
            <p className="stocks__note">Nothing in today’s news looks likely to move Indian stocks. That happens.</p>
          ) : (
            data.events.map((e) => <EventCard key={e.title} e={e} read={data.read} />)
          )}

          {data.feedback.length > 0 && (
            <p className="stocks__foot">Track record shown to the model: {data.feedback.join('; ')}.</p>
          )}
        </>
      )}
    </div>
  );
}
