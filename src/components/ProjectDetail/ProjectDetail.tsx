import { useCallback, useEffect, useState } from 'react';
import { ArrowLeft, Pencil, Trash2 } from 'lucide-react';
import {
  deleteProject,
  listAgents,
  listIdes,
  listProjects,
  updateProject,
} from '../../lib/nexus-db';
import type { NexusView, ProjectFormValues } from '../../types';
import type { Project, RegistryEntry } from '../../types/db';
import { ProjectForm } from '../ProjectForm/ProjectForm';
import { formatStamp } from '../ProjectCard/ProjectCard';
import { TaskList } from '../TaskList/TaskList';
import './ProjectDetail.css';

interface ProjectDetailProps {
  projectId: number;
  navigate: (view: NexusView) => void;
  onActiveProjectChange: (name: string | null) => void;
}

function optional(value: string): string | undefined {
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : undefined;
}

function toFormValues(project: Project): ProjectFormValues {
  return {
    name:           project.name,
    description:    project.description ?? '',
    repositoryPath: project.repositoryPath ?? '',
    repositoryUrl:  project.repositoryUrl ?? '',
    defaultIdeId:   project.defaultIdeId,
    defaultAgentId: project.defaultAgentId,
  };
}

export function ProjectDetail({
  projectId,
  navigate,
  onActiveProjectChange,
}: ProjectDetailProps) {
  const [project, setProject] = useState<Project | null>(null);
  // enabledOnly = false: a disabled entry that is still assigned must resolve
  // to a name here and stay selectable in the edit form (F-15).
  const [ides, setIdes] = useState<RegistryEntry[]>([]);
  const [agents, setAgents] = useState<RegistryEntry[]>([]);
  const [mode, setMode] = useState<'view' | 'edit'>('view');
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Reported upward by TaskList; ProjectDetail never fetches tasks itself.
  const [taskCount, setTaskCount] = useState(0);

  const handleTaskCountChange = useCallback((count: number) => {
    setTaskCount(count);
  }, []);

  // NEXUS-003 adds no single-project command; the list is the source of truth.
  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [projects, ideRows, agentRows] = await Promise.all([
        listProjects(),
        listIdes(false),
        listAgents(false),
      ]);
      setIdes(ideRows);
      setAgents(agentRows);
      const found = projects.find((p) => p.id === projectId);
      if (!found) {
        setProject(null);
        setError(`Project ${projectId} not found`);
        onActiveProjectChange(null);
        return;
      }
      setProject(found);
      onActiveProjectChange(found.name);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, [projectId, onActiveProjectChange]);

  useEffect(() => {
    void load();
  }, [load]);

  async function handleSave(values: ProjectFormValues) {
    setSaving(true);
    setError(null);
    try {
      const updated = await updateProject({
        id: projectId,
        name: values.name.trim(),
        description: optional(values.description),
        repositoryPath: optional(values.repositoryPath),
        repositoryUrl: optional(values.repositoryUrl),
        defaultIdeId: values.defaultIdeId,
        defaultAgentId: values.defaultAgentId,
      });
      setProject(updated);
      onActiveProjectChange(updated.name);
      setMode('view');
    } catch (err) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  }

  async function handleDelete() {
    setDeleting(true);
    setError(null);
    try {
      await deleteProject(projectId);
      onActiveProjectChange(null);
      navigate({ screen: 'projects' });
    } catch (err) {
      setError(String(err));
      setDeleting(false);
      setConfirmDelete(false);
    }
  }

  return (
    <section className="project-detail" aria-label="Project detail">
      <div className="project-detail__bar">
        <button
          className="nexus-btn nexus-btn--secondary"
          type="button"
          onClick={() => navigate({ screen: 'projects' })}
        >
          <ArrowLeft size={12} strokeWidth={2} aria-hidden="true" />
          Back
        </button>

        <span className="project-detail__bar-name">
          {project?.name ?? (loading ? 'Loading...' : 'Unknown project')}
        </span>

        <div className="project-detail__bar-actions">
          <button
            className="nexus-btn nexus-btn--secondary"
            type="button"
            onClick={() => {
              setConfirmDelete(false);
              setMode((prev) => (prev === 'edit' ? 'view' : 'edit'));
            }}
            disabled={!project || deleting}
            aria-pressed={mode === 'edit'}
          >
            <Pencil size={12} strokeWidth={2} aria-hidden="true" />
            {mode === 'edit' ? 'Cancel Edit' : 'Edit'}
          </button>
          <button
            className="nexus-btn nexus-btn--danger"
            type="button"
            onClick={() => {
              setMode('view');
              setConfirmDelete(true);
            }}
            disabled={!project || deleting || confirmDelete}
          >
            <Trash2 size={12} strokeWidth={2} aria-hidden="true" />
            Delete
          </button>
        </div>
      </div>

      {confirmDelete && (
        <div className="project-detail__confirm" role="alertdialog" aria-label="Confirm deletion">
          <span className="project-detail__confirm-text">
            {taskCount === 0
              ? 'Delete this project? This cannot be undone.'
              : taskCount === 1
                ? 'Delete this project and its 1 task? This cannot be undone.'
                : `Delete this project and its ${taskCount} tasks? This cannot be undone.`}
          </span>
          <div className="project-detail__confirm-actions">
            <button
              className="nexus-btn nexus-btn--secondary"
              type="button"
              onClick={() => setConfirmDelete(false)}
              disabled={deleting}
            >
              Cancel
            </button>
            <button
              className="nexus-btn nexus-btn--primary"
              type="button"
              onClick={() => void handleDelete()}
              disabled={deleting}
            >
              {deleting ? 'Deleting...' : 'Confirm Delete'}
            </button>
          </div>
        </div>
      )}

      {error && (
        <p className="project-detail__error" role="alert">
          {error}
        </p>
      )}

      {loading && <p className="project-detail__loading">Loading project...</p>}

      {project && mode === 'view' && (
        <div className="project-detail__fields">
          <Field label="Name" value={project.name} />
          <Field label="Description" value={project.description} />
          <Field label="Repository Path" value={project.repositoryPath} mono />
          <Field label="Repository URL" value={project.repositoryUrl} mono />
          <div className="project-detail__stamps">
            <Field
              label="Default IDE"
              value={resolveName(ides, project.defaultIdeId)}
            />
            <Field
              label="Default Agent"
              value={resolveName(agents, project.defaultAgentId)}
            />
          </div>
          <div className="project-detail__stamps">
            <Field label="Created" value={formatStamp(project.createdAt)} />
            <Field label="Updated" value={formatStamp(project.updatedAt)} />
          </div>
        </div>
      )}

      {project && mode === 'view' && (
        <TaskList
          projectId={project.id}
          onCountChange={handleTaskCountChange}
        />
      )}

      {project && mode === 'edit' && (
        <ProjectForm
          mode="edit"
          ides={ides}
          agents={agents}
          initialValues={toFormValues(project)}
          onSubmit={handleSave}
          onCancel={() => setMode('view')}
          submitting={saving}
        />
      )}
    </section>
  );
}

/** Resolve a registry id to a name, including entries that are now disabled. */
function resolveName(entries: RegistryEntry[], id: number | null): string | null {
  if (id === null) return null;
  const found = entries.find((e) => e.id === id);
  if (!found) return `Unknown (id ${id})`;
  return found.enabled ? found.name : `${found.name} (disabled)`;
}

function Field({
  label,
  value,
  mono,
}: {
  label: string;
  value?: string | null;
  mono?: boolean;
}) {
  const isSet = value != null && value.length > 0;
  return (
    <div className="project-detail__field">
      <span className="project-detail__field-label">{label}</span>
      <span
        className={`project-detail__field-value${
          isSet ? '' : ' project-detail__field-value--unset'
        }${mono ? ' project-detail__field-value--path' : ''}`}
      >
        {isSet ? value : 'Not set'}
      </span>
    </div>
  );
}
