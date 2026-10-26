import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  Briefcase,
  Search,
  ExternalLink,
  Plus,
  X,
  BellRing,
  Copy,
  AlertTriangle,
  Wand2,
  Mail,
  MessagesSquare,
  Globe,
  Newspaper,
  GraduationCap,
  Download,
  Send,
} from 'lucide-react';
import {
  addApplication,
  addSkill,
  coverLetter,
  deleteInterviewQuestion,
  exportApplicationDoc,
  generateInterviewQuestions,
  generatePlan,
  interviewFeedback,
  interviewQuestions,
  roleBrief,
  discoverJobs,
  getDocument,
  getProfile,
  listApplications,
  openUrl,
  removeApplication,
  setApplicationDescription,
  setDocument,
  tailorResume,
  updateApplication,
  type Application,
  type ApplicationStatus,
  type InterviewQuestion,
  type RoleBrief,
  type ScoredJob,
  type TailoredResume,
} from '../../lib/nexus-db';
import './CareerScreen.css';

const STAGES: { id: ApplicationStatus; label: string }[] = [
  { id: 'saved', label: 'Saved' },
  { id: 'applied', label: 'Applied' },
  { id: 'interviewing', label: 'Interviewing' },
  { id: 'offer', label: 'Offer' },
  { id: 'rejected', label: 'Closed' },
];

function errText(err: unknown): string {
  return String(err).replace(/^Error:\s*/, '');
}

/** Whole days from today (local) to a YYYY-MM-DD date. */
function daysUntil(ymd: string): number {
  const [y, m, d] = ymd.split('-').map(Number);
  const target = new Date(y, (m || 1) - 1, d || 1);
  const now = new Date();
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  return Math.round((target.getTime() - today.getTime()) / 86_400_000);
}

function dueText(days: number): string {
  if (days < 0) return `${-days}d overdue`;
  if (days === 0) return 'today';
  if (days === 1) return 'tomorrow';
  return `in ${days}d`;
}

const ELIGIBILITY_LABEL: Record<ScoredJob['eligibility'], string> = {
  open: 'Open to India',
  restricted: 'Region-restricted',
  unclear: 'Location unclear',
};

/**
 * Career: find roles worth your time, and keep your applications moving.
 *
 * Discover searches Remotive and We Work Remotely, the sources that publish
 * listings for programs to read, and ranks them against the skills and target
 * role in your Me profile. The score is keyword overlap, shown with its
 * working (what matched, what the listing asks for that you don't list). It
 * ranks; it doesn't judge fit.
 *
 * The pipeline tracks every application, including ones from LinkedIn, Naukri
 * or referrals, which NEXUS does not read. It never applies on your behalf; it
 * remembers what you did and nudges the next step you set.
 */
