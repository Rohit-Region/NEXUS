import { useCallback, useEffect, useRef, useState } from 'react';
import { RemindersPanel } from '../RemindersPanel/RemindersPanel';
import { Scan } from '../Scan/Scan';
import { CoreHud } from '../CoreHud/CoreHud';
import { WeatherCard } from '../WeatherCard/WeatherCard';
import { SchedulePanel } from '../SchedulePanel/SchedulePanel';
import { JiraPanel } from '../JiraPanel/JiraPanel';
import type { JiraPanelHandle } from '../JiraPanel/JiraPanel';
import { PrPanel } from '../PrPanel/PrPanel';
import { KaiVersionsPanel } from '../KaiVersionsPanel/KaiVersionsPanel';
import { VaultPanel } from '../VaultPanel/VaultPanel';
import { StockWatchPanel } from '../StockWatchPanel/StockWatchPanel';
import type { PrPanelHandle } from '../PrPanel/PrPanel';
import { Briefing } from '../Briefing/Briefing';
import { StandupPanel } from '../StandupPanel/StandupPanel';
import { FolderPlus, ListTodo, RefreshCw } from 'lucide-react';
import { getWorkspaceSummary, listRecentTasks } from '../../lib/nexus-db';
import type { NexusView } from '../../types';
import type { TaskWithProject, WorkspaceSummary } from '../../types/db';
import { StatTile } from '../StatTile/StatTile';
import { formatStatus, StatusPill, TASK_STATUS_ORDER } from '../TaskCard/TaskCard';
import { formatStamp } from '../ProjectCard/ProjectCard';
import './OverviewScreen.css';

interface OverviewScreenProps {
  navigate: (view: NexusView) => void;
  /** Opens the assistant panel, for the suggestion card's call to action. */
  onOpenAssistant?: (greeting?: string) => void;
  /**
   * Connector ids the rail has switched on. A connector with a card here
   * shows it; everything else stays off. Weather is the only one so far.
   */
  openConnectors?: string[];
}

const RECENT_TASK_LIMIT = 8;

