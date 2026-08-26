import { useState } from 'react';
import type { ProjectFormMode, ProjectFormValues } from '../../types';
import type { RegistryEntry } from '../../types/db';
import { RegistrySelect } from '../RegistryPanel/RegistrySelect';
import './ProjectForm.css';

interface ProjectFormProps {
  mode: ProjectFormMode;
  /** Supplied by the parent; the form itself never calls a command. */
  ides: RegistryEntry[];
  agents: RegistryEntry[];
  initialValues?: ProjectFormValues;
  onSubmit: (values: ProjectFormValues) => Promise<void>;
  onCancel: () => void;
  submitting: boolean;
}

const EMPTY_VALUES: ProjectFormValues = {
  name: '',
  description: '',
  repositoryPath: '',
  repositoryUrl: '',
  defaultIdeId: null,
  defaultAgentId: null,
};

/**
 * Presentational form shared by create and edit flows.
 * It never touches the database: the parent supplies onSubmit.
 */
export function ProjectForm({
  mode,
  ides,
  agents,
  initialValues,
  onSubmit,
  onCancel,
  submitting,
}: ProjectFormProps) {
  const [values, setValues] = useState<ProjectFormValues>(
    initialValues ?? EMPTY_VALUES,
  );
  const [touchedName, setTouchedName] = useState(false);

  const nameIsEmpty = values.name.trim().length === 0;
  const showNameError = touchedName && nameIsEmpty;

  function setField<K extends keyof ProjectFormValues>(
    field: K,
    value: ProjectFormValues[K],
  ) {
    setValues((prev) => ({ ...prev, [field]: value }));
  }

  function handleSubmit(e: React.FormEvent<HTMLFormElement>) {
    e.preventDefault();
    setTouchedName(true);
    if (nameIsEmpty || submitting) return;
    void onSubmit(values);
  }

  return (
    <form className="project-form" onSubmit={handleSubmit} noValidate>
      <span className="project-form__heading">
        {mode === 'create' ? 'New Project' : 'Edit Project'}
      </span>

      <div className="project-form__field">
        <label className="project-form__label" htmlFor="project-form-name">
          Name <span className="project-form__required">*</span>
        </label>
        <input
          id="project-form-name"
          className={`nexus-field${showNameError ? ' nexus-field--invalid' : ''}`}
          type="text"
          value={values.name}
          onChange={(e) => setField('name', e.target.value)}
          onBlur={() => setTouchedName(true)}
          placeholder="Project name"
          disabled={submitting}
          autoComplete="off"
          spellCheck={false}
          aria-required="true"
          aria-invalid={showNameError}
        />
        {showNameError && (
          <span className="project-form__error" role="alert">
            Project name is required
          </span>
        )}
      </div>

      <div className="project-form__field">
        <label className="project-form__label" htmlFor="project-form-description">
          Description
        </label>
        <input
          id="project-form-description"
          className="nexus-field"
          type="text"
          value={values.description}
          onChange={(e) => setField('description', e.target.value)}
          placeholder="What this project is about"
          disabled={submitting}
          autoComplete="off"
        />
      </div>

      <div className="project-form__field">
        <label className="project-form__label" htmlFor="project-form-path">
          Repository Path
        </label>
        <input
          id="project-form-path"
          className="nexus-field"
          type="text"
          value={values.repositoryPath}
          onChange={(e) => setField('repositoryPath', e.target.value)}
          placeholder="/Users/you/code/project"
          disabled={submitting}
          autoComplete="off"
          spellCheck={false}
        />
      </div>

      <div className="project-form__field">
        <label className="project-form__label" htmlFor="project-form-url">
          Repository URL
        </label>
        <input
          id="project-form-url"
          className="nexus-field"
          type="text"
          value={values.repositoryUrl}
          onChange={(e) => setField('repositoryUrl', e.target.value)}
          placeholder="https://github.com/you/project"
          disabled={submitting}
          autoComplete="off"
          spellCheck={false}
        />
      </div>

      <div className="project-form__row">
        <div className="project-form__field">
          <label className="project-form__label" htmlFor="project-form-ide">
            Default IDE
          </label>
          <RegistrySelect
            id="project-form-ide"
            entries={ides}
            value={values.defaultIdeId}
            onChange={(id) => setField('defaultIdeId', id)}
            disabled={submitting}
          />
        </div>

        <div className="project-form__field">
          <label className="project-form__label" htmlFor="project-form-agent">
            Default Agent
          </label>
          <RegistrySelect
            id="project-form-agent"
            entries={agents}
            value={values.defaultAgentId}
            onChange={(id) => setField('defaultAgentId', id)}
            disabled={submitting}
          />
        </div>
      </div>

      <div className="project-form__actions">
        <button
          className="nexus-btn nexus-btn--secondary"
          type="button"
          onClick={onCancel}
          disabled={submitting}
        >
          Cancel
        </button>
        <button
          className="nexus-btn nexus-btn--primary"
          type="submit"
          disabled={submitting || nameIsEmpty}
        >
          {submitting
            ? 'Saving...'
            : mode === 'create'
              ? 'Create Project'
              : 'Save Changes'}
        </button>
      </div>
    </form>
  );
}
