# NEXUS-007: List Search, Filter & Sort

## Overview

Three milestones have deferred list controls. Every list in NEXUS is creation-date descending with no way to narrow it: NEXUS-003 deferred "Project search or filtering" and "Project sorting controls", NEXUS-004 deferred "Task filtering, search, sorting controls", and NEXUS-005 deferred "Registry search, filtering, sorting". NEXUS-007 discharges all three.

### The defining property of this milestone

**NEXUS-007 requires no new commands and no database changes.**

Every list component already holds its complete dataset in memory. `ProjectList` fetches all projects. `TaskList` fetches all tasks for its project. `RegistryPanel` fetches all entries with `enabledOnly = false`. Filtering and sorting are pure functions over arrays that are already present in component state.

Adding SQL `WHERE` and `ORDER BY` commands would mean an IPC round trip per keystroke and additional command registrations to do what `Array.prototype.filter` does locally, against datasets that a local single-user command center will never grow large enough to justify. NEXUS-007 is therefore an entirely frontend milestone: no Rust module added, none modified, no schema, no migration, no IPC surface change.

### The consequence, stated plainly

The project has no frontend test framework. `package.json` devDependencies are `@tauri-apps/cli`, `@types/react`, `@types/react-dom`, `@vitejs/plugin-react`, `typescript`, and `vite`. Adding a test runner is a new dependency, and the no-new-dependencies rule remains in force by explicit decision.

**NEXUS-007 therefore has no automated test coverage of its own.** Its correctness rests entirely on the manual verification checklist in section 9.5, and on the discipline of section 7.1: all filtering, sorting, and matching logic is extracted into pure, deterministic, side-effect-free functions in one module, so that the logic is isolated, reviewable, and ready to be tested the moment a runner is introduced.

This is a known and accepted weakness of this milestone, not an oversight. It is recorded here so that a future decision to add a test runner has a concrete first target.

### Locked decisions carried into this milestone

- **No global cross-entity search.** Search stays local to each list. Unified search across projects, tasks, IDEs, and agents is deferred to NEXUS-009, where it can share the command-palette infrastructure.
- **No global search Rust command and no search screen.**
- **The `CommandBar` is not touched.**
- **No new frontend testing dependency.**

### Dependency on outstanding verification

NEXUS-004 and NEXUS-005 manual verification remains outstanding. NEXUS-007 modifies `ProjectList`, `TaskList`, and `RegistryPanel`, all of which are in that unverified surface, and it changes what those lists display. If the NEXUS-004 or NEXUS-005 pass turns up defects in those components, fix them before starting T-03.

---

## 1. Existing State

### 1.1 Assumed baseline

This specification is written against the state after NEXUS-006. If NEXUS-007 is implemented before NEXUS-006 (see 1.4), read every reference to twenty-four commands as twenty, and ignore the `taskCount` prop on `ProjectCard`.

### 1.2 What each list currently holds

| Component       | Fetches                | Holds in state                                 | Current order                       |
| --------------- | ---------------------- | ---------------------------------------------- | ----------------------------------- |
| `ProjectList`   | `listProjects()`       | `Project[]`, all rows                          | `created_at DESC` from SQL          |
| `TaskList`      | `listTasks(projectId)` | `Task[]`, all rows for one project             | `created_at DESC, id DESC` from SQL |
| `RegistryPanel` | `kind.list(false)`     | `RegistryEntry[]`, all rows including disabled | `created_at DESC, id DESC` from SQL |

Each also holds supporting data: `ProjectList` holds enabled IDE and agent lists for its create form; `TaskList` holds the full agent list for assignment; `RegistryPanel` holds project usage counts.

### 1.3 Established list conventions

- Each list owns its own fetch in a `useCallback` named `refresh`, invoked from a `useEffect`.
- Each mutation handler awaits its command, then awaits `refresh()`.
- Single-open edit and single-open delete confirmation, tracked as `number | null`, never a set.
- Empty state shown when `!loading && items.length === 0`.
- Header shows a title and a count.
- Errors render in a `role="alert"` paragraph.

NEXUS-007 preserves every one of these.

### 1.4 Relationship to NEXUS-006

NEXUS-006 and NEXUS-007 are **independent**. Neither depends on the other's behaviour. They may be implemented in either order or in parallel.

Their only overlap is that both modify `ProjectList.tsx` and `RegistryPanel.tsx`, for unrelated reasons. That is a merge risk if the two are developed simultaneously on separate branches, not a logical dependency.

---

## 2. Requirements

### 2.1 Functional Requirements

**Search**

| ID   | Requirement                                                                                                                          |
| ---- | ------------------------------------------------------------------------------------------------------------------------------------ |
| F-01 | Each of the three lists must provide a free-text search input.                                                                       |
| F-02 | Search must be case-insensitive in both directions: a lowercase query must match uppercase content and the reverse.                  |
| F-03 | Search must match on a substring, not require a prefix or a whole word.                                                              |
| F-04 | Search must be trimmed of leading and trailing whitespace before matching. A query of only whitespace must behave as an empty query. |
| F-05 | Project search must match against name, description, repository path, and repository URL.                                            |
| F-06 | Task search must match against title and description.                                                                                |
| F-07 | Registry search must match against name, type, and executable path.                                                                  |
| F-08 | A field that is `null` must never match and must never throw.                                                                        |
| F-09 | A query matching any one declared field must include the item. Fields are combined with OR.                                          |
| F-10 | Clearing the search input must restore the full list immediately, subject to any other active filter.                                |
| F-11 | A visible control must clear the search in one action when a query is present.                                                       |

