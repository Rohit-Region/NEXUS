import { useCallback, useEffect, useRef, useState } from 'react';
import { ExternalLink, X } from 'lucide-react';
import {
  addIpoAllotment,
  ipoAllotments,
  ipoDesk,
  openUrl,
  removeIpoAllotment,
  setIpoSymbol,
  type Ipo,
  type IpoAllotment,
  type IpoDesk,
  type IpoVerdict,
} from '../../lib/nexus-db';
import { errText, inr, signed, tone, whole } from './Picks';

/** Each verdict borrows the colour of its nearest stock verdict. */
const VERDICT: Record<IpoVerdict, { label: string; cls: string }> = {
  apply: { label: 'Apply', cls: 'buy' },
  consider: { label: 'Consider', cls: 'accumulate' },
  watch: { label: 'Watch', cls: 'hold' },
  skip: { label: 'Skip', cls: 'avoid' },
};

function day(iso: string): string {
  if (!iso) return '?';
  return new Date(`${iso}T00:00:00`).toLocaleDateString('en-IN', { day: 'numeric', month: 'short' });
}

function ago(unix: number): string {
  const m = Math.max(1, Math.floor((Date.now() / 1000 - unix) / 60));
  return m < 60 ? `${m} min ago` : `${Math.floor(m / 60)} h ago`;
}

function IpoCard({ ipo }: { ipo: Ipo }) {
  const v = VERDICT[ipo.verdict];
  const band =
    ipo.priceHigh === null
      ? 'price not announced'
      : ipo.priceLow !== null && ipo.priceLow !== ipo.priceHigh
        ? `₹${whole.format(ipo.priceLow)}–${whole.format(ipo.priceHigh)}`
        : `₹${whole.format(ipo.priceHigh)}`;
  return (
    <article className="radar-event">
      <header className="radar-event__head">
        <span className={`pick-verdict pick-verdict--${v.cls}`}>{v.label}</span>
        <h3 className="radar-event__title">{ipo.name}</h3>
        <span className="stocks__dim">
          {ipo.status === 'open' ? 'Open' : 'Opens'} {day(ipo.openDate)} to {day(ipo.closeDate)}
          {ipo.listingDate && ` · lists ${day(ipo.listingDate)}`}
        </span>
      </header>
      <p className="ipo__facts">
        <span>{band}</span>
        {ipo.lotSize !== null && ipo.priceHigh !== null && (
          <span className="stocks__dim">
            lot {ipo.lotSize} = ₹{whole.format(ipo.lotSize * ipo.priceHigh)}
          </span>
        )}
        {ipo.issueSizeCr !== null && <span className="stocks__dim">₹{whole.format(ipo.issueSizeCr)} cr issue</span>}
        <span>
          GMP {ipo.gmp === null ? '–' : `₹${whole.format(ipo.gmp)}`}{' '}
          <span className={tone(ipo.gmpPct)}>{ipo.gmpPct === null ? '' : `(${signed(ipo.gmpPct, 0)})`}</span>
        </span>
        <span>Subscribed {ipo.subscribed === null ? '–' : `${ipo.subscribed.toFixed(1)}x`}</span>
        <span>QIB {ipo.qib === null ? '–' : `${ipo.qib.toFixed(1)}x`}</span>
      </p>
      <p className="radar-event__text">
        <strong>Why: </strong>
        {ipo.why}
      </p>
      {ipo.business && <p className="radar-event__text stocks__dim">{ipo.business}</p>}
      {ipo.sources.length > 0 && (
        <p className="stocks__dim radar-event__meta">
          {ipo.sources.map((s) => (
            <button key={s} type="button" className="pick-news__link" onClick={() => void openUrl(s)}>
              {host(s)} <ExternalLink size={10} strokeWidth={2} aria-hidden="true" />{' '}
            </button>
          ))}
        </p>
      )}
    </article>
  );
}

function host(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, '');
  } catch {
    return url;
  }
}

/** The NSE symbol for an allotment added before it listed. */
function SymbolField({ name, onSave }: { name: string; onSave: (symbol: string) => Promise<void> }) {
  const [value, setValue] = useState('');
  return (
    <form
      className="ipo-allot__symbol"
      onSubmit={(e) => {
        e.preventDefault();
        if (value.trim()) void onSave(value);
      }}
    >
      <label className="fund-budget__input">
        <input
          placeholder="NSE symbol"
          aria-label={`NSE symbol for ${name}`}
          value={value}
          onChange={(e) => setValue(e.target.value)}
        />
      </label>
    </form>
  );
}

