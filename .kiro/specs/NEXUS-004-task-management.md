# NEXUS-004: Task Management

## Overview

Surface the `tasks` table. NEXUS-003 made projects usable; NEXUS-004 gives each project a real task list, scoped to that project, with create, edit, status change, and delete.

This is the last table with a direct foreign key to `projects`, and its `ON DELETE CASCADE` relationship is already implemented and covered by tests from NEXUS-003. NEXUS-004 makes that relationship visible to the user rather than merely correct in the database.

The milestone deliberately reuses the component and command patterns established in NEXUS-003 rather than inventing new ones. If NEXUS-004 requires a new architectural concept, that is a signal the design is wrong.

---

## 1. Existing State (from NEXUS-001, NEXUS-002, NEXUS-003)

### 1.1 Architecture already in place

**Rust layer**

- `db/mod.rs`: `DbState`, `init()`, migration runner, `PRAGMA foreign_keys = ON` on every connection
- `db/migrations.rs`: versioned `MIGRATIONS` array, one entry (migration 001), live database at level 1
- `db/projects.rs`: `insert_project`, `list_projects`, `update_project`, `delete_project`, `count_projects`, `count_all_tables`; `Project`, `CreateProjectInput`, `UpdateProjectInput`; a `#[cfg(test)]` module using an in-memory connection seeded from the real `MIGRATIONS`
- `commands/mod.rs`: six registered commands
- `lib.rs`: thin orchestrator, registers `DbState` and the command list

**Frontend**

- `src/lib/nexus-db.ts`: the only module that imports `@tauri-apps/api`; one typed wrapper per command
- `src/types/db.ts`: payload types mirroring the Rust structs
- `src/types/index.ts`: `NexusScreen`, `NexusView`, `ProjectFormValues`, `ProjectFormMode`
- `App -> AppShell -> [Logo, active project badge, StatusBar, Dashboard, CommandBar]`
- `AppShell` owns `view` and `activeProjectName` only, and passes `navigate` down
- `Dashboard` switches between `ProjectList` and `ProjectDetail`
- `ProjectList`, `ProjectCard`, `ProjectForm`, `ProjectDetail` implement the list/card/form/detail pattern
- `ProjectForm` is presentational: it calls `onSubmit(values)` and never touches the database
- `DbPanel` retained on disk, not rendered, tree-shaken out of the production bundle
- Shared `.nexus-btn` and `.nexus-field` classes in `globals.css`

### 1.2 What NEXUS-003 left for this milestone

The `tasks` table exists, is migrated, and is enforced by foreign keys, but nothing reads or writes it through the application:

- No task commands exist in the Rust layer.
- No task types exist in TypeScript.
- `ProjectDetail` tells the user that deleting a project removes its tasks, but cannot show how many.
- `tasks.status` has a database default of `'open'` and no defined vocabulary anywhere in the codebase.
- `tasks.external_id` and `tasks.assigned_agent` exist for future integration milestones and have no producer.

---

## 2. Requirements

### 2.1 Functional Requirements

| ID | Requirement |
|----|-------------|
| F-01 | The user must be able to create a task within a project by providing a title (required), description (optional), and status (defaulting to `open`). |
| F-02 | The user must be able to view all tasks belonging to the currently open project. |
| F-03 | Each task in the list must display: title, status, description (if set), and creation date. |
| F-04 | Tasks must be listed only for the project the user has open. A task from another project must never appear. |
| F-05 | The user must be able to edit an existing task's title, description, and status. |
| F-06 | The user must be able to change a task's status directly from the task list without opening the edit form. |
| F-07 | The user must be able to delete a task. Deletion must require an inline confirmation step, matching the NEXUS-003 project delete pattern. |
| F-08 | The task list must show a meaningful empty state when the open project has no tasks, prompting the user to create the first one. |
| F-09 | The project delete confirmation in `ProjectDetail` must state how many tasks will be deleted along with the project. |
| F-10 | The task list must display a count of tasks, and a breakdown by status. |
| F-11 | All task operations (create, read, update, status change, delete) must persist to SQLite and survive application restart. |
| F-12 | Deleting a project must continue to delete its tasks. After the delete, no task row may reference the removed project. |

