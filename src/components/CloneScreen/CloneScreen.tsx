import { useCallback, useEffect, useMemo, useState } from 'react';
import { Bot, Check, X, ShieldCheck, FlaskConical, Play } from 'lucide-react';
import {
  cloneApply,
  cloneDiscard,
  cloneProjects,
  cloneRuns,
  cloneStart,
  setCloneAllowed,
  type CloneProject,
  type CloneRun,
  type CloneStatus,
} from '../../lib/nexus-db';
import './CloneScreen.css';

const LIVE: CloneStatus[] = ['queued', 'preparing', 'editing', 'testing'];

const STATUS_TEXT: Record<CloneStatus, string> = {
  queued: 'Queued',
  preparing: 'Making a scratch copy',
  editing: 'Claude Code is working',
  testing: 'Running your tests',
  ready: 'Ready for review',
  failed: 'Failed',
  applied: 'Applied',
  discarded: 'Discarded',
};

function errText(err: unknown): string {
  return String(err).replace(/^Error:\s*/, '');
}

/** A unified diff, each line coloured by what it is. */
function Diff({ text }: { text: string }) {
  const lines = text.split('\n');
  return (
    <pre className="clone__diff">
      {lines.map((line, i) => {
        const kind = line.startsWith('+++') || line.startsWith('---') || line.startsWith('diff ') || line.startsWith('index ')
          ? 'meta'
          : line.startsWith('@@')
            ? 'hunk'
            : line.startsWith('+')
              ? 'add'
              : line.startsWith('-')
                ? 'del'
                : 'ctx';
        return (
          <span className={`clone__line clone__line--${kind}`} key={i}>
            {line || ' '}
          </span>
        );
      })}
    </pre>
  );
}

/**
 * The supervised task clone.
 *
 * You describe a scoped task; Claude Code makes the change in a scratch copy
 * of the repository with only file tools (no shell, no connectors); NEXUS runs
 * your test suite itself; you read the diff and apply it, or throw it away.
 * Nothing is ever committed or pushed. A project's code is only sent to
 * Claude Code once you allow that project here.
 */
