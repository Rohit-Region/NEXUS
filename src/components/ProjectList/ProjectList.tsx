import { useCallback, useEffect, useState } from 'react';
import { FolderPlus, Plus, RefreshCw } from 'lucide-react';
import { createProject, listProjects } from '../../lib/nexus-db';
import type { NexusView, ProjectFormValues } from '../../types';
import type { Project } from '../../types/db';
import { ProjectCard } from '../ProjectCard/ProjectCard';
import { ProjectForm } from '../ProjectForm/ProjectForm';
import './ProjectList.css';

interface ProjectListProps {
  navigate: (view: NexusView) => void;
}

/** Empty strings become undefined so optional columns stay NULL in SQLite. */
function optional(value: string): string | undefined {
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : undefined;
}

export function ProjectList({ navigate }: ProjectListProps) {
  const [projects, setProjects] = useState<Project[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showCreateForm, setShowCreateForm] = useState(false);
  const [submitting, setSubmitting] = useState(false);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setProjects(await listProjects());
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  async function handleCreate(values: ProjectFormValues) {
    setSubmitting(true);
    setError(null);
    try {
      await createProject({
        name: values.name.trim(),
        description: optional(values.description),
        repositoryPath: optional(values.repositoryPath),
        repositoryUrl: optional(values.repositoryUrl),
      });
      setShowCreateForm(false);
      await refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setSubmitting(false);
    }
  }

  const isEmpty = !loading && projects.length === 0;

  return (
    <section className="project-list" aria-label="Projects">
      <div className="project-list__header">
        <div className="project-list__title-group">
          <h2 className="project-list__title">Projects</h2>
          {!loading && (
            <span className="project-list__count">{projects.length}</span>
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

      {showCreateForm && (
        <ProjectForm
          mode="create"
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

      {projects.length > 0 && (
        <div className="project-list__items">
          {projects.map((project) => (
            <ProjectCard
              key={project.id}
              project={project}
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