**Sort**

| ID   | Requirement                                                                                                                                                                                    |
| ---- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| F-12 | Each list must provide a sort control offering that list's declared sort modes.                                                                                                                |
| F-13 | Every sort must be total and deterministic: two items that compare equal on the sort key must be ordered by `id` descending, and repeated sorts of the same data must produce identical order. |
| F-14 | Project sort modes: newest first, oldest first, recently updated, name A to Z, name Z to A.                                                                                                    |
| F-15 | Task sort modes: newest first, oldest first, recently updated, title A to Z, status.                                                                                                           |
| F-16 | Registry sort modes: newest first, oldest first, name A to Z, type A to Z.                                                                                                                     |
| F-17 | Sorting by status must order by the position of the status in `TASK_STATUS_ORDER`, not alphabetically. A status outside the vocabulary must sort last and must not throw.                      |
| F-18 | Name and title sorts must be case-insensitive.                                                                                                                                                 |

**Filter**

| ID   | Requirement                                                                                                                           |
| ---- | ------------------------------------------------------------------------------------------------------------------------------------- |
| F-19 | `TaskList` must offer a multi-select status filter with one toggle per entry in `TASK_STATUS_ORDER`.                                  |
| F-20 | Selecting multiple statuses must show tasks matching any of them. Statuses combine with OR.                                           |
| F-21 | An empty status selection must mean no status filtering, showing all statuses. It must never mean show nothing.                       |
| F-22 | `RegistryPanel` must offer an enabled filter with three states: all, enabled only, disabled only.                                     |
| F-23 | Search and filter must combine with AND: an item must satisfy the search and the filter to be shown.                                  |
| F-24 | A visible indicator must show that filtering is active, and a single control must reset all controls for that list to their defaults. |

**States**

| ID   | Requirement                                                                                                                                                                                                                  |
| ---- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| F-25 | The existing empty state must continue to appear only when the underlying list is genuinely empty.                                                                                                                           |
| F-26 | A distinct no-results state must appear when the underlying list is non-empty but no item passes the active controls. It must be visually and textually distinguishable from the empty state and must offer a reset control. |
| F-27 | Each list header must show the visible count and the total when controls are active, and the total alone when they are not.                                                                                                  |

**Mutations under active controls**

| ID   | Requirement                                                                                                                                                                    |
| ---- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| F-28 | Creating, editing, or deleting an item while controls are active must never make the mutation appear to have failed or to have lost data. Exact behaviour is specified in 2.3. |
| F-29 | When a create or edit produces an item that does not pass the active controls, a notice must state that the item exists but is hidden, and offer to reset the controls.        |
| F-30 | Deleting a visible item must remove it from the view and decrement both the visible and total counts. No notice is required.                                                   |
| F-31 | Active controls must never be silently reset by a mutation.                                                                                                                    |

### 2.2 Non-Functional Requirements

| ID   | Requirement                                                                                                                                                                                                                                 |
| ---- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| N-01 | No new Rust or frontend dependencies, including no test framework.                                                                                                                                                                          |
| N-02 | No new Tauri commands. The registered command count is unchanged.                                                                                                                                                                           |
| N-03 | No database change of any kind: no migration, no table, no column, no index.                                                                                                                                                                |
| N-04 | No Rust file is created or modified. `cargo test --lib` must report an identical test count before and after this milestone.                                                                                                                |
| N-05 | All matching, filtering, and sorting logic lives in `src/lib/list-filters.ts` as pure functions with no imports from `src/lib/nexus-db.ts` and no React imports.                                                                            |
| N-06 | No component imports `@tauri-apps/api`.                                                                                                                                                                                                     |
| N-07 | Filtering happens client-side over data already in component state. No component issues an additional command in response to a keystroke, a sort change, or a filter toggle.                                                                |
| N-08 | `AppShell`, `Dashboard`, `App.tsx`, and `NexusScreen` are unchanged. NEXUS-007 adds no screen and no navigation.                                                                                                                            |
| N-09 | `Logo`, `StatusBar`, `CommandBar`, `DbPanel`, `ProjectCard`, `ProjectForm`, `ProjectDetail`, `TaskCard`, `TaskForm`, `RegistryCard`, `RegistryForm`, `RegistrySelect`, `RegistryScreen`, `OverviewScreen`, and `StatTile` are not modified. |
| N-10 | The task status vocabulary comes from `TASK_STATUS_ORDER` in `TaskCard`. It is not re-declared.                                                                                                                                             |
| N-11 | Control state is local to each list component. No context, no global store, no lifting into `AppShell`.                                                                                                                                     |
| N-12 | Local-first. No remote search, no indexing service.                                                                                                                                                                                         |

### 2.3 Design Principle: mutations must never look like data loss

This is the most important behavioural rule in the milestone and the one most likely to be got wrong.

**The failure mode.** A user filters `TaskList` to `blocked`. They click New Task, fill in a title, and submit. The task is created with status `open`, `refresh()` runs, the filter reapplies, and the new task is not in the visible set. From the user's seat: the form closed and nothing happened. The reasonable conclusion is that the create failed. If they retry, they create duplicates.

The same applies to an edit that moves an item out of the filtered set: changing a task's status from `blocked` to `done` while filtering `blocked` makes the row vanish the instant it is saved.

**Rejected resolutions.**

- *Clear the filter automatically on mutation.* Discards a choice the user made deliberately, and does it invisibly.
- *Pin the mutated item into the view regardless of the filter.* The row appears, then disappears on the next refresh. Worse than either consistent behaviour.
- *Disable creating while filtered.* Hostile, and does nothing for the edit case.

