# NEXUS-003: Project & Workspace Management

## Overview

Build the first real user-facing feature of NEXUS: the ability to create, view, edit, and delete projects through a proper UI. This milestone replaces the NEXUS-002 development verification panel with a functional Project Manager screen, giving the user meaningful control over their workspace data for the first time.

NEXUS-001 established the desktop shell. NEXUS-002 established the persistence layer and proved it works. NEXUS-003 makes that persistence layer useful.

---

## 1. Existing State (from NEXUS-001 and NEXUS-002)

### 1.1 Architecture already in place

**Rust layer**
- `db/mod.rs` — `DbState`, `init()`, migration runner, FK enforcement
- `db/migrations.rs` — versioned `MIGRATIONS` array (migration 001 applied)
- `db/projects.rs` — `insert_project`, `list_projects`, `delete_project`, `count_projects`, `count_all_tables`; `Project` and `CreateProjectInput` structs
- `commands/mod.rs` — five registered commands: `nexus_get_db_status`, `nexus_get_db_counts`, `nexus_create_project`, `nexus_list_projects`, `nexus_delete_project`
- `lib.rs` — thin orchestrator; registers `DbState` and all commands

**Frontend**
- `src/lib/nexus-db.ts` — isolated `invoke()` wrappers for all five commands
- `src/types/db.ts` — `DbStatus`, `DbCounts`, `Project`, `CreateProjectInput`
- `src/components/DbPanel/DbPanel.tsx` — development-only verification panel (to be retired from the main view in NEXUS-003, but the file is not deleted)
- `App.tsx → AppShell → [Logo, StatusBar, Dashboard, CommandBar]`
- `Dashboard` currently renders `DbPanel` inline

### 1.2 What NEXUS-002 left incomplete

The `projects` table supports: `name`, `description`, `repository_path`, `repository_url`, `default_ide_id`, `default_agent_id`. However:
- There is no **update** command — only create/delete.
- `default_ide_id` and `default_agent_id` exist in the schema but the `ides` and `ai_agents` tables are empty; they cannot be meaningfully used yet.
- The UI for project management is a raw developer panel, not a user-facing feature.
- There is no navigation model — `Dashboard` is a single fixed screen.

---

## 2. Requirements

### 2.1 Functional Requirements

| ID | Requirement |
|----|-------------|
| F-01 | The user must be able to create a new project by providing a name (required), description (optional), repository path (optional), and repository URL (optional). |
| F-02 | The user must be able to view all projects in a list. |
| F-03 | Each project in the list must display: name, description (if set), repository path (if set), and creation date. |
| F-04 | The user must be able to open a project detail view showing all project fields. |
| F-05 | The user must be able to edit an existing project's name, description, repository path, and repository URL. |
| F-06 | The user must be able to delete a project. Deletion must require a confirmation step to prevent accidental data loss. |
| F-07 | The UI must show a meaningful empty state when no projects exist, prompting the user to create their first project. |
| F-08 | The application must display the active/selected project context in the header. |
| F-09 | The NEXUS-002 `DbPanel` must be removed from the primary Dashboard view. It may be retained as a hidden component for future debug use but must not be visible to the user in normal operation. |
| F-10 | All project operations (create, read, update, delete) must persist to SQLite and survive application restart. |

### 2.2 Non-Functional Requirements