export function CareerScreen() {
  const [tab, setTab] = useState<'discover' | 'pipeline' | 'resume'>('discover');
  const [apps, setApps] = useState<Application[]>([]);
  const [status, setStatus] = useState<string | null>(null);

  // Discover.
  const [query, setQuery] = useState('frontend');
  const [jobs, setJobs] = useState<ScoredJob[] | null>(null);
  const [searching, setSearching] = useState(false);
  const [openOnly, setOpenOnly] = useState(true);
  const [hasSkills, setHasSkills] = useState(true);

  const loadApps = useCallback(async () => {
    try {
      setApps(await listApplications());
    } catch (err) {
      setStatus(`Could not load applications: ${errText(err)}`);
    }
  }, []);

  useEffect(() => {
    void loadApps();
    void getProfile()
      .then((p) => setHasSkills(Boolean(p.skills?.trim())))
      .catch(() => {});
  }, [loadApps]);

  const search = useCallback(async () => {
    setSearching(true);
    setStatus(null);
    try {
      setJobs(await discoverJobs(query));
    } catch (err) {
      setStatus(errText(err));
    } finally {
      setSearching(false);
    }
  }, [query]);

  const tracked = useMemo(() => new Set(apps.map((a) => a.url).filter(Boolean)), [apps]);

  const track = useCallback(
    async (j: ScoredJob) => {
      try {
        await addApplication(j.company, j.title, j.url, j.source, 'saved', j.description);
        await loadApps();
        setStatus(`Tracking "${j.title}". Find it under Pipeline, in Saved.`);
      } catch (err) {
        setStatus(`Could not track: ${errText(err)}`);
      }
    },
    [loadApps],
  );

  const followUps = useMemo(
    () =>
      apps.filter(
        (a) =>
          a.nextActionDate &&
          a.status !== 'rejected' &&
          daysUntil(a.nextActionDate) <= 0,
      ).length,
    [apps],
  );

  const shown = (jobs ?? []).filter((j) => !openOnly || j.eligibility !== 'restricted');

  return (
    <div className="career">
      <header className="career__head">
        <Briefcase size={18} strokeWidth={1.8} aria-hidden="true" />
        <div className="career__headings">
          <h1 className="career__title">Career</h1>
          <p className="career__subtitle">
            Find roles, track applications. NEXUS prepares; you decide and apply.
          </p>
        </div>
        {followUps > 0 && (
          <button className="career__due" type="button" onClick={() => setTab('pipeline')}>
            <BellRing size={12} strokeWidth={2} aria-hidden="true" />
            {followUps} follow-up{followUps === 1 ? '' : 's'} due
          </button>
        )}
      </header>

      <div className="career__tabs" role="group" aria-label="Career section">
        <button
          type="button"
          className={`career__tab${tab === 'discover' ? ' career__tab--on' : ''}`}
          aria-pressed={tab === 'discover'}
          onClick={() => setTab('discover')}
        >
          Discover
        </button>
        <button
          type="button"
          className={`career__tab${tab === 'pipeline' ? ' career__tab--on' : ''}`}
          aria-pressed={tab === 'pipeline'}
          onClick={() => setTab('pipeline')}
        >
          Pipeline <span className="career__tab-count">{apps.length}</span>
        </button>
        <button
          type="button"
          className={`career__tab${tab === 'resume' ? ' career__tab--on' : ''}`}
          aria-pressed={tab === 'resume'}
          onClick={() => setTab('resume')}
        >
          Resume
        </button>
      </div>

      {status && <p className="career__status">{status}</p>}

      {tab === 'discover' && (
        <div className="discover">
          <div className="discover__bar">
            <input
              className="career__input"
              type="text"
              placeholder="Search roles, e.g. frontend, react"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') void search();
              }}
              aria-label="Search roles"
            />
            <button className="career__btn" type="button" onClick={() => void search()} disabled={searching}>
              <Search size={13} strokeWidth={2.2} aria-hidden="true" />
              {searching ? 'Searching…' : 'Search'}
            </button>
            <label className="discover__toggle">
              <input type="checkbox" checked={openOnly} onChange={(e) => setOpenOnly(e.target.checked)} />
              Hide region-restricted
            </label>
          </div>

          <p className="career__note">
            Sources: Remotive and We Work Remotely, which publish listings for programs to read.
            LinkedIn and Naukri forbid it, so roles from there go in the Pipeline by hand.
            {!hasSkills && ' Add your skills in Me to get match scores.'}
          </p>

          {jobs === null && !searching && (
            <p className="career__empty">Search to see roles ranked against your profile.</p>
          )}
          {jobs !== null && shown.length === 0 && (
            <p className="career__empty">No roles match. Try a broader search, or show restricted ones.</p>
          )}

          <ul className="discover__list">
            {shown.map((j) => {
              const isTracked = tracked.has(j.url);
              return (
                <li className="job" key={j.url}>
                  <span
                    className={`job__score job__score--${j.score >= 60 ? 'hi' : j.score >= 35 ? 'mid' : 'lo'}`}
                    title="Keyword overlap with your Me profile. Ranks; does not judge fit."
                  >
                    {j.score}
                  </span>
                  <div className="job__body">
                    <div className="job__line">
                      <span className="job__title">{j.title}</span>
                      <span className={`job__elig job__elig--${j.eligibility}`}>{ELIGIBILITY_LABEL[j.eligibility]}</span>
                    </div>
                    <span className="job__meta">
                      {j.company || 'Unknown company'} · {j.source}
                      {j.location && ` · ${j.location}`}
                      {j.posted && ` · ${j.posted}`}
                    </span>
                    {(j.matched.length > 0 || j.gaps.length > 0) && (
                      <div className="job__chips">
                        {j.matched.map((m) => (
                          <span className="job__chip job__chip--have" key={`h-${m}`}>{m}</span>
                        ))}
                        {j.gaps.map((g) => (
                          <span className="job__chip job__chip--gap" key={`g-${g}`} title="Listed by the role; not in your skills">
                            {g}
                          </span>
                        ))}
                      </div>
                    )}
                  </div>
                  <div className="job__actions">
                    <button className="career__ghost" type="button" onClick={() => void openUrl(j.url).catch(() => {})}>
                      <ExternalLink size={11} strokeWidth={2} aria-hidden="true" />
                      Open
                    </button>
                    <button
                      className="career__ghost"
                      type="button"
                      onClick={() => void track(j)}
                      disabled={isTracked}
                    >
                      {isTracked ? 'Tracked' : 'Track'}
                    </button>
                  </div>
                </li>
              );
            })}
          </ul>
        </div>
      )}

      {tab === 'pipeline' && <Pipeline apps={apps} reload={loadApps} onStatus={setStatus} />}
      {tab === 'resume' && <ResumeEditor onStatus={setStatus} />}
    </div>
  );
}