**The specified behaviour.**

| Rule | Behaviour                                                                                                                                                                                            |
| ---- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| R-01 | Controls are never reset by a mutation. The user's filter survives create, edit, and delete.                                                                                                         |
| R-02 | After a successful create or edit, the component evaluates whether the resulting item passes the active controls, using the same pure predicate the list render uses.                                |
| R-03 | If it passes, no notice is shown. The item appears in the list in its sorted position.                                                                                                               |
| R-04 | If it does not pass, an inline notice appears directly above the list, stating that the item was saved and is hidden by the current controls, naming the item, and offering a Reset Controls action. |
| R-05 | The notice persists until the user resets the controls, changes any control, dismisses it, or performs another mutation. It is not a timed toast.                                                    |
| R-06 | Delete requires no notice: the row disappearing is the confirmation. Both counts decrement.                                                                                                          |
| R-07 | The header count reads `{visible} of {total}` whenever any control is active, so a hidden item is always accounted for numerically even if the notice is dismissed.                                  |
| R-08 | Resetting controls from the notice must reveal the item that triggered it.                                                                                                                           |

R-07 is the safety net. Even with every notice dismissed, the difference between the visible and total counts always tells the user that data exists beyond the current view.

### 2.4 Design Principle: stable, total ordering

`Array.prototype.sort` is required to be stable from ES2019, and the bundled runtime honours that. NEXUS-007 does not rely on it.

Every comparator ends with an explicit `id` descending tiebreak, producing a total order in which no two distinct items compare equal. Ordering is then a property of the comparator rather than of the engine, repeated sorts of the same data are byte-identical, and the frontend ordering rule matches the `created_at DESC, id DESC` convention already used by `list_tasks`, `list_ides`, and `list_agents`.

F-13 is verifiable because of this: sorting twice must produce the same order, and that can be checked by inspection.

### 2.5 Design Principle: empty is not the same as no results

Two states that look similar and mean opposite things:

- **Empty:** the underlying list has no items. The correct action is to create one. This state already exists in all three components and its copy and its create button are unchanged.
- **No results:** the underlying list has items, and the controls exclude all of them. The correct action is to relax the controls. Offering a create button here would be misleading, since creating another item may well be hidden too.

The distinction is `items.length === 0` against `items.length > 0 && visible.length === 0`. Conflating them is a defect, and F-25 and F-26 exist to make it observable.

### 2.6 Design Principle: control state is local and resets on unmount

Control state lives in `useState` inside each list component. When the component unmounts, the state is discarded.

Consequences, all intentional:

- Navigating from the projects screen to a project detail and back resets the project list controls.
- Opening a different project resets the task controls, which is required: a status filter from project A has no meaning in project B, and carrying it over would show a filtered task list with no explanation.
- Switching to the registry screen and back resets the registry controls.
- Controls do not survive an application restart.

This is specified rather than incidental. A future milestone wanting sticky controls would need a deliberate decision about scope and lifetime. NEXUS-008 persists *default* sort modes and a default status filter, which seed the initial state on mount; it does not persist a live session's controls.

---

## 3. Architecture

### 3.1 Component Tree

```
App                                              (unchanged)
└── AppShell                                     (unchanged)
    ├── Dashboard                                (unchanged)
    │   ├── [overview]       -> OverviewScreen   (unchanged)
    │   ├── [projects]       -> ProjectList      (MODIFIED)
    │   │     ├── ListControls                   (NEW)
    │   │     └── ProjectCard                    (unchanged)
    │   ├── [project-detail] -> ProjectDetail    (unchanged)
    │   │     └── TaskList                       (MODIFIED)
    │   │           ├── ListControls             (NEW)
    │   │           ├── status filter chips      (NEW, inline, reuses StatusPill)
    │   │           └── TaskCard                 (unchanged)
    │   └── [registry]       -> RegistryScreen   (unchanged)
    │         └── RegistryPanel                  (MODIFIED)
    │               ├── ListControls             (NEW)
    │               └── RegistryCard             (unchanged)
    └── CommandBar                               (unchanged)
```

No screen is added. Navigation is untouched.

### 3.2 Ownership

| Concern                                   | Owner                                                                      |
| ----------------------------------------- | -------------------------------------------------------------------------- |
| Matching, comparing, sorting              | `src/lib/list-filters.ts`, pure functions                                  |
| Search text, sort mode, filter selections | The owning list component, in `useState`                                   |
| Rendering the controls                    | `ListControls`, presentational                                             |
| Deciding what is visible                  | The owning list component, by applying the pure functions to its own array |
| Detecting a hidden mutation result        | The owning list component, using the same predicate                        |

`ListControls` holds no data state. It receives values and callbacks, exactly as `ProjectForm`, `TaskForm`, and `RegistryForm` do.

### 3.3 Backend

None. There is no Rust module, no command, no query, and no schema element in this milestone.

---

## 4. Database

**No change of any kind.**

- No migration. `MIGRATIONS` stays at one entry and the live database stays at migration level 1, for the fifth milestone running.
- No table, column, index, constraint, trigger, or foreign key is added, altered, or removed.
- No `SELECT`, `INSERT`, `UPDATE`, or `DELETE` statement is added or modified.
- Existing foreign-key semantics are untouched: `tasks.project_id` remains `ON DELETE CASCADE`; `projects.default_ide_id`, `projects.default_agent_id`, and `tasks.assigned_agent` remain `ON DELETE SET NULL`.
- `external_id` is neither read nor written by this milestone. It is not a searchable field, because no producer writes it and searching a column that is always `NULL` would be dead functionality.

