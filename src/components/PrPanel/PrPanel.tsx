import { useCallback, useEffect, useImperativeHandle, useState, forwardRef } from 'react';
import { ExternalLink, GitPullRequest, MessageSquare, RefreshCw } from 'lucide-react';
import { runAction } from '../../lib/assistant';
import { listProjects } from '../../lib/nexus-db';
import type { Project } from '../../types/db';
import './PrPanel.css';

interface RawPr {
  number: number;
  title: string;
  state: string;
  isDraft: boolean;
  url: string;
  author?: { login?: string };
}

/** One pull request you have open, and how it is doing. */
interface Row {
  /** The Jira key from the title, if the title carries one. */
  key: string | null;
  title: string;
  projectId: number;
  projectName: string;
  number: number;
  state: string;
  isDraft: boolean;
  checks?: CheckSummary;
  comments?: number;
  /** Who has commented, and how many each left, most talkative first. */
  commenters?: Commenter[];
}

interface Commenter {
  login: string;
  count: number;
  /** What they actually said, each piece trimmed to a short read. */
  bodies: string[];
}

/** A raw GitHub comment or review, reduced to who left it and what it said. */
interface RawAuthored {
  author?: { login?: string } | null;
  body?: string | null;
}

/** One comment, trimmed to a short readable preview. Collapses whitespace and
 *  caps the length so a long review does not fill the card. */
function shorten(body: string): string {
  const clean = body.replace(/\s+/g, ' ').trim();
  return clean.length > 180 ? `${clean.slice(0, 177)}...` : clean;
}

/**
 * Tally everyone who has weighed in, across both channels.
 *
 * A PR carries conversation `comments` and, separately, `reviews` left through
 * the review flow. Counting only the first dropped reviewers entirely, so this
 * folds both together: each comment and each review counts once toward its
 * author, most active first.
 */
function tallyCommenters(comments: RawAuthored[], reviews: RawAuthored[]): Commenter[] {
  const byLogin = new Map<string, { count: number; bodies: string[] }>();
  for (const a of [...comments, ...reviews]) {
    const login = a.author?.login;
    if (!login) continue;
    const entry = byLogin.get(login) ?? { count: 0, bodies: [] };
    entry.count += 1; // Count every contribution, even an approval with no text.
    const body = a.body?.trim();
    if (body) entry.bodies.push(shorten(body));
    byLogin.set(login, entry);
  }
  return [...byLogin.entries()]
    .map(([login, { count, bodies }]) => ({ login, count, bodies }))
    .sort((a, b) => b.count - a.count);
}

/** GitHub's check rollup, reduced by the connector. Not a string: rendering
 *  it as one is what crashed the card the first time a tracked PR had a PR. */
interface CheckSummary {
  total: number;
  failing: number;
  pending: number;
  failingNames: string[];
}

/**
 * The rollup as one short phrase, or null when there is nothing to say.
 *
 * Order is worst-first: a failure is the thing you need to see, a pending run
 * is next, and "all passed" only when nothing is failing or pending. An
 * unknown check counts as pending upstream, so green here means genuinely
 * green.
 */
function checksLabel(c: CheckSummary | undefined): string | null {
  if (!c || c.total === 0) return null;
  if (c.failing > 0) return `${c.failing} failing`;
  if (c.pending > 0) return `${c.pending} pending`;
  return `${c.total} passed`;
}

/** A repository NEXUS could not ask about, and why. Kept separate from an
 *  empty result: "no PRs" is a fact, "could not look" is a fault. */
interface Blocked {
  projectName: string;
  reason: string;
}

export interface PrPanelHandle {
  refresh: () => void;
}

/**
 * The Jira key a pull request is for, from its title.
 *
 * The team names PRs "KAI-1114 - ..." or "[KAI-1106] - ...", so the key is
 * whatever matches that shape. Null when the title has none: a PR without a
 * ticket is still your PR and still worth showing, it just has no key to
 * badge.
 */
function keyOf(title: string): string | null {
  const m = /\b([A-Z][A-Z0-9]+-\d+)\b/.exec(title.toUpperCase());
  return m ? m[1] : null;
}

