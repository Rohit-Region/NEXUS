import { useCallback, useEffect, useRef, useState } from 'react';
import { PackageSearch, Plus, RefreshCw, X } from 'lucide-react';
import {
  checkStock,
  desktopNotification,
  ntfySendAlert,
  ntfyTopic,
  setKeepAwake,
  setNtfyTopic,
  setWhatsAppAlertNumber,
  voiceSay,
  whatsAppAlertNumber,
  whatsAppSendAlert,
} from '../../lib/nexus-db';
import './StockWatchPanel.css';

/** How often each watch is re-checked while NEXUS is open. 20 minutes is fast
 *  enough to catch a restock and slow enough not to get the IP throttled. */
const CHECK_MS = 20 * 60 * 1000;

/** Spread the first check out a little so several watches do not all hit at
 *  once on mount. */
const STAGGER_MS = 4000;

type Availability = 'available' | 'unavailable' | 'unknown';

interface Watch {
  id: string;
  url: string;
  /** Short name, from the page when a check has found one, else the host. */
  label: string;
  state: Availability;
  /** Unix ms of the last check, for "checked Nm ago". */
  checkedAt: number | null;
  /** Whether a restock has already been announced for the current available
   *  run, so the alert fires once per transition, not every 20 minutes. */
  announced: boolean;
}

const STORE_KEY = 'nexus.stockWatches';

/** Whether the keep-awake toggle was last left on. Persisted per-device, so a
 *  restart re-applies it: the toggle drives a `caffeinate` process, which dies
 *  with NEXUS, so without this it would silently turn itself off every launch. */
const KEEP_AWAKE_KEY = 'nexus.keepAwake';

function load(): Watch[] {
  try {
    const raw = localStorage.getItem(STORE_KEY);
    return raw ? (JSON.parse(raw) as Watch[]) : [];
  } catch {
    return [];
  }
}

function save(watches: Watch[]) {
  try {
    localStorage.setItem(STORE_KEY, JSON.stringify(watches));
  } catch {
    // A watch list that cannot be persisted still works for this session.
  }
}

/** A readable name for a URL before any check has named the product. */
function hostLabel(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, '');
  } catch {
    return url;
  }
}

function agoOf(ms: number | null): string {
  if (ms === null) return 'not yet';
  const mins = Math.floor((Date.now() - ms) / 60000);
  if (mins < 1) return 'just now';
  if (mins < 60) return `${mins}m ago`;
  const h = Math.floor(mins / 60);
  return `${h}h ago`;
}

/**
 * Watch a product page and shout when it comes back in stock.
 *
 * The check runs in Rust (a plain fetch of the page, availability read from
 * the buy box), on a 20-minute timer per watch, and only while NEXUS is open:
 * this is a local app, not a service, so a closed app checks nothing. A watch
 * surviving a restart is why the list lives in localStorage.
 *
 * On the one transition that matters, out of stock to in stock, it fans the
 * alert out across every channel the user asked for: a macOS banner, a spoken
 * line, a lit row here, and WhatsApp opened to their own number with the link
 * ready to send. The phone could not be made to ring; placing a real call
 * needs a telephony service this app does not have.
 */