Section 8's verification task asserts this by diffing `src-tauri/` and expecting no output.

---

## 5. Backend / IPC

**No commands are added, modified, or removed.**

The registered command count is unchanged: twenty-four after NEXUS-006, or twenty if NEXUS-007 ships first. `src/lib/nexus-db.ts` gains no wrapper. `src-tauri/src/commands/mod.rs` and `src-tauri/src/lib.rs` are not opened.

Every existing serde and TypeScript contract is preserved by not being touched. The IPC contract check in section 9.1 must report the same numbers before and after this milestone; any change is a defect introduced by this milestone.

**On backend tests.** The instruction to include comprehensive backend and database tests where backend behaviour is involved applies vacuously here: no backend behaviour is involved. What section 9.1 requires instead is a **regression gate**: `cargo test --lib` must pass with an identical test count and identical test names before and after. A changed count means Rust was touched, which this milestone forbids.

---

## 6. TypeScript Types

### 6.1 Additions to `src/types/index.ts`

```typescript
// NEXUS-007: list controls. Sort modes are per entity because the sortable
// fields differ; a shared union would permit invalid combinations.
export type ProjectSortMode =
  | 'created-desc' | 'created-asc' | 'updated-desc' | 'name-asc' | 'name-desc';

export type TaskSortMode =
  | 'created-desc' | 'created-asc' | 'updated-desc' | 'title-asc' | 'status';

export type RegistrySortMode =
  | 'created-desc' | 'created-asc' | 'name-asc' | 'type-asc';

/** Registry enabled filter. 'all' is the default and means no filtering. */
export type EnabledFilter = 'all' | 'enabled' | 'disabled';

/** One entry in a sort control. */
export interface SortOption<T extends string> {
  value: T;
  label: string;
}
```

Per-entity sort unions rather than one shared union: `'title-asc'` is meaningless for a project and `'type-asc'` is meaningless for a task. Separate unions make an invalid pairing a compile error.

### 6.2 No additions to `src/types/db.ts`

No payload crosses the IPC boundary in this milestone.

### 6.3 No additions to `src/lib/nexus-db.ts`

No wrapper is added. The file is not modified.

---

## 7. React Component Design

### 7.1 `src/lib/list-filters.ts`

**File:** `src/lib/list-filters.ts` (new)

Pure functions only. No React import, no import from `nexus-db.ts`, no I/O, no `Date.now()`, no mutation of arguments. Every function returns a new value and is deterministic for a given input.

```typescript
/** Case-insensitive substring match against a set of optionally-null fields. */
export function matchesQuery(query: string, fields: (string | null | undefined)[]): boolean;

/** Trim and lowercase once, so callers do not repeat it per item. */
export function normalizeQuery(query: string): string;

/** Case-insensitive string compare, used by every name and title sort. */
export function compareText(a: string, b: string): number;

/** ISO-8601 timestamps compare correctly as strings; no Date parsing. */
export function compareStamp(a: string, b: string): number;

/** Position within TASK_STATUS_ORDER. An unknown status sorts last. */
export function compareStatus(a: string, b: string, order: readonly string[]): number;

/** Applies a comparator, then a descending id tiebreak. Returns a new array. */
export function sortWithIdTiebreak<T extends { id: number }>(
  items: T[],
  compare: (a: T, b: T) => number,
): T[];
```

Notes on the implementations:

- `matchesQuery` returns `true` for an empty normalized query, so "no query" and "matches everything" are the same code path and cannot diverge.
- A `null` or `undefined` field is skipped, never coerced to the string `"null"`. F-08.
- `compareStamp` compares the ISO strings directly. The schema writes `strftime('%Y-%m-%dT%H:%M:%fZ', 'now')`, which is fixed-width and lexicographically ordered, so `Date` parsing would add cost and a failure mode for no benefit.
- `compareStatus` takes the order array as a parameter rather than importing `TASK_STATUS_ORDER`, keeping the module free of component imports. An unknown status yields an index of `-1`, which is mapped to `order.length` so it sorts last rather than first. F-17.
- `sortWithIdTiebreak` copies before sorting, so a component's state array is never mutated in place.

### 7.2 `ListControls`

**File:** `src/components/ListControls/ListControls.tsx` (new)

```typescript
interface ListControlsProps<T extends string> {
  searchValue: string;
  onSearchChange: (value: string) => void;
  searchPlaceholder: string;
  sortValue: T;
  sortOptions: SortOption<T>[];
  onSortChange: (value: T) => void;
  /** Rendered between search and sort; used for the task status chips. */
  filterSlot?: React.ReactNode;
  /** True when any control differs from its default. */
  isActive: boolean;
  onReset: () => void;
  disabled?: boolean;
}
```

Presentational. No state beyond what the DOM inputs hold, no effects, no command calls.

Renders a search input using the existing `.nexus-field`, a clear affordance shown only when `searchValue` is non-empty, an optional `filterSlot`, a sort `<select>` using the existing `.nexus-select`, and a Reset control using `.nexus-btn--secondary` shown only when `isActive`.

### 7.3 `ProjectList` changes

New state: `query: string`, `sort: ProjectSortMode` (default `'created-desc'`), `hiddenNotice: string | null`.

Derived, computed on each render from state, not stored:

```typescript
const visible = sortWithIdTiebreak(
  projects.filter((p) =>
    matchesQuery(normalizeQuery(query), [
      p.name, p.description, p.repositoryPath, p.repositoryUrl,
    ]),
  ),
  comparatorFor(sort),
);
```