### 2.2 Non-Functional Requirements

| ID | Requirement |
|----|-------------|
| N-01 | No new Rust or frontend dependencies. |
| N-02 | No routing library and no global state manager. The `NexusView` model from NEXUS-003 is not extended (see 2.3). |
| N-03 | All new Tauri commands use the `nexus_` prefix and the `Result<T, String>` error pattern. |
| N-04 | All new TypeScript types go in `src/types/`. |
| N-05 | All new `invoke()` wrappers go in `src/lib/nexus-db.ts`. No component may import `@tauri-apps/api`. |
| N-06 | Task database logic goes in a new `src-tauri/src/db/tasks.rs`. Commands stay in `src-tauri/src/commands/mod.rs`. `lib.rs` stays thin. |
| N-07 | `AppShell`, `Logo`, `StatusBar`, `CommandBar`, and `Dashboard` are not restructured. `AppShell` gains no task awareness whatsoever. |
| N-08 | `ProjectList`, `ProjectCard`, and `ProjectForm` are not modified. `ProjectDetail` is modified only to host the task list and to report the task count in its delete confirmation. |
| N-09 | `TaskForm` is presentational. It receives `onSubmit` and never calls a command. This mirrors `ProjectForm`. |
| N-10 | The status vocabulary is defined once in Rust and once in TypeScript, and the two must agree. Rust rejects unknown status values with a typed error string. |
| N-11 | The `#[cfg(test)]` pattern from `db/projects.rs` is extended to `db/tasks.rs`. Cascade behavior stays covered. |
| N-12 | Local-first. No remote backend, no external issue tracker, no authentication. |

### 2.3 Design Principle: Tasks Are Not a Screen

`NexusView` is deliberately left at two screens. Tasks are rendered **inside** `ProjectDetail`, below the project fields, in view mode only. There is no `task-detail` screen and no task route.

Rationale:

- A task has three user-editable fields. A dedicated screen for three fields is not worth a navigation level.
- Task context is project context. A task list that can be reached without an open project would need its own project selector, which duplicates `ProjectList`.
- Keeping `NexusView` at two screens means `AppShell` requires zero changes in this milestone, which is the strongest possible evidence that the NEXUS-003 navigation model was correct.

Task creation and editing therefore use inline form expansion, the same interaction `ProjectList` already uses for project creation.

**Consequence for edit mode:** when `ProjectDetail` is in `mode === 'edit'`, the task list is hidden. The user is editing the project, not its tasks. This keeps the panel from presenting two concurrent edit surfaces.

---

## 3. Architecture

### 3.1 Updated Component Tree

```
App
└── AppShell                                    (UNCHANGED in NEXUS-004)
    ├── header: Logo + active project badge + StatusBar
    ├── Dashboard(view, navigate)               (UNCHANGED in NEXUS-004)
    │   ├── [view = 'projects']       → ProjectList          (unchanged)
    │   └── [view = 'project-detail'] → ProjectDetail        (modified)
    │         ├── top bar: Back | name | Edit | Delete
    │         ├── [view mode]
    │         │     ├── project fields
    │         │     └── TaskList(projectId, onCountChange)   (new)
    │         │           ├── header: "Tasks" + count + status breakdown + New Task
    │         │           ├── [TaskForm mode='create']       (new, conditional)
    │         │           ├── [empty state]
    │         │           └── TaskCard x n                   (new)
    │         │                 └── [TaskForm mode='edit']   (conditional, inline)
    │         └── [edit mode] → ProjectForm (task list hidden)
    └── CommandBar
```

### 3.2 Ownership

| Concern | Owner |
|---------|-------|
| Which screen is showing | `AppShell` (unchanged) |
| Which project is open | `AppShell` (unchanged) |
| Project data and project CRUD calls | `ProjectDetail` (unchanged) |
| Task data and all task CRUD calls | `TaskList` |
| Task count, for the project delete confirmation | `TaskList` reports upward via `onCountChange` |
| Form field state and client-side validation | `TaskForm` |