interface PipelineProps {
  apps: Application[];
  reload: () => Promise<void>;
  onStatus: (s: string | null) => void;
}

/** Applications grouped by stage, with a manual add for roles found elsewhere. */
function Pipeline({ apps, reload, onStatus }: PipelineProps) {
  const [company, setCompany] = useState('');
  const [role, setRole] = useState('');
  const [url, setUrl] = useState('');

  const add = useCallback(async () => {
    if (!role.trim()) {
      onStatus('Name the role.');
      return;
    }
    try {
      await addApplication(company.trim(), role.trim(), url.trim(), 'manual', 'applied');
      setCompany('');
      setRole('');
      setUrl('');
      onStatus(null);
      await reload();
    } catch (err) {
      onStatus(`Could not add: ${errText(err)}`);
    }
  }, [company, role, url, reload, onStatus]);

  return (
    <div className="pipeline">
      <div className="pipeline__add">
        <input className="career__input" placeholder="Company" value={company} onChange={(e) => setCompany(e.target.value)} aria-label="Company" />
        <input className="career__input" placeholder="Role" value={role} onChange={(e) => setRole(e.target.value)} aria-label="Role" />
        <input
          className="career__input"
          placeholder="Link (optional)"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void add();
          }}
          aria-label="Link"
        />
        <button className="career__btn" type="button" onClick={() => void add()} aria-label="Add application">
          <Plus size={14} strokeWidth={2.5} aria-hidden="true" />
          Add applied role
        </button>
      </div>

      {apps.length === 0 && (
        <p className="career__empty">
          Nothing tracked yet. Track roles from Discover, or add one you applied to elsewhere.
        </p>
      )}

      {STAGES.map((stage) => {
        const inStage = apps.filter((a) => a.status === stage.id);
        if (inStage.length === 0) return null;
        return (
          <section className="stage" key={stage.id}>
            <h2 className="stage__title">
              {stage.label} <span className="stage__count">{inStage.length}</span>
            </h2>
            <ul className="stage__list">
              {inStage.map((a) => (
                <ApplicationRow key={a.id} app={a} reload={reload} onStatus={onStatus} />
              ))}
            </ul>
          </section>
        );
      })}
    </div>
  );
}

interface RowProps {
  app: Application;
  reload: () => Promise<void>;
  onStatus: (s: string | null) => void;
}