Header count becomes `visible.length` of `projects.length` when `isActive`, and `projects.length` alone otherwise. F-27.

`handleCreate` gains the R-02 check: after `refresh()`, test the created project against the current predicate and set `hiddenNotice` if it fails. `createProject` already returns the created `Project`, so no extra query is needed.

Empty state condition is unchanged: `!loading && projects.length === 0`. A new no-results block renders when `!loading && projects.length > 0 && visible.length === 0`.

Nothing else changes. The create flow, error handling, registry-list fetching, and `taskCount` wiring from NEXUS-006 are untouched.

### 7.4 `TaskList` changes

New state: `query: string`, `sort: TaskSortMode` (default `'created-desc'`), `statusFilter: TaskStatus[]` (default `[]`, meaning all, per F-21), `hiddenNotice: string | null`.

The status filter renders in `ListControls`'s `filterSlot` as one toggle per entry in `TASK_STATUS_ORDER`, reusing the existing `StatusPill` in its `as="button"` form with `aria-pressed`. Clicking toggles membership. `[]` means no filtering.

```typescript
const visible = sortWithIdTiebreak(
  tasks.filter(
    (t) =>
      (statusFilter.length === 0 || statusFilter.includes(t.status)) &&
      matchesQuery(normalizeQuery(query), [t.title, t.description]),
  ),
  comparatorFor(sort),
);
```

`'status'` sort uses `compareStatus(a.status, b.status, TASK_STATUS_ORDER)`.

R-02 applies to three handlers, not one: `handleCreate`, `handleSave`, and `handleStatusChange`. A status change is the most likely trigger of all: moving a task out of the filtered status is a single click. `updateTask`, `updateTaskStatus`, and `createTask` all return the resulting `Task`, so the check needs no extra query.

`handleAgentChange` also returns a `Task`, but agent assignment is not a filterable or searchable field, so a task cannot leave the visible set because of it. No check is needed there, and adding one would produce a notice that never fires.

The existing status breakdown in the `TaskList` header is computed from the **unfiltered** `tasks` array. It reports what the project contains, not what the filter shows, and must not be recomputed from `visible`.

### 7.5 `RegistryPanel` changes

New state: `query: string`, `sort: RegistrySortMode` (default `'created-desc'`), `enabledFilter: EnabledFilter` (default `'all'`), `hiddenNotice: string | null`.

```typescript
const visible = sortWithIdTiebreak(
  entries.filter(
    (e) =>
      (enabledFilter === 'all' ||
        (enabledFilter === 'enabled' ? e.enabled : !e.enabled)) &&
      matchesQuery(normalizeQuery(query), [e.name, e.entryType, e.executablePath]),
  ),
  comparatorFor(sort),
);
```

The enabled filter renders in `filterSlot` as three `.nexus-btn` toggles.

R-02 applies to `handleCreate`, `handleSave`, and `handleToggleEnabled`. The toggle is the likely trigger: disabling an entry while filtering to enabled removes it from view in one click.

The existing disabled-count chip in the panel header is computed from the **unfiltered** `entries` array, for the same reason as 7.4.

Both `RegistryPanel` instances hold independent control state, because each is a separate component instance. Filtering IDEs does not filter agents. This is a property of the existing design and requires nothing new.

### 7.6 The hidden-result notice

Rendered directly above the list in all three components, as a single inline element. It is not a modal, not a toast, and not timed. It uses `.nexus-chip`-adjacent styling in `--color-accent-dim` with `role="status"`, not `role="alert"`: it reports a successful outcome, and `alert` is reserved for errors in this codebase.

Copy, with `{name}` being the project name, task title, or entry name:

> Saved "{name}", but it is hidden by the current filters. [Reset Filters]

Cleared when: the user clicks Reset Filters, changes any control, dismisses it, or performs another mutation. R-05.

### 7.7 The no-results state

Rendered when `!loading && items.length > 0 && visible.length === 0`. Distinguishable from the empty state in three ways: different icon, different copy, and a Reset Filters button in place of a Create button.

> No {items} match the current filters. {n} exist in total.
> [Reset Filters]

The total is included so the user can see immediately that the data is present and the controls are the cause. F-26.

### 7.8 Styling

New CSS file: `ListControls.css`. Append-only additions to `globals.css` if any shared class is warranted, likely `.nexus-filter-bar` and `.nexus-notice`. Existing tokens only, no new tokens, no theme change.

Reuses `.nexus-field`, `.nexus-select`, `.nexus-btn`, `.nexus-chip`, and `.nexus-status-pill`.

---

## 8. Implementation Tasks

Frontend tasks gate on `npx tsc --noEmit`. The final task gates on `pnpm build`, `pnpm tauri build`, and an unchanged `cargo test --lib`.

---

### T-01: Add list control types

**Objective.** Declare the sort and filter unions before anything consumes them.

**Files.** `src/types/index.ts`.

**Dependencies.** None.

**Implementation details.** Add `ProjectSortMode`, `TaskSortMode`, `RegistrySortMode`, `EnabledFilter`, and `SortOption<T>` per 6.1. Do not modify existing types.

**Acceptance criteria.** `npx tsc --noEmit` exits 0. `NexusScreen` and `NexusView` are unchanged. No type in `src/types/db.ts` is touched.

**Tests.** None automated. Type-level only.

---

### T-02: Build `src/lib/list-filters.ts`

**Objective.** All matching and comparison logic, isolated and pure.

**Files.** `src/lib/list-filters.ts` (new).

