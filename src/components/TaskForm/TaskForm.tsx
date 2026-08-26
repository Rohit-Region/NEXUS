import { useState } from 'react';
import type { TaskFormMode, TaskFormValues } from '../../types';
import type { TaskStatus } from '../../types/db';
import { StatusPill, TASK_STATUS_ORDER } from '../TaskCard/TaskCard';
import './TaskForm.css';

interface TaskFormProps {
  mode: TaskFormMode;
  initialValues?: TaskFormValues;
  onSubmit: (values: TaskFormValues) => Promise<void>;
  onCancel: () => void;
  submitting: boolean;
}

const EMPTY_VALUES: TaskFormValues = {
  title: '',
  description: '',
  status: 'open',
};

/**
 * Presentational form shared by task create and edit flows.
 * It never touches the database: the parent supplies onSubmit.
 */
export function TaskForm({
  mode,
  initialValues,
  onSubmit,
  onCancel,
  submitting,
}: TaskFormProps) {
  const [values, setValues] = useState<TaskFormValues>(
    initialValues ?? EMPTY_VALUES,
  );
  const [touchedTitle, setTouchedTitle] = useState(false);

  const titleIsEmpty = values.title.trim().length === 0;
  const showTitleError = touchedTitle && titleIsEmpty;

  function setField<K extends keyof TaskFormValues>(
    field: K,
    value: TaskFormValues[K],
  ) {
    setValues((prev) => ({ ...prev, [field]: value }));
  }

  function handleSubmit(e: React.FormEvent<HTMLFormElement>) {
    e.preventDefault();
    setTouchedTitle(true);
    if (titleIsEmpty || submitting) return;
    void onSubmit(values);
  }

  return (
    <form className="task-form" onSubmit={handleSubmit} noValidate>
      <span className="task-form__heading">
        {mode === 'create' ? 'New Task' : 'Edit Task'}
      </span>

      <div className="task-form__field">
        <label className="task-form__label" htmlFor="task-form-title">
          Title <span className="task-form__required">*</span>
        </label>
        <input
          id="task-form-title"
          className={`nexus-field${showTitleError ? ' nexus-field--invalid' : ''}`}
          type="text"
          value={values.title}
          onChange={(e) => setField('title', e.target.value)}
          onBlur={() => setTouchedTitle(true)}
          placeholder="What needs doing"
          disabled={submitting}
          autoComplete="off"
          aria-required="true"
          aria-invalid={showTitleError}
        />
        {showTitleError && (
          <span className="task-form__error" role="alert">
            Task title is required
          </span>
        )}
      </div>

      <div className="task-form__field">
        <label className="task-form__label" htmlFor="task-form-description">
          Description
        </label>
        <input
          id="task-form-description"
          className="nexus-field"
          type="text"
          value={values.description}
          onChange={(e) => setField('description', e.target.value)}
          placeholder="Optional detail"
          disabled={submitting}
          autoComplete="off"
        />
      </div>

      <fieldset className="task-form__field task-form__fieldset">
        <legend className="task-form__label">Status</legend>
        <div className="task-form__status-group">
          {TASK_STATUS_ORDER.map((status: TaskStatus) => (
            <StatusPill
              key={status}
              status={status}
              as="button"
              selected={status === values.status}
              disabled={submitting}
              onClick={() => setField('status', status)}
              ariaPressed={status === values.status}
              ariaLabel={`Set status to ${status.replace('_', ' ')}`}
            />
          ))}
        </div>
      </fieldset>

      <div className="task-form__actions">
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
          disabled={submitting || titleIsEmpty}
        >
          {submitting
            ? 'Saving...'
            : mode === 'create'
              ? 'Create Task'
              : 'Save Changes'}
        </button>
      </div>
    </form>
  );
}