function ApplicationRow({ app, reload, onStatus }: RowProps) {
  const [nextAction, setNextAction] = useState(app.nextAction);
  const [date, setDate] = useState(app.nextActionDate ?? '');
  const [notes, setNotes] = useState(app.notes);
  const [open, setOpen] = useState(false);
  const [detailTab, setDetailTab] = useState<'notes' | 'tailor' | 'interview' | 'apply'>('notes');

  // Re-sync when the row's saved data changes underneath (after a reload).
  useEffect(() => {
    setNextAction(app.nextAction);
    setDate(app.nextActionDate ?? '');
    setNotes(app.notes);
  }, [app.nextAction, app.nextActionDate, app.notes]);

  const save = useCallback(
    async (stage: ApplicationStatus = app.status) => {
      try {
        await updateApplication(app.id, stage, nextAction, date || null, notes);
        await reload();
      } catch (err) {
        onStatus(`Could not save: ${errText(err)}`);
      }
    },
    [app.id, app.status, nextAction, date, notes, reload, onStatus],
  );

  const days = app.nextActionDate ? daysUntil(app.nextActionDate) : null;
  const urgency = days === null ? null : days < 0 ? 'over' : days <= 2 ? 'soon' : 'later';

  return (
    <li className="app">
      <div className="app__main">
        <button className="app__name" type="button" onClick={() => setOpen((o) => !o)} aria-expanded={open}>
          <span className="app__role">{app.role}</span>
          <span className="app__company">{app.company || 'No company'}</span>
        </button>
        {app.nextAction && days !== null && app.status !== 'rejected' && (
          <span className={`app__next app__next--${urgency}`}>
            {app.nextAction} · {dueText(days)}
          </span>
        )}
        <select
          className="career__select"
          value={app.status}
          onChange={(e) => void save(e.target.value as ApplicationStatus)}
          aria-label={`Stage for ${app.role}`}
        >
          {STAGES.map((s) => (
            <option key={s.id} value={s.id}>
              {s.label}
            </option>
          ))}
        </select>
        {app.url && (
          <button className="app__icon" type="button" onClick={() => void openUrl(app.url).catch(() => {})} aria-label="Open listing">
            <ExternalLink size={12} strokeWidth={2} aria-hidden="true" />
          </button>
        )}
        <button
          className="app__icon app__icon--danger"
          type="button"
          onClick={() => void removeApplication(app.id).then(reload)}
          aria-label={`Stop tracking ${app.role}`}
        >
          <X size={12} strokeWidth={2.5} aria-hidden="true" />
        </button>
      </div>

      {open && (
        <div className="app__panel">
          <div className="app__tabs" role="group" aria-label="Application tools">
            {(
              [
                ['notes', 'Notes'],
                ['tailor', 'Tailor'],
                ['interview', 'Interview prep'],
                ['apply', 'Apply'],
              ] as const
            ).map(([id, label]) => (
              <button
                key={id}
                type="button"
                className={`app__tab${detailTab === id ? ' app__tab--on' : ''}`}
                aria-pressed={detailTab === id}
                onClick={() => setDetailTab(id)}
              >
                {label}
              </button>
            ))}
          </div>
          {detailTab === 'notes' && (
        <div className="app__detail">
          <label className="app__field">
            <span>Next step</span>
            <input
              className="career__input"
              placeholder="e.g. Follow up with recruiter"
              value={nextAction}
              onChange={(e) => setNextAction(e.target.value)}
              onBlur={() => void save()}
            />
          </label>
          <label className="app__field app__field--date">
            <span>By</span>
            <input className="career__input" type="date" value={date} onChange={(e) => setDate(e.target.value)} onBlur={() => void save()} />
          </label>
          <label className="app__field app__field--wide">
            <span>Notes</span>
            <textarea
              className="career__input career__textarea"
              rows={3}
              placeholder="Contacts, salary range, interview notes…"
              value={notes}
              onChange={(e) => setNotes(e.target.value)}
              onBlur={() => void save()}
            />
          </label>
        </div>
          )}
          {detailTab === 'tailor' && <Tailoring app={app} reload={reload} onStatus={onStatus} />}
          {detailTab === 'interview' && <InterviewPrep app={app} onStatus={onStatus} />}
          {detailTab === 'apply' && (
            <ApplyPanel app={app} reload={reload} onStatus={onStatus} onGoTailor={() => setDetailTab('tailor')} />
          )}
        </div>
      )}
    </li>
  );
}

/** Copy text to the clipboard, reporting the outcome on the card. */
function copy(text: string, onStatus: (s: string | null) => void) {
  void navigator.clipboard
    .writeText(text)
    .then(() => onStatus('Copied.'))
    .catch(() => onStatus('Could not copy. Select the text and copy it manually.'));
}

/**
 * The base resume, pasted once. Tailoring only reorders its lines, so its
 * shape matters: one bullet per line starting with "-", and a "Skills:" line
 * of comma-separated skills, and the tailoring can move them for each role.
 */
function ResumeEditor({ onStatus }: { onStatus: (s: string | null) => void }) {
  const [text, setText] = useState('');
  const [loaded, setLoaded] = useState(false);
  const [dirty, setDirty] = useState(false);

  useEffect(() => {
    void getDocument('resume')
      .then((t) => setText(t))
      .catch((err) => onStatus(`Could not load your resume: ${errText(err)}`))
      .finally(() => setLoaded(true));
  }, [onStatus]);

  const save = useCallback(async () => {
    try {
      await setDocument('resume', text);
      setDirty(false);
      onStatus(text.trim() ? 'Resume saved.' : 'Resume cleared.');
    } catch (err) {
      onStatus(`Could not save: ${errText(err)}`);
    }
  }, [text, onStatus]);

  return (
    <div className="resume">
      <p className="career__note">
        Paste your resume as plain text. Put each achievement on its own line starting with
        "-", and list skills on one line as "Skills: React, TypeScript, ...". Tailoring
        reorders these lines for each role. It never rewrites or adds to them, so nothing
        you didn't write ends up on your resume.
      </p>
      <textarea
        className="career__input career__textarea resume__text"
        rows={22}
        placeholder={'Your Name - Frontend Engineer\nCompany, Title (2022 - present)\n- Achievement with a result\n- Another achievement\nSkills: React, TypeScript, CSS'}
        value={text}
        disabled={!loaded}
        onChange={(e) => {
          setText(e.target.value);
          setDirty(true);
        }}
        aria-label="Base resume"
      />
      <div className="resume__bar">
        <span className="career__note">
          {text.length.toLocaleString()} / 20,000 characters
          {dirty && ' · unsaved'}
        </span>
        <button className="career__btn" type="button" onClick={() => void save()} disabled={!dirty}>
          Save resume
        </button>
      </div>
    </div>
  );
}

