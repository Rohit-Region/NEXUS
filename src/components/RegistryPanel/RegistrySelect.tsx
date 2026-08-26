import type { RegistryEntry } from '../../types/db';

interface RegistrySelectProps {
  id?: string;
  entries: RegistryEntry[];
  value: number | null;
  onChange: (id: number | null) => void;
  emptyLabel?: string;
  ariaLabel?: string;
  disabled?: boolean;
}

/**
 * Select over a registry list.
 *
 * Offers enabled entries only (F-15), plus whatever is currently selected even
 * if it has since been disabled: otherwise opening the control would silently
 * drop an existing assignment. This rule lives here so both consumers share it.
 */
export function RegistrySelect({
  id,
  entries,
  value,
  onChange,
  emptyLabel = 'Not set',
  ariaLabel,
  disabled = false,
}: RegistrySelectProps) {
  const offerable = entries.filter((e) => e.enabled || e.id === value);
  const dangling = value !== null && !entries.some((e) => e.id === value);

  return (
    <select
      id={id}
      className="nexus-select"
      value={value ?? ''}
      onChange={(e) => onChange(e.target.value === '' ? null : Number(e.target.value))}
      disabled={disabled}
      aria-label={ariaLabel}
    >
      <option value="">{emptyLabel}</option>
      {offerable.map((entry) => (
        <option key={entry.id} value={entry.id}>
          {entry.name}
          {entry.enabled ? '' : ' (disabled)'}
        </option>
      ))}
      {dangling && <option value={value}>{`Unknown (id ${value})`}</option>}
    </select>
  );
}