**Dependencies.** None.

**Implementation details.** Implement the six functions of 7.1. `matchesQuery` returns `true` on an empty normalized query. Null and undefined fields are skipped. `compareStamp` compares ISO strings directly. `compareStatus` maps an unknown status to `order.length`. `sortWithIdTiebreak` copies before sorting and appends a descending `id` tiebreak.

**Acceptance criteria.** `grep -E "react|nexus-db" src/lib/list-filters.ts` returns nothing. No function mutates an argument. No function references `Date`, `Math.random`, or any global other than standard string and array methods. `npx tsc --noEmit` exits 0.

**Tests.** None automated (no runner). Written as pure functions specifically so they become testable the moment one exists.

---

### T-03: Build `ListControls`

**Objective.** The shared presentational control bar.

**Files.** `src/components/ListControls/ListControls.tsx`, `src/components/ListControls/ListControls.css` (both new).

**Dependencies.** T-01.

**Implementation details.** Props per 7.2. Clear affordance shown only when `searchValue` is non-empty. Reset shown only when `isActive`. Search input uses `.nexus-field`; sort uses `.nexus-select`.

**Acceptance criteria.** `ListControls.tsx` contains no `useEffect` and no import from `../../lib/nexus-db`. Sort select renders one option per entry in `sortOptions`. `npx tsc --noEmit` exits 0.

**Tests.** Manual, as part of each list's scenarios.

---

### T-04: Add the hidden-notice and no-results styles

**Objective.** Shared styling for the two new states, before three components need them.

**Files.** `src/assets/styles/globals.css`.

**Dependencies.** T-03.

**Implementation details.** Append `.nexus-notice` and `.nexus-filter-bar` if shared styling is warranted. Append only: no existing rule, token, or selector is modified.

**Acceptance criteria.** `git diff src/assets/styles/globals.css` shows additions only, zero deletions and zero modified lines. Existing tokens only.

**Tests.** Manual.

---

### T-05: `ProjectList` search and sort

**Objective.** Wire controls into the project list.

**Files.** `src/components/ProjectList/ProjectList.tsx`.

**Dependencies.** T-02, T-03.

**Implementation details.** Per 7.3. `visible` is derived on render, never stored in state. Search fields are name, description, repository path, repository URL. Five sort modes per F-14. Header count per F-27.

**Acceptance criteria.** Searching `alpha` matches a project named `ALPHA`. A project with a null description does not throw. Clearing the search restores all projects. Each of the five sort modes reorders the list, and applying the same mode twice produces identical order. `visible` is not held in `useState`.

**Tests.** Manual scenarios 1, 2, 3, 4, 5.

---

### T-06: `ProjectList` no-results and hidden-notice

**Objective.** The two new states, and R-02 on create.

**Files.** `src/components/ProjectList/ProjectList.tsx`.

**Dependencies.** T-05, T-04.

**Implementation details.** Per 7.6 and 7.7. Empty condition stays `projects.length === 0`. No-results condition is `projects.length > 0 && visible.length === 0`. `handleCreate` tests the returned project against the current predicate and sets `hiddenNotice` on failure.

**Acceptance criteria.** With projects present and a query matching none, the no-results block appears with a Reset Filters button and no Create button. With zero projects, the original empty state appears unchanged. Creating a project whose name does not match the active query shows the notice; clicking Reset Filters reveals it.

**Tests.** Manual scenarios 7, 8.

---

### T-07: `TaskList` search, sort, and status filter

**Objective.** Wire controls into the task list.

**Files.** `src/components/TaskList/TaskList.tsx`.

**Dependencies.** T-02, T-03.

**Implementation details.** Per 7.4. Status chips render in `filterSlot`, reusing `StatusPill` with `aria-pressed`. `[]` means all. `'status'` sort uses `compareStatus` with `TASK_STATUS_ORDER`. The header status breakdown continues to read from the unfiltered `tasks`.

**Acceptance criteria.** Selecting `blocked` alone shows only blocked tasks. Selecting `blocked` and `done` shows both. Deselecting all shows every task, not none. Status sort orders open, in_progress, blocked, done, not alphabetically. The header breakdown counts do not change when a filter is applied.

**Tests.** Manual scenarios 9, 10, 11.

---

### T-08: `TaskList` no-results and hidden-notice

**Objective.** The two new states, and R-02 on the three mutating paths.

**Files.** `src/components/TaskList/TaskList.tsx`.

**Dependencies.** T-07, T-04.

**Implementation details.** Per 7.6 and 7.7. R-02 applied in `handleCreate`, `handleSave`, and `handleStatusChange`. Not applied in `handleAgentChange`, since agent is neither searchable nor filterable.

**Acceptance criteria.** Filtering to `blocked` and creating a task shows the notice naming the new task. Filtering to `blocked` and cycling a visible task's status to `done` shows the notice and the row disappears. Deleting a visible task shows no notice and decrements both counts. Resetting from the notice reveals the item.

**Tests.** Manual scenarios 12, 13, 14.

---

### T-09: `RegistryPanel` search, sort, and enabled filter

**Objective.** Wire controls into both registry panels.

**Files.** `src/components/RegistryPanel/RegistryPanel.tsx`.

**Dependencies.** T-02, T-03.

**Implementation details.** Per 7.5. Enabled filter renders in `filterSlot` as three toggles. Search fields are name, type, executable path. The header disabled-count chip continues to read from the unfiltered `entries`.

**Acceptance criteria.** The IDE panel and the agent panel filter independently: a query in one does not affect the other. Setting the filter to disabled shows only disabled entries. Setting it to all shows every entry. Searching by type matches an entry whose name does not match.