/** Warnings about generated text, phrased as checks rather than verdicts. */
function Warnings({ items, basis = 'your resume' }: { items: string[]; basis?: string }) {
  if (items.length === 0) return null;
  return (
    <div className="tailor__warn" role="note">
      <AlertTriangle size={13} strokeWidth={2} aria-hidden="true" />
      <div>
        <strong>Check before sending.</strong> Not in {basis}:{' '}
        {items.map((w, i) => (
          <span key={w}>
            <code>{w}</code>
            {i < items.length - 1 ? ', ' : ''}
          </span>
        ))}
        . Fine if you're naming the role or it's a figure you can stand behind; remove it if
        it claims something you haven't done.
      </div>
    </div>
  );
}

/**
 * Per-application tailoring: the job description it works against, a
 * reordered resume with a short summary, and a cover letter. Generated text
 * comes with its warnings; the reordered resume needs none, because every line
 * in it is the user's own.
 */
function Tailoring({ app, reload, onStatus }: RowProps) {
  const [description, setDescription] = useState(app.description);
  const [busy, setBusy] = useState<'resume' | 'letter' | null>(null);
  const [resume, setResume] = useState<TailoredResume | null>(null);
  const [letter, setLetter] = useState<{ text: string; warnings: string[] } | null>(null);

  useEffect(() => setDescription(app.description), [app.description]);

  const saveDescription = useCallback(async () => {
    if (description === app.description) return;
    try {
      await setApplicationDescription(app.id, description);
      await reload();
    } catch (err) {
      onStatus(`Could not save the description: ${errText(err)}`);
    }
  }, [app.id, app.description, description, reload, onStatus]);

  const runResume = useCallback(async () => {
    setBusy('resume');
    onStatus(null);
    try {
      await saveDescription();
      setResume(await tailorResume(app.id));
      void reload();
    } catch (err) {
      onStatus(errText(err));
    } finally {
      setBusy(null);
    }
  }, [app.id, saveDescription, reload, onStatus]);

  const runLetter = useCallback(async () => {
    setBusy('letter');
    onStatus(null);
    try {
      await saveDescription();
      setLetter(await coverLetter(app.id));
      void reload();
    } catch (err) {
      onStatus(errText(err));
    } finally {
      setBusy(null);
    }
  }, [app.id, saveDescription, reload, onStatus]);

  // Show the last saved versions until something new is generated.
  const resumeText = resume
    ? [resume.summary, resume.reordered].filter(Boolean).join('\n\n')
    : app.tailoredResume;
  const letterText = letter ? letter.text : app.coverLetter;

  return (
    <div className="tailor">
      <label className="app__field app__field--wide">
        <span>Job description</span>
        <textarea
          className="career__input career__textarea"
          rows={4}
          placeholder="Paste the job description. Roles tracked from Discover fill this in."
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          onBlur={() => void saveDescription()}
        />
      </label>

      <div className="tailor__actions">
        <button className="career__btn" type="button" onClick={() => void runResume()} disabled={busy !== null}>
          <Wand2 size={13} strokeWidth={2} aria-hidden="true" />
          {busy === 'resume' ? 'Tailoring…' : 'Tailor resume'}
        </button>
        <button className="career__btn" type="button" onClick={() => void runLetter()} disabled={busy !== null}>
          <Mail size={13} strokeWidth={2} aria-hidden="true" />
          {busy === 'letter' ? 'Writing…' : 'Write cover letter'}
        </button>
      </div>

      {resumeText && (
        <div className="tailor__out">
          <div className="tailor__out-head">
            <span>Tailored resume</span>
            <button className="career__ghost" type="button" onClick={() => copy(resumeText, onStatus)}>
              <Copy size={11} strokeWidth={2} aria-hidden="true" />
              Copy
            </button>
          </div>
          {resume && (
            <p className="career__note">
              Your own lines, reordered for this role
              {resume.moved.length > 0 && `; ${resume.moved.length} moved up`}
              {resume.matched.length > 0 && ` (matching ${resume.matched.slice(0, 6).join(', ')})`}.
              {resume.summary && ' The summary at the top is written by NEXUS.'}
              {resume.note && ` ${resume.note}`}
            </p>
          )}
          {resume && <Warnings items={resume.warnings} />}
          <pre className="tailor__text">{resumeText}</pre>
        </div>
      )}

      {letterText && (
        <div className="tailor__out">
          <div className="tailor__out-head">
            <span>Cover letter · composed by NEXUS</span>
            <button className="career__ghost" type="button" onClick={() => copy(letterText, onStatus)}>
              <Copy size={11} strokeWidth={2} aria-hidden="true" />
              Copy
            </button>
          </div>
          {letter && <Warnings items={letter.warnings} />}
          <pre className="tailor__text">{letterText}</pre>
        </div>
      )}
    </div>
  );
}