`TaskList` owns every task command call. `TaskCard` and `TaskForm` are presentational and receive callbacks. This matches the `ProjectList` / `ProjectCard` / `ProjectForm` split exactly.

`ProjectDetail` does not fetch tasks. It receives the count through `onCountChange`, the same callback shape `ProjectDetail` already uses to report the active project name to `AppShell`.

### 3.3 Rust Module Structure (changes from NEXUS-003)

```
src-tauri/src/
├── main.rs             (unchanged)
├── lib.rs              (add five commands to invoke_handler)
├── db/
│   ├── mod.rs          (add `pub mod tasks;`, one line)
│   ├── migrations.rs   (UNCHANGED, see section 4.5)
│   ├── projects.rs     (unchanged)
│   └── tasks.rs        (NEW)
└── commands/
    └── mod.rs          (add five task commands)
```

---

## 4. Database Schema Assessment

### 4.1 The existing `tasks` table

Migration 001 defined:

```sql
CREATE TABLE IF NOT EXISTS tasks (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    external_id    TEXT,
    title          TEXT    NOT NULL,
    description    TEXT,
    status         TEXT    NOT NULL DEFAULT 'open',
    project_id     INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    assigned_agent INTEGER REFERENCES ai_agents(id) ON DELETE SET NULL,
    created_at     TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at     TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_tasks_project_external
    ON tasks(project_id, external_id)
    WHERE external_id IS NOT NULL;
```

Every column NEXUS-004 needs already exists.

### 4.2 Status vocabulary

`status` is `TEXT NOT NULL DEFAULT 'open'` with no `CHECK` constraint. NEXUS-004 defines the vocabulary in the Rust layer:

| Value | Meaning |
|-------|---------|
| `open` | Not started. The database default. |
| `in_progress` | Actively being worked. |
| `blocked` | Cannot proceed. |
| `done` | Complete. |

**No `CHECK` constraint is added.** Reasons:

1. Adding one requires migration 002, and SQLite cannot add a `CHECK` constraint to an existing table without a full table rebuild (`ALTER TABLE ... RENAME`, recreate, copy, drop). That is a disproportionate amount of migration risk for a four-value enum.
2. The Rust layer is the only writer. `React` cannot reach SQL, so validation at the command boundary is sufficient and is the pattern already used for the non-empty project name.
3. Future integration milestones may need to map foreign status vocabularies onto this column. A hard constraint would make that a migration, not a mapping change.

`db/tasks.rs` validates the value on both insert and update, and returns `Err("Invalid task status: {value}")` for anything outside the set.

### 4.3 `external_id` and `assigned_agent`

Neither column is surfaced in the NEXUS-004 UI.

- `external_id` stays `NULL` for every task this milestone creates. The partial unique index only applies `WHERE external_id IS NOT NULL`, so it can never fire on NEXUS-004 data.
- `assigned_agent` stays `NULL`. The `ai_agents` table is empty, so any non-null value would fail the foreign-key check.

**Critical implementation constraint:** `update_task` must not clobber these columns. Its `UPDATE` statement lists `title`, `description`, `status`, and `updated_at` only. A statement that also set `external_id = ?` or `assigned_agent = ?` from the input struct would silently null out data that a future milestone writes. This is called out explicitly because it is the kind of mistake that is invisible until the integration milestone lands.

### 4.4 Project to Task delete behavior

`tasks.project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE` is unchanged and must stay unchanged.

NEXUS-003 already verified this with `delete_project_cascades_to_tasks` in `db/projects.rs`. NEXUS-004 does not weaken it and does not intercept it: deleting a project still issues a single `DELETE FROM projects`, and SQLite removes the children. The application must not delete tasks manually before deleting a project. Doing so would work, but it would move a database invariant into application code and hide a regression if the foreign key were ever dropped.

NEXUS-004 adds one thing on top: the confirmation text tells the user how many tasks the cascade will take with it (F-09).

### 4.5 Migration 002: not required