/** Your allotments: what you paid, what it trades at now, and the gain. */
function Allotments() {
  const [rows, setRows] = useState<IpoAllotment[] | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [form, setForm] = useState({ name: '', symbol: '', shares: '', price: '' });

  const load = useCallback(async () => {
    try {
      setRows(await ipoAllotments());
      setStatus(null);
    } catch (err) {
      setStatus(`Could not load your allotments: ${errText(err)}`);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const act = async (work: Promise<void>) => {
    try {
      await work;
      await load();
    } catch (err) {
      setStatus(errText(err));
    }
  };

  const add = async () => {
    await act(addIpoAllotment(form.name, form.symbol, Number(form.shares), Number(form.price)));
    setForm({ name: '', symbol: '', shares: '', price: '' });
  };

  const total = rows?.reduce((sum, r) => sum + (r.gain ?? 0), 0) ?? 0;

  return (
    <section className="ipo-allot">
      <h3 className="track__sub">My allotments</h3>
      {status && <p className="stocks__status">{status}</p>}
      {rows && rows.length > 0 && (
        <table className="track-table">
          <thead>
            <tr>
              <th scope="col">IPO</th>
              <th scope="col">Shares</th>
              <th scope="col">Issue price</th>
              <th scope="col">Now</th>
              <th scope="col">Gain</th>
              <th scope="col" aria-label="Remove" />
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => (
              <tr key={r.id}>
                <td>
                  {r.name}
                  {r.symbol ? (
                    <span className="stocks__dim"> · {r.symbol}</span>
                  ) : (
                    <SymbolField name={r.name} onSave={(symbol) => act(setIpoSymbol(r.id, symbol))} />
                  )}
                </td>
                <td>{r.shares}</td>
                <td>₹{inr.format(r.issuePrice)}</td>
                <td>
                  {r.price === null ? <span className="stocks__dim">{r.note ?? '–'}</span> : `₹${inr.format(r.price)}`}
                </td>
                <td className={tone(r.gain)}>
                  {r.gain === null ? '–' : `₹${inr.format(r.gain)} (${signed(r.gainPct)})`}
                </td>
                <td>
                  <button
                    type="button"
                    className="pick-news__link"
                    aria-label={`Remove ${r.name}`}
                    onClick={() => void act(removeIpoAllotment(r.id))}
                  >
                    <X size={12} strokeWidth={2} aria-hidden="true" />
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {rows?.some((r) => r.gain !== null) && (
        <p className="stocks__foot">
          Total listing gain: <span className={tone(total)}>₹{inr.format(total)}</span>
        </p>
      )}
      <form
        className="ipo-allot__form"
        onSubmit={(e) => {
          e.preventDefault();
          void add();
        }}
      >
        <label className="fund-budget__input">
          <input
            required
            placeholder="IPO name"
            aria-label="IPO name"
            value={form.name}
            onChange={(e) => setForm({ ...form, name: e.target.value })}
          />
        </label>
        <label className="fund-budget__input">
          <input
            placeholder="NSE symbol (after listing)"
            aria-label="NSE symbol"
            value={form.symbol}
            onChange={(e) => setForm({ ...form, symbol: e.target.value })}
          />
        </label>
        <label className="fund-budget__input">
          <input
            required
            type="number"
            min="1"
            step="1"
            placeholder="Shares"
            aria-label="Shares allotted"
            value={form.shares}
            onChange={(e) => setForm({ ...form, shares: e.target.value })}
          />
        </label>
        <label className="fund-budget__input">
          ₹
          <input
            required
            type="number"
            min="0.01"
            step="0.01"
            placeholder="Issue price"
            aria-label="Issue price"
            value={form.price}
            onChange={(e) => setForm({ ...form, price: e.target.value })}
          />
        </label>
        <button className="stocks-tab stocks-tab--advice" type="submit">
          Add allotment
        </button>
      </form>
    </section>
  );
}

/**
 * IPOs: open and upcoming mainboard issues researched by Claude on the web,
 * each judged by one stated rule over its GMP and institutional demand, plus
 * the investor's own allotments priced against the market.
 */
export function IposView({ refreshSeq }: { refreshSeq: number }) {
  const [data, setData] = useState<IpoDesk | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const seen = useRef(refreshSeq);

  const load = useCallback(async (refresh: boolean) => {
    setBusy(true);
    setStatus(
      refresh
        ? 'Claude is searching the web for open and upcoming IPOs. This takes two to four minutes…'
        : 'Loading IPOs (a fresh search takes two to four minutes)…',
    );
    try {
      setData(await ipoDesk(refresh));
      setStatus(null);
    } catch (err) {
      setStatus(`Could not research IPOs: ${errText(err)}`);
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
      <Allotments />

      <div className="radar-head">
        <p className="stocks__rule">
          {data?.rule ?? 'Claude researches open and upcoming mainboard IPOs; NEXUS judges each by its GMP and institutional demand.'}
          {data && ` Researched ${ago(data.generatedAt)}.`} GMP is an unofficial grey-market number and can change
          before listing.
        </p>
        <button className="stocks-tab stocks-tab--advice" type="button" disabled={busy} onClick={() => void load(true)}>
          Research again
        </button>
      </div>
      {status && <p className="stocks__status">{status}</p>}

      {data &&
        (data.ipos.length === 0 ? (
          <p className="stocks__note">No mainboard IPOs are open or announced for the next two weeks.</p>
        ) : (
          data.ipos.map((ipo) => <IpoCard key={ipo.name} ipo={ipo} />)
        ))}
    </div>
  );
}