**Tests.** Manual scenarios 15, 16.

---

### T-10: `RegistryPanel` no-results and hidden-notice

**Objective.** The two new states, and R-02 including the enable toggle.

**Files.** `src/components/RegistryPanel/RegistryPanel.tsx`.

**Dependencies.** T-09, T-04.

**Implementation details.** Per 7.6 and 7.7. R-02 applied in `handleCreate`, `handleSave`, and `handleToggleEnabled`.

**Acceptance criteria.** With the filter on enabled, disabling a visible entry shows the notice and the row disappears. With a query matching nothing and entries present, the no-results block appears. Deleting an entry shows no notice.

**Tests.** Manual scenarios 17, 18.

---

### T-11: Verify control isolation and reset behaviour

**Objective.** Confirm 2.6 across navigation, and confirm no control leaks between lists.

**Files.** None. Review and manual verification only.

**Dependencies.** T-05 through T-10.

**Implementation details.** Confirm all control state is `useState` inside the three list components. Confirm no control value is passed into or out of `AppShell`, `Dashboard`, or `ProjectDetail`.

**Acceptance criteria.** `grep -n "query\|sortValue\|statusFilter\|enabledFilter" src/components/AppShell/AppShell.tsx src/components/Dashboard/Dashboard.tsx src/components/ProjectDetail/ProjectDetail.tsx` returns nothing. Navigating away from and back to each list resets its controls to defaults. Opening project B after filtering project A's tasks shows project B unfiltered.

**Tests.** Manual scenario 19.

---

### T-12: Full verification

**Objective.** Prove the milestone, including that the backend was not touched.

**Files.** None.

**Dependencies.** All preceding tasks.

**Implementation details.** Run `pnpm build`, `pnpm tauri build`, `cargo test --lib`, the IPC contract check, and the structural greps of 9.1. Perform the complete manual checklist of 9.5.

**Acceptance criteria.** Section 9 in full, including `git diff --stat src-tauri/` producing no output.

**Tests.** Regression suite plus the manual checklist.

---

## 9. Acceptance Criteria

### 9.1 Build and structure

- [ ] `pnpm build` completes with zero TypeScript and zero Vite errors.
- [ ] `pnpm tauri build` produces `NEXUS.app` and `NEXUS_0.1.0_aarch64.dmg`.
- [ ] **`git diff --stat src-tauri/` produces no output.** No Rust file is created or modified.
- [ ] `cargo test --lib` passes with an identical test count and identical test names to before this milestone.
- [ ] `git diff --stat package.json pnpm-lock.yaml src-tauri/Cargo.toml src-tauri/Cargo.lock` produces no output.
- [ ] `SELECT MAX(id) FROM _migrations` returns 1.
- [ ] The IPC contract check reports the same registered, invoked, and defined counts as before this milestone, with zero mismatches.
- [ ] `grep -rl "@tauri-apps/api" src/` returns exactly `src/lib/nexus-db.ts`.
- [ ] `grep -E "react|nexus-db" src/lib/list-filters.ts` returns nothing.
- [ ] No raw SQL under `src/`.
- [ ] `git diff --stat` shows no change to `AppShell`, `Dashboard`, `App.tsx`, `ProjectCard`, `ProjectForm`, `ProjectDetail`, `TaskCard`, `TaskForm`, `RegistryCard`, `RegistryForm`, `RegistrySelect`, `RegistryScreen`, `OverviewScreen`, `StatTile`, `Logo`, `StatusBar`, `CommandBar`, `DbPanel`.
- [ ] `git diff src/assets/styles/globals.css` shows additions only, with zero deletions.
- [ ] `NexusScreen` is unchanged.
- [ ] No component holds a derived visible list in `useState`.

### 9.2 Search

- [ ] A lowercase query matches uppercase content, and an uppercase query matches lowercase content.
- [ ] A query matching the middle of a word includes the item.
- [ ] A query of only spaces behaves identically to an empty query.
- [ ] Project search matches on name, on description, on repository path, and on repository URL, each verified independently.
- [ ] Task search matches on title and on description, each verified independently.
- [ ] Registry search matches on name, on type, and on executable path, each verified independently.
- [ ] An item whose matched field is `null` is excluded without throwing.
- [ ] Clearing the input restores the full list, subject to any remaining filter.
- [ ] The clear control appears only when a query is present, and clears in one action.

### 9.3 Sort and filter

- [ ] All five project sort modes reorder the list as labelled.
- [ ] All five task sort modes reorder the list as labelled.
- [ ] All four registry sort modes reorder the list as labelled.
- [ ] Applying the same sort mode twice produces identical order.
- [ ] Two items with the same sort key appear in descending id order.
- [ ] Name and title sorts ignore case.
- [ ] Task status sort orders open, in_progress, blocked, done.
- [ ] Selecting two statuses shows tasks in either.
- [ ] Deselecting all statuses shows every task.
- [ ] The registry enabled filter shows all, enabled only, and disabled only as selected.
- [ ] Search and filter combine with AND.
- [ ] The reset control returns every control in that list to its default.
- [ ] The IDE panel's controls have no effect on the agent panel, and the reverse.

### 9.4 States and mutations

