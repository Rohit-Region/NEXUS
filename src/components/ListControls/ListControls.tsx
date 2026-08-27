import { Search, X } from 'lucide-react';
import type { SortOption } from '../../types';
import './ListControls.css';

interface ListControlsProps<T extends string> {
  searchValue: string;
  onSearchChange: (value: string) => void;
  searchPlaceholder: string;
  sortValue: T;
  sortOptions: SortOption<T>[];
  onSortChange: (value: T) => void;
  /** Rendered between search and sort; used for status and enabled filters. */
  filterSlot?: React.ReactNode;
  /** True when any control differs from its default. */
  isActive: boolean;
  onReset: () => void;
  disabled?: boolean;
}

/** Presentational. Holds no data state and issues no command. */
export function ListControls<T extends string>({
  searchValue,
  onSearchChange,
  searchPlaceholder,
  sortValue,
  sortOptions,
  onSortChange,
  filterSlot,
  isActive,
  onReset,
  disabled = false,
}: ListControlsProps<T>) {
  return (
    <div className="list-controls">
      <div className="list-controls__search">
        <Search
          size={12}
          strokeWidth={2}
          className="list-controls__search-icon"
          aria-hidden="true"
        />
        <input
          className="nexus-field list-controls__input"
          type="text"
          value={searchValue}
          onChange={(e) => onSearchChange(e.target.value)}
          placeholder={searchPlaceholder}
          disabled={disabled}
          autoComplete="off"
          spellCheck={false}
          aria-label={searchPlaceholder}
        />
        {searchValue.length > 0 && (
          <button
            className="list-controls__clear"
            type="button"
            onClick={() => onSearchChange('')}
            disabled={disabled}
            aria-label="Clear search"
          >
            <X size={12} strokeWidth={2.5} aria-hidden="true" />
          </button>
        )}
      </div>

      {filterSlot}

      <select
        className="nexus-select"
        value={sortValue}
        onChange={(e) => onSortChange(e.target.value as T)}
        disabled={disabled}
        aria-label="Sort order"
      >
        {sortOptions.map((option) => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
      </select>

      {isActive && (
        <button
          className="nexus-btn nexus-btn--secondary"
          type="button"
          onClick={onReset}
          disabled={disabled}
        >
          Reset
        </button>
      )}
    </div>
  );
}
