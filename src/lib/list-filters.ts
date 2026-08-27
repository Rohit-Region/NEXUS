/**
 * Pure list matching, comparison and sorting.
 *
 * Deliberately free of React and of any database import: every function here
 * is deterministic, side-effect free, and mutates no argument. NEXUS has no
 * frontend test runner, so isolating this logic is what makes it reviewable
 * now and testable the moment a runner exists (spec 007 overview).
 */
import type {
  Project,
  RegistryEntry,
  Task,
} from '../types/db';
import type {
  ProjectSortMode,
  RegistrySortMode,
  SortOption,
  TaskSortMode,
} from '../types';

// ── Matching ────────────────────────────────────────────────────────────────

/** Trim and lowercase once, so callers do not repeat it per item. */
export function normalizeQuery(query: string): string {
  return query.trim().toLowerCase();
}

/**
 * Case-insensitive substring match across a set of optionally-null fields.
 *
 * An empty normalized query returns true, so "no query" and "matches
 * everything" are the same code path and cannot diverge. A null field is
 * skipped, never coerced to the string "null".
 */
export function matchesQuery(
  normalized: string,
  fields: (string | null | undefined)[],
): boolean {
  if (normalized.length === 0) return true;
  return fields.some(
    (f) => typeof f === 'string' && f.toLowerCase().includes(normalized),
  );
}

// ── Comparison primitives ───────────────────────────────────────────────────

/** Case-insensitive text compare, used by every name and title sort. */
export function compareText(a: string, b: string): number {
  const la = a.toLowerCase();
  const lb = b.toLowerCase();
  return la < lb ? -1 : la > lb ? 1 : 0;
}

/**
 * ISO-8601 timestamps compare correctly as strings. The schema writes
 * strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), which is fixed width and
 * lexicographically ordered, so Date parsing would add cost and a failure
 * mode for no benefit.
 */
export function compareStamp(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

/**
 * Position within the supplied vocabulary. An unrecognised status sorts last
 * rather than first, which is what indexOf's -1 would otherwise produce.
 * The order is a parameter so this module imports no component.
 */
export function compareStatus(
  a: string,
  b: string,
  order: readonly string[],
): number {
  const rank = (s: string) => {
    const i = order.indexOf(s);
    return i === -1 ? order.length : i;
  };
  return rank(a) - rank(b);
}

/**
 * Applies a comparator, then a descending id tiebreak, over a copy.
 *
 * The tiebreak makes the order total, so no two distinct items compare equal
 * and repeated sorts are byte-identical regardless of engine stability. It
 * also matches the `created_at DESC, id DESC` convention the SQL layer uses.
 */
export function sortWithIdTiebreak<T extends { id: number }>(
  items: T[],
  compare: (a: T, b: T) => number,
): T[] {
  return [...items].sort((a, b) => {
    const primary = compare(a, b);
    return primary !== 0 ? primary : b.id - a.id;
  });
}

// ── Per-entity comparators and their labels ─────────────────────────────────

export const PROJECT_SORT_OPTIONS: SortOption<ProjectSortMode>[] = [
  { value: 'created-desc', label: 'Newest first' },
  { value: 'created-asc', label: 'Oldest first' },
  { value: 'updated-desc', label: 'Recently updated' },
  { value: 'name-asc', label: 'Name A-Z' },
  { value: 'name-desc', label: 'Name Z-A' },
];

export function projectComparator(
  mode: ProjectSortMode,
): (a: Project, b: Project) => number {
  switch (mode) {
    case 'created-asc':
      return (a, b) => compareStamp(a.createdAt, b.createdAt);
    case 'updated-desc':
      return (a, b) => compareStamp(b.updatedAt, a.updatedAt);
    case 'name-asc':
      return (a, b) => compareText(a.name, b.name);
    case 'name-desc':
      return (a, b) => compareText(b.name, a.name);
    case 'created-desc':
    default:
      return (a, b) => compareStamp(b.createdAt, a.createdAt);
  }
}

export const TASK_SORT_OPTIONS: SortOption<TaskSortMode>[] = [
  { value: 'created-desc', label: 'Newest first' },
  { value: 'created-asc', label: 'Oldest first' },
  { value: 'updated-desc', label: 'Recently updated' },
  { value: 'title-asc', label: 'Title A-Z' },
  { value: 'status', label: 'Status' },
];

export function taskComparator(
  mode: TaskSortMode,
  statusOrder: readonly string[],
): (a: Task, b: Task) => number {
  switch (mode) {
    case 'created-asc':
      return (a, b) => compareStamp(a.createdAt, b.createdAt);
    case 'updated-desc':
      return (a, b) => compareStamp(b.updatedAt, a.updatedAt);
    case 'title-asc':
      return (a, b) => compareText(a.title, b.title);
    case 'status':
      return (a, b) => compareStatus(a.status, b.status, statusOrder);
    case 'created-desc':
    default:
      return (a, b) => compareStamp(b.createdAt, a.createdAt);
  }
}

export const REGISTRY_SORT_OPTIONS: SortOption<RegistrySortMode>[] = [
  { value: 'created-desc', label: 'Newest first' },
  { value: 'created-asc', label: 'Oldest first' },
  { value: 'name-asc', label: 'Name A-Z' },
  { value: 'type-asc', label: 'Type A-Z' },
];

export function registryComparator(
  mode: RegistrySortMode,
): (a: RegistryEntry, b: RegistryEntry) => number {
  switch (mode) {
    case 'created-asc':
      return (a, b) => compareStamp(a.createdAt, b.createdAt);
    case 'name-asc':
      return (a, b) => compareText(a.name, b.name);
    case 'type-asc':
      return (a, b) => compareText(a.entryType, b.entryType);
    case 'created-desc':
    default:
      return (a, b) => compareStamp(b.createdAt, a.createdAt);
  }
}
