import { useMemo, useState } from 'react';
import { Plus } from 'lucide-react';
import { createCommitment } from '../../lib/assistant';
import type { Commitment } from '../../lib/assistant';

/**
 * Offsets people actually reach for, in minutes.
 *
 * Chips rather than a time picker: a reminder is nearly always "a bit from
 * now", and a picker makes you name an absolute time you would have to work
 * out first. The exact-time field is still there for the times it isn't.
 */
const QUICK: Array<{ label: string; minutes: number }> = [
  { label: '5m', minutes: 5 },
  { label: '15m', minutes: 15 },
  { label: '30m', minutes: 30 },
  { label: '1h', minutes: 60 },
  { label: '3h', minutes: 180 },
  { label: 'Tomorrow', minutes: -1 },
];

/** 9am tomorrow, as unix seconds. */
function tomorrowMorning(): number {
  const d = new Date();
  d.setDate(d.getDate() + 1);
  d.setHours(9, 0, 0, 0);
  return Math.floor(d.getTime() / 1000);
}

function clockOf(unix: number): string {
  const d = new Date(unix * 1000);
  const hh = String(d.getHours()).padStart(2, '0');
  const mm = String(d.getMinutes()).padStart(2, '0');
  const today = new Date();
  const sameDay = d.toDateString() === today.toDateString();
  return sameDay ? `${hh}:${mm}` : `${hh}:${mm} tomorrow`;
}

interface Props {
  onCreated: (made: Commitment) => void;
}

export function ReminderComposer({ onCreated }: Props) {
  const [what, setWhat] = useState('');
  const [minutes, setMinutes] = useState<number>(QUICK[0].minutes);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const dueAt = useMemo(
    () =>
      minutes === -1
        ? tomorrowMorning()
        : Math.floor(Date.now() / 1000) + minutes * 60,
    // Recomputed when the offset changes. Not on a timer: it is read at
    // submit, and a preview that crept forward every second would make the
    // field look unstable.
    [minutes],
  );

  async function submit() {
    if (what.trim() === '' || busy) return;
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
            className={`composer__chip${minutes === q.minutes ? ' composer__chip--on' : ''}`}
            type="button"
            onClick={() => setMinutes(q.minutes)}
            disabled={busy}
            aria-pressed={minutes === q.minutes}
          >
            {q.label}
          </button>
        ))}
        {/* Said outright rather than left to be worked out. The whole class
            of defect here is a reminder that looks set and never fires. */}
        <span className="composer__when">fires at {clockOf(dueAt)}</span>
      </div>

      {error && (
        <p className="composer__error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