| ID | Requirement |
|----|-------------|
| N-01 | No new Rust or frontend dependencies may be introduced unless strictly required and explicitly justified. |
| N-02 | No routing library. Navigation between views is managed with a simple `activeView` state in `AppShell` or a dedicated navigation hook — no React Router, no Tanstack Router. |
| N-03 | All new Tauri commands follow the existing `nexus_` prefix and `Result<T, String>` error pattern. |
| N-04 | All new TypeScript types go in `src/types/`. |
| N-05 | All new `invoke()` wrappers go in `src/lib/nexus-db.ts`. No component may call `invoke()` directly. |
| N-06 | Database logic stays in `src-tauri/src/db/`. Commands stay in `src-tauri/src/commands/`. `lib.rs` stays thin. |
| N-07 | The existing `AppShell`, `Logo`, `StatusBar`, and `CommandBar` components must not be restructured. They may receive new props if strictly required, but their files and CSS are otherwise unchanged. |
| N-08 | The schema migration number must increment correctly; migration 002 must be added for the `UPDATE` support column if needed, not applied ad hoc. |
| N-09 | Form validation must be handled in React. The Rust layer validates non-emptiness of required fields and returns a typed error string. |
| N-10 | Local-first. No remote backend, no authentication, no cloud services. |

### 2.3 Design Principle: View vs. Modal

For NEXUS-003, project detail and editing are presented as an **in-place panel swap within the Dashboard area** — not a modal dialog, not a separate route. The Dashboard area transitions between:

- `projects-list` — the default view showing all projects
- `project-detail` — a detail/edit view for the selected project

This avoids introducing a routing library while establishing the navigation pattern NEXUS will use for future views.

---

## 3. Architecture

### 3.1 Updated Data Flow

```
AppShell
  ├── header: Logo  +  [ActiveProject badge]  +  StatusBar
  ├── Dashboard  (activeView state lives here or in AppShell)
  │     ├── view = "projects-list"  →  ProjectList + ProjectForm
  │     └── view = "project-detail" →  ProjectDetail (read + edit + delete)
  └── CommandBar
```

### 3.2 Navigation Model

A `useNexusNav` hook (or inline state in `AppShell`) holds:

```typescript
type NexusView =
  | { screen: 'projects' }
  | { screen: 'project-detail'; projectId: number };
```

`AppShell` passes the current view and a `navigate` function down to `Dashboard`. `Dashboard` renders the correct sub-panel based on `screen`. No URL, no router.

### 3.3 Rust Module Structure (changes from NEXUS-002)

```
src-tauri/src/
├── main.rs             (unchanged)
├── lib.rs              (add new commands to invoke_handler)
├── db/
│   ├── mod.rs          (unchanged)
│   ├── migrations.rs   (add migration 002 if schema changes needed)
│   ├── projects.rs     (add update_project helper)
│   └── [no new files]
└── commands/
    └── mod.rs          (add nexus_update_project command)
```

### 3.4 Frontend Structure (changes from NEXUS-002)

```
src/
├── lib/
│   └── nexus-db.ts          (add updateProject wrapper)
├── types/
│   ├── db.ts                (add UpdateProjectInput)
│   └── index.ts             (add NexusView type)
└── components/
    ├── AppShell/             (minor: pass view/navigate to Dashboard)
    ├── Dashboard/            (replace DbPanel with ProjectsView or ProjectDetail)
    ├── DbPanel/              (unchanged file, just no longer rendered)
    ├── ProjectList/
    │   ├── ProjectList.tsx
    │   └── ProjectList.css
    ├── ProjectCard/
    │   ├── ProjectCard.tsx
    │   └── ProjectCard.css
    ├── ProjectForm/
    │   ├── ProjectForm.tsx   (used for both create and edit)
    │   └── ProjectForm.css
    └── ProjectDetail/
        ├── ProjectDetail.tsx
        └── ProjectDetail.css
```

---

## 4. Database Schema Changes

### 4.1 Assessment of NEXUS-002 Schema

The existing `projects` table is sufficient for all NEXUS-003 operations. No new columns are needed. The update operation (`UPDATE projects SET ... WHERE id = ?`) operates on existing columns.

**The `updated_at` column exists but is not automatically updated on row modification.** SQLite does not have `ON UPDATE` triggers by default. The `update_project` Rust function must explicitly set `updated_at = strftime(...)` in the `UPDATE` statement. No schema change is needed for this — it is a query responsibility.

### 4.2 Project → Task Delete Behavior

The NEXUS-002 schema defined:

```sql
tasks.project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE
```

This means: **when a project row is deleted, all task rows whose `project_id` matches are automatically deleted by SQLite before the project row is removed.**

This is the correct and intentional behavior:
- Tasks are children of projects. A task cannot exist without a parent project.
- No orphaned task rows can remain after a project deletion.
- `PRAGMA foreign_keys = ON` is enforced on every connection in `db::init()`, so the cascade fires reliably.

**Verification required in T-14:** The acceptance test must confirm cascade behavior. The procedure is:
1. Create a project.
2. Manually insert a task row referencing that project's ID directly via the DB (or via a future task command once available).
3. Delete the project via the NEXUS UI.
4. Confirm the project is gone from the project list.
5. Confirm via `sqlite3` CLI that the `tasks` table contains zero rows for that `project_id`.

Since NEXUS-003 does not expose a task-creation UI, step 2 uses `sqlite3` CLI directly against `nexus.db` to seed a test task row. This is a verification-only step and does not affect the production code path.

**Acceptance criterion added:** After deleting a project that has associated task rows, the tasks table must contain no rows with that project's ID. This is verified in T-14.

### 4.3 Migration 002 — Not Required

No new tables, columns, indexes, or constraints are needed for NEXUS-003. **Migration 002 is not added in this milestone.** The migration array in `migrations.rs` remains at one entry.

If a future milestone requires schema changes, migration 002 will be added then.

---

## 5. Rust/Tauri Command Design

### 5.1 New Command

One new command is added. All five existing NEXUS-002 commands are retained unchanged.

| Command | Input | Output | Purpose |
|---------|-------|--------|---------|
| `nexus_update_project` | `UpdateProjectInput` | `Project` | Update name, description, repository_path, repository_url for an existing project. Sets `updated_at`. Returns the updated full row. |

### 5.2 `UpdateProjectInput` Rust Struct

```rust
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProjectInput {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub repository_path: Option<String>,
    pub repository_url: Option<String>,
}
```

### 5.3 `update_project` Rust Function (in `db/projects.rs`)

```rust
pub fn update_project(
    conn: &Connection,
    input: &UpdateProjectInput,
) -> Result<Project, String> {
    // Validate name is not empty
    if input.name.trim().is_empty() {
        return Err("Project name cannot be empty".to_string());
    }

    let affected = conn.execute(
        "UPDATE projects
         SET name             = ?1,
             description      = ?2,
             repository_path  = ?3,
             repository_url   = ?4,
             updated_at       = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE id = ?5",
        rusqlite::params![
            input.name.trim(),
            input.description,
            input.repository_path,
            input.repository_url,
            input.id,
        ],
    )
    .map_err(|e| format!("Failed to update project: {e}"))?;

    if affected == 0 {
        return Err(format!("Project {} not found", input.id));
    }

    get_project_by_id(conn, input.id)
}
```

### 5.4 Command Registration

`lib.rs` `invoke_handler` is extended:

```rust
.invoke_handler(tauri::generate_handler![
    commands::nexus_get_db_status,
    commands::nexus_get_db_counts,
    commands::nexus_create_project,
    commands::nexus_list_projects,
    commands::nexus_delete_project,
    commands::nexus_update_project,   // ← new
])
```

### 5.5 Existing Commands

All five NEXUS-002 commands (`nexus_get_db_status`, `nexus_get_db_counts`, `nexus_create_project`, `nexus_list_projects`, `nexus_delete_project`) are retained without modification.

---

## 6. TypeScript Types

### 6.1 New types in `src/types/db.ts`

```typescript
// Added to existing src/types/db.ts

export interface UpdateProjectInput {
  id: number;
  name: string;
  description?: string;
  repositoryPath?: string;
  repositoryUrl?: string;
}
```

### 6.2 New type in `src/types/index.ts`