No new tables, columns, indexes, or constraints. `MIGRATIONS` stays at one entry and the live database stays at level 1.

An index on `tasks(project_id)` was considered for the per-project list query and rejected. A local single-user command center will not accumulate enough rows for a sequential scan to be measurable, and `idx_tasks_project_external` already leads with `project_id` for the subset of rows that have an `external_id`. Adding an index is cheap later if profiling ever justifies it; adding a migration that is not needed is not free.

---

## 5. Rust / Tauri Command Design

### 5.1 New commands

Five commands are added. All six existing commands are retained unchanged.

| Command | Input | Output | Purpose |
|---------|-------|--------|---------|
| `nexus_create_task` | `CreateTaskInput` | `Task` | Insert a task for a project. Validates non-empty title and known status. |
| `nexus_list_tasks` | `projectId: i64` | `Vec<Task>` | All tasks for one project, ordered by creation date descending. |
| `nexus_update_task` | `UpdateTaskInput` | `Task` | Update title, description, status. Sets `updated_at`. Leaves `external_id` and `assigned_agent` untouched. |
| `nexus_update_task_status` | `UpdateTaskStatusInput` | `Task` | Change status only. Sets `updated_at`. |
| `nexus_delete_task` | `id: i64` | `()` | Delete one task. Errors if not found. |

**On `nexus_update_task_status` existing separately:** F-06 requires a one-click status change from the task row. Routing that through `nexus_update_task` would force `TaskCard` to hold and resend every field, which makes a presentational component responsible for not losing data. A narrow command is the smaller mistake. It shares its implementation with `update_task` through a common private helper.

### 5.2 Structs in `db/tasks.rs`

```rust
/// A task row returned to the frontend.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: i64,
    pub external_id: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
    pub project_id: i64,
    pub assigned_agent: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTaskInput {
    pub project_id: i64,
    pub title: String,
    pub description: Option<String>,
    pub status: Option<String>,   // None means the schema default, 'open'
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTaskInput {
    pub id: i64,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTaskStatusInput {
    pub id: i64,
    pub status: String,
}
```

`Task` exposes `external_id` and `assigned_agent` read-only so the frontend can display integration state in a later milestone without another schema pass. NEXUS-004 does not render them.

### 5.3 Status validation

```rust
/// The task status vocabulary. Mirrored by TaskStatus in src/types/db.ts.
pub const TASK_STATUSES: [&str; 4] = ["open", "in_progress", "blocked", "done"];

fn validate_status(status: &str) -> Result<(), String> {
    if TASK_STATUSES.contains(&status) {
        Ok(())
    } else {
        Err(format!(
            "Invalid task status: {status}. Expected one of: {}",
            TASK_STATUSES.join(", ")
        ))
    }
}
```

### 5.4 `update_task` shape

The statement lists only the three editable columns plus `updated_at`, for the reason given in 4.3:

```rust
pub fn update_task(conn: &Connection, input: &UpdateTaskInput) -> Result<Task, String> {
    let title = input.title.trim();
    if title.is_empty() {
        return Err("Task title cannot be empty".to_string());
    }
    validate_status(&input.status)?;

    let affected = conn
        .execute(
            "UPDATE tasks
                SET title       = ?1,
                    description = ?2,
                    status      = ?3,
                    updated_at  = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
              WHERE id = ?4",
            rusqlite::params![title, input.description, input.status, input.id],
        )
        .map_err(|e| format!("Failed to update task {}: {e}", input.id))?;

    if affected == 0 {
        return Err(format!("Task {} not found", input.id));
    }

    get_task_by_id(conn, input.id)
}
```

`updated_at` is set explicitly by the statement. No triggers, matching the `update_project` precedent.

### 5.5 `insert_task` and the foreign key

`insert_task` writes `project_id`, `title`, `description`, and `status`. It does not write `external_id` or `assigned_agent`, so both take their schema default of `NULL`.

Because `PRAGMA foreign_keys = ON`, inserting a task with a `project_id` that does not exist fails at the database level. The resulting rusqlite error is mapped to a readable string rather than allowed to surface raw:

```
Err("Failed to insert task: FOREIGN KEY constraint failed")
```

This is the correct behavior. The application does not pre-check that the project exists; the database is the authority.

### 5.6 Command registration

`lib.rs` `invoke_handler` grows to eleven entries:

```rust
.invoke_handler(tauri::generate_handler![
    commands::nexus_get_db_status,
    commands::nexus_get_db_counts,
    commands::nexus_create_project,
    commands::nexus_list_projects,
    commands::nexus_update_project,
    commands::nexus_delete_project,
    commands::nexus_create_task,
    commands::nexus_list_tasks,
    commands::nexus_update_task,
    commands::nexus_update_task_status,
    commands::nexus_delete_task,
])
```

### 5.7 Tests in `db/tasks.rs`

The `#[cfg(test)]` module reuses the in-memory pattern from `db/projects.rs`: open `:memory:`, enable foreign keys, apply the real `MIGRATIONS`. Required cases:

| Test | Asserts |
|------|---------|
| `insert_task_defaults_to_open` | Omitting status yields `'open'`; `external_id` and `assigned_agent` are `NULL`. |
| `insert_task_rejects_unknown_project` | Foreign key rejects a dangling `project_id`. |
| `insert_task_rejects_empty_title` | Whitespace-only title is rejected before the insert. |
| `insert_task_rejects_bad_status` | An unknown status value is rejected with the vocabulary in the message. |
| `list_tasks_is_scoped_to_project` | Tasks from project B never appear in project A's list. |
| `update_task_changes_fields_and_updated_at` | Fields change, `created_at` is stable, `updated_at` advances. |
| `update_task_preserves_external_id_and_agent` | Seed a row with a non-null `external_id` via direct SQL, run `update_task`, assert it survives. |
| `update_task_status_only` | Status and `updated_at` change; title and description do not. |
| `delete_task_removes_only_that_task` | Sibling tasks in the same project survive. |
| `delete_project_still_cascades_to_tasks` | The NEXUS-003 invariant, re-asserted from the task module's perspective. |

`update_task_preserves_external_id_and_agent` is the load-bearing one. It is the only automated guard against the 4.3 failure mode.

---

## 6. TypeScript Types

### 6.1 Additions to `src/types/db.ts`

```typescript
export type TaskStatus = 'open' | 'in_progress' | 'blocked' | 'done';

export interface Task {
  id: number;
  externalId: string | null;
  title: string;
  description: string | null;
  status: TaskStatus;
  projectId: number;
  assignedAgent: number | null;
  createdAt: string;
  updatedAt: string;
}

export interface CreateTaskInput {
  projectId: number;
  title: string;
  description?: string;
  status?: TaskStatus;
}

export interface UpdateTaskInput {
  id: number;
  title: string;
  description?: string;
  status: TaskStatus;
}

export interface UpdateTaskStatusInput {
  id: number;
  status: TaskStatus;
}
```

`Task.status` is typed as `TaskStatus` rather than `string`. The Rust layer rejects anything outside the set on write, so any value the database can return is in the set. A row hand-edited through `sqlite3` could violate this; the UI must render an unrecognized status as-is rather than crash, which the status pill handles with a fallback style.

### 6.2 Additions to `src/types/index.ts`

```typescript
export interface TaskFormValues {
  title: string;
  description: string;
  status: TaskStatus;
}

export type TaskFormMode = 'create' | 'edit';
```

`NexusScreen` and `NexusView` are not changed. See 2.3.

### 6.3 Additions to `src/lib/nexus-db.ts`

```typescript
export function createTask(input: CreateTaskInput): Promise<Task> {
  return invoke<Task>('nexus_create_task', { input });
}

export function listTasks(projectId: number): Promise<Task[]> {
  return invoke<Task[]>('nexus_list_tasks', { projectId });
}

export function updateTask(input: UpdateTaskInput): Promise<Task> {
  return invoke<Task>('nexus_update_task', { input });
}

export function updateTaskStatus(input: UpdateTaskStatusInput): Promise<Task> {
  return invoke<Task>('nexus_update_task_status', { input });
}

export function deleteTask(id: number): Promise<void> {
  return invoke<void>('nexus_delete_task', { id });
}
```

