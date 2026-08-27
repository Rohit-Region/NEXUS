import { useState, useEffect, useCallback } from 'react';
import { RefreshCw } from 'lucide-react';
import {
  getDbStatus,
  getDbCounts,
  createProject,
  listProjects,
} from '../../lib/nexus-db';
import { isNeedsApproval, runAction } from '../../lib/assistant';
import type { DbStatus, DbCounts, Project } from '../../types/db';
import './DbPanel.css';

export function DbPanel() {
  const [status, setStatus]     = useState<DbStatus | null>(null);
  const [counts, setCounts]     = useState<DbCounts | null>(null);
  const [projects, setProjects] = useState<Project[]>([]);
  const [loading, setLoading]   = useState(true);
  const [error, setError]       = useState<string | null>(null);
  const [newName, setNewName]   = useState('');
  const [creating, setCreating] = useState(false);
  const [deleting, setDeleting] = useState<number | null>(null);

  // ── Data fetching ────────────────────────────────────────────────────────

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [s, c, p] = await Promise.all([
        getDbStatus(),
        getDbCounts(),
        listProjects(),
      ]);
      setStatus(s);
      setCounts(c);
      setProjects(p);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // ── Actions ──────────────────────────────────────────────────────────────

  async function handleCreate() {
    const name = newName.trim();
    if (!name) return;
    setCreating(true);
    setError(null);
    try {
      await createProject({ name, description: 'Test project created by DbPanel' });
      setNewName('');
      await refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setCreating(false);
    }
  }

  async function handleDelete(id: number) {
    setDeleting(id);
    setError(null);
    try {
      // NEXUS-012: deletion has exactly one path, and it asks first. The
      // panel is a diagnostic surface, so it approves in the same breath.
      await runAction({ actionId: 'nexus.delete_project', input: { id } })
        .catch(async (err) => {
          if (!isNeedsApproval(err)) throw err;
          return runAction({
            actionId: 'nexus.delete_project',
            input: { id },
            approval: err.token,
          });
        });
      await refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setDeleting(null);
    }
  }

  function handleKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (e.key === 'Enter') void handleCreate();
  }

  // ── Render ───────────────────────────────────────────────────────────────

  return (
    <section className="db-panel" aria-label="Database verification panel">
      {/* Header */}
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
        <span className="db-panel__heading">Persistence Verification</span>
        <button
          className="db-panel__btn db-panel__btn--secondary"
          onClick={() => void refresh()}
          disabled={loading}
          aria-label="Refresh"
          style={{ height: 28, padding: '0 var(--space-sm)', fontSize: 10 }}
        >
          <RefreshCw size={12} style={{ marginRight: 4, display: 'inline' }} />
          Refresh
        </button>
      </div>

      {/* DB status */}
      {loading && !status && <p className="db-panel__loading">Connecting to database…</p>}

      {status && (
        <div className="db-panel__status">
          <span className="db-panel__status-dot db-panel__status-dot--ok" aria-hidden="true" />
          <span className="db-panel__status-text">DB INITIALIZED</span>
          <span className="db-panel__status-level">migration v{status.migrationLevel}</span>
          <span className="db-panel__status-path" title={status.dbPath}>{status.dbPath}</span>
        </div>
      )}

      {error && !status && (
        <div className="db-panel__status">
          <span className="db-panel__status-dot db-panel__status-dot--error" aria-hidden="true" />
          <span className="db-panel__status-text" style={{ color: 'var(--color-accent)' }}>
            DB ERROR
          </span>
        </div>
      )}

      {/* Counts grid */}
      {counts && (
        <div className="db-panel__counts" role="group" aria-label="Record counts">
          <CountCard label="Projects"  value={counts.projects}  />
          <CountCard label="Tasks"     value={counts.tasks}     />
          <CountCard label="AI Agents" value={counts.aiAgents}  />
          <CountCard label="IDEs"      value={counts.ides}      />
          <CountCard label="Settings"  value={counts.settings}  />
        </div>
      )}

      {/* Create project */}
      <div>
        <span className="db-panel__heading">Create Test Project</span>
        <div className="db-panel__create" style={{ marginTop: 'var(--space-sm)' }}>
          <input
            className="db-panel__input"
            type="text"
            placeholder="Project name…"
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
            onKeyDown={handleKeyDown}
            disabled={creating}
            aria-label="New project name"
          />
          <button
            className="db-panel__btn db-panel__btn--primary"
            onClick={() => void handleCreate()}
            disabled={creating || !newName.trim()}
          >
            {creating ? 'Creating…' : 'Create'}
          </button>
        </div>
      </div>

      {/* Error display */}
      {error && <p className="db-panel__error" role="alert">{error}</p>}

      {/* Project list */}
      {projects.length > 0 && (
        <div>
          <span className="db-panel__heading">Projects</span>
          <div className="db-panel__project-list" style={{ marginTop: 'var(--space-sm)' }}>
            {projects.map((p) => (
              <div key={p.id} className="db-panel__project-row">
                <div className="db-panel__project-info">
                  <span className="db-panel__project-name">{p.name}</span>
                  <span className="db-panel__project-meta">
                    id:{p.id} &nbsp;·&nbsp; {p.createdAt}
                  </span>
                </div>
                <button
                  className="db-panel__btn db-panel__btn--danger"
                  onClick={() => void handleDelete(p.id)}
                  disabled={deleting === p.id}
                  aria-label={`Delete project ${p.name}`}
                >
                  {deleting === p.id ? '…' : 'Delete'}
                </button>
              </div>
            ))}
          </div>
        </div>
      )}
    </section>
  );
}

// ── Sub-component ─────────────────────────────────────────────────────────────

function CountCard({ label, value }: { label: string; value: number }) {
  return (
    <div className="db-panel__count-card">
      <span className="db-panel__count-label">{label}</span>
      <span className="db-panel__count-value">{value}</span>
    </div>
  );
}