/**
 * Interview prep for one application.
 *
 * The role brief is built from the job description and resume, never
 * generated: asked for a company brief, the local model invented a founding
 * year, a headcount and a funding round. So company facts are left to the
 * user's own search, one click away, and NEXUS sticks to what it can know:
 * which requirements the resume covers, which it does not, and practice.
 */
function InterviewPrep({ app, onStatus }: { app: Application; onStatus: (s: string | null) => void }) {
  const [brief, setBrief] = useState<RoleBrief | null>(null);
  const [briefError, setBriefError] = useState<string | null>(null);
  const [questions, setQuestions] = useState<InterviewQuestion[]>([]);
  const [busy, setBusy] = useState<string | null>(null);

  useEffect(() => {
    void roleBrief(app.id)
      .then((b) => {
        setBrief(b);
        setBriefError(null);
      })
      .catch((err) => setBriefError(errText(err)));
    void interviewQuestions(app.id)
      .then(setQuestions)
      .catch(() => setQuestions([]));
  }, [app.id, app.description]);

  const learnGap = useCallback(
    async (gap: string) => {
      setBusy(`learn-${gap}`);
      try {
        const id = await addSkill(gap);
        onStatus(`Added "${gap}" to Learn. Drafting its plan…`);
        await generatePlan(id);
        onStatus(`"${gap}" is in Learn with a plan ready.`);
      } catch (err) {
        onStatus(`Could not add to Learn: ${errText(err)}`);
      } finally {
        setBusy(null);
      }
    },
    [onStatus],
  );

  const generate = useCallback(async () => {
    setBusy('generate');
    onStatus(null);
    try {
      setQuestions(await generateInterviewQuestions(app.id));
    } catch (err) {
      onStatus(errText(err));
    } finally {
      setBusy(null);
    }
  }, [app.id, onStatus]);

  const search = (q: string, news = false) =>
    void openUrl(
      `https://www.google.com/search?q=${encodeURIComponent(q)}${news ? '&tbm=nws' : ''}`,
    ).catch(() => {});

  const company = app.company.trim();

  return (
    <div className="prep">
      <section className="prep__block">
        <h3 className="prep__title">What this role asks for</h3>
        {briefError && <p className="career__note">{briefError}</p>}
        {brief && !brief.found && (
          <p className="career__note">
            No "Must have" or "Requirements" list found in the description, so there is
            nothing to check against. The practice questions below still work.
          </p>
        )}
        {brief && brief.found && (
          <>
            <div className="prep__row">
              <span className="prep__label">Your resume shows</span>
              <div className="job__chips">
                {brief.have.length === 0 && <span className="career__note">none of them yet</span>}
                {brief.have.map((h) => (
                  <span className="job__chip job__chip--have" key={h}>{h}</span>
                ))}
              </div>
            </div>
            <div className="prep__row">
              <span className="prep__label">Not on your resume</span>
              <div className="job__chips">
                {brief.gaps.length === 0 && <span className="career__note">nothing, you cover the list</span>}
                {brief.gaps.map((g) => (
                  <span className="prep__gap" key={g}>
                    <span className="job__chip job__chip--gap">{g}</span>
                    <button
                      className="prep__learn"
                      type="button"
                      onClick={() => void learnGap(g)}
                      disabled={busy !== null}
                      title={`Add ${g} to Learn with a plan`}
                    >
                      <GraduationCap size={10} strokeWidth={2} aria-hidden="true" />
                      {busy === `learn-${g}` ? '…' : 'Learn'}
                    </button>
                  </span>
                ))}
              </div>
            </div>
            <p className="career__note">
              Expect questions on the gaps. If you have real experience with one that just
              isn't on your resume, add it there; if not, say so honestly and show how you'd
              pick it up.
            </p>
          </>
        )}
      </section>

      <section className="prep__block">
        <h3 className="prep__title">Research the company yourself</h3>
        <p className="career__note">
          NEXUS doesn't write company facts: tested on a real company, the local model invented
          its founding year, headcount and funding. Use real sources.
        </p>
        {company ? (
          <div className="tailor__actions">
            <button className="career__ghost" type="button" onClick={() => search(company)}>
              <Globe size={11} strokeWidth={2} aria-hidden="true" />
              {company}
            </button>
            <button className="career__ghost" type="button" onClick={() => search(company, true)}>
              <Newspaper size={11} strokeWidth={2} aria-hidden="true" />
              Recent news
            </button>
            {app.url && (
              <button className="career__ghost" type="button" onClick={() => void openUrl(app.url).catch(() => {})}>
                <ExternalLink size={11} strokeWidth={2} aria-hidden="true" />
                The listing
              </button>
            )}
          </div>
        ) : (
          <p className="career__note">Add the company name to this application to get search links.</p>
        )}
      </section>

      {brief && (
        <section className="prep__block">
          <h3 className="prep__title">Questions to ask them</h3>
          <ul className="prep__ask">
            {brief.askThem.map((q) => (
              <li key={q}>{q}</li>
            ))}
          </ul>
        </section>
      )}

      <section className="prep__block">
        <div className="prep__head">
          <h3 className="prep__title">Mock interview</h3>
          <button className="career__btn" type="button" onClick={() => void generate()} disabled={busy !== null}>
            <MessagesSquare size={13} strokeWidth={2} aria-hidden="true" />
            {busy === 'generate' ? 'Writing questions…' : questions.length ? 'New questions' : 'Generate questions'}
          </button>
        </div>
        <p className="career__note">
          Questions come from this job and your resume. Answer as you would out loud, then get
          feedback. The coach is a practice partner, not a grader, and can be wrong on technical
          details.
        </p>
        <ol className="prep__questions">
          {questions.map((q) => (
            <PracticeQuestion
              key={q.id}
              item={q}
              onUpdate={(next) => setQuestions((cur) => cur.map((x) => (x.id === next.id ? next : x)))}
              onDelete={() => {
                void deleteInterviewQuestion(q.id).then(() =>
                  setQuestions((cur) => cur.filter((x) => x.id !== q.id)),
                );
              }}
              onStatus={onStatus}
            />
          ))}
        </ol>
      </section>
    </div>
  );
}