```typescript
// Navigation views — no routing library required
export type NexusScreen =
  | 'projects'
  | 'project-detail';

export interface NexusView {
  screen: NexusScreen;
  projectId?: number;   // set when screen === 'project-detail'
}
```

### 6.3 New wrapper in `src/lib/nexus-db.ts`

```typescript
// Added to existing nexus-db.ts
import type { UpdateProjectInput } from '../types/db';

export function updateProject(input: UpdateProjectInput): Promise<Project> {
  return invoke<Project>('nexus_update_project', { input });
}
```

### 6.4 Component prop types (defined inline or in `src/types/`)

```typescript
export interface ProjectFormValues {
  name: string;
  description: string;
  repositoryPath: string;
  repositoryUrl: string;
}

export type ProjectFormMode = 'create' | 'edit';
```

---

## 7. React Component Design

### 7.1 Navigation State

Navigation state is owned by `AppShell` and passed to `Dashboard` via props:

```typescript
// in AppShell.tsx
const [view, setView] = useState<NexusView>({ screen: 'projects' });

function navigate(next: NexusView) {
  setView(next);
}
```

`AppShell` passes `view` and `navigate` to `Dashboard`. `Dashboard` renders the correct sub-panel based on `view.screen`.

### 7.2 `Dashboard` Changes

`Dashboard.tsx` is modified to:
- Accept `view: NexusView` and `navigate: (v: NexusView) => void` props.
- Render `<ProjectList>` when `view.screen === 'projects'`.
- Render `<ProjectDetail>` when `view.screen === 'project-detail'`.
- Remove the `<DbPanel>` render. `DbPanel` file is not deleted.
- The background watermark and scroll wrapper are retained.

```typescript
// Dashboard.tsx — updated prop signature
interface DashboardProps extends PanelProps {
  view: NexusView;
  navigate: (v: NexusView) => void;
}
```

### 7.3 `ProjectList` Component

**File:** `src/components/ProjectList/ProjectList.tsx`

Responsibilities:
- Fetch all projects via `listProjects()` on mount.
- Render a list of `<ProjectCard>` components.
- Render an empty state when no projects exist.
- Render a "New Project" button that expands an inline `<ProjectForm mode="create">`.
- On project card click, call `navigate({ screen: 'project-detail', projectId: p.id })`.

State:
- `projects: Project[]`
- `loading: boolean`
- `error: string | null`
- `showCreateForm: boolean`

```
ProjectList
├── header row: "Projects" heading + "New Project" button
├── [empty state] — shown when projects.length === 0
├── [ProjectForm mode="create"] — shown when showCreateForm === true
└── projects.map → <ProjectCard> × n
```

### 7.4 `ProjectCard` Component

**File:** `src/components/ProjectCard/ProjectCard.tsx`

A single project row/card. Displays:
- Project name (prominent)
- Description (truncated to one line, if set)
- Repository path or URL (if set, as a dimmed secondary line)
- Creation date (right-aligned, muted)

Props:
```typescript
interface ProjectCardProps {
  project: Project;
  onClick: () => void;
}
```

No actions on the card itself (delete/edit are on the detail view). Clicking the card calls `onClick`.

### 7.5 `ProjectForm` Component

**File:** `src/components/ProjectForm/ProjectForm.tsx`

Reusable form used for both create and edit modes.

Props:
```typescript
interface ProjectFormProps {
  mode: ProjectFormMode;           // 'create' | 'edit'
  initialValues?: ProjectFormValues;
  onSubmit: (values: ProjectFormValues) => Promise<void>;
  onCancel: () => void;
  submitting: boolean;
}
```

Fields:
- `name` — text input, required. Validation: non-empty after trim.
- `description` — text input, optional.
- `repositoryPath` — text input, optional.
- `repositoryUrl` — text input, optional.

Buttons:
- Submit: "Create Project" (mode=create) or "Save Changes" (mode=edit)
- Cancel: calls `onCancel`

Form does not call `invoke()` directly — it calls `onSubmit(values)` and the parent handles the command call.

