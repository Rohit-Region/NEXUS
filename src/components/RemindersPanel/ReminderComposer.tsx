import { useMemo, useState } from 'react';
import { Plus } from 'lucide-react';
import { createCommitment } from '../../lib/assistant';
import type { Commitment } from '../../lib/assistant';

/**
 * Offsets people actually reach for, in minutes.
 *
 * Chips rather than a time picker: a reminder is nearly always "a bit from
 * now", and a picker makes you name an absolute time you would have to work
 * out first. `At...` is there for the times it isn't.
 */
const QUICK: Array<{ label: string; minutes: number }> = [
  { label: '5m', minutes: 5 },
  { label: '15m', minutes: 15 },
  { label: '30m', minutes: 30 },
  { label: '1h', minutes: 60 },
  { label: '3h', minutes: 180 },
  { label: 'Tomorrow', minutes: -1 },
];

const WEEKDAYS = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];
const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

/** 9am tomorrow, as unix seconds. */
function tomorrowMorning(): number {
  const d = new Date();
  d.setDate(d.getDate() + 1);
  d.setHours(9, 0, 0, 0);
  return Math.floor(d.getTime() / 1000);
}

/**
 * A `datetime-local` value for a date, in local wall time.
 *
 * Built by hand rather than from `toISOString`, which converts to UTC first.
 * East of Greenwich that shifts the default backwards by the offset and the
 * field opens on the wrong day; the earlier delay-parsing bug in this
 * feature was the same mistake in Rust.
 */
function toInputValue(d: Date): string {
  const pad = (n: number) => String(n).padStart(2, '0');
  return (
    `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}` +
    `T${pad(d.getHours())}:${pad(d.getMinutes())}`
  );
}

/** The next five-minute mark an hour out: a sane place for the field to open. */
function defaultExact(): string {
  const d = new Date();
  d.setHours(d.getHours() + 1, Math.ceil(d.getMinutes() / 5) * 5, 0, 0);
  return toInputValue(d);
}

/**
 * When this will fire, said the way a person would say it.
 *
 * Calendar days apart, not hours apart: 23:50 tonight and 00:10 tonight are
 * twenty minutes apart and are different days, and the reminder that matters
 * is the one you can tell apart from the other one.
 *
 * This used to call anything that was not today "tomorrow", which was true
 * while the only choices were offsets of at most a day. With a date picker
 * it would have called next Friday tomorrow.
 */
function whenOf(unix: number): string {
  const d = new Date(unix * 1000);
  const at = `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;

  const midnightToday = new Date();
  midnightToday.setHours(0, 0, 0, 0);
  const midnightThen = new Date(unix * 1000);
  midnightThen.setHours(0, 0, 0, 0);
  const days = Math.round(
    (midnightThen.getTime() - midnightToday.getTime()) / 86_400_000,
  );

  if (days === 0) return at;
  if (days === 1) return `${at} tomorrow`;
  // Inside the coming week a weekday name is the fastest thing to read.
  if (days > 1 && days < 7) return `${at} ${WEEKDAYS[d.getDay()]}`;
  return `${at} on ${d.getDate()} ${MONTHS[d.getMonth()]}`;
}

interface Props {
  onCreated: (made: Commitment) => void;
}

export function ReminderComposer({ onCreated }: Props) {
  const [what, setWhat] = useState('');
  const [minutes, setMinutes] = useState<number>(QUICK[0].minutes);
  // Empty until `At...` is pressed, which is also what says the picker is
  // not the active choice. One piece of state, not a mode flag beside it.
  const [exact, setExact] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const dueAt = useMemo(() => {
    if (exact !== '') {
      // A `datetime-local` value carries no offset, so this parses as local
      // wall time, which is what the user typed into a local field.
      const parsed = new Date(exact).getTime();
      return Number.isNaN(parsed) ? null : Math.floor(parsed / 1000);
    }
    return minutes === -1
      ? tomorrowMorning()
      : Math.floor(Date.now() / 1000) + minutes * 60;
    // Recomputed when the choice changes. Not on a timer: it is read at
    // submit, and a preview that crept forward every second would make the
    // field look unstable.
  }, [minutes, exact]);

  async function submit() {
    if (what.trim() === '' || busy) return;

    // Refused rather than accepted and fired at once. A time in the past is
    // a typo in the date nine times out of ten, and the failure it produces
    // otherwise (a reminder that announces itself the moment it is made)
    // looks like a bug rather than like the mistake it was.
    if (dueAt === null) {
      setError('Pick a date and a time.');
      return;
    }
    if (dueAt <= Math.floor(Date.now() / 1000)) {
      setError('That time has already passed.');
      return;
    }

    setBusy(true);
    setError(null);
    try {
      onCreated(await createCommitment(what.trim(), dueAt));
      setWhat('');
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="composer">
      <div className="composer__row">
        <span className="composer__prompt" aria-hidden="true">
          &gt;
        </span>
        <input
          className="composer__input"
          type="text"
          placeholder="Remind me to..."
          value={what}
          onChange={(e) => setWhat(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void submit();
          }}
          disabled={busy}
          aria-label="What to be reminded about"
        />
        <button
          className="composer__add"
          type="button"
          onClick={() => void submit()}
          disabled={busy || what.trim() === ''}
          aria-label="Create reminder"
        >
          <Plus size={14} strokeWidth={2.5} aria-hidden="true" />
        </button>
      </div>

      <div className="composer__chips" role="group" aria-label="When">
        {QUICK.map((q) => (
          <button
            key={q.label}
            className={`composer__chip${
              exact === '' && minutes === q.minutes ? ' composer__chip--on' : ''
            }`}
            type="button"
            onClick={() => {
              setMinutes(q.minutes);
              setExact('');
              setError(null);
            }}
            disabled={busy}
            aria-pressed={exact === '' && minutes === q.minutes}
          >
            {q.label}
          </button>
        ))}

        {/* The offsets cover "a bit from now". This covers the rest, and it
            is last because it is the one you reach for least. */}
        <button
          className={`composer__chip${exact !== '' ? ' composer__chip--on' : ''}`}
          type="button"
          onClick={() => {
            setExact((prev) => (prev === '' ? defaultExact() : ''));
            setError(null);
          }}
          disabled={busy}
          aria-pressed={exact !== ''}
        >
          At&hellip;
        </button>

        {/* Said outright rather than left to be worked out. The whole class
            of defect here is a reminder that looks set and never fires. */}
        <span className="composer__when">
          {dueAt === null ? 'no time set' : `fires at ${whenOf(dueAt)}`}
        </span>
      </div>

      {exact !== '' && (
        <input
          className="composer__exact"
          type="datetime-local"
          value={exact}
          min={toInputValue(new Date())}
          onChange={(e) => {
            setExact(e.target.value);
            setError(null);
          }}
          disabled={busy}
          aria-label="Exact date and time to be reminded"
        />
      )}

      {error && (
        <p className="composer__error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
