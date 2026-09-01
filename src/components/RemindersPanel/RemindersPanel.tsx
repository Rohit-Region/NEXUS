import { useCallback, useEffect, useState } from 'react';
import { X } from 'lucide-react';
import { deleteCommitment, listCommitments } from '../../lib/assistant';
import type { Commitment } from '../../lib/assistant';
import { ReminderComposer } from './ReminderComposer';
import { CountdownRing } from './CountdownRing';
import './RemindersPanel.css';

/** How often the countdown redraws. A timer that does not tick is a label. */
const TICK_MS = 1000;
/** How often the list is re-read, to pick up ones NEXUS raised on its own. */
const REFRESH_MS = 8000;

type Phase = 'untimed' | 'scheduled' | 'imminent' | 'due' | 'raised';

/**
 * Where a reminder is in its life.
 *
 * `untimed` is the one worth showing loudly: a commitment with no due time
 * is never raised, and until it was visible the only way to find out was to
 * wait for a reminder that could not come.
 */
function phaseOf(c: Commitment, now: number): Phase {
  if (c.dueAt === null) return 'untimed';
  if (c.raisedAt !== null) return 'raised';
  const left = c.dueAt - now;
  if (left <= 0) return 'due';
  if (left <= 60) return 'imminent';
  return 'scheduled';
}

const PHASE_LABEL: Record<Phase, string> = {
  untimed: 'no timer',
  scheduled: 'armed',
  imminent: 'imminent',
  due: 'due',
  raised: 'raised',
};

/**
 * Time remaining, in the largest two units that still say something.
 *
 * Seconds are shown under an hour and dropped above it: "2h 14m" is what a
 * person wants, "2h 14m 09s" is a stopwatch pretending to be useful.
 */
function countdown(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;

  if (d > 0) return `${d}d ${h}h`;
  if (h > 0) return `${h}h ${String(m).padStart(2, '0')}m`;
  if (m > 0) return `${m}m ${String(sec).padStart(2, '0')}s`;
  return `${sec}s`;
}

/**
 * What NEXUS has been asked to remember, and when it will say so.
 *
 * Sits with recent activity because that is where the eye already goes for
 * "what is happening", and a reminder due in ninety seconds is the most
 * live thing on the screen.
 */
export function RemindersPanel() {
  const [items, setItems] = useState<Commitment[]>([]);
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setItems(await listCommitments(true));
      setError(null);
    } catch (err) {
      setError(String(err));
    }
  }, []);

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(), REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [load]);

  // Separate from the reload: the clock has to move every second, and
  // re-reading the database at that rate to redraw a number would be
  // absurd.
  useEffect(() => {
    const timer = window.setInterval(
      () => setNow(Math.floor(Date.now() / 1000)),
      TICK_MS,
    );
    return () => window.clearInterval(timer);
  }, []);

  async function dismiss(id: number) {
    try {
      await deleteCommitment(id);
      setItems((prev) => prev.filter((c) => c.id !== id));
    } catch (err) {
      setError(String(err));
    }
  }

  // The composer keeps the section on screen even with nothing pending: a
  // panel that vanishes when empty is one you cannot use to add the first
  // thing to it.

  // Soonest first, and the ones that can never fire last: they are a
  // problem to fix rather than something to wait for.
  const ordered = [...items].sort((a, b) => {
    if (a.dueAt === null) return 1;
    if (b.dueAt === null) return -1;
    return a.dueAt - b.dueAt;
  });

  return (
    <div className="overview__section">
      <span className="overview__section-title">Reminders</span>

      {error && (
        <p className="reminders__error" role="alert">
          {error}
        </p>
      )}

      <ReminderComposer
        onCreated={(made) => setItems((prev) => [...prev, made])}
      />

      {ordered.length === 0 && (
        <p className="reminders__empty">Nothing pending.</p>
      )}

      <div className="reminders">
        {ordered.map((c) => {
          const phase = phaseOf(c, now);
          const left = c.dueAt === null ? 0 : c.dueAt - now;
          // How long it was set for, so the ring has a span to empty over.
          // Falls back to an hour when the row predates a created_at we can
          // read, which only affects how full the ring starts.
          const span =
            c.dueAt === null
              ? 3600
              : Math.max(60, c.dueAt - Math.floor(Date.parse(c.createdAt) / 1000));
          return (
            <div className={`reminders__row reminders__row--${phase}`} key={c.id}>
              <span className={`reminders__pill reminders__pill--${phase}`}>
                {PHASE_LABEL[phase]}
              </span>

              <span className="reminders__what">{c.what}</span>

              {phase !== 'untimed' && phase !== 'raised' && (
                <CountdownRing
                  left={left}
                  span={span}
                  imminent={phase === 'imminent' || phase === 'due'}
                />
              )}

              <span className="reminders__timer" aria-live="off">
                {phase === 'untimed' ? (
                  <span className="reminders__never">will not fire</span>
                ) : phase === 'raised' ? (
                  'told you'
                ) : phase === 'due' ? (
                  `overdue ${countdown(-left)}`
                ) : (
                  countdown(left)
                )}
              </span>

              <button
                className="reminders__dismiss"
                type="button"
                onClick={() => void dismiss(c.id)}
                aria-label={`Dismiss reminder: ${c.what}`}
              >
                <X size={12} strokeWidth={2.5} aria-hidden="true" />
              </button>
            </div>
          );
        })}
      </div>
    </div>
  );
}