export function OverviewScreen({
  navigate,
  onOpenAssistant = () => {},
  openConnectors = [],
}: OverviewScreenProps) {
  const [summary, setSummary] = useState<WorkspaceSummary | null>(null);
  const [recent, setRecent] = useState<TaskWithProject[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  // Refresh has to reach Jira too. It read only the local database, so the
  // button appeared to do nothing to the one list that actually changes
  // during a working day.
  const jira = useRef<JiraPanelHandle>(null);
  const prs = useRef<PrPanelHandle>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [s, r] = await Promise.all([
        getWorkspaceSummary(),
        listRecentTasks(RECENT_TASK_LIMIT),
      ]);
      setSummary(s);
      setRecent(r);
      jira.current?.refresh();
      prs.current?.refresh();
    } catch (err) {
      setError(String(err));
      setSummary(null);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // The workspace is empty only when there is nothing to work on. Registry
  // entries alone do not count: a user may register tools before any project.
  const isEmpty =
    summary !== null && summary.projects === 0 && summary.tasks === 0;

  const statusCounts: Record<string, number> = summary
    ? {
        open: summary.tasksOpen,
        in_progress: summary.tasksInProgress,
        blocked: summary.tasksBlocked,
        done: summary.tasksDone,
      }
    : {};

  return (
    <section className="overview" aria-label="Workspace overview">
      <div className="overview__header">
        <div>
          <h2 className="overview__title">Overview</h2>
          <p className="overview__subtitle">
            Your workspace at a glance, read live from the local database.
          </p>
        </div>
        <button
          className="nexus-btn nexus-btn--secondary"
          type="button"
          onClick={() => void refresh()}
          disabled={loading}
          aria-label="Refresh overview"
        >
          <RefreshCw size={12} strokeWidth={2} aria-hidden="true" />
          Refresh
        </button>
      </div>

      {error && (
        <p className="overview__error" role="alert">
          {error}
        </p>
      )}

      {loading && !summary && (
        <Scan label="Reading workspace" rows={4} />
      )}

      {summary && isEmpty && (
        <div className="overview__empty">
          <FolderPlus size={28} strokeWidth={1.5} aria-hidden="true" />
          <span className="overview__empty-title">Workspace is empty</span>
          <span className="overview__empty-text">
            Create your first project to start tracking work. Counts and recent
            activity appear here once there is something to show.
          </span>
          <button
            className="nexus-btn nexus-btn--primary"
            type="button"
            onClick={() => navigate({ screen: 'projects' })}
          >
            Go to Projects
          </button>
        </div>
      )}

      {summary && !isEmpty && (
        <>
          {/* The command centre: your work on the left, the machine's own
              state on the right, with the core at the head of it. The core
              used to stand in a column of its own between the two, which
              gave the widest thing on the page the middle third of the
              window and left everything under it empty. */}
          <div className="cc">
            {/* Your day, in the order you need it: what it is like out
                there, what is assigned to you, and what you promised to do.
                Reminders were below the stat tiles and under the fold,
                which for the one panel with a live countdown in it made
                them easy to miss entirely. */}
            <div className="cc__left">
              {/* Off unless the rail says otherwise. It is the one card here
                  that is not about work, so it is the one that has to be
                  asked for. */}
              {openConnectors.includes('weather') && <WeatherCard />}

              {/* Your day before your tickets. It sits first because it is
                  the only card here with a deadline attached. */}
              <SchedulePanel />

              <JiraPanel ref={jira} />
              <PrPanel ref={prs} />
              <StandupPanel />

              {/* Reads the versions from your open Avetta tab. Hides itself
                  when that tab is closed, like the schedule on a clear day. */}
              <KaiVersionsPanel />

              {/* The weekly-rotated credential. Always present, because its
                  job is to nag when the week is up, and a card that hides
                  cannot nag. */}
              <VaultPanel />

              {/* Checks product pages on a 20-minute timer and shouts, across
                  every channel, the moment one comes back in stock. */}
              <StockWatchPanel />

              <RemindersPanel />

              {/* Tasks by status and Recent activity were two full-width
                  strips below the command centre. They were the same
                  subject split in half: the counts summarise exactly the
                  rows underneath them. Together they are one card, and
                  putting it here fills the space the left column was
                  leaving under the reminders. */}
              <section className="tasks" aria-label="Tasks">
                <div className="tasks__head">
                  <ListTodo size={12} strokeWidth={2} aria-hidden="true" />
                  <span className="tasks__title">Tasks</span>
                  <span className="tasks__total">{summary.tasks}</span>
                </div>

                {/*
                  The counts as a length, not just a number. Zero-count
                  statuses contribute no segment at all, so the bar is the
                  shape of the work rather than a fixed four-part strip
                  that happens to have gaps in it.
                */}
                <div
                  className="tasks__meter"
                  role="img"
                  aria-label={TASK_STATUS_ORDER.map(
                    (st) => `${formatStatus(st)} ${statusCounts[st] ?? 0}`,
                  ).join(', ')}
                >
                  {summary.tasks === 0 ? (
                    <span className="tasks__meter-empty" />
                  ) : (
                    TASK_STATUS_ORDER.filter((st) => (statusCounts[st] ?? 0) > 0).map(
                      (st) => (
                        <span
                          key={st}
                          className={`tasks__seg tasks__seg--${st}`}
                          style={{
                            flexGrow: statusCounts[st] ?? 0,
                          }}
                        />
                      ),
                    )
                  )}
                </div>

                <div className="overview__statuses">
                  {TASK_STATUS_ORDER.map((status) => (
                    <span key={status} className="overview__status-item">
                      <StatusPill status={status} />
                      <span className="overview__status-count">
                        {statusCounts[status] ?? 0}
                      </span>
                    </span>
                  ))}
                  <span className="overview__status-item">
                    <span className="nexus-chip nexus-chip--muted">Unassigned</span>
                    <span className="overview__status-count">
                      {summary.tasksUnassigned}
                    </span>
                  </span>
                </div>

                {recent.length === 0 ? (
                  <p className="overview__loading">No tasks yet.</p>
                ) : (
                  <div className="overview__recent">
                    {recent.map((entry) => (
                      <button
                        key={entry.task.id}
                        className="overview__recent-row"
                        type="button"
                        onClick={() =>
                          navigate({
                            screen: 'project-detail',
                            projectId: entry.task.projectId,
                          })
                        }
                        aria-label={`Open ${entry.projectName}, task ${entry.task.title}`}
                      >
                        <StatusPill status={entry.task.status} />
                        <span className="overview__recent-title">
                          {entry.task.title}
                        </span>
                        <span className="overview__recent-project">
                          {entry.projectName}
                        </span>
                        <span className="overview__recent-date">
                          {formatStamp(entry.task.updatedAt)}
                        </span>
                      </button>
                    ))}
                  </div>
                )}
              </section>
            </div>

            {/* Everything NEXUS knows about itself, under the clock: the
                counts the rings are already drawing, then what it thinks
                you should look at, then what it has actually been doing. */}
            <div className="cc__right">
              <CoreHud
                summary={summary}
                navigate={(screen) => navigate({ screen })}
                onActivate={() => {
                  // Refresh everything the screen is showing, then, once the
                  // core's animation has played, open the assistant asking how
                  // it can help. The delay matches the ring redraw so the
                  // question lands as the animation settles, not over it.
                  void refresh();
                  window.setTimeout(
                    () => onOpenAssistant('How can I help you, boss?'),
                    1900,
                  );
                }}
              />

              <div className="overview__tiles">
                <StatTile label="Projects" value={summary.projects} accent />
                <StatTile
                  label="Tasks"
                  value={summary.tasks}
                  detail={`${summary.tasksUnassigned} unassigned`}
                />
                <StatTile
                  label="IDEs"
                  value={summary.idesTotal}
                  detail={`${summary.idesEnabled} enabled`}
                />
                <StatTile
                  label="AI Agents"
                  value={summary.agentsTotal}
                  detail={`${summary.agentsEnabled} enabled`}
                />
              </div>

              {/* The hero is now the briefing: your news and what matters,
                  not the connector audit log it replaced. NexusSays and
                  LiveActivity are kept in the codebase for the audit view. */}
              <Briefing navigate={navigate} />
            </div>
          </div>

        </>
      )}
    </section>
  );
}
