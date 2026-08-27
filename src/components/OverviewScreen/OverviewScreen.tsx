import { useCallback, useEffect, useState } from 'react';
import { FolderPlus, RefreshCw } from 'lucide-react';
import { getWorkspaceSummary, listRecentTasks } from '../../lib/nexus-db';
import type { NexusView } from '../../types';
import type { TaskWithProject, WorkspaceSummary } from '../../types/db';
import { StatTile } from '../StatTile/StatTile';
import { StatusPill, TASK_STATUS_ORDER } from '../TaskCard/TaskCard';
import { formatStamp } from '../ProjectCard/ProjectCard';
import './OverviewScreen.css';

interface OverviewScreenProps {
  navigate: (view: NexusView) => void;
}

const RECENT_TASK_LIMIT = 8;

export function OverviewScreen({ navigate }: OverviewScreenProps) {
  const [summary, setSummary] = useState<WorkspaceSummary | null>(null);
  const [recent, setRecent] = useState<TaskWithProject[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

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
        <p className="overview__loading">Loading workspace summary...</p>
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

          <div className="overview__section">
            <span className="overview__section-title">Tasks by status</span>
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
          </div>

          <div className="overview__section">
            <span className="overview__section-title">Recent activity</span>
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
          </div>
        </>
      )}
    </section>
  );
}
