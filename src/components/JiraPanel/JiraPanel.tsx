import { useCallback, useEffect, useImperativeHandle, useState, forwardRef } from 'react';
import { ExternalLink, RefreshCw } from 'lucide-react';
import { runAction } from '../../lib/assistant';
import './JiraPanel.css';

interface Issue {
  key: string;
  summary: string;
  status: string;
  updated: string | null;
}

export interface JiraPanelHandle {
  /** Re-read from Jira. Called by the screen's own refresh button. */
  refresh: () => void;
}

/**
 * Rough status grouping, for colour only.
 *
 * Status names are per-workflow and this team has several ("In Development",
 * "In Review", "Ready for Team Testing"), so this matches on what the words
 * mean rather than on an enumeration that would go stale the first time
 * somebody edits the board.
 */
function toneOf(status: string): 'waiting' | 'review' | 'testing' | 'building' {
  const s = status.toLowerCase();
  // Not started. Matched exactly, because "reopened" is not the backlog.
  if (s === 'open' || s === 'to do' || s === 'backlog') return 'waiting';
  if (s.includes('review')) return 'review';
  if (s.includes('test') || s.includes('qa')) return 'testing';
  return 'building';
}

function agoOf(iso: string | null): string {
  if (!iso) return '';
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return '';
  const mins = Math.max(0, Math.floor((Date.now() - then) / 60000));
  if (mins < 60) return `${mins}m`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h`;
  return `${Math.floor(hours / 24)}d`;
}

/**
 * Where a status sits in the flow, for ordering the list.
 *
 * The next thing to do comes first: a ticket ready to pick up, then one being
 * built, then one in review, then one in testing. Anything the match misses
 * sorts to the end rather than jumping the queue. Case-insensitive and matched
 * on a substring, because board statuses vary ("In Review" vs "Code Review").
 */
const STATUS_ORDER: string[] = [
  'in development',
  'in progress',
  'review',
  'test',
  'qa',
  // Ready for development sits last: it is the not-yet-started queue, below
  // everything actually in motion, and it is the one capped to two.
  'ready for development',
];

/** How many Ready-for-Development tickets to show; the rest are a count. */
const DEV_SHOWN = 2;

function statusRank(status: string): number {
  const s = status.toLowerCase();
  const i = STATUS_ORDER.findIndex((p) => s.includes(p));
  return i < 0 ? STATUS_ORDER.length : i;
}

/** Most recently updated first, within a status. Missing dates sort last. */
function updatedRank(i: Issue): number {
  if (!i.updated) return Number.MAX_SAFE_INTEGER;
  const t = Date.parse(i.updated);
  return Number.isNaN(t) ? Number.MAX_SAFE_INTEGER : -t;
}

/**
 * Your next tickets to pick up: the ones in "Ready for Development".
 *
 * Read through `jira.my_issues`, so it goes through the gate, is audited, and
 * refuses cleanly without the connector's Read grant. The action returns
 * everything not Done; this card narrows that to the development queue and
 * shows the top couple, because the question it answers is "what do I start
 * next", not "what is assigned to me in total". When one leaves the status it
 * drops off and the next in line takes its place; the count says how deep the
 * queue is so a hidden one is never a surprise.
 *
 * It fetches on mount and when the screen is refreshed, and never on a timer.
 * Jira is somebody else's service and this is a panel, not a monitor; polling
 * it every few seconds would spend the user's rate limit to redraw a list
 * that changes a few times a day.
 */
export const JiraPanel = forwardRef<JiraPanelHandle>(function JiraPanel(_props, ref) {
  const [issues, setIssues] = useState<Issue[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [checked, setChecked] = useState<Date | null>(null);

  const load = useCallback(async () => {
    setBusy(true);
    try {
      const outcome = await runAction({ actionId: 'jira.my_issues' });
      const output = outcome.output as { issues?: Issue[] };
      setIssues(output.issues ?? []);
      setError(null);
      setChecked(new Date());
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  useImperativeHandle(ref, () => ({ refresh: () => void load() }), [load]);

  // Every assigned ticket, with one cap: the Ready-for-Development queue is
  // shown two deep, because that is the "what do I pick up next" short list
  // and it can be long. Every other status shows in full. Ordered so the next
  // thing to do leads, most recently touched first within a status.
  const isDev = (i: Issue) => i.status.toLowerCase().includes('ready for development');
  const dev = issues.filter(isDev).sort((a, b) => updatedRank(a) - updatedRank(b));
  const rest = issues.filter((i) => !isDev(i));
  const devHidden = Math.max(0, dev.length - DEV_SHOWN);
  const ordered = [...dev.slice(0, DEV_SHOWN), ...rest].sort(
    (a, b) => statusRank(a.status) - statusRank(b.status) || updatedRank(a) - updatedRank(b),
  );

  return (
    <section className="jira" aria-label="Jira issues assigned to me">
      <div className="jira__head">
        <span className="jira__title">Assigned to me</span>
        {devHidden > 0 && (
          <span
            className="jira__chip"
            title={`${devHidden} more ready for development, not shown`}
          >
            +{devHidden} ready
          </span>
        )}
        {ordered.length > 0 && (
          <span className="jira__chip jira__chip--count" title="Tickets assigned to you, not yet done">
            {issues.length}
          </span>
        )}

        <span className="jira__meta">
          {checked && `${String(checked.getHours()).padStart(2, '0')}:${String(checked.getMinutes()).padStart(2, '0')}`}
        </span>
        <button
          className={`jira__refresh${busy ? ' jira__refresh--busy' : ''}`}
          type="button"
          onClick={() => void load()}
          disabled={busy}
          aria-label="Refresh Jira"
        >
          <RefreshCw size={12} strokeWidth={2} aria-hidden="true" />
        </button>
      </div>

      {error && (
        <p className="jira__error" role="alert">
          {error}
        </p>
      )}

      {!error && ordered.length === 0 && !busy && (
        <p className="jira__empty">Nothing assigned to you.</p>
      )}

      <ul className="jira__list">
        {ordered.map((i) => (
          <li className={`jira__row jira__row--${toneOf(i.status)}`} key={i.key}>
            <button
              className="jira__open"
              type="button"
              title={i.summary}
              onClick={() =>
                void runAction({
                  actionId: 'jira.open_issue',
                  input: { key: i.key },
                }).catch(() => {
                  // Opening a browser tab is not worth an error banner over.
                })
              }
            >
              <span className="jira__key">{i.key}</span>
              <span className="jira__summary">{i.summary}</span>
              <span className={`jira__status jira__status--${toneOf(i.status)}`}>{i.status}</span>
              <span className="jira__ago">{agoOf(i.updated)}</span>
              <ExternalLink size={11} strokeWidth={2} aria-hidden="true" />
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
});
