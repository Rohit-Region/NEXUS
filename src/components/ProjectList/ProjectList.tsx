import { useCallback, useEffect, useState } from 'react';
import { FolderPlus, Plus, RefreshCw } from 'lucide-react';
import {
  countTasksByProject,
  createProject,
  listAgents,
  listIdes,
  listProjects,
} from '../../lib/nexus-db';
import {
  matchesQuery,
  normalizeQuery,
  projectComparator,
  sortWithIdTiebreak,
  PROJECT_SORT_OPTIONS,
} from '../../lib/list-filters';
import type {
  NexusIntent,
  NexusView,
  ProjectFormValues,
  ProjectSortMode,
} from '../../types';
import type { Project, RegistryEntry, Settings } from '../../types/db';
import { ListControls } from '../ListControls/ListControls';
import { ProjectCard } from '../ProjectCard/ProjectCard';
import { ProjectForm } from '../ProjectForm/ProjectForm';
import './ProjectList.css';

interface ProjectListProps {
  navigate: (view: NexusView) => void;
  settings: Settings;
  /** NEXUS-009: one-shot, consumed as mount state below. */
  intent?: NexusIntent;
}

/** Empty strings become undefined so optional columns stay NULL in SQLite. */
function optional(value: string): string | undefined {
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : undefined;
}

/** The single predicate used by both the render and the R-02 hidden check. */
function projectMatches(project: Project, normalized: string): boolean {
  return matchesQuery(normalized, [
    project.name,
    project.description,
    project.repositoryPath,
    project.repositoryUrl,
  ]);
}