- [ ] The empty state appears only when the underlying list has zero items.
- [ ] The no-results state appears when items exist and none pass the controls, and offers reset rather than create.
- [ ] The header reads `{visible} of {total}` when controls are active and `{total}` when they are not.
- [ ] Creating an item that does not pass the active controls shows a `role="status"` notice naming the item.
- [ ] The notice persists until reset, control change, dismissal, or another mutation. It does not disappear on a timer.
- [ ] Resetting from the notice reveals the item that triggered it.
- [ ] Editing a task's status out of the active status filter shows the notice and removes the row.
- [ ] Disabling a registry entry under an enabled-only filter shows the notice and removes the row.
- [ ] Deleting a visible item shows no notice and decrements both counts.
- [ ] No mutation resets any control.
- [ ] The `TaskList` header status breakdown and the `RegistryPanel` disabled-count chip are computed from the unfiltered arrays and do not change when a filter is applied.

### 9.5 Manual UI verification

**Carried forward and still owed.** Every NEXUS-004 and NEXUS-005 manual scenario. The full list is maintained in the NEXUS-008 specification, section 9.5.

**New NEXUS-007 scenarios**

1. **Case-insensitive search.** Create a project named `ALPHA`. Search `alpha`, then `AlPhA`. Both match. Rename to `alpha`, search `ALPHA`. Matches.
2. **Substring search.** With `ALPHA` present, search `LPH`. Matches.
3. **Whitespace query.** Type three spaces into the search. The full list is shown, identical to an empty query.
4. **Search across every project field.** Create a project with a distinctive value in the description only, then one with a distinctive value in the repository path only, then one in the repository URL only. Search each value. Each matches exactly one project. Confirm a project with a null description does not break the search.
5. **Every project sort mode.** Create three projects at different times with names that sort differently from their creation order. Apply each of the five modes and confirm the order matches the label. Apply one mode, switch away, switch back: identical order.
6. **Stable order.** Create two projects within the same second. Apply a sort keyed on their shared value. Note the order. Switch sort modes and back. The order is unchanged.
7. **No results versus empty.** With three projects present, search a string matching none. The no-results block appears, states three exist, and offers Reset Filters and no Create button. Clear the search. All three return. Then delete all three: the original empty state appears with its Create button.
8. **Create under an active project filter.** Search `zzz`. Create a project named `Kappa`. The notice appears naming Kappa. The header shows 0 of 4. Click Reset Filters. Kappa is visible.
9. **Task status filter, single.** In a project with tasks in several statuses, select `blocked`. Only blocked tasks are shown. The header breakdown still shows all statuses.
10. **Task status filter, multiple.** Select `blocked` and `done`. Both appear. Deselect both. Every task appears.
11. **Task status sort.** Apply the status sort. Order is open, in_progress, blocked, done.
12. **Create under an active task filter.** Filter to `blocked`. Create a task. It is created as `open`, the notice appears naming it, and the header count reflects the hidden item. Reset. It appears.
13. **Status change out of an active filter.** Filter to `blocked`. Click a visible blocked task's status pill to advance it. The row disappears and the notice appears. Reset. It is present with its new status.
14. **Delete under an active task filter.** Filter to `blocked`. Delete a visible blocked task. It disappears, no notice appears, and both counts decrement by one.
15. **Registry panels are independent.** Search `zzz` in the IDE panel. The agent panel is unaffected and still lists every agent.
16. **Registry enabled filter.** With one enabled and one disabled entry, cycle all, enabled, disabled. Each shows the right set. Search by type and confirm it matches an entry whose name does not.
17. **Disable under an enabled-only filter.** Set the filter to enabled. Disable a visible entry. The row disappears and the notice appears. Reset. It is present, marked disabled.
18. **Registry no-results.** With entries present, search a string matching none. The no-results block appears and states the total.
19. **Controls across navigation.** Filter project A's tasks to `blocked`. Go back to the project list, open project B. Project B's tasks are unfiltered. Return to project A: its filter has reset to the default. Repeat for the project list and the registry.

---

## 10. Explicitly Out of Scope

Deferred deliberately:

- **Global cross-entity search.** Searching projects, tasks, IDEs, and agents from one input. Deferred to NEXUS-009, where it can share the command-palette infrastructure.
- **Any global search Rust command, and any search screen.**
- **Server-side filtering, sorting, or pagination.** Everything is client-side over data already in state.
- **Fuzzy matching, regular expressions, boolean query syntax, or field-scoped query syntax such as `status:blocked`.** Plain case-insensitive substring only.
- **Saved searches, search history, or recently-used filters.**
- **Persisting live control state across navigation or restart.** NEXUS-008 persists default sort modes and a default status filter, which seed initial state; it does not persist a session's controls.
- **Debouncing or virtualisation.** At local scale, neither is justified. Revisit only on measured evidence.
- **Filtering, searching, or sorting the Overview screen.**
- **Sorting or filtering by assigned agent, default IDE, or default agent.** These are relational, not textual, and would need a different control shape.
- **Searching `external_id`.** No producer writes it; it is always `NULL`, so searching it would be dead functionality.
- **Bulk actions on a filtered set.** Deferred as in NEXUS-004 and NEXUS-005.
- **Drag reordering or manual ordering.**
- **A frontend test framework.** Explicitly decided against for this milestone. Reconsider as a separate architectural decision.

Also out of scope, per the standing NEXUS constraints:

- Jira, Claude, PlayerZero, Cursor, Grok, and ChatGPT integrations
- AI orchestration or execution of any kind
- IDE launching, terminal execution, browser automation
- Settings management (NEXUS-008)
- Voice recognition, text to speech
- News, weather, morning briefings, notifications
- Authentication, cloud sync, CI/CD, auto-update
- Custom title bar or window chrome
- Any routing library, state manager, UI library, form library, or ORM
- Any new Rust or frontend dependency