export function StockWatchPanel() {
  const [watches, setWatches] = useState<Watch[]>(() => load());
  const [draft, setDraft] = useState('');
  // The alert number is stored in the database now, not localStorage, because
  // the Rust side reads it to decide where an unattended send may go. This
  // holds the editable copy; it is loaded on mount and saved on blur.
  const [waNumber, setWaNumber] = useState('');
  // The ntfy topic, likewise stored server-side so Rust reads it to decide
  // where a push goes. This is the free, phone-ringing channel: a POST to
  // ntfy.sh fires a loud notification on any device subscribed to the topic.
  const [ntfy, setNtfy] = useState('');
  // Whether NEXUS is holding the Mac awake so watches keep running while the
  // user is away from the laptop.
  const [keepAwake, setKeepAwakeState] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);
  // The last alert outcome, shown on the card. Mostly the WhatsApp result,
  // because that is the channel with the most ways to fail quietly.
  const [status, setStatus] = useState<string | null>(null);

  // The checker reads and writes the list on a timer, so it works off a ref to
  // avoid restarting the interval every time a check updates a row.
  const watchesRef = useRef(watches);
  watchesRef.current = watches;

  const persist = useCallback((next: Watch[]) => {
    // Update the ref synchronously, not only on the next render, so a check
    // fired right after a change (adding a watch) sees the new list rather
    // than the stale one and silently no-ops.
    watchesRef.current = next;
    setWatches(next);
    save(next);
  }, []);

  // Pull the saved alert number and topic once, so the fields show what Rust
  // will use.
  useEffect(() => {
    void whatsAppAlertNumber()
      .then((n) => setWaNumber(n ?? ''))
      .catch(() => {});
    void ntfyTopic()
      .then((t) => setNtfy(t ?? ''))
      .catch(() => {});
  }, []);

  // Re-apply the keep-awake preference on mount. The caffeinate process does
  // not survive a restart, so if it was left on, start it again. Idempotent on
  // the Rust side, so a webview reload (which re-runs this) does not stack a
  // second one.
  useEffect(() => {
    let on = false;
    try {
      on = localStorage.getItem(KEEP_AWAKE_KEY) === '1';
    } catch {
      on = false;
    }
    setKeepAwakeState(on);
    if (on) void setKeepAwake(true).catch(() => {});
  }, []);

  /** Turn background watching on or off and remember the choice. */
  const toggleKeepAwake = useCallback((next: boolean) => {
    setKeepAwakeState(next);
    try {
      localStorage.setItem(KEEP_AWAKE_KEY, next ? '1' : '0');
    } catch {
      // A preference that cannot be saved still applies for this session.
    }
    void setKeepAwake(next)
      .then(() =>
        setStatus(
          next
            ? 'Background watching on. NEXUS will keep the Mac awake (lid up) so checks keep running.'
            : 'Background watching off. The Mac can sleep again.',
        ),
      )
      .catch((err) => {
        // Roll the switch back if the process would not start.
        setKeepAwakeState(!next);
        setStatus(`Could not change background watching: ${String(err)}`);
      });
  }, []);

  /**
   * Fire the WhatsApp alert and return what happened in words.
   *
   * The restock path used to swallow this error, so a send that failed
   * because no number was saved, or Accessibility was off, or WhatsApp could
   * not come to the front, looked identical to a send that worked. Returning
   * the reason is what makes the Test button able to say which link is broken.
   */
  const sendWhatsApp = useCallback(async (message: string): Promise<string> => {
    try {
      await whatsAppSendAlert(message);
      return 'WhatsApp sent.';
    } catch (err) {
      return `WhatsApp did not send: ${String(err).replace(/^Error:\s*/, '')}`;
    }
  }, []);

  /**
   * Fire the ntfy push and return what happened in words.
   *
   * This is the loud-phone-alert channel. A plain HTTPS POST, so it has none of
   * WhatsApp's focus and Accessibility failure modes: if the topic is set and
   * the phone has data, it rings. Returning the reason, like the WhatsApp
   * helper, is what lets the Test button say which link is broken.
   */
  const sendNtfy = useCallback(async (message: string): Promise<string> => {
    try {
      await ntfySendAlert('Back in stock', message);
      return 'Phone alerted.';
    } catch (err) {
      return `Phone not alerted: ${String(err).replace(/^Error:\s*/, '')}`;
    }
  }, []);

  /** Everything that happens the moment a watch flips to in stock. Returns
   *  whether the WhatsApp send landed, so the caller can decide whether the
   *  alert is done or should be retried. */
  const announce = useCallback(
    async (w: Watch): Promise<boolean> => {
      const name = w.label;
      const line = `${name} is back in stock on Amazon.`;
      const full = `${name} is back in stock: ${w.url}`;
      // Each channel is independent: one failing must not stop the others.
      void desktopNotification('Back in stock', line).catch(() => {});
      void voiceSay(line).catch(() => {});
      // Fire both remote channels at once; neither waits on the other.
      const [waResult, ntfyResult] = await Promise.all([
        sendWhatsApp(full),
        sendNtfy(full),
      ]);
      // Surface both outcomes so a silent failure on either is no longer silent.
      setStatus(`${waResult} ${ntfyResult}`);
      // The alert "landed" if either channel got through: the latch should hold
      // so a working ntfy push is not undone by a flaky WhatsApp send, and
      // retried only when both failed.
      return waResult.startsWith('WhatsApp sent') || ntfyResult.startsWith('Phone alerted');
    },
    [sendWhatsApp, sendNtfy],
  );

  /** Fire all three channels on demand, to prove they work without waiting for
   *  a real restock. The WhatsApp outcome is shown, not swallowed. */
  const testAlert = useCallback(async () => {
    setStatus('Testing...');
    void desktopNotification('NEXUS test', 'Stock alert test. All channels firing.').catch(
      () => {},
    );
    void voiceSay('This is a NEXUS stock alert test.').catch(() => {});
    const message = 'NEXUS test: this is what a restock alert looks like.';
    const [waResult, ntfyResult] = await Promise.all([
      sendWhatsApp(message),
      sendNtfy(message),
    ]);
    setStatus(`${waResult} ${ntfyResult}`);
  }, [sendWhatsApp, sendNtfy]);

  // One channel at a time, so a working one is not hidden behind a failing one.
  // This is what tells apart "NEXUS did not send" from "the phone did not show
  // it": the result is the POST outcome, and the card says it in words.
  const testNtfyOnly = useCallback(async () => {
    setStatus('Testing phone (ntfy)...');
    const r = await sendNtfy('NEXUS test: phone alert only.');
    setStatus(r);
  }, [sendNtfy]);

  const testWaOnly = useCallback(async () => {
    setStatus('Testing WhatsApp...');
    const r = await sendWhatsApp('NEXUS test: WhatsApp alert only.');
    setStatus(r);
  }, [sendWhatsApp]);

  const checkOne = useCallback(
    // `force` is the refresh button: a person pressing it on an in-stock item
    // is asking to be told now, so it re-alerts even if the latch is set. The
    // timer passes force = false, so automatic checks still alert only once.
    async (id: string, force = false) => {
      const w = watchesRef.current.find((x) => x.id === id);
      if (!w) return;
      setBusyId(id);
      try {
        const result = await checkStock(w.url);
        const state: Availability =
          result.available === true
            ? 'available'
            : result.available === false
              ? 'unavailable'
              : 'unknown';

        // Alert when it is in stock and either this is a forced check or it
        // has not been alerted yet. Unknown (a bot wall) is not a transition.
        const shouldAlert = state === 'available' && (force || !w.announced);

        const updated: Watch = {
          ...w,
          state,
          label: result.title ?? w.label,
          checkedAt: Date.now(),
          // Reset the latch when it leaves stock, so the next restock alerts
          // again. Hold it while unknown, so a block does not clear it.
          announced:
            state === 'available'
              ? w.announced || shouldAlert
              : state === 'unavailable'
                ? false
                : w.announced,
        };
        persist(watchesRef.current.map((x) => (x.id === id ? updated : x)));

        if (shouldAlert) {
          const sent = await announce(updated);
          // If the WhatsApp send failed, clear the latch so the next check
          // tries again rather than going quiet after one failed attempt.
          if (!sent) {
            persist(
              watchesRef.current.map((x) =>
                x.id === id ? { ...x, announced: false } : x,
              ),
            );
          }
        } else if (state === 'available') {
          setStatus(`${updated.label} is in stock (already alerted).`);
        }
      } catch (err) {
        // A failed check is not a state change: note it was looked at, and
        // surface the reason so a silent fetch failure is visible.
        setStatus(`Check failed: ${String(err).replace(/^Error:\s*/, '')}`);
        persist(
          watchesRef.current.map((x) =>
            x.id === id ? { ...x, checkedAt: Date.now() } : x,
          ),
        );
      } finally {
        setBusyId(null);
      }
    },
    [announce, persist],
  );

  // One timer, checking whichever watches are due. A per-watch interval would
  // multiply as the list grows; this walks the list each minute and checks the
  // ones whose 20 minutes are up.
  useEffect(() => {
    let alive = true;
    const tick = () => {
      if (!alive) return;
      const now = Date.now();
      watchesRef.current.forEach((w, i) => {
        const due = w.checkedAt === null || now - w.checkedAt >= CHECK_MS;
        if (due) {
          // Stagger the mount-time burst so the fetches do not land together.
          window.setTimeout(() => void checkOne(w.id), i * STAGGER_MS);
        }
      });
    };
    tick();
    const timer = window.setInterval(tick, 60_000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [checkOne]);

  function add() {
    const url = draft.trim();
    if (!/^https?:\/\//.test(url)) return;
    const w: Watch = {
      id: `${Date.now()}`,
      url,
      label: hostLabel(url),
      state: 'unknown',
      checkedAt: null,
      announced: false,
    };
    const next = [...watches, w];
    persist(next);
    setDraft('');
    void checkOne(w.id);
  }

  function remove(id: string) {
    persist(watches.filter((w) => w.id !== id));
  }

  // Persisted server-side on blur rather than on every keystroke, and the
  // normalised form Rust validated is read back, so the field shows exactly
  // what an alert will send to (or clears if the number was rejected). The
  // outcome is shown, because a number silently rejected for bad format is
  // one reason an alert never sends.
  function commitWa() {
    const typed = waNumber.trim();
    void setWhatsAppAlertNumber(typed)
      .then((saved) => {
        setWaNumber(saved ?? '');
        if (typed && !saved) setStatus('That number was not a valid international format.');
        else if (saved) setStatus(`Alerts will send to ${saved}.`);
      })
      .catch((err) => setStatus(`Could not save the number: ${String(err)}`));
  }

  // Same shape for the ntfy topic: Rust validates it to the character set a
  // topic may have and hands back the stored value, so the field reflects
  // exactly what a push will use (or clears if the topic was rejected).
  function commitNtfy() {
    const typed = ntfy.trim();
    void setNtfyTopic(typed)
      .then((saved) => {
        setNtfy(saved ?? '');
        if (typed && !saved) setStatus('That topic is not valid (letters, numbers, - and _ only).');
        else if (saved) setStatus(`Phone alerts will post to ntfy.sh/${saved}.`);
      })
      .catch((err) => setStatus(`Could not save the topic: ${String(err)}`));
  }

  return (
    <section className="watch" aria-label="Stock watches">
      <div className="watch__head">
        <PackageSearch size={12} strokeWidth={2} aria-hidden="true" />
        <span className="watch__title">Stock Watch</span>
        {watches.length > 0 && <span className="watch__count">{watches.length}</span>}
        <button
          className="watch__test"
          type="button"
          onClick={() => void testAlert()}
          title="Fire all alert channels now, to prove they work"
        >
          Test alert
        </button>
      </div>

      {status && <p className="watch__status">{status}</p>}

      <div className="watch__add">
        <input
          className="watch__input"
          type="text"
          placeholder="Paste a product URL to watch..."
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') add();
          }}
          aria-label="Product URL to watch"
        />
        <button
          className="watch__add-btn"
          type="button"
          onClick={add}
          disabled={!/^https?:\/\//.test(draft.trim())}
          aria-label="Add watch"
        >
          <Plus size={14} strokeWidth={2.5} aria-hidden="true" />
        </button>
      </div>

      {watches.length === 0 && (
        <p className="watch__empty">
          Nothing watched. Paste a product link and NEXUS will check it every 20
          minutes while it is open.
        </p>
      )}

      <ul className="watch__list">
        {watches.map((w) => (
          <li className={`watch__row watch__row--${w.state}`} key={w.id}>
            <span className={`watch__dot watch__dot--${w.state}`} aria-hidden="true" />
            <a
              className="watch__name"
              href={w.url}
              target="_blank"
              rel="noreferrer"
              title={w.url}
            >
              {w.label}
            </a>
            <span className="watch__state">
              {w.state === 'available'
                ? 'in stock'
                : w.state === 'unavailable'
                  ? 'out of stock'
                  : "can't tell"}
            </span>
            <span className="watch__ago">{agoOf(w.checkedAt)}</span>
            <button
              className={`watch__recheck${busyId === w.id ? ' watch__recheck--busy' : ''}`}
              type="button"
              onClick={() => void checkOne(w.id, true)}
              disabled={busyId === w.id}
              aria-label={`Check ${w.label} now`}
            >
              <RefreshCw size={11} strokeWidth={2} aria-hidden="true" />
            </button>
            <button
              className="watch__remove"
              type="button"
              onClick={() => remove(w.id)}
              aria-label={`Stop watching ${w.label}`}
            >
              <X size={11} strokeWidth={2.5} aria-hidden="true" />
            </button>
          </li>
        ))}
      </ul>

      {/* Where an alert goes, set apart from the watch list because it is
          configured once and then left alone. Each channel is its own labelled
          block so WhatsApp and the phone push are not two unexplained boxes. */}
      <div className="watch__alerts">
        <span className="watch__alerts-label">Alerts</span>

        <div className="watch__channel">
          <span className="watch__channel-name">WhatsApp</span>
          <div className="watch__channel-row">
            <input
              className="watch__wa"
              type="text"
              placeholder="Number for auto-send, e.g. +9190..."
              value={waNumber}
              onChange={(e) => setWaNumber(e.target.value)}
              onBlur={commitWa}
              onKeyDown={(e) => {
                if (e.key === 'Enter') commitWa();
              }}
              aria-label="WhatsApp number for restock alerts"
            />
            <button
              className="watch__channel-test"
              type="button"
              onClick={() => void testWaOnly()}
              disabled={!waNumber.trim()}
              title="Send a test to WhatsApp only"
            >
              Test
            </button>
          </div>
        </div>

        <div className="watch__channel">
          <span className="watch__channel-name">Phone (ntfy)</span>
          <div className="watch__channel-row">
            <input
              className="watch__wa"
              type="text"
              placeholder="ntfy topic, e.g. rohit-stock-9f2a"
              value={ntfy}
              onChange={(e) => setNtfy(e.target.value)}
              onBlur={commitNtfy}
              onKeyDown={(e) => {
                if (e.key === 'Enter') commitNtfy();
              }}
              aria-label="ntfy topic for restock alerts"
            />
            <button
              className="watch__channel-test"
              type="button"
              onClick={() => void testNtfyOnly()}
              disabled={!ntfy.trim()}
              title="Send a test to your phone only"
            >
              Test
            </button>
          </div>
          <span className="watch__channel-hint">
            A loud notification on your phone when you're away from the Mac.
          </span>
        </div>

        {/* The reason the phone alert is worth setting up: checks only run
            while the Mac is awake. This holds it awake so they keep going. */}
        <label className="watch__toggle">
          <input
            type="checkbox"
            className="watch__toggle-box"
            checked={keepAwake}
            onChange={(e) => toggleKeepAwake(e.target.checked)}
          />
          <span className="watch__toggle-text">
            Keep watching when I'm away
            <span className="watch__toggle-hint">
              Holds the Mac awake so checks keep running with the lid up. It
              can't run with the lid closed.
            </span>
          </span>
        </label>
      </div>
    </section>
  );
}
