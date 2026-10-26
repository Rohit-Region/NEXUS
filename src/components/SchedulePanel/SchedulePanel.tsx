import { useCallback, useEffect, useState } from 'react';
import { CalendarClock, RefreshCw } from 'lucide-react';
import { runAction, isActionError, describeActionError } from '../../lib/assistant';
import { requestCalendarAccess } from '../../lib/nexus-db';
import './SchedulePanel.css';

/** Whether an error is the macOS calendar-permission one, which has a button
 *  remedy rather than just words. Matched on the connector's own phrasing. */
function isPermissionError(message: string | null): boolean {
  if (!message) return false;
  const m = message.toLowerCase();
  return m.includes('calendar') && (m.includes('asking') || m.includes('allow') || m.includes('not allowed') || m.includes('permission'));
}

interface RawEvent {
  subject: string;
  /** Local wall clock, "HH:MM". */
  start: string;
  endsAt?: string;
}

interface Slot {
  subject: string;
  /** Minutes past local midnight. */
  from: number;
  to: number;
}

/** How often to re-ask. A day's shape changes, but not by the second. */
const REFRESH_MS = 120_000;

/** The strip's span. Outside these hours a working calendar is mostly empty,
 *  and a 24-hour strip spends most of its width saying nothing. */
const DAY_FROM = 7 * 60;
const DAY_TO = 21 * 60;

function minutesOf(clock: string): number | null {
  const m = /^(\d{1,2}):(\d{2})$/.exec(clock.trim());
  if (!m) return null;
  const h = Number(m[1]);
  const min = Number(m[2]);
  if (h > 23 || min > 59) return null;
  return h * 60 + min;
}

function hhmm(minutes: number): string {
  const h = Math.floor(minutes / 60) % 24;
  const m = minutes % 60;
  return `${String(h).padStart(2, '0')}:${String(m).padStart(2, '0')}`;
}

/** Where a minute sits on the strip, clamped so a 06:00 start still shows. */
function place(minute: number): number {
  const span = DAY_TO - DAY_FROM;
  return Math.max(0, Math.min(100, ((minute - DAY_FROM) / span) * 100));
}

/**
 * Today, as a shape and as a list.
 *
 * The connector answers from Microsoft Graph when signed in and from this
 * Mac's own calendar when not, in the same shape either way. The second is
 * how this works at all in a tenant whose administrator has not consented to
 * the app registration.
 *
 * A failure is shown as a failure. It is never rendered as an empty day,
 * because "nothing on today" is a claim about your calendar and a refused
 * permission is not entitled to make it.
 */