### 7.6 `ProjectDetail` Component

**File:** `src/components/ProjectDetail/ProjectDetail.tsx`

Shows the full detail of one project and supports in-place editing.

Props:
```typescript
interface ProjectDetailProps {
  projectId: number;
  navigate: (v: NexusView) => void;
}
```

State:
- `project: Project | null`
- `mode: 'view' | 'edit'`
- `loading: boolean`
- `saving: boolean`
- `deleting: boolean`
- `confirmDelete: boolean`
- `error: string | null`

Layout:
```
ProjectDetail
├── top bar: ← Back button | project name | Edit button | Delete button
├── [view mode]
│     ├── name, description, repository path, repository URL
│     └── created_at, updated_at
└── [edit mode]
      └── <ProjectForm mode="edit" initialValues={...} onSubmit={handleSave} onCancel={exitEdit}>
```

Delete flow:
1. User clicks "Delete".
2. `confirmDelete` state becomes `true`.
3. An inline confirmation row appears: "Delete this project?" + "Confirm" + "Cancel".
4. On "Confirm", `deleteProject(id)` is called, then `navigate({ screen: 'projects' })`.

Back button: calls `navigate({ screen: 'projects' })`.

### 7.7 Header Active Project Badge

`AppShell` passes the active project name (or `null`) to a new optional element in the header. When a project is selected (`view.screen === 'project-detail'`), a small project name badge appears between the `Logo` and `StatusBar`. It is absent on the projects list screen.

This does not require a new component — it can be a conditional element inside `AppShell`'s existing header. A `selectedProject: Project | null` state in `AppShell` stores the last selected project for display.

### 7.8 Component Tree (after NEXUS-003)

```
App
└── AppShell
    ├── header
    │   ├── Logo
    │   ├── [active project badge — conditional]
    │   └── StatusBar
    ├── Dashboard(view, navigate)
    │   ├── watermark (unchanged)
    │   └── dashboard__scroll
    │       ├── [view = 'projects']
    │       │   └── ProjectList(navigate)
    │       │       ├── empty state OR
    │       │       ├── ProjectForm(mode='create') [conditional]
    │       │       └── ProjectCard × n
    │       └── [view = 'project-detail']
    │           └── ProjectDetail(projectId, navigate)
    │               ├── view mode: fields
    │               └── edit mode: ProjectForm(mode='edit')
    └── CommandBar
```

---

## 8. Implementation Tasks

Tasks are ordered for sequential execution. Rust tasks include a `cargo check` gate.

| # | Task | Description |
|---|------|-------------|
| T-01 | **Add `update_project` to `db/projects.rs`** | Implement the `UpdateProjectInput` struct and `update_project()` function. Validates non-empty name, runs `UPDATE`, sets `updated_at`, returns updated row. Run `cargo check`. |
| T-02 | **Add `nexus_update_project` to `commands/mod.rs`** | Implement the `#[tauri::command]` wrapper. Run `cargo check`. |
| T-03 | **Register command in `lib.rs`** | Add `commands::nexus_update_project` to the `generate_handler!` macro. Run `cargo check`. |
| T-04 | **Add `UpdateProjectInput` to `src/types/db.ts`** | Add the TypeScript type mirroring the Rust struct. |
| T-05 | **Add `NexusView` / `NexusScreen` to `src/types/index.ts`** | Add navigation types. |
| T-06 | **Add `updateProject` to `src/lib/nexus-db.ts`** | Add the typed `invoke()` wrapper. |
| T-07 | **Build `ProjectCard` component** | `ProjectCard.tsx` + `ProjectCard.css`. Displays project name, description, repo, date. Calls `onClick` on click. |
| T-08 | **Build `ProjectForm` component** | `ProjectForm.tsx` + `ProjectForm.css`. Handles both create and edit modes. Client-side name validation. No `invoke()` calls. |
| T-09 | **Build `ProjectList` component** | `ProjectList.tsx` + `ProjectList.css`. Fetches projects, renders cards, empty state, inline create form toggle. |
| T-10 | **Build `ProjectDetail` component** | `ProjectDetail.tsx` + `ProjectDetail.css`. Fetches single project, view/edit mode, delete with inline confirmation, back navigation. |
| T-11 | **Update `AppShell`** | Add `view` state, `navigate` function, `selectedProject` state. Pass `view` and `navigate` to `Dashboard`. Add conditional active project badge to header. |
| T-12 | **Update `Dashboard`** | Accept `view` and `navigate` props. Render `ProjectList` or `ProjectDetail` based on `view.screen`. Remove `DbPanel` from render output. Retain watermark and scroll wrapper. |
| T-13 | **Verify frontend build** | Run `pnpm build`. Fix any TypeScript errors. Confirm zero type errors. |
| T-14 | **Verify full build and functional test** | Run `pnpm tauri build`. Launch app. Verify: empty state shown on first launch; create a project; project appears in list; open project detail; edit name and save; confirm change persisted after app restart; delete project with confirmation; confirm project is removed. |