export function ProjectList({ navigate, settings, intent }: ProjectListProps) {
  const [projects, setProjects] = useState<Project[]>([]);
  // Create has no pre-existing assignment, so enabled-only is the right set.
  const [ides, setIdes] = useState<RegistryEntry[]>([]);
  const [agents, setAgents] = useState<RegistryEntry[]>([]);
  const [taskCounts, setTaskCounts] = useState<Map<number, number>>(new Map());
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  // One-shot by construction: seeds mount state and is never read again, so
  // it cannot re-fire on a later render.
  const [showCreateForm, setShowCreateForm] = useState(
    intent === 'create-project',
  );
  const [submitting, setSubmitting] = useState(false);

  // NEXUS-007 controls. Session-local: discarded on unmount by design.
  const [query, setQuery] = useState('');
  // Seeded from settings, not bound to them: changing the stored default must
  // not re-sort a list the user has already adjusted (spec 008 7.6).
  const [sort, setSort] = useState<ProjectSortMode>(settings.projectSort);
  // The mount-time seed is what "no filtering" means here, and what Reset
  // returns to. Comparing against a hardcoded default would make a list open
  // as already-filtered whenever the user's stored preference differs.
  const [seededSort] = useState<ProjectSortMode>(settings.projectSort);
  const [hiddenNotice, setHiddenNotice] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [rows, ideRows, agentRows, counts] = await Promise.all([
        listProjects(),
        listIdes(true),
        listAgents(true),
        countTasksByProject(),
      ]);
      setProjects(rows);
      setIdes(ideRows);
      setAgents(agentRows);
      setTaskCounts(new Map(counts.map((c) => [c.projectId, c.total])));
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // ── Controls ─────────────────────────────────────────────────────────────

  const normalized = normalizeQuery(query);
  const isActive = normalized.length > 0 || sort !== seededSort;

  // Derived on render, never stored: a stored copy would desync from state.
  const visible = sortWithIdTiebreak(
    projects.filter((p) => projectMatches(p, normalized)),
    projectComparator(sort),
  );

  function resetControls() {
    setQuery('');
    setSort(seededSort);
    setHiddenNotice(null);
  }

  // ── Actions ──────────────────────────────────────────────────────────────

  async function handleCreate(values: ProjectFormValues) {
    setSubmitting(true);
    setError(null);
    try {
      const created = await createProject({
        name: values.name.trim(),
        description: optional(values.description),
        repositoryPath: optional(values.repositoryPath),
        repositoryUrl: optional(values.repositoryUrl),
        defaultIdeId: values.defaultIdeId,
        defaultAgentId: values.defaultAgentId,
      });
      setShowCreateForm(false);
      // R-02: a save that lands outside the active controls must never look
      // like it failed. Controls are preserved; the user is told instead.
      setHiddenNotice(
        projectMatches(created, normalized) ? null : created.name,
      );
      await refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setSubmitting(false);
    }
  }

  const isEmpty = !loading && projects.length === 0;
  const isNoResults = !loading && projects.length > 0 && visible.length === 0;

  return (
    <section className="project-list" aria-label="Projects">
      <div className="project-list__header">
        <div className="project-list__title-group">
          <h2 className="project-list__title">Projects</h2>
          {!loading && (
            <span className="project-list__count">
              {isActive
                ? `${visible.length} of ${projects.length}`
                : projects.length}
            </span>
          )}
        </div>

        <div className="project-list__header-actions">
          <button
            className="nexus-btn nexus-btn--secondary"
            type="button"
            onClick={() => void refresh()}
            disabled={loading}
            aria-label="Refresh project list"
          >
            <RefreshCw size={12} strokeWidth={2} aria-hidden="true" />
            Refresh
          </button>
          <button
            className="nexus-btn nexus-btn--primary"
            type="button"
            onClick={() => setShowCreateForm((prev) => !prev)}
            aria-expanded={showCreateForm}
          >
            <Plus size={12} strokeWidth={2.5} aria-hidden="true" />
            New Project
          </button>
        </div>
      </div>

      {projects.length > 0 && (
        <ListControls
          searchValue={query}
          onSearchChange={(v) => {
            setQuery(v);
            setHiddenNotice(null);
          }}
          searchPlaceholder="Search name, description, path or URL"
          sortValue={sort}
          sortOptions={PROJECT_SORT_OPTIONS}
          onSortChange={(v) => {
            setSort(v);
            setHiddenNotice(null);
          }}
          isActive={isActive}
          onReset={resetControls}
          disabled={loading}
        />
      )}

      {hiddenNotice && (
        <div className="nexus-notice" role="status">
          <span>
            Saved &quot;{hiddenNotice}&quot;, but it is hidden by the current
            filters.
          </span>
          <div className="nexus-notice__actions">
            <button
              className="nexus-btn nexus-btn--secondary"
              type="button"
              onClick={resetControls}
            >
              Reset Filters
            </button>
            <button
              className="nexus-btn nexus-btn--secondary"
              type="button"
              onClick={() => setHiddenNotice(null)}
            >
              Dismiss
            </button>
          </div>
        </div>
      )}

      {showCreateForm && (
        <ProjectForm
          mode="create"
          ides={ides}
          agents={agents}
          initialValues={{
            name: '',
            description: '',
            repositoryPath: '',
            repositoryUrl: '',
            defaultIdeId: settings.newProjectDefaultIdeId,
            defaultAgentId: settings.newProjectDefaultAgentId,
          }}
          onSubmit={handleCreate}
          onCancel={() => setShowCreateForm(false)}
          submitting={submitting}
        />
      )}

      {error && (
        <p className="project-list__error" role="alert">
          {error}
        </p>
      )}

      {loading && <p className="project-list__loading">Loading projects...</p>}

      {isEmpty && !showCreateForm && (
        <div className="project-list__empty">
          <FolderPlus size={28} strokeWidth={1.5} aria-hidden="true" />
          <span className="project-list__empty-title">No projects yet</span>
          <span className="project-list__empty-text">
            Create your first project to start building out your NEXUS workspace.
          </span>
          <button
            className="nexus-btn nexus-btn--primary"
            type="button"
            onClick={() => setShowCreateForm(true)}
          >
            <Plus size={12} strokeWidth={2.5} aria-hidden="true" />
            Create Project
          </button>
        </div>
      )}

      {isNoResults && (
        <div className="nexus-no-results">
          <span className="nexus-no-results__title">No matching projects</span>
          <span className="nexus-no-results__text">
            {projects.length} {projects.length === 1 ? 'project exists' : 'projects exist'} in
            total. Relax the filters to see them.
          </span>
          <button
            className="nexus-btn nexus-btn--secondary"
            type="button"
            onClick={resetControls}
          >
            Reset Filters
          </button>
        </div>
      )}

      {visible.length > 0 && (
        <div className="project-list__items">
          {visible.map((project) => (
            <ProjectCard
              key={project.id}
              project={project}
              taskCount={taskCounts.get(project.id) ?? 0}
              onClick={() =>
                navigate({ screen: 'project-detail', projectId: project.id })
              }
            />
          ))}
        </div>
      )}
    </section>
  );
}
