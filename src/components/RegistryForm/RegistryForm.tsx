import { useState } from 'react';
import type { RegistryFormMode, RegistryFormValues } from '../../types';
import './RegistryForm.css';

interface RegistryFormProps {
  mode: RegistryFormMode;
  /** Copy supplied by the panel's descriptor, e.g. "IDE" / "agent". */
  singular: string;
  typeLabel: string;
  typePlaceholder: string;
  pathPlaceholder: string;
  initialValues?: RegistryFormValues;
  onSubmit: (values: RegistryFormValues) => Promise<void>;
  onCancel: () => void;
  submitting: boolean;
}

const EMPTY_VALUES: RegistryFormValues = {
  name: '',
  entryType: '',
  executablePath: '',
  enabled: true,
};

/**
 * Presentational form shared by both registry kinds and both modes.
 * It never touches the database: the panel supplies onSubmit.
 */
export function RegistryForm({
  mode,
  singular,
  typeLabel,
  typePlaceholder,
  pathPlaceholder,
  initialValues,
  onSubmit,
  onCancel,
  submitting,
}: RegistryFormProps) {
  const [values, setValues] = useState<RegistryFormValues>(
    initialValues ?? EMPTY_VALUES,
  );
  const [touched, setTouched] = useState(false);

  const nameIsEmpty = values.name.trim().length === 0;
  const typeIsEmpty = values.entryType.trim().length === 0;
  const invalid = nameIsEmpty || typeIsEmpty;

  function setField<K extends keyof RegistryFormValues>(
    field: K,
    value: RegistryFormValues[K],
  ) {
    setValues((prev) => ({ ...prev, [field]: value }));
  }

  function handleSubmit(e: React.FormEvent<HTMLFormElement>) {
    e.preventDefault();
    setTouched(true);
    if (invalid || submitting) return;
    void onSubmit(values);
  }

  const fieldId = `registry-form-${mode}-${singular.toLowerCase()}`;

  return (
    <form className="registry-form" onSubmit={handleSubmit} noValidate>
      <span className="registry-form__heading">
        {mode === 'create' ? `New ${singular}` : `Edit ${singular}`}
      </span>

      <div className="registry-form__field">
        <label className="registry-form__label" htmlFor={`${fieldId}-name`}>
          Name <span className="registry-form__required">*</span>
        </label>
        <input
          id={`${fieldId}-name`}
          className={`nexus-field${touched && nameIsEmpty ? ' nexus-field--invalid' : ''}`}
          type="text"
          value={values.name}
          onChange={(e) => setField('name', e.target.value)}
          onBlur={() => setTouched(true)}
          placeholder={`${singular} name`}
          disabled={submitting}
          autoComplete="off"
          aria-required="true"
          aria-invalid={touched && nameIsEmpty}
        />
        {touched && nameIsEmpty && (
          <span className="registry-form__error" role="alert">
            Name is required
          </span>
        )}
      </div>

      <div className="registry-form__field">
        <label className="registry-form__label" htmlFor={`${fieldId}-type`}>
          {typeLabel} <span className="registry-form__required">*</span>
        </label>
        <input
          id={`${fieldId}-type`}
          className={`nexus-field${touched && typeIsEmpty ? ' nexus-field--invalid' : ''}`}
          type="text"
          value={values.entryType}
          onChange={(e) => setField('entryType', e.target.value)}
          onBlur={() => setTouched(true)}
          placeholder={typePlaceholder}
          disabled={submitting}
          autoComplete="off"
          spellCheck={false}
          aria-required="true"
          aria-invalid={touched && typeIsEmpty}
        />
        {touched && typeIsEmpty && (
          <span className="registry-form__error" role="alert">
            Type is required
          </span>
        )}
      </div>

      <div className="registry-form__field">
        <label className="registry-form__label" htmlFor={`${fieldId}-path`}>
          Executable Path
        </label>
        <input
          id={`${fieldId}-path`}
          className="nexus-field"
          type="text"
          value={values.executablePath}
          onChange={(e) => setField('executablePath', e.target.value)}
          placeholder={pathPlaceholder}
          disabled={submitting}
          autoComplete="off"
          spellCheck={false}
        />
        <span className="registry-form__hint">
          Recorded only. NEXUS does not launch or validate it.
        </span>
      </div>

      <label className="registry-form__toggle">
        <input
          type="checkbox"
          checked={values.enabled}
          onChange={(e) => setField('enabled', e.target.checked)}
          disabled={submitting}
        />
        <span className="registry-form__toggle-text">
          Enabled (offer this {singular.toLowerCase()} when assigning)
        </span>
      </label>

      <div className="registry-form__actions">
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
          disabled={submitting || invalid}
        >
          {submitting
            ? 'Saving...'
            : mode === 'create'
              ? `Register ${singular}`
              : 'Save Changes'}
        </button>
      </div>
    </form>
  );
}
