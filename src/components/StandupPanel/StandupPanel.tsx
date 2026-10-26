import { useCallback, useState } from 'react';
import { ClipboardList, Copy, CalendarClock } from 'lucide-react';
import { runAction, isActionError, describeActionError } from '../../lib/assistant';
import { composeStandup, type Standup, type StandupInput } from '../../lib/nexus-db';
import './StandupPanel.css';

function reason(err: unknown): string {
  return isActionError(err) ? describeActionError(err) : String(err).replace(/^Error:\s*/, '');
}

/** Local YYYY-MM-DD for `daysBack` days ago. */
function localDate(daysBack: number): string {
  const d = new Date();
  d.setDate(d.getDate() - daysBack);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
}

/** Back to the previous working day: Friday on a Monday, Friday on a Sunday. */
function daysToLastWorkday(): number {
  const dow = new Date().getDay();
  return dow === 1 ? 3 : dow === 0 ? 2 : 1;
}

function nowHHMM(): string {
  const d = new Date();
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
}

type Raw = Record<string, unknown>;
const str = (v: unknown) => (typeof v === 'string' ? v : '');

/**
 * The standup draft: Yesterday, Today, Blockers, from Jira, GitHub and the
 * calendar, plus what's linked to your next meeting.
 *
 * Every line comes from those sources; nothing is written by a model, because
 * a standup is read to the team and a line about work you didn't do is worse
 * than a short standup. Each source is fetched through the same permission
 * gate as the rest of NEXUS, and any that fail are named, so a thin draft is
 * never mistaken for a quiet day. It's a draft: edit it, then copy it.
 */
export function StandupPanel() {
  const [standup, setStandup] = useState<Standup | null>(null);
  const [text, setText] = useState('');
  const [busy, setBusy] = useState(false);
  const [notes, setNotes] = useState<string[]>([]);
  const [counts, setCounts] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const draft = useCallback(async () => {
    setBusy(true);
    setCopied(false);
    const days = daysToLastWorkday();
    const since = localDate(days);
    const [recent, open, prs, cal] = await Promise.allSettled([
      runAction({ actionId: 'jira.recent_work', input: { days } }),
      runAction({ actionId: 'jira.my_issues' }),
      runAction({ actionId: 'github.my_prs', input: { since } }),
      runAction({ actionId: 'outlook.today_schedule', input: {} }),
    ]);

    const failed: string[] = [];
    const out = <T,>(r: PromiseSettledResult<{ output: unknown }>, label: string, pick: (o: Raw) => T, empty: T): T => {
      if (r.status === 'fulfilled') return pick((r.value.output ?? {}) as Raw);
      failed.push(`${label}: ${reason(r.reason)}`);
      return empty;
    };
    const issues = (o: Raw) =>
      ((o.issues as Raw[]) ?? []).map((i) => ({
        key: str(i.key),
        summary: str(i.summary),
        status: str(i.status),
        category: str(i.category) || null,
      }));

    const input: StandupInput = {
      since,
      recent: out(recent, 'Jira (recent)', issues, []),
      open: out(open, 'Jira (assigned)', issues, []),
      prs: out(
        prs,
        'GitHub',
        (o) => {
          for (const e of (o.errors as string[]) ?? []) failed.push(`GitHub ${e}`);
          return ((o.prs as Raw[]) ?? []).map((p) => {
            const checks = p.checks as Raw | null;
            return {
              repo: str(p.repo),
              number: Number(p.number) || 0,
              title: str(p.title),
              state: str(p.state),
              isDraft: Boolean(p.isDraft),
              mergedAt: str(p.mergedAt) || null,
              reviewDecision: str(p.reviewDecision) || null,
              checks: checks
                ? { failing: Number(checks.failing) || 0, pending: Number(checks.pending) || 0 }
                : null,
            };
          });
        },
        [],
      ),
      meetings: out(
        cal,
        'Calendar',
        (o) => {
          const now = nowHHMM();
          return ((o.events as Raw[]) ?? []).map((e) => ({
            title: str(e.subject),
            at: str(e.start),
            upcoming: str(e.start) > now,
          }));
        },
        [],
      ),
    };

    try {
      const s = await composeStandup(input);
      setStandup(s);
      setText(s.text);
      setCounts(
        `From ${input.recent.length + input.open.length} tickets, ${input.prs.length} PRs and ${input.meetings.length} meetings since ${since}.`,
      );
    } catch (err) {
      failed.push(`Composing: ${reason(err)}`);
    } finally {
      setNotes(failed);
      setBusy(false);
    }
  }, []);

  const copy = useCallback(() => {
    void navigator.clipboard
      .writeText(text)
      .then(() => setCopied(true))
      .catch(() => setCopied(false));
  }, [text]);

  return (
    <section className="standup" aria-label="Standup draft">
      <div className="standup__head">
        <ClipboardList size={12} strokeWidth={2} aria-hidden="true" />
        <span className="standup__title">Standup</span>
        <button className="standup__btn" type="button" onClick={() => void draft()} disabled={busy}>
          {busy ? 'Gathering…' : standup ? 'Redraft' : 'Draft standup'}
        </button>
      </div>

      {!standup && !busy && (
        <p className="standup__hint">
          Builds Yesterday, Today and Blockers from your Jira tickets, your PRs and today's
          calendar. Every line comes from those; nothing is made up. Edit, then copy.
        </p>
      )}

      {notes.length > 0 && (
        <ul className="standup__notes" role="note">
          {notes.map((n) => (
            <li key={n}>Couldn't read {n}</li>
          ))}
        </ul>
      )}

      {standup && (
        <>
          <textarea
            className="standup__text"
            rows={Math.min(18, Math.max(8, text.split('\n').length + 1))}
            value={text}
            onChange={(e) => {
              setText(e.target.value);
              setCopied(false);
            }}
            aria-label="Standup draft"
          />
          <div className="standup__bar">
            <span className="standup__hint">{counts}</span>
            <button className="standup__btn" type="button" onClick={copy}>
              <Copy size={11} strokeWidth={2} aria-hidden="true" />
              {copied ? 'Copied' : 'Copy'}
            </button>
          </div>

          {standup.nextMeeting && (
            <div className="standup__meeting">
              <div className="standup__meeting-head">
                <CalendarClock size={12} strokeWidth={2} aria-hidden="true" />
                <span>
                  Next: {standup.nextMeeting.at} {standup.nextMeeting.title}
                </span>
              </div>
              {standup.nextMeeting.related.length > 0 ? (
                <ul className="standup__related">
                  {standup.nextMeeting.related.map((r) => (
                    <li key={r}>{r}</li>
                  ))}
                </ul>
              ) : (
                <p className="standup__hint">
                  No ticket key in the meeting title, so nothing is linked. Put the key (like
                  CONN-706) in the title and the ticket and its PR show up here.
                </p>
              )}
            </div>
          )}
        </>
      )}
    </section>
  );
}