interface PracticeProps {
  item: InterviewQuestion;
  onUpdate: (next: InterviewQuestion) => void;
  onDelete: () => void;
  onStatus: (s: string | null) => void;
}

function PracticeQuestion({ item, onUpdate, onDelete, onStatus }: PracticeProps) {
  const [answer, setAnswer] = useState(item.answer);
  const [busy, setBusy] = useState(false);

  const ask = useCallback(async () => {
    setBusy(true);
    onStatus(null);
    try {
      onUpdate(await interviewFeedback(item.id, answer));
    } catch (err) {
      onStatus(errText(err));
    } finally {
      setBusy(false);
    }
  }, [item.id, answer, onUpdate, onStatus]);

  return (
    <li className="practice">
      <div className="practice__q">
        <span>{item.question}</span>
        <button className="app__icon app__icon--danger" type="button" onClick={onDelete} aria-label="Remove question">
          <X size={12} strokeWidth={2.5} aria-hidden="true" />
        </button>
      </div>
      <textarea
        className="career__input career__textarea"
        rows={3}
        placeholder="Your answer…"
        value={answer}
        onChange={(e) => setAnswer(e.target.value)}
        aria-label="Your answer"
      />
      <div>
        <button className="career__ghost" type="button" onClick={() => void ask()} disabled={busy || answer.trim().length < 10}>
          {busy ? 'Thinking…' : item.feedback ? 'Get feedback again' : 'Get feedback'}
        </button>
      </div>
      {item.feedback && (
        <div className="tailor__out">
          <div className="tailor__out-head">
            <span>Feedback · composed by NEXUS</span>
          </div>
          <Warnings items={item.warnings} basis="your resume or your answer" />
          <pre className="tailor__text">{item.feedback}</pre>
        </div>
      )}
    </li>
  );
}

/** Today plus `n` days, as a local YYYY-MM-DD. */
function plusDays(n: number): string {
  const d = new Date();
  d.setDate(d.getDate() + n);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
}

/** The contact fields a form asks for, from the Me profile. */
const CONTACT_FIELDS: { key: string; label: string }[] = [
  { key: 'display_name', label: 'Name' },
  { key: 'contact_email', label: 'Email' },
  { key: 'contact_phone', label: 'Phone' },
  { key: 'contact_linkedin', label: 'LinkedIn' },
  { key: 'contact_portfolio', label: 'Portfolio' },
];

interface ApplyProps extends RowProps {
  onGoTailor: () => void;
}