/** Ready PRs before drafts; newest number within each. */
function rank(r: Row): number {
  return r.isDraft ? 1 : 0;
}

/**
 * Every pull request you have open, across every repository NEXUS knows.
 *
 * This used to be ticket-driven: it walked the in-progress tasks and asked
 * whether each had a PR. That made it only as complete as the tasks list,
 * and when the tasks list emptied, the card went blank while real PRs sat
 * open and untracked. Now it is repository-driven. It asks GitHub, per
 * registered repo, for the pull requests you authored, so a PR is tracked the
 * moment it exists, whether or not a task was ever made for it. Registering a
 * repo is the whole of the setup.
 *
 * A repository this account cannot see fails as itself, named, rather than
 * taking the card down or being mistaken for "no PRs".
 */
export const PrPanel = forwardRef<PrPanelHandle>(function PrPanel(_props, ref) {
  const [rows, setRows] = useState<Row[]>([]);
  const [blocked, setBlocked] = useState<Blocked[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Which reviewer's comment is expanded, keyed project-number-login. One at a
  // time, so clicking another closes the first.
  const [expanded, setExpanded] = useState<string | null>(null);

  const load = useCallback(async () => {
    setBusy(true);
    try {
      const projects: Project[] = await listProjects();
      // Only projects that are actually GitHub repositories. A project with
      // no repository has nothing for this card to ask about.
      const repos = projects.filter((p) => (p.repositoryUrl ?? '').includes('github.com'));

      const found: Row[] = [];
      const cannot: Blocked[] = [];

      for (const project of repos) {
        let prs: RawPr[] = [];
        try {
          const outcome = await runAction({
            actionId: 'github.list_prs',
            // Open PRs you authored. Merged and closed are deliberately not
            // asked for: this card is what still needs you, and a shipped or
            // abandoned PR needs nothing. Filtered server-side via search, and
            // again below in case a backend ignores the qualifier.
            input: { projectId: project.id, query: 'author:@me state:open' },
          });
          prs = (outcome.output as { pullRequests?: RawPr[] }).pullRequests ?? [];
        } catch (err) {
          // Named and set aside, never folded into "no PRs".
          cannot.push({ projectName: project.name, reason: String(err) });
          continue;
        }

        for (const pr of prs) {
          // Belt and braces: only open PRs, whatever the search returned.
          if (pr.state.toLowerCase() !== 'open') continue;
          const row: Row = {
            key: keyOf(pr.title),
            title: pr.title,
            projectId: project.id,
            projectName: project.name,
            number: pr.number,
            state: pr.state,
            isDraft: pr.isDraft,
          };

          // Every row here is an open PR. Checks and comments are worth a
          // call each on those; a draft is skipped only because its checks
          // are usually not running yet.
          if (!pr.isDraft) {
            try {
              const detail = await runAction({
                actionId: 'github.read_pr',
                input: { projectId: project.id, number: pr.number },
              });
              row.checks = (detail.output as { checks?: CheckSummary }).checks;
            } catch {
              // Unknown, not passed.
            }
            try {
              const said = await runAction({
                actionId: 'github.read_pr_comments',
                input: { projectId: project.id, number: pr.number },
              });
              const out = said.output as {
                comments?: RawAuthored[];
                reviews?: RawAuthored[];
              };
              const comments = Array.isArray(out.comments) ? out.comments : [];
              const reviews = Array.isArray(out.reviews) ? out.reviews : [];
              const commenters = tallyCommenters(comments, reviews);
              if (commenters.length > 0) {
                row.comments = comments.length + reviews.length;
                row.commenters = commenters;
              }
            } catch {
              // Unknown, not zero.
            }
          }
          found.push(row);
        }
      }

      found.sort((a, b) => rank(a) - rank(b) || b.number - a.number);
      setRows(found);
      setBlocked(cannot);
      setError(null);
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

  return (
    <section className="pr" aria-label="My pull requests">
      <div className="pr__head">
        <GitPullRequest size={12} strokeWidth={2} aria-hidden="true" />
        <span className="pr__title">Pull requests</span>
        {rows.length > 0 && <span className="pr__count">{rows.length}</span>}
        <button
          className={`pr__refresh${busy ? ' pr__refresh--busy' : ''}`}
          type="button"
          onClick={() => void load()}
          disabled={busy}
          aria-label="Refresh pull requests"
        >
          <RefreshCw size={12} strokeWidth={2} aria-hidden="true" />
        </button>
      </div>

      {error && (
        <p className="pr__error" role="alert">
          {error}
        </p>
      )}

      {!error && rows.length === 0 && !busy && (
        <p className="pr__empty">No open pull requests.</p>
      )}

      <ul className="pr__list">
        {rows.map((r) => (
          <li className="pr__row pr__row--open" key={`${r.projectId}-${r.number}`}>
            <button
              className="pr__link"
              type="button"
              title={r.title}
              onClick={() =>
                void runAction({
                  actionId: 'github.open_pr',
                  input: { projectId: r.projectId, number: r.number },
                }).catch(() => {})
              }
            >
              <span className="pr__key">{r.key ?? `#${r.number}`}</span>
              <span className="pr__what">{r.title}</span>
              <span
                className={`pr__state pr__state--${
                  r.isDraft ? 'draft' : r.state.toLowerCase()
                }`}
              >
                {r.isDraft ? 'draft' : r.state.toLowerCase()}
              </span>
              {/* Formatted to a phrase; the raw object must never reach the
                  DOM, which is exactly the crash this card had. Unknown or
                  no checks render as nothing, never as a tick. */}
              {checksLabel(r.checks) && (
                <span className="pr__checks">{checksLabel(r.checks)}</span>
              )}
              {r.comments !== undefined && r.comments > 0 && (
                <span className="pr__comments">
                  <MessageSquare size={10} strokeWidth={2} aria-hidden="true" />
                  {r.comments}
                </span>
              )}
              <span className="pr__repo">{r.projectName}</span>
              <ExternalLink size={10} strokeWidth={2} aria-hidden="true" />
            </button>

            {/* Who is in the conversation, and how much each has said. A PR
                with review comments is one someone is waiting on; naming them
                is the difference between "there are comments" and "Priya left
                three, go look". */}
            {r.commenters && r.commenters.length > 0 && (
              <div className="pr__reviewers">
                {r.commenters.map((c) => {
                  const openKey = `${r.projectId}-${r.number}-${c.login}`;
                  const isOpen = expanded === openKey;
                  return (
                    <button
                      className={`pr__reviewer${isOpen ? ' pr__reviewer--open' : ''}`}
                      key={c.login}
                      type="button"
                      title={c.bodies.length > 0 ? 'Show what they said' : 'No written comment'}
                      onClick={() => setExpanded(isOpen ? null : openKey)}
                    >
                      <span className="pr__reviewer-name">{c.login}</span>
                      <span className="pr__reviewer-count">{c.count}</span>
                    </button>
                  );
                })}
              </div>
            )}

            {/* The clicked reviewer's comments, read short. Below the chips so
                opening one does not shove the row around. */}
            {r.commenters?.map((c) => {
              const openKey = `${r.projectId}-${r.number}-${c.login}`;
              if (expanded !== openKey) return null;
              return (
                <div className="pr__comment" key={`body-${c.login}`}>
                  <span className="pr__comment-who">{c.login}</span>
                  {c.bodies.length > 0 ? (
                    c.bodies.map((b, i) => (
                      <p className="pr__comment-body" key={i}>
                        {b}
                      </p>
                    ))
                  ) : (
                    <p className="pr__comment-body pr__comment-body--empty">
                      Reviewed with no written comment.
                    </p>
                  )}
                </div>
              );
            })}
          </li>
        ))}
      </ul>

      {/* Repositories that could not be read, said plainly at the foot so the
          list above is only ever real PRs. */}
      {blocked.length > 0 && (
        <ul className="pr__blocked">
          {blocked.map((b) => (
            <li className="pr__blocked-row" key={b.projectName} title={b.reason}>
              {b.projectName}: couldn’t check
            </li>
          ))}
        </ul>
      )}
    </section>
  );
});
