import { useCallback, useEffect, useState } from 'react';
import { Boxes, ExternalLink, RefreshCw } from 'lucide-react';
import { runAction, isActionError, describeActionError } from '../../lib/assistant';
import { openUrl } from '../../lib/nexus-db';

/** The versions page. Opened in the browser so its SSO login runs there, since
 *  NEXUS cannot authenticate the page itself. */
const VERSIONS_URL = 'https://int-app.avetta.com/serviceVersions.html';
import './KaiVersionsPanel.css';

interface ServiceVersion {
  service: string;
  version: string;
}

/** Slower than the work cards: a deployed version changes on a release, not a
 *  minute, so re-reading the tab often would be churn for no news. */
const REFRESH_MS = 300_000;

/** The environments worth switching between here. Each is a column on the
 *  Avetta page; picking one re-reads that column. */
const ENVS = ['KAI', 'QA', 'LRG', 'PA'] as const;

/**
 * The services this card cares about, lowercased for matching.
 *
 * The page lists thirty-odd services; only a handful are the ones being
 * watched. Filtering to them here keeps the card a glance rather than a
 * spreadsheet. Change this line to change what shows.
 */
const SERVICES = ['admin', 'ui', 'auth', 'gateway'];

/**
 * Repos whose "version" is their latest GitHub Actions run, not a row on the
 * Avetta SSO page. These two deploy through Actions, so the honest version is
 * whatever the top of their Actions page says: a build number, whether it
 * passed, and the branch it ran on. Read through `gh`, which is already signed
 * in for the pull-request cards, so no extra auth is needed.
 */
const BUILD_REPOS: { repo: string; label: string }[] = [
  { repo: 'avetta/site-directory-ui', label: 'Site Directory (Find Work)' },
  { repo: 'avetta/find-client-ui', label: 'Find Client UI' },
];

interface BuildRun {
  number: number;
  displayTitle: string;
  headBranch: string;
  /** "queued" | "in_progress" | "completed". */
  status: string;
  /** Only set once completed: "success" | "failure" | "cancelled" | ... */
  conclusion: string | null;
  workflowName: string;
  createdAt: string;
  url: string;
}

interface BuildRow {
  label: string;
  repo: string;
  run: BuildRun | null;
  error: string | null;
}

/** The three states a build row can be in, from its run. A run still going is
 *  not a failure, which is why status is checked before conclusion. */
function buildState(run: BuildRun | null): 'pass' | 'fail' | 'running' | 'none' {
  if (!run) return 'none';
  if (run.status !== 'completed') return 'running';
  return run.conclusion === 'success' ? 'pass' : 'fail';
}

/** Short right-hand label for a build row: the run number and what happened. */
function buildLabel(run: BuildRun | null): string {
  if (!run) return 'no runs';
  const state = buildState(run);
  if (state === 'running') return `#${run.number} running`;
  if (state === 'pass') return `#${run.number} passed`;
  return `#${run.number} ${run.conclusion ?? 'failed'}`;
}

/**
 * What the watched Avetta services are running, per environment.
 *
 * Read out of the Avetta tab you already have open and signed in, because the
 * version endpoints are behind SSO a stored password cannot drive. So this
 * card needs one thing of you the others do not: the page open. Its absence is
 * said plainly rather than shown as an empty list, because "no versions" is a
 * claim about the deployment and a closed tab has no standing to make it.
 */