`listTasks` passes `projectId` as a bare argument, matching how `deleteProject` passes `id`. Tauri maps the camelCase argument name onto the snake_case Rust parameter.

---

## 7. React Component Design

### 7.1 `TaskList`

**File:** `src/components/TaskList/TaskList.tsx`

The only task component that talks to the database.

```typescript
interface TaskListProps {
  projectId: number;
  onCountChange: (count: number) => void;
}
```

State: `tasks`, `loading`, `error`, `showCreateForm`, `submitting`, `editingId: number | null`, `confirmDeleteId: number | null`, `busyId: number | null`.

Responsibilities:

- Fetch via `listTasks(projectId)` on mount and whenever `projectId` changes.
- Call `onCountChange(tasks.length)` after every successful fetch or mutation.
- Render the header: "Tasks", total count, and a status breakdown (F-10).
- Toggle an inline `TaskForm mode="create"`.
- Render one `TaskCard` per task.
- Own `createTask`, `updateTask`, `updateTaskStatus`, and `deleteTask` calls.
- Track which single task is expanded for edit, and which single task is awaiting delete confirmation. Both are `number | null`, not sets: only one task may be in either state at a time. Opening one closes the other.

`onCountChange` must be wrapped in `useCallback` by `ProjectDetail`, since `TaskList` will list it in effect dependencies. This is the same discipline `ProjectDetail` already applies to `onActiveProjectChange`.

### 7.2 `TaskCard`

**File:** `src/components/TaskCard/TaskCard.tsx`

Presentational. One task row.

```typescript
interface TaskCardProps {
  task: Task;
  isEditing: boolean;
  isConfirmingDelete: boolean;
  busy: boolean;
  onStatusChange: (status: TaskStatus) => void;
  onEditToggle: () => void;
  onDeleteRequest: () => void;
  onDeleteCancel: () => void;
  onDeleteConfirm: () => void;
  children?: React.ReactNode;   // the inline edit form, injected by TaskList
}
```

Layout:

- Status pill on the left, colored per status, clickable to advance status (F-06)
- Title, then description on a second line if set
- Creation date, muted, right-aligned
- Edit and Delete buttons
- Inline confirmation row when `isConfirmingDelete`, matching the `ProjectDetail` delete pattern
- `children` renders below the row when `isEditing`

The status pill advances through the vocabulary in order and wraps: `open -> in_progress -> blocked -> done -> open`. The control is a `<button>` with an `aria-label` naming both the current and the next status, so the interaction is not mouse-only or color-only.

**Unlike `ProjectCard`, the whole card is not a button.** `TaskCard` contains buttons, and nesting interactive elements is invalid HTML and breaks keyboard navigation. There is no task detail screen to navigate to, so the card needs no click target of its own.

### 7.3 `TaskForm`

**File:** `src/components/TaskForm/TaskForm.tsx`

Presentational, shared by create and edit, mirroring `ProjectForm`.

```typescript
interface TaskFormProps {
  mode: TaskFormMode;
  initialValues?: TaskFormValues;
  onSubmit: (values: TaskFormValues) => Promise<void>;
  onCancel: () => void;
  submitting: boolean;
}
```

Fields:

- `title`: text input, required, non-empty after trim
- `description`: text input, optional
- `status`: a segmented row of buttons, one per status, current selection highlighted

Buttons: submit reads "Create Task" or "Save Changes"; cancel calls `onCancel`.

The status control is a button group rather than a `<select>`. `globals.css` resets `button` and `input` but not `select`, so a native select would arrive unstyled against the black and red theme and would need new reset CSS. A button group reuses `.nexus-btn` and adds nothing.

`TaskForm` never calls a command. It calls `onSubmit(values)`.

### 7.4 `ProjectDetail` changes

The only NEXUS-003 component this milestone modifies. Two changes:

1. Add `taskCount` state, a `useCallback` `handleTaskCountChange`, and render `<TaskList projectId={project.id} onCountChange={handleTaskCountChange} />` below the fields block, in `mode === 'view'` only.
2. Make the delete confirmation text reflect the count (F-09):
   - `taskCount === 0`: "Delete this project? This cannot be undone."
   - `taskCount === 1`: "Delete this project and its 1 task? This cannot be undone."
   - otherwise: "Delete this project and its N tasks? This cannot be undone."

Nothing else in `ProjectDetail` changes. Its project CRUD, edit mode, and navigation are untouched.

### 7.5 Styling

New CSS files: `TaskList.css`, `TaskCard.css`, `TaskForm.css`, using existing design tokens and the shared `.nexus-btn` / `.nexus-field` classes.

One addition to `globals.css` is permitted: a `.nexus-status-pill` block with a modifier per status. Status is the one piece of task vocabulary that appears in three components (`TaskCard`, `TaskForm`, and the `TaskList` header breakdown), so defining it once is the same reasoning that justified `.nexus-btn` in NEXUS-003.

Status colors, from existing tokens only:

| Status | Token |
|--------|-------|
| `open` | `--color-text-secondary` |
| `in_progress` | `--color-accent` |
| `blocked` | `--color-accent-dim` |
| `done` | `--color-status-online` |
| unrecognized | `--color-text-muted` |

No new tokens. No theme changes.

---

## 8. Implementation Tasks

Sequential. Rust tasks gate on `cargo check`; the test task gates on `cargo test --lib`.

| # | Task | Description |
|---|------|-------------|
| T-01 | **Create `db/tasks.rs`** | `Task`, `CreateTaskInput`, `UpdateTaskInput`, `UpdateTaskStatusInput`, `TASK_STATUSES`, `validate_status`, `map_task_row`, `get_task_by_id`. Run `cargo check`. |
| T-02 | **Register the module** | Add `pub mod tasks;` to `db/mod.rs`. Run `cargo check`. |
| T-03 | **Implement `insert_task` and `list_tasks`** | Insert writes `project_id`, `title`, `description`, `status` only. List filters by `project_id`, orders by `created_at DESC`. Run `cargo check`. |
| T-04 | **Implement `update_task`, `update_task_status`, `delete_task`** | Explicit `updated_at`. `external_id` and `assigned_agent` are never in an `UPDATE` column list. Run `cargo check`. |
| T-05 | **Add the `#[cfg(test)]` module** | All ten cases from 5.7. Run `cargo test --lib`; all tests including the NEXUS-003 project tests must pass. |
| T-06 | **Add the five commands** | `commands/mod.rs`, following the existing lock-then-delegate shape. Run `cargo check`. |
| T-07 | **Register commands in `lib.rs`** | Extend `generate_handler!` to eleven entries. Run `cargo check`. |
| T-08 | **Add TypeScript types** | `TaskStatus`, `Task`, `CreateTaskInput`, `UpdateTaskInput`, `UpdateTaskStatusInput` in `types/db.ts`; `TaskFormValues`, `TaskFormMode` in `types/index.ts`. |
| T-09 | **Add the five wrappers** | `src/lib/nexus-db.ts`. |
| T-10 | **Add `.nexus-status-pill` to `globals.css`** | Append only. No token or existing-rule changes. |
| T-11 | **Build `TaskForm`** | `TaskForm.tsx` + `.css`. Title validation, status button group, no command calls. |
| T-12 | **Build `TaskCard`** | `TaskCard.tsx` + `.css`. Status pill, inline delete confirmation, `children` slot for the edit form. No nested interactive elements. |
| T-13 | **Build `TaskList`** | `TaskList.tsx` + `.css`. Fetch, empty state, count and breakdown, create toggle, single-open edit and delete-confirm state, all command calls. |
| T-14 | **Modify `ProjectDetail`** | Add `taskCount` state and the memoized callback, render `TaskList` in view mode, update the delete confirmation copy. Change nothing else. |
| T-15 | **Verify frontend build** | `pnpm build`. Zero TypeScript errors. |
| T-16 | **Verify release build** | `pnpm tauri build`. `.app` and ARM64 DMG produced. |
| T-17 | **Manual functional pass** | Section 9 checklist, including a restart and a `sqlite3` cascade confirmation. |