---

## 9. Acceptance Criteria

The milestone is complete when all of the following are true:

- [ ] `pnpm build` completes without TypeScript or Vite errors.
- [ ] `pnpm tauri build` produces a macOS `.app` bundle without errors.
- [ ] The app launches and the NEXUS window opens correctly. NEXUS-001 header (Logo, StatusBar, CommandBar) is intact.
- [ ] The `DbPanel` is no longer visible in normal application use.
- [ ] On first launch with no projects, the `ProjectList` shows a meaningful empty state.
- [ ] The user can create a project with name, description, repository path, and repository URL.
- [ ] A project with an empty name cannot be submitted (client-side validation).
- [ ] Created projects appear in the `ProjectList` immediately.
- [ ] Clicking a project card navigates to the `ProjectDetail` view.
- [ ] `ProjectDetail` displays all fields of the selected project.
- [ ] The active project name is shown in the `AppShell` header when a project is selected.
- [ ] The user can edit a project's fields and save. The `updated_at` timestamp changes.
- [ ] Clicking "Back" from `ProjectDetail` returns to the `ProjectList`.
- [ ] The user can delete a project. An inline confirmation step is required before deletion executes.
- [ ] After deletion, the user is returned to `ProjectList` and the deleted project is absent.
- [ ] All project data (create, edit, delete) persists across application restart.
- [ ] After restarting the app, edited project fields reflect the saved values.
- [ ] `nexus_update_project` is the only new Tauri command added.
- [ ] No new Rust or frontend dependencies are introduced.
- [ ] No routing library is present.
- [ ] All DB logic remains in `src-tauri/src/db/`. All commands remain in `src-tauri/src/commands/`.
- [ ] No component calls `invoke()` directly; all IPC goes through `src/lib/nexus-db.ts`.

---

## 10. Explicitly Out of Scope

The following must NOT be implemented in NEXUS-003:

- Jira, Claude, PlayerZero, Cursor, Grok, ChatGPT integrations
- AI agent management or assignment (the `ai_agents` table exists but is not surfaced in the UI)
- IDE management or assignment (the `ides` table exists but is not surfaced in the UI)
- Task management (the `tasks` table exists but is not surfaced in the UI)
- Settings management
- Project search or filtering
- Project sorting controls
- Project reordering / drag-and-drop
- Multiple project selection
- Project import/export
- Repository cloning or git operations
- IDE launching or terminal execution
- Voice recognition or command execution
- Browser automation
- News, weather, notifications, morning briefings
- Authentication or user accounts
- Cloud synchronization or remote APIs
- CI/CD, auto-update
- Custom title bar or window chrome
- Routing library (React Router, Tanstack Router, etc.)
- Global state manager (Redux, Zustand, Jotai, etc.)
- Any new Rust or frontend dependencies