export function SchedulePanel() {
  const [slots, setSlots] = useState<Slot[]>([]);
  const [source, setSource] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [everLoaded, setEverLoaded] = useState(false);
  const [now, setNow] = useState(() => {
    const d = new Date();
    return d.getHours() * 60 + d.getMinutes();
  });

  const load = useCallback(async () => {
    setBusy(true);
    try {
      const outcome = await runAction({
        actionId: 'outlook.today_schedule',
        input: {},
      });
      const out = outcome.output as { events?: RawEvent[]; source?: string };
      const parsed: Slot[] = (out.events ?? [])
        .map((e) => {
          const from = minutesOf(e.start);
          if (from === null) return null;
          const to = e.endsAt ? minutesOf(e.endsAt) : null;
          return {
            subject: e.subject,
            from,
            // No end, or an end before the start (a meeting over midnight):
            // an hour is a better guess than a zero-width block.
            to: to !== null && to > from ? to : from + 60,
          };
        })
        .filter((s): s is Slot => s !== null)
        .sort((a, b) => a.from - b.from);

      setSlots(parsed);
      setSource(out.source ?? null);
      setError(null);
    } catch (err) {
      setError(isActionError(err) ? describeActionError(err) : String(err));
    } finally {
      setBusy(false);
      setEverLoaded(true);
    }
  }, []);

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(), REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [load]);

  // The marker has to move, but only as fast as it means anything.
  useEffect(() => {
    const timer = window.setInterval(() => {
      const d = new Date();
      setNow(d.getHours() * 60 + d.getMinutes());
    }, 30_000);
    return () => window.clearInterval(timer);
  }, []);

  const current = slots.find((s) => now >= s.from && now < s.to) ?? null;
  const next = slots.find((s) => s.from > now) ?? null;

  // Hidden only until the first load has happened, so no empty frame flashes
  // on boot. After that the card stays, because "the calendar is just gone"
  // was more confusing than a card that says the day is clear. A clear day
  // reads "Nothing scheduled", a permission problem shows its button, and a
  // day with meetings shows them.
  if (!everLoaded && slots.length === 0 && !error) {
    return null;
  }

  return (
    <section className="sched" aria-label="Today's schedule">
      <div className="sched__head">
        <CalendarClock size={12} strokeWidth={2} aria-hidden="true" />
        <span className="sched__title">Schedule</span>
        {slots.length > 0 && (
          <span className="sched__count">
            {slots.length} today
          </span>
        )}
        <button
          className={`sched__refresh${busy ? ' sched__refresh--busy' : ''}`}
          type="button"
          onClick={() => void load()}
          disabled={busy}
          aria-label="Refresh schedule"
        >
          <RefreshCw size={12} strokeWidth={2} aria-hidden="true" />
        </button>
      </div>

      {error && isPermissionError(error) && (
        <div className="sched__permission" role="alert">
          <p className="sched__error">NEXUS needs permission to read your calendar.</p>
          <button
            className="sched__grant"
            type="button"
            onClick={() => {
              // Raises the macOS prompt, then re-reads once it is answered.
              void requestCalendarAccess()
                .catch(() => {})
                .finally(() => window.setTimeout(() => void load(), 1500));
            }}
          >
            Allow calendar access
          </button>
        </div>
      )}

      {error && !isPermissionError(error) && (
        <p className="sched__error" role="alert">
          {error}
        </p>
      )}

      {!error && slots.length === 0 && everLoaded && (
        <p className="sched__clear">Nothing scheduled today.</p>
      )}

      {!error && slots.length > 0 && (
        <>
          {/* The day as a shape. Where the gaps are reads faster here than
              from any list of times. */}
          <div
            className="sched__strip"
            role="img"
            aria-label={`${slots.length} meetings between ${hhmm(DAY_FROM)} and ${hhmm(DAY_TO)}`}
          >
            {slots.map((s, i) => (
              <span
                key={`${s.from}-${i}`}
                className={`sched__block${current === s ? ' sched__block--now' : ''}`}
                style={{
                  left: `${place(s.from)}%`,
                  // A floor, so a fifteen-minute stand-up is still visible.
                  width: `${Math.max(1.2, place(s.to) - place(s.from))}%`,
                }}
                title={`${hhmm(s.from)}–${hhmm(s.to)} ${s.subject}`}
              />
            ))}
            {now >= DAY_FROM && now <= DAY_TO && (
              <span className="sched__now" style={{ left: `${place(now)}%` }} />
            )}
          </div>

          <div className="sched__scale" aria-hidden="true">
            <span>{hhmm(DAY_FROM)}</span>
            <span>{hhmm((DAY_FROM + DAY_TO) / 2)}</span>
            <span>{hhmm(DAY_TO)}</span>
          </div>

          <ul className="sched__list">
            {slots.map((s, i) => {
              const state =
                current === s ? 'now' : next === s ? 'next' : s.to <= now ? 'past' : 'later';
              return (
                <li className={`sched__row sched__row--${state}`} key={`${s.from}-${i}`}>
                  <span className="sched__when">
                    {hhmm(s.from)}
                    <span className="sched__dash">–</span>
                    {hhmm(s.to)}
                  </span>
                  <span className="sched__what">{s.subject}</span>
                  {state === 'now' && <span className="sched__tag">now</span>}
                  {state === 'next' && (
                    <span className="sched__tag sched__tag--next">
                      in {s.from - now}m
                    </span>
                  )}
                </li>
              );
            })}
          </ul>

          {/* Which calendar answered. Without this, a meeting that is in
              Outlook on the web but missing here has no explanation. */}
          {source && <p className="sched__source">{source}</p>}
        </>
      )}
    </section>
  );
}