export function KaiVersionsPanel() {
  const [versions, setVersions] = useState<ServiceVersion[]>([]);
  const [env, setEnv] = useState<(typeof ENVS)[number]>('KAI');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [everLoaded, setEverLoaded] = useState(false);
  const [builds, setBuilds] = useState<BuildRow[]>([]);

  // Returns how many watched versions it found, so a caller polling after
  // sign-in can stop the moment the page has rendered rather than guessing a
  // fixed delay.
  const load = useCallback(async (which: string): Promise<number> => {
    setBusy(true);
    try {
      const outcome = await runAction({
        actionId: 'kai.service_versions',
        input: { env: which },
      });
      const out = outcome.output as { env?: string; versions?: ServiceVersion[] };
      // Coerce defensively, then keep only the watched services. Matched by
      // exact name, case-insensitive, so "auth" does not also pull "auditai".
      const clean = (out.versions ?? [])
        .map((v) => ({
          service: String((v as ServiceVersion).service ?? ''),
          version: String((v as ServiceVersion).version ?? ''),
        }))
        .filter(
          (v) => v.service && v.version && SERVICES.includes(v.service.toLowerCase()),
        );
      setVersions(clean);
      setError(null);
      return clean.length;
    } catch (err) {
      // A runAction rejection is a structured ActionError object, not a
      // string: String(err) on it gives "[object Object]", which is what hid
      // the real reason. describeActionError pulls the message out.
      setError(isActionError(err) ? describeActionError(err) : String(err));
      return 0;
    } finally {
      setBusy(false);
      setEverLoaded(true);
    }
  }, []);

  // After "Open & sign in": the page opens in the default browser, SSO runs,
  // and the Elm app fetches and renders the versions. All of that takes longer
  // than any single fixed delay, so this polls, and stops the instant the
  // versions appear. That is the "update immediately once I'm signed in" the
  // one-shot retry never delivered: sign in, and within a few seconds of the
  // page finishing, the card fills itself with no second click.
  const openAndWatch = useCallback(() => {
    void openUrl(VERSIONS_URL).catch(() => {});
    let tries = 0;
    const tick = async () => {
      tries += 1;
      const found = await load(env);
      // Up to ~70s of patience (15 tries, 4s apart, after a 6s head start),
      // which covers a cold SSO sign-in without polling forever.
      if (found > 0 || tries >= 15) return;
      window.setTimeout(() => void tick(), 4000);
    };
    window.setTimeout(() => void tick(), 6000);
  }, [load, env]);

  // The GitHub build rows, read in parallel and independent of the SSO page
  // and the environment switch: a build's status is the same whichever Avetta
  // column is selected. Each repo fails on its own, so one being unreachable
  // never blanks the other.
  const loadBuilds = useCallback(async () => {
    const rows = await Promise.all(
      BUILD_REPOS.map(async ({ repo, label }): Promise<BuildRow> => {
        try {
          const outcome = await runAction({
            actionId: 'github.build_status',
            input: { repo },
          });
          const out = outcome.output as { run?: BuildRun | null };
          return { label, repo, run: out.run ?? null, error: null };
        } catch (err) {
          return {
            label,
            repo,
            run: null,
            error: isActionError(err) ? describeActionError(err) : String(err),
          };
        }
      }),
    );
    setBuilds(rows);
  }, []);

  useEffect(() => {
    void load(env);
    const timer = window.setInterval(() => void load(env), REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [load, env]);

  useEffect(() => {
    void loadBuilds();
    const timer = window.setInterval(() => void loadBuilds(), REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [loadBuilds]);

  // Hidden only until the first load, so no empty frame flashes on boot. Once
  // it has loaded once the card stays put: the env switcher has to remain
  // reachable, and an env with none of the watched services says so in words.
  if (!everLoaded) {
    return null;
  }

  return (
    <section className="kai" aria-label="Service versions">
      <div className="kai__head">
        <Boxes size={12} strokeWidth={2} aria-hidden="true" />
        <span className="kai__title">Service Versions</span>
        {versions.length > 0 && <span className="kai__count">{versions.length}</span>}
        <button
          className={`kai__refresh${busy ? ' kai__refresh--busy' : ''}`}
          type="button"
          onClick={() => {
            void load(env);
            void loadBuilds();
          }}
          disabled={busy}
          aria-label="Refresh service versions"
        >
          <RefreshCw size={12} strokeWidth={2} aria-hidden="true" />
        </button>
      </div>

      {/* Environment switch. The selected column is what the list below shows;
          picking another re-reads that column from the same tab. */}
      <div className="kai__envs" role="group" aria-label="Environment">
        {ENVS.map((e) => (
          <button
            key={e}
            type="button"
            className={`kai__env${e === env ? ' kai__env--on' : ''}`}
            aria-pressed={e === env}
            onClick={() => setEnv(e)}
          >
            {e}
          </button>
        ))}
      </div>

      {error && (
        <div className="kai__needs" role="alert">
          <p className="kai__error">{error}</p>
          {/* The page is behind SSO NEXUS cannot drive, so the remedy is to
              open it and sign in; then the card reads the versions from that
              tab. A button, not an instruction to do it yourself. */}
          <button
            className="kai__open"
            type="button"
            onClick={openAndWatch}
          >
            Open &amp; sign in
            <ExternalLink size={11} strokeWidth={2} aria-hidden="true" />
          </button>
        </div>
      )}

      {!error && versions.length === 0 && !busy && (
        <p className="kai__error">
          None of {SERVICES.join(', ')} are deployed to {env}.
        </p>
      )}

      {versions.length > 0 && (
        <ul className="kai__list">
          {versions.map((v) => (
            <li className="kai__row" key={v.service}>
              <span className="kai__service">{String(v.service)}</span>
              <span className="kai__dots" aria-hidden="true" />
              <span className="kai__version">{String(v.version)}</span>
            </li>
          ))}
        </ul>
      )}

      {/* The GitHub-built repos. A group of their own, under a label, because
          their version is a build run rather than a deployed service version
          and they do not follow the environment switch above. */}
      {builds.length > 0 && (
        <div className="kai__builds">
          <span className="kai__builds-label">Builds (GitHub)</span>
          <ul className="kai__list">
            {builds.map((b) => {
              const state = buildState(b.run);
              const href = b.run?.url ?? `https://github.com/${b.repo}/actions`;
              return (
                <li className={`kai__row kai__build kai__build--${state}`} key={b.repo}>
                  <button
                    className="kai__build-open"
                    type="button"
                    title={b.error ?? b.run?.displayTitle ?? 'Open Actions'}
                    onClick={() => void openUrl(href).catch(() => {})}
                  >
                    <span className="kai__service">{b.label}</span>
                    <span className={`kai__build-status kai__build-status--${state}`}>
                      {b.error ? 'unavailable' : buildLabel(b.run)}
                    </span>
                    <span className="kai__dots" aria-hidden="true" />
                    {b.run?.headBranch && (
                      <span className="kai__version kai__build-branch">{b.run.headBranch}</span>
                    )}
                    <ExternalLink size={11} strokeWidth={2} aria-hidden="true" />
                  </button>
                </li>
              );
            })}
          </ul>
        </div>
      )}
    </section>
  );
}