export function CloneScreen() {
  const [projects, setProjects] = useState<CloneProject[]>([]);
  const [runs, setRuns] = useState<CloneRun[]>([]);
  const [projectId, setProjectId] = useState<number | null>(null);
  const [task, setTask] = useState('');
  const [status, setStatus] = useState<string | null>(null);
  const [open, setOpen] = useState<number | null>(null);
  const [busy, setBusy] = useState<number | 'start' | null>(null);

  const loadProjects = useCallback(async () => {
    try {
      const p = await cloneProjects();
      setProjects(p);
      setProjectId((cur) => cur ?? p.find((x) => x.allowed)?.id ?? null);
    } catch (err) {
      setStatus(`Could not load projects: ${errText(err)}`);
    }
  }, []);

  const loadRuns = useCallback(async () => {
    try {
      setRuns(await cloneRuns());
    } catch {
      // A failed poll keeps the last view; the next one usually succeeds.
    }
  }, []);

  useEffect(() => {
    void loadProjects();
    void loadRuns();
  }, [loadProjects, loadRuns]);

  // Poll only while something is running.
  const anyLive = runs.some((r) => LIVE.includes(r.status));
  useEffect(() => {
    if (!anyLive) return;
    const timer = window.setInterval(() => void loadRuns(), 3000);
    return () => window.clearInterval(timer);
  }, [anyLive, loadRuns]);

  const allowed = useMemo(() => projects.filter((p) => p.allowed && p.isGit), [projects]);

  const toggle = useCallback(
    async (p: CloneProject) => {
      try {
        await setCloneAllowed(p.id, !p.allowed);
        await loadProjects();
        setStatus(
          p.allowed
            ? `${p.name}: tasks switched off.`
            : `${p.name}: tasks allowed. Its code will be read by Claude Code when you run one.`,
        );
      } catch (err) {
        setStatus(errText(err));
      }
    },
    [loadProjects],
  );

  const start = useCallback(async () => {
    if (projectId === null) return;
    setBusy('start');
    setStatus(null);
    try {
      const id = await cloneStart(projectId, task);
      setTask('');
      setOpen(id);
      await loadRuns();
    } catch (err) {
      setStatus(errText(err));
    } finally {
      setBusy(null);
    }
  }, [projectId, task, loadRuns]);

  const apply = useCallback(
    async (id: number) => {
      setBusy(id);
      try {
        setStatus(await cloneApply(id));
        await loadRuns();
      } catch (err) {
        setStatus(errText(err));
      } finally {
        setBusy(null);
      }
    },
    [loadRuns],
  );

  const discard = useCallback(
    async (id: number) => {
      setBusy(id);
      try {
        await cloneDiscard(id);
        setStatus('Discarded. Your repository was not touched.');
        await loadRuns();
      } catch (err) {
        setStatus(errText(err));
      } finally {
        setBusy(null);
      }
    },
    [loadRuns],
  );

  return (
    <div className="clone">
      <header className="clone__head">
        <Bot size={18} strokeWidth={1.8} aria-hidden="true" />
        <div>
          <h1 className="clone__title">Clone</h1>
          <p className="clone__subtitle">
            Give a scoped task. Claude Code makes the change in a scratch copy, NEXUS runs your
            tests, you review the diff. Nothing is committed or pushed.
          </p>
        </div>
      </header>

      {status && <p className="clone__status">{status}</p>}

      <section className="clone__card">
        <h2 className="clone__h2">
          <ShieldCheck size={13} strokeWidth={2} aria-hidden="true" /> Projects
        </h2>
        <p className="clone__note">
          A task reads the project's code, which goes to Anthropic through Claude Code. Allow only
          projects you're permitted to share. In a task, Claude can only read and edit files: no
          shell, no connectors, no commits.
        </p>
        {projects.length === 0 && (
          <p className="clone__note">No projects have a local folder yet. Add one under Projects.</p>
        )}
        <ul className="clone__projects">
          {projects.map((p) => (
            <li className="clone__project" key={p.id}>
              <label className="clone__switch">
                <input
                  type="checkbox"
                  checked={p.allowed}
                  disabled={!p.isGit}
                  onChange={() => void toggle(p)}
                  aria-label={`Allow tasks in ${p.name}`}
                />
                <span className="clone__pname">{p.name}</span>
              </label>
              <span className="clone__meta">
                {p.isGit ? p.testLabel ?? 'no test command found' : 'not a git folder'}
              </span>
            </li>
          ))}
        </ul>
      </section>

      <section className="clone__card">
        <h2 className="clone__h2">New task</h2>
        {allowed.length === 0 ? (
          <p className="clone__note">Allow a project above to give it a task.</p>
        ) : (
          <>
            <div className="clone__row">
              <select
                className="clone__input"
                value={projectId ?? ''}
                onChange={(e) => setProjectId(Number(e.target.value))}
                aria-label="Project"
              >
                {allowed.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name}
                  </option>
                ))}
              </select>
            </div>
            <textarea
              className="clone__input clone__task"
              rows={4}
              placeholder="A scoped change, e.g. 'Add an empty state to the results list when the search returns nothing, with a test.'"
              value={task}
              onChange={(e) => setTask(e.target.value)}
              aria-label="Task"
            />
            <div className="clone__row clone__row--end">
              <span className="clone__note">Small, concrete tasks work best. One change per task.</span>
              <button
                className="clone__btn"
                type="button"
                onClick={() => void start()}
                disabled={busy === 'start' || task.trim().length < 10 || projectId === null}
              >
                <Play size={12} strokeWidth={2.2} aria-hidden="true" />
                {busy === 'start' ? 'Starting…' : 'Start task'}
              </button>
            </div>
          </>
        )}
      </section>

      <section className="clone__runs">
        {runs.map((r) => {
          const live = LIVE.includes(r.status);
          const expanded = open === r.id;
          return (
            <article className={`clone__run clone__run--${r.status}`} key={r.id}>
              <button className="clone__run-head" type="button" onClick={() => setOpen(expanded ? null : r.id)} aria-expanded={expanded}>
                <span className={`clone__pill clone__pill--${r.status}`}>
                  {live && <span className="clone__spin" aria-hidden="true" />}
                  {STATUS_TEXT[r.status]}
                </span>
                <span className="clone__run-task">{r.task}</span>
                <span className="clone__meta">{r.projectName}</span>
              </button>

              {expanded && (
                <div className="clone__body">
                  {r.error && <p className="clone__error">{r.error}</p>}

                  {r.summary && (
                    <div className="clone__block">
                      <h3 className="clone__h3">Claude Code's summary</h3>
                      <p className="clone__summary">{r.summary}</p>
                    </div>
                  )}

                  {r.diffStat && (
                    <div className="clone__block">
                      <h3 className="clone__h3">
                        <FlaskConical size={11} strokeWidth={2} aria-hidden="true" /> Tests (run by NEXUS)
                      </h3>
                      <p className={`clone__tests clone__tests--${r.testPassed === null ? 'none' : r.testPassed ? 'pass' : 'fail'}`}>
                        {r.testPassed === null
                          ? `Not run: ${r.testOutput || r.testLabel || 'no test command found'}. Review the change carefully.`
                          : r.testPassed
                            ? `${r.testLabel} passed`
                            : `${r.testLabel} failed`}
                      </p>
                      {r.testOutput && r.testPassed !== null && (
                        <details className="clone__details">
                          <summary>Test output</summary>
                          <pre className="clone__log">{r.testOutput}</pre>
                        </details>
                      )}
                    </div>
                  )}

                  {r.diff && (
                    <div className="clone__block">
                      <h3 className="clone__h3">The change</h3>
                      <pre className="clone__stat">{r.diffStat}</pre>
                      <Diff text={r.diff} />
                    </div>
                  )}

                  {r.status === 'ready' && (
                    <div className="clone__row clone__row--end">
                      <span className="clone__note">
                        Apply adds these as uncommitted changes to your working copy. You commit.
                      </span>
                      <button className="clone__btn clone__btn--quiet" type="button" onClick={() => void discard(r.id)} disabled={busy === r.id}>
                        <X size={12} strokeWidth={2.2} aria-hidden="true" />
                        Discard
                      </button>
                      <button className="clone__btn" type="button" onClick={() => void apply(r.id)} disabled={busy === r.id}>
                        <Check size={12} strokeWidth={2.2} aria-hidden="true" />
                        Apply to my working copy
                      </button>
                    </div>
                  )}
                  {r.status === 'failed' && (
                    <div className="clone__row clone__row--end">
                      <button className="clone__btn clone__btn--quiet" type="button" onClick={() => void discard(r.id)} disabled={busy === r.id}>
                        Clean up
                      </button>
                    </div>
                  )}
                </div>
              )}
            </article>
          );
        })}
      </section>
    </div>
  );
}