---

## 9. Acceptance Criteria

**Build and structure**

- [ ] `pnpm build` completes with zero TypeScript or Vite errors.
- [ ] `pnpm tauri build` produces `NEXUS.app` and the ARM64 DMG.
- [ ] `cargo test --lib` passes, including the five NEXUS-003 project tests.
- [ ] No new Rust or frontend dependencies. `package.json`, `pnpm-lock.yaml`, `Cargo.toml`, and `Cargo.lock` are unchanged.
- [ ] `migrations.rs` is unchanged and the live database is still at migration level 1.
- [ ] `AppShell.tsx`, `Dashboard.tsx`, `ProjectList.tsx`, `ProjectCard.tsx`, and `ProjectForm.tsx` are unchanged.
- [ ] No component imports `@tauri-apps/api`. All IPC goes through `src/lib/nexus-db.ts`.
- [ ] No raw SQL exists anywhere under `src/`.
- [ ] `NexusScreen` still has exactly two values.

**Task behavior**

- [ ] Opening a project with no tasks shows the task empty state.
- [ ] A task can be created with a title, and it appears in the list immediately.
- [ ] A task with an empty title cannot be submitted.
- [ ] A new task defaults to `open`.
- [ ] Description and status can be set at creation time.
- [ ] The task list shows the total count and a per-status breakdown.
- [ ] A task can be edited inline; title, description, and status all save.
- [ ] Clicking the status pill advances the status and wraps from `done` back to `open`.
- [ ] `updated_at` changes after an edit or a status change; `created_at` does not.
- [ ] Deleting a task requires inline confirmation. Cancel preserves it, confirm removes it.
- [ ] Only one task can be in edit mode or delete-confirm at a time.

**Scoping and persistence**

- [ ] Tasks created under project A do not appear under project B.
- [ ] Navigating back to the list and reopening a project shows the same tasks.
- [ ] All task data survives a full application restart.
- [ ] After restart, edited titles, descriptions, and statuses reflect the saved values.

**Cascade and integration columns**

- [ ] The project delete confirmation names the correct task count, with correct singular and plural wording.
- [ ] Deleting a project with tasks removes the project and, verified through `sqlite3`, leaves zero rows in `tasks` with that `project_id`.
- [ ] Deleting a project does not affect any other project's tasks.
- [ ] Seeding a task with a non-null `external_id` through `sqlite3`, then editing that task in the UI, leaves `external_id` intact. Verified through `sqlite3` after the edit.
- [ ] Every task the UI creates has `external_id` and `assigned_agent` set to `NULL`.

---

## 10. Explicitly Out of Scope

Deferred deliberately, not forgotten:

- **Task counts on `ProjectCard`.** Would need either a sixth query shape returning counts per project or an N+1 fetch from the list screen. Revisit when a project dashboard exists.
- **Task filtering, search, sorting controls, and reordering.** The list is creation-date descending, full stop.
- **Task detail as its own screen.** See 2.3.
- **Bulk selection or bulk status changes.**
- **Subtasks, dependencies, due dates, priorities, labels, or estimates.** None have columns; all would require migration 002.
- **`external_id` in the UI.** It belongs to whichever integration milestone first writes it.
- **`assigned_agent` in the UI.** Blocked on the agent registry milestone; `ai_agents` is empty.

Also out of scope, per the standing NEXUS constraints:

- Jira, Claude, PlayerZero, Cursor, Grok, and ChatGPT integrations
- AI orchestration or execution of any kind
- IDE management, IDE launching, terminal execution
- Settings management
- Voice recognition, text to speech, browser automation
- News, weather, morning briefings, notifications
- Authentication, cloud sync, CI/CD, auto-update
- Custom title bar or window chrome
- Any routing library, state manager, UI library, form library, or ORM
- Any new Rust or frontend dependency