/**
 * Assisted apply: everything an application form needs, one click each, in
 * the order you need it. NEXUS never fills in a form or presses submit.
 * LinkedIn and Naukri forbid automating that, and an application sent without
 * your eyes on it is not one you can stand behind. You paste, you attach, you
 * submit; NEXUS then moves the role to Applied and sets a follow-up.
 */
function ApplyPanel({ app, reload, onStatus, onGoTailor }: ApplyProps) {
  const [profile, setProfile] = useState<Record<string, string>>({});

  useEffect(() => {
    void getProfile()
      .then(setProfile)
      .catch(() => setProfile({}));
  }, []);

  const save = useCallback(
    async (kind: 'resume' | 'cover') => {
      try {
        const path = await exportApplicationDoc(app.id, kind);
        onStatus(`Saved to ${path}. It's shown in Finder, ready to attach.`);
      } catch (err) {
        onStatus(errText(err));
      }
    },
    [app.id, onStatus],
  );

  const submitted = useCallback(async () => {
    try {
      await updateApplication(app.id, 'applied', 'Follow up if no reply', plusDays(7), app.notes);
      await reload();
      onStatus(`Marked as applied. Follow-up set for ${plusDays(7)}.`);
    } catch (err) {
      onStatus(`Could not update: ${errText(err)}`);
    }
  }, [app.id, app.notes, reload, onStatus]);

  const missing = CONTACT_FIELDS.filter((f) => !profile[f.key]?.trim());

  return (
    <div className="apply">
      <p className="career__note">
        NEXUS never fills in or submits the form. It gets everything ready; you paste, attach and
        click submit.
      </p>

      <ol className="apply__steps">
        <li className="apply__step">
          <span className="apply__label">Open the application</span>
          {app.url ? (
            <button className="career__ghost" type="button" onClick={() => void openUrl(app.url).catch(() => {})}>
              <ExternalLink size={11} strokeWidth={2} aria-hidden="true" />
              Open listing
            </button>
          ) : (
            <span className="career__note">No link saved for this role. Add it in Notes or apply from where you found it.</span>
          )}
        </li>

        <li className="apply__step">
          <span className="apply__label">Resume</span>
          {app.tailoredResume ? (
            <div className="tailor__actions">
              <button className="career__ghost" type="button" onClick={() => copy(app.tailoredResume, onStatus)}>
                <Copy size={11} strokeWidth={2} aria-hidden="true" />
                Copy text
              </button>
              <button className="career__ghost" type="button" onClick={() => void save('resume')}>
                <Download size={11} strokeWidth={2} aria-hidden="true" />
                Save file to attach
              </button>
            </div>
          ) : (
            <button className="career__ghost" type="button" onClick={onGoTailor}>
              Tailor it first
            </button>
          )}
        </li>

        <li className="apply__step">
          <span className="apply__label">Cover letter</span>
          {app.coverLetter ? (
            <div className="tailor__actions">
              <button className="career__ghost" type="button" onClick={() => copy(app.coverLetter, onStatus)}>
                <Copy size={11} strokeWidth={2} aria-hidden="true" />
                Copy text
              </button>
              <button className="career__ghost" type="button" onClick={() => void save('cover')}>
                <Download size={11} strokeWidth={2} aria-hidden="true" />
                Save file to attach
              </button>
            </div>
          ) : (
            <button className="career__ghost" type="button" onClick={onGoTailor}>
              Write it first
            </button>
          )}
        </li>

        <li className="apply__step">
          <span className="apply__label">Your details</span>
          <div className="tailor__actions">
            {CONTACT_FIELDS.filter((f) => profile[f.key]?.trim()).map((f) => (
              <button
                key={f.key}
                className="career__ghost"
                type="button"
                onClick={() => copy(profile[f.key].trim(), onStatus)}
                title={profile[f.key]}
              >
                <Copy size={11} strokeWidth={2} aria-hidden="true" />
                {f.label}
              </button>
            ))}
          </div>
          {missing.length > 0 && (
            <span className="career__note">
              Add {missing.map((m) => m.label).join(', ')} in Me for one-click copy.
            </span>
          )}
        </li>

        <li className="apply__step">
          <span className="apply__label">Submitted?</span>
          <button className="career__btn" type="button" onClick={() => void submitted()}>
            <Send size={12} strokeWidth={2} aria-hidden="true" />
            {app.status === 'applied' ? 'Reset follow-up to 7 days' : "I've submitted it"}
          </button>
          <span className="career__note">Moves it to Applied and reminds you to follow up in a week.</span>
        </li>
      </ol>
    </div>
  );
}
