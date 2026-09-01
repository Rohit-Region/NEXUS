import { useEffect, useState } from 'react';
import { listAudit } from '../../lib/assistant';
import type { AuditEntry } from '../../types/assistant';
import './LiveActivity.css';

const REFRESH_MS = 8000;
const LIMIT = 12;

/** Outcome to a dot colour. Refusals are not failures and read differently. */
function toneOf(entry: AuditEntry): 'ok' | 'warn' | 'bad' | 'wait' {
  switch (entry.outcome) {
    case 'succeeded':
      return 'ok';
    case 'refused':
      return 'warn';
    case 'failed':
      return 'bad';
    default:
      // Still open. A row stuck here means NEXUS never came back, which is
      // itself worth seeing rather than hiding.
      return 'wait';
  }
}

/** One row, possibly standing for several identical ones. */
interface Row {
  entry: AuditEntry;
  repeats: number;
}

/**
 * Collapse a run of identical entries into one row with a count.
 *
 * A watcher that polls every few minutes writes the same failure every few
 * minutes: five rows of "Not signed in to Microsoft" is one fact reported
 * five times, and it pushes everything that actually happened off the list.
 * Only consecutive runs are folded, so the same action failing again after
 * something else happened is still visibly a new event.
 */
function collapse(entries: AuditEntry[]): Row[] {
  const rows: Row[] = [];
  for (const entry of entries) {
    const last = rows[rows.length - 1];
    const same =
      last !== undefined &&
      last.entry.actionId === entry.actionId &&
      last.entry.outcome === entry.outcome &&
      last.entry.error === entry.error;
    if (same) {
      last.repeats += 1;
    } else {
      rows.push({ entry, repeats: 1 });
    }
  }
  return rows;
}

function clockOf(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime())
    ? '--:--'
    : `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
}

/**
 * What NEXUS has actually done, newest first.
 *
 * Read from `action_audit`, which is written before dispatch and closed
 * after, so this is a record rather than a reconstruction. It answers "what
 * did NEXUS do on my behalf", including the times it refused.
 *
 * Deliberately not a feed of everything happening in GitHub, Teams and Jira.
 * NEXUS holds no such stream: it asks those services questions when asked to
 * and keeps nothing. A panel that appeared to show live external events
 * would either be inventing them or quietly polling every connector on a
 * timer, and the honest version of this panel is the one that shows what is
 * genuinely known.
 */
export function LiveActivity() {
  const [entries, setEntries] = useState<AuditEntry[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    const load = async () => {
      try {
        const rows = await listAudit(LIMIT);
        if (!alive) return;
        setEntries(rows);
        setError(null);
      } catch (err) {
        if (alive) setError(String(err));
      }
    };
    void load();
    const timer = window.setInterval(() => void load(), REFRESH_MS);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, []);

  return (
    <section className="live" aria-label="Live activity">
      <div className="live__head">
        <span className="live__title">Live Activity</span>
        <span className="live__count">{collapse(entries).length}</span>
      </div>

      {error && (
        <p className="live__error" role="alert">
          {error}
        </p>
      )}

      {entries.length === 0 && !error && (
        <p className="live__empty">Nothing yet. Ask NEXUS to do something.</p>
      )}

      <ol className="live__list">
        {collapse(entries).map(({ entry: e, repeats }) => (
          <li className={`live__row live__row--${toneOf(e)}`} key={e.id}>
            <span className="live__time">{clockOf(e.createdAt)}</span>
            <span className="live__body">
              <span className="live__where">
                {e.connectorId}
                {repeats > 1 && (
                  <span className="live__repeats">&times;{repeats}</span>
                )}
              </span>
              {/* The rendered summary the user already saw, never raw input:
                  the audit trail records what was done, not what was read. */}
              <span className="live__what">{e.summary}</span>
              {e.error && <span className="live__why">{e.error}</span>}
            </span>
            <span className="live__dot" aria-hidden="true" />
          </li>
        ))}
      </ol>
    </section>
  );
}
