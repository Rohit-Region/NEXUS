import { ChevronRight, FolderGit2 } from 'lucide-react';
import type { Project } from '../../types/db';
import './ProjectCard.css';

interface ProjectCardProps {
  project: Project;
  /**
   * Required, not optional: an optional prop defaulting to zero would make a
   * missing count indistinguishable from a real zero (spec 006 F-11).
   */
  taskCount: number;
  onClick: () => void;
}

function taskCountLabel(count: number): string {
  return count === 1 ? '1 task' : `${count} tasks`;
}

/** Format an ISO timestamp as a short local date. */
export function formatStamp(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString('en-US', {
    month:  'short',
    day:    'numeric',
    year:   'numeric',
    hour:   '2-digit',
    minute: '2-digit',
    hour12: false,
  });
}

export function ProjectCard({ project, taskCount, onClick }: ProjectCardProps) {
  const repo = project.repositoryPath ?? project.repositoryUrl;

  return (
    <button
      className="project-card"
      type="button"
      onClick={onClick}
      aria-label={`Open project ${project.name}`}
    >
      <div className="project-card__body">
        <span className="project-card__name">{project.name}</span>

        {project.description && (
          <span className="project-card__description">{project.description}</span>
        )}

        {repo && (
          <span className="project-card__repo">
            <FolderGit2 size={11} strokeWidth={2} aria-hidden="true" />
            {repo}
          </span>
        )}
      </div>

      <div className="project-card__aside">
        <span className="nexus-chip">{taskCountLabel(taskCount)}</span>
        <span className="project-card__date">{formatStamp(project.createdAt)}</span>
        <ChevronRight size={14} strokeWidth={2} aria-hidden="true" />
      </div>
    </button>
  );
}
