# NEXUS-005: IDE & Agent Registry

## Overview

Surface `ides` and `ai_agents`, the last two unused tables, then wire the three foreign-key columns that have been decorative since NEXUS-002: `projects.default_ide_id`, `projects.default_agent_id`, and `tasks.assigned_agent`.

After NEXUS-005 every table in migration 001 has a producer and every foreign key has a purpose. That is the point of this milestone: it closes the schema, not because registration is thrilling on its own, but because every integration milestone on the roadmap is blocked on there being a registry to integrate against.

**Stated plainly: the payoff here is deferred.** Launching an IDE and executing an agent are out of scope and stay out of scope. NEXUS-005 delivers registration and assignment. What it buys is that when launching does come into scope, it is one milestone rather than three.

NEXUS-005 is also the first milestone that changes the shell. It adds the third screen, which means `NexusScreen` grows past two values and `AppShell` gains a navigation control. Both were explicitly frozen in NEXUS-004. Section 2.3 explains why unfreezing them now is correct rather than a regression.

### Dependency on NEXUS-004

This spec is written against NEXUS-004's intended end state. At time of writing NEXUS-004 is implemented, tested at the database layer, and building, but its manual click-through is incomplete and it is uncommitted. Anything in section 1.1 describing task behavior is therefore assumed, not confirmed. If the NEXUS-004 pass turns up defects, fix those before starting T-01.

### Severability

The milestone splits cleanly at T-12:

- **Part A (T-01 to T-12):** the registry itself. Self-contained and shippable alone.
- **Part B (T-13 to T-20):** assignment wiring into projects and tasks. Depends on Part A, and nothing depends on it.

If Part A plus Part B is too large for one increment, Part B becomes NEXUS-006 with no rework. The task numbering and the acceptance checklist are already grouped for that split.

---

## 1. Existing State (from NEXUS-001 through NEXUS-004)

### 1.1 Architecture in place

**Rust layer**

- `db/mod.rs`: `DbState`, `init()`, migration runner, `PRAGMA foreign_keys = ON` on every connection
- `db/migrations.rs`: one entry, migration 001; live database at level 1
- `db/projects.rs`: project CRUD, `count_all_tables`, 5 tests
- `db/tasks.rs`: task CRUD, `TASK_STATUSES`, `validate_status`, 10 tests
- `commands/mod.rs`: eleven registered commands
- `lib.rs`: thin orchestrator

**Frontend**

- `src/lib/nexus-db.ts`: the only module importing `@tauri-apps/api`; one typed wrapper per command
- `src/types/db.ts`, `src/types/index.ts`
- `AppShell` owns `view` and `activeProjectName`; `NexusScreen` has two values
- `Dashboard` switches between `ProjectList` and `ProjectDetail`
- `ProjectList` / `ProjectCard` / `ProjectForm` / `ProjectDetail`
- `TaskList` / `TaskCard` / `TaskForm`, rendered inside `ProjectDetail` in view mode
- Shared `.nexus-btn`, `.nexus-field`, `.nexus-status-pill` in `globals.css`
- `DbPanel` on disk, not rendered

### 1.2 The two unused tables

```sql
CREATE TABLE IF NOT EXISTS ides (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    name            TEXT    NOT NULL,
    ide_type        TEXT    NOT NULL,
    executable_path TEXT,
    enabled         INTEGER NOT NULL DEFAULT 1,
    created_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS ai_agents (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    name            TEXT    NOT NULL,
    agent_type      TEXT    NOT NULL,
    enabled         INTEGER NOT NULL DEFAULT 1,
    executable_path TEXT,
    created_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
```

The two are isomorphic. Column-for-column they differ only in the name of the type field (`ide_type` against `agent_type`) and in column order, which is not semantically meaningful. Section 2.4 turns that observation into a design decision rather than leaving it as an accident.

### 1.3 The three decorative foreign keys

Verified against the live database:

| Table | Column | References | On delete |
|-------|--------|-----------|-----------|
| `projects` | `default_ide_id` | `ides(id)` | **SET NULL** |
| `projects` | `default_agent_id` | `ai_agents(id)` | **SET NULL** |
| `tasks` | `assigned_agent` | `ai_agents(id)` | **SET NULL** |
| `tasks` | `project_id` | `projects(id)` | CASCADE |

All three registry references are `SET NULL`, not `CASCADE`. This is the central invariant of NEXUS-005 and the mirror image of NEXUS-004's concern: deleting a registry row must **blank the reference and leave the referring row alive**. A cascade here would destroy projects when an IDE is removed.

---

## 2. Requirements

### 2.1 Functional Requirements

**Part A: the registry**

| ID | Requirement |
|----|-------------|
| F-01 | The user must be able to register an IDE with a name (required), type (required), and executable path (optional). |
| F-02 | The user must be able to register an AI agent with a name (required), type (required), and executable path (optional). |
| F-03 | The user must be able to view all registered IDEs and all registered agents. |
| F-04 | The user must be able to edit any field of a registered IDE or agent. |
| F-05 | The user must be able to enable or disable an IDE or agent without deleting it. |
| F-06 | The user must be able to delete an IDE or agent, with an inline confirmation matching the established pattern. |
| F-07 | The delete confirmation must warn that references to the entry will be cleared, and for IDEs and agents must state how many projects currently use it as a default. |
| F-08 | Both lists must show a meaningful empty state. |
| F-09 | The registry must be reachable from the application header, and the header must indicate which top-level screen is active. |
| F-10 | All registry operations must persist to SQLite and survive application restart. |

**Part B: assignment**

| ID | Requirement |
|----|-------------|
| F-11 | `ProjectForm` must let the user pick a default IDE and a default agent for a project, or leave either unset. |
| F-12 | `ProjectDetail` must display the project's default IDE and default agent by name in view mode. |
| F-13 | The user must be able to assign an agent to a task, or clear that assignment, from the task list. |
| F-14 | `TaskCard` must display the assigned agent's name when one is set. |
| F-15 | Only **enabled** IDEs and agents may be offered in any assignment control. An already-assigned entry that is later disabled must still render by name wherever it is displayed. |
| F-16 | Deleting an IDE or agent must clear every reference to it and must not delete any project or task. |
| F-17 | Assignments must persist to SQLite and survive application restart. |

### 2.2 Non-Functional Requirements

| ID | Requirement |
|----|-------------|
| N-01 | No new Rust or frontend dependencies. |
| N-02 | No routing library and no global state manager. Navigation stays view state in `AppShell`. |
| N-03 | All new commands use the `nexus_` prefix and the `Result<T, String>` error pattern. |
| N-04 | All new TypeScript types go in `src/types/`. |
| N-05 | All new `invoke()` wrappers go in `src/lib/nexus-db.ts`. No component may import `@tauri-apps/api`. |
| N-06 | Registry database logic goes in new `src-tauri/src/db/ides.rs` and `src-tauri/src/db/agents.rs`. Commands stay in `commands/mod.rs`. `lib.rs` stays thin. |
| N-07 | `Logo`, `StatusBar`, `CommandBar`, and `DbPanel` are not modified. |
| N-08 | `AppShell` is modified only to extend the view model and host the screen navigation. No registry or project data may enter `AppShell`. It stays a thin orchestrator. |
| N-09 | `update_task` must continue to exclude `external_id` and `assigned_agent` from its column list. NEXUS-004's `update_task_preserves_external_id_and_agent` test must pass unchanged. See 2.5. |
| N-10 | Forms stay presentational: they receive `onSubmit` and never call a command. |
| N-11 | The `#[cfg(test)]` pattern extends to both new modules. All 15 existing tests must continue to pass. |
| N-12 | Local-first. Registering an executable path records a string. Nothing is launched, spawned, validated against the filesystem, or executed. |

### 2.3 Design Principle: the third screen, and unfreezing `AppShell`

NEXUS-004 kept `NexusScreen` at two values and changed `AppShell` by zero lines, and treated that as evidence the navigation model was right. NEXUS-005 extends it to three:

```typescript
type NexusScreen = 'projects' | 'project-detail' | 'registry';
```

This is not a reversal. The NEXUS-004 argument was that *tasks* did not deserve a screen, because task context is project context. The registry is genuinely different: it is workspace-level configuration that exists independently of any project, and it is where the user goes when no project is open. A modal would be worse, and nesting it under a project would be incoherent.

`AppShell` gains exactly two things:

1. A navigation control in the header with two targets, Projects and Registry, indicating which is active.
2. `'registry'` handling in the existing `navigate` function, which clears `activeProjectName` the same way `'projects'` already does.

`AppShell` gains no data fetching, no registry state, and no CRUD. The test of whether this stayed honest: `AppShell` must not import anything from `src/lib/nexus-db.ts`.

### 2.4 Design Principle: one panel, two instances

`ides` and `ai_agents` are isomorphic (1.2). Two options were considered:

**Concrete duplication** (`IdeList` + `IdeForm`, `AgentList` + `AgentForm`) matches how projects and tasks are built and needs no new concept, but duplicates roughly 250 lines of near-identical list, form, empty-state, and delete-confirmation logic. Every future fix has to be made twice, and the second copy is the one that gets missed.

**One configured panel** (`RegistryPanel`, instantiated twice from a descriptor) writes the logic once.

**NEXUS-005 uses the configured panel.** The two entities are not merely similar, they are the same shape, and the descriptor is small and explicit:

```typescript
interface RegistryKind {
  key: 'ide' | 'agent';
  title: string;          // "IDEs" / "AI Agents"
  singular: string;       // "IDE" / "agent"
  typeLabel: string;      // "IDE Type" / "Agent Type"
  typePlaceholder: string;
  list: () => Promise<RegistryEntry[]>;
  create: (input: CreateRegistryEntryInput) => Promise<RegistryEntry>;
  update: (input: UpdateRegistryEntryInput) => Promise<RegistryEntry>;
  remove: (id: number) => Promise<void>;
}
```

The command functions are injected, so `RegistryPanel` still never chooses which command to call; it calls what it was handed. The Rust layer stays concrete: `db/ides.rs` and `db/agents.rs` are separate modules over separate tables, mirroring `projects.rs` and `tasks.rs`. Only the presentation is shared.

This is the one new concept NEXUS-005 introduces. If it turns out awkward in practice, the fallback is mechanical: instantiate the descriptor inline twice and let the panels diverge.

### 2.5 Design Principle: how assignment reaches the database

Assignment touches three columns across two existing tables, and the right command shape differs per table because the UI shape differs.

**Projects: extend the existing inputs.** `default_ide_id` and `default_agent_id` are edited inside `ProjectForm`, which already submits the whole project. `CreateProjectInput` and `UpdateProjectInput` gain both fields, and `insert_project` and `update_project` gain both columns. No new command. The UI now owns these columns completely, so there is no invisible-data risk.

**Tasks: a narrow command.** `tasks.assigned_agent` is **not** added to `UpdateTaskInput`. NEXUS-004 deliberately excluded it, alongside `external_id`, so that a struct-shaped UPDATE could never silently null out columns the UI does not render, and it locked that in with `update_task_preserves_external_id_and_agent`. Adding `assigned_agent` to `update_task` would force that test to be weakened.

Instead NEXUS-005 adds `nexus_assign_task_agent`, following the `nexus_update_task_status` precedent: a narrow command for an inline control. Consequences, all good:

- NEXUS-004's exclusion rule survives intact and its test passes unchanged.
- `external_id` stays excluded from every write path, which is still correct since no milestone produces it yet.
- The command shape matches the UI shape: task agent assignment is a dropdown on the card, not a form field.

### 2.6 Design Principle: type fields stay free text

`ide_type` and `agent_type` are `TEXT NOT NULL` with no constraint. Unlike `tasks.status`, NEXUS-005 does **not** define a closed vocabulary for them.

`status` is a fixed semantic set of four values that the application reasons about. A type label is user-supplied metadata describing something the user has installed. A closed enum would reject whatever editor or agent is not on a list written today, and the list would need a migration or a code change every time the user's toolchain changed.

Validation is therefore non-emptiness after trim, the same rule as `projects.name`. The frontend offers no fixed options and no dropdown for these fields; they are plain text inputs.

### 2.7 Design Principle: `enabled` means "offer this"

`enabled INTEGER NOT NULL DEFAULT 1` is the schema's only boolean. Without launching, "enabled" could easily be a flag that does nothing, which would be worse than not surfacing it.

NEXUS-005 gives it exactly one meaning: **an entry that is disabled is not offered in assignment controls** (F-15). It remains in the registry, remains editable, and remains rendered by name anywhere it is already assigned. That makes disabling a real, reversible action with a visible effect, and it is the semantics launching will want later anyway.

Stored as `INTEGER`, exposed to Rust and TypeScript as `bool` / `boolean`. rusqlite maps SQLite INTEGER to `bool` directly; no manual conversion.

---

## 3. Architecture

### 3.1 Component Tree

```
App
└── AppShell                                        (MODIFIED: 3 screens + header nav)
    ├── header
    │   ├── Logo
    │   ├── ScreenNav [Projects | Registry]         (new, inline in AppShell)
    │   ├── active project badge (conditional)
    │   └── StatusBar
    ├── Dashboard(view, navigate)                   (MODIFIED: one new branch)
    │   ├── [view = 'projects']       → ProjectList              (unchanged)
    │   ├── [view = 'project-detail'] → ProjectDetail            (MODIFIED, Part B)
    │   │     ├── project fields + default IDE / default agent
    │   │     ├── [edit] ProjectForm                             (MODIFIED, Part B)
    │   │     └── TaskList                                       (MODIFIED, Part B)
    │   │           └── TaskCard + agent selector                (MODIFIED, Part B)
    │   └── [view = 'registry']       → RegistryScreen           (new)
    │         ├── RegistryPanel(kind=ide)                        (new)
    │         │     ├── RegistryCard x n                         (new)
    │         │     └── RegistryForm (create / edit)             (new)
    │         └── RegistryPanel(kind=agent)                      (new, same components)
    └── CommandBar                                   (unchanged)
```

### 3.2 Ownership

| Concern | Owner |
|---------|-------|
| Which screen is showing | `AppShell` |
| Registry data and registry CRUD calls | `RegistryPanel` (one instance per kind) |
| Which registry entries exist, for assignment controls | Each consumer fetches its own enabled list |
| Project data and project CRUD | `ProjectDetail` / `ProjectList` (unchanged owners) |
| Task data and task CRUD, including agent assignment | `TaskList` |
| Form field state and validation | `RegistryForm`, `ProjectForm`, `TaskForm` |

`RegistryScreen` is a layout shell: it renders two `RegistryPanel`s and owns no data.

**On assignment controls fetching their own lists:** `ProjectForm` and `TaskList` each call `listEnabledIdes()` / `listEnabledAgents()` rather than receiving them through props from a common ancestor. Threading them down would push registry data into `AppShell`, which N-08 forbids. The duplicate fetch is two cheap local queries against a table with a handful of rows.

### 3.3 Rust Module Structure

```
src-tauri/src/
├── main.rs             (unchanged)
├── lib.rs              (add nine commands to invoke_handler, 20 total)
├── db/
│   ├── mod.rs          (add two `pub mod` lines)
│   ├── migrations.rs   (UNCHANGED, see 4.4)
│   ├── projects.rs     (MODIFIED, Part B: two columns on insert and update)
│   ├── tasks.rs        (MODIFIED, Part B: assign_task_agent only)
│   ├── ides.rs         (NEW)
│   └── agents.rs       (NEW)
└── commands/
    └── mod.rs          (add nine commands)
```

---

## 4. Database Schema Assessment

### 4.1 Sufficiency

Both tables have every column NEXUS-005 needs. `projects` and `tasks` already carry the three reference columns. No schema change.

### 4.2 The SET NULL invariant

This is the load-bearing behavior of the milestone, and it is the opposite of NEXUS-004's cascade.

When an IDE is deleted, SQLite sets `projects.default_ide_id` to NULL for every project that referenced it. The project row survives with its name, description, tasks, and timestamps intact. When an agent is deleted, both `projects.default_agent_id` and `tasks.assigned_agent` are nulled across every referring row, and every project and task survives.

`PRAGMA foreign_keys = ON` is set in `db::init()` on every connection, so this fires reliably.

**The application must not emulate this.** Deleting a registry entry issues a single `DELETE FROM ides` or `DELETE FROM ai_agents`. Manually nulling the referring columns first would work, would move a database invariant into application code, and would hide a regression if the foreign key were ever changed. Section 5.7 makes this a test, not a comment.

### 4.3 `updated_at`

Explicit in every UPDATE statement, no triggers, matching `update_project` and `update_task`. This applies to the registry updates, to the enable/disable toggle, and to `assign_task_agent`.

### 4.4 Migration 002: still not required

No new tables, columns, indexes, or constraints. `MIGRATIONS` stays at one entry and the live database stays at level 1, for the third milestone running.

An index on `projects(default_ide_id)` and `projects(default_agent_id)` was considered for the usage counts in F-07 and rejected: the registry holds a handful of rows and projects are counted in application code from a list already fetched.

---

## 5. Rust / Tauri Command Design

### 5.1 New commands

Nine added, bringing the total to twenty. All eleven existing commands are retained unchanged, except `nexus_create_project` and `nexus_update_project`, whose **input structs gain two optional fields** while their names and signatures stay the same (5.5).

**Part A**

| Command | Input | Output |
|---------|-------|--------|
| `nexus_create_ide` | `CreateRegistryEntryInput` | `RegistryEntry` |
| `nexus_list_ides` | `enabledOnly: bool` | `Vec<RegistryEntry>` |
| `nexus_update_ide` | `UpdateRegistryEntryInput` | `RegistryEntry` |
| `nexus_delete_ide` | `id: i64` | `()` |
| `nexus_create_agent` | `CreateRegistryEntryInput` | `RegistryEntry` |
| `nexus_list_agents` | `enabledOnly: bool` | `Vec<RegistryEntry>` |
| `nexus_update_agent` | `UpdateRegistryEntryInput` | `RegistryEntry` |
| `nexus_delete_agent` | `id: i64` | `()` |

**Part B**

| Command | Input | Output |
|---------|-------|--------|
| `nexus_assign_task_agent` | `AssignTaskAgentInput` | `Task` |

**On `enabledOnly` as a parameter rather than two commands:** the registry screen needs every entry, assignment controls need only enabled ones (F-15). A boolean filter is one command; separate `nexus_list_enabled_ides` commands would be four more registrations for one WHERE clause. The enable/disable toggle itself is `nexus_update_ide` / `nexus_update_agent` with a different `enabled` value, not its own command, because unlike task status it is not a one-click control on a dense list row.

### 5.2 Shared structs

Both modules use the same three shapes. They are declared once in `db/ides.rs` and re-exported by `db/agents.rs`, so the serde contract cannot drift between the two tables.

```rust
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryEntry {
    pub id: i64,
    pub name: String,
    /// ides.ide_type or ai_agents.agent_type, normalised to one field name.
    pub entry_type: String,
    pub executable_path: Option<String>,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRegistryEntryInput {
    pub name: String,
    pub entry_type: String,
    pub executable_path: Option<String>,
    /// None means the schema default, enabled.
    pub enabled: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRegistryEntryInput {
    pub id: i64,
    pub name: String,
    pub entry_type: String,
    pub executable_path: Option<String>,
    pub enabled: bool,
}
```

`entry_type` maps to a differently named column per table. The column name is a `&'static str` constant in each module and is interpolated into the SQL, never taken from input, so there is no injection surface.

### 5.3 Validation

```rust
fn validate_entry(name: &str, entry_type: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("Name cannot be empty".to_string());
    }
    if entry_type.trim().is_empty() {
        return Err("Type cannot be empty".to_string());
    }
    Ok(())
}
```

Non-emptiness only, per 2.6. Both fields are stored trimmed.

### 5.4 Delete

```rust
pub fn delete_ide(conn: &Connection, id: i64) -> Result<(), String> {
    let affected = conn
        .execute("DELETE FROM ides WHERE id = ?1", rusqlite::params![id])
        .map_err(|e| format!("Failed to delete IDE {id}: {e}"))?;

    if affected == 0 {
        return Err(format!("IDE {id} not found"));
    }

    Ok(())
}
```

One statement. SQLite nulls the referring columns. See 4.2.

### 5.5 Changes to `projects.rs` (Part B)

`CreateProjectInput` and `UpdateProjectInput` each gain:

```rust
    pub default_ide_id: Option<i64>,
    pub default_agent_id: Option<i64>,
```

`insert_project` and `update_project` add both columns to their statements. `update_project`'s column list becomes name, description, repository_path, repository_url, default_ide_id, default_agent_id, updated_at.

Assigning a non-existent id is rejected by the foreign key, not by a pre-check, consistent with `insert_task`.

NEXUS-003's `update_project_changes_fields_and_updated_at` must be extended to cover the two new columns rather than left asserting a narrower row.

### 5.6 Changes to `tasks.rs` (Part B)

One function added. `update_task` and `update_task_status` are **not** touched (N-09, 2.5).

```rust
pub fn assign_task_agent(
    conn: &Connection,
    input: &AssignTaskAgentInput,
) -> Result<Task, String> {
    let result = conn.execute(
        "UPDATE tasks
            SET assigned_agent = ?1,
                updated_at     = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
          WHERE id = ?2",
        rusqlite::params![input.agent_id, input.id],
    );

    finish_task_update(conn, input.id, result)
}
```

`agent_id: Option<i64>`; `None` clears the assignment (F-13). Reuses NEXUS-004's `finish_task_update` helper.

### 5.7 Tests

**`db/ides.rs` and `db/agents.rs`**, the same set in each, using the established in-memory pattern:

| Test | Asserts |
|------|---------|
| `insert_defaults_to_enabled` | Omitting `enabled` yields `true`; name and type are trimmed. |
| `insert_rejects_empty_name` | Whitespace-only name rejected, no row written. |
| `insert_rejects_empty_type` | Whitespace-only type rejected, no row written. |
| `list_returns_all_or_enabled_only` | `enabled_only = false` returns both; `true` filters disabled out. |
| `update_changes_fields_and_updated_at` | Fields change, `created_at` stable, `updated_at` advances. |
| `update_toggles_enabled` | Round-trips true to false to true. |
| `update_rejects_unknown_id` | Typed "not found" error. |
| `delete_removes_only_that_entry` | Siblings survive; second delete errors. |

**Cross-table tests**, the load-bearing set for 4.2, in `db/ides.rs` and `db/agents.rs` respectively:

| Test | Asserts |
|------|---------|
| `deleting_ide_nulls_project_default_and_keeps_project` | Project survives with name intact; `default_ide_id` is NULL; other projects' defaults untouched. |
| `deleting_agent_nulls_project_and_task_references` | Both `projects.default_agent_id` and `tasks.assigned_agent` go NULL across every referring row; no project or task is deleted; counts before and after are equal. |
| `deleting_agent_leaves_other_agents_assigned` | Only references to the deleted agent are cleared. |

**`db/tasks.rs` additions:**

| Test | Asserts |
|------|---------|
| `assign_task_agent_sets_and_clears` | Set then clear with `None`; `updated_at` advances; title, description, status, and `external_id` all unchanged. |
| `assign_task_agent_rejects_unknown_agent` | Foreign key rejects a dangling `agent_id`. |

**`db/projects.rs` additions:**

| Test | Asserts |
|------|---------|
| `insert_and_update_project_carry_defaults` | Both columns round-trip on create and update, including being set back to NULL. |
| `update_project_rejects_unknown_ide` | Foreign key rejects a dangling `default_ide_id`. |

All 15 existing tests must still pass, `update_task_preserves_external_id_and_agent` unmodified.

---

## 6. TypeScript Types

### 6.1 `src/types/db.ts`

```typescript
export interface RegistryEntry {
  id: number;
  name: string;
  entryType: string;
  executablePath: string | null;
  enabled: boolean;
  createdAt: string;
  updatedAt: string;
}

export interface CreateRegistryEntryInput {
  name: string;
  entryType: string;
  executablePath?: string;
  enabled?: boolean;
}

export interface UpdateRegistryEntryInput {
  id: number;
  name: string;
  entryType: string;
  executablePath?: string;
  enabled: boolean;
}

export interface AssignTaskAgentInput {
  id: number;
  agentId: number | null;
}
```

`Project`, `CreateProjectInput`, and `UpdateProjectInput` gain `defaultIdeId` and `defaultAgentId` (`Project` already has them as read-only fields; the two inputs gain them as optional).

### 6.2 `src/types/index.ts`

```typescript
export type NexusScreen = 'projects' | 'project-detail' | 'registry';

export interface RegistryFormValues {
  name: string;
  entryType: string;
  executablePath: string;
  enabled: boolean;
}

export type RegistryFormMode = 'create' | 'edit';
```

`NexusView` keeps its shape; `projectId` remains optional and unset on `'registry'`.

### 6.3 `src/lib/nexus-db.ts`

Nine wrappers, following the established shapes: `createIde`, `listIdes(enabledOnly)`, `updateIde`, `deleteIde`, `createAgent`, `listAgents(enabledOnly)`, `updateAgent`, `deleteAgent`, `assignTaskAgent`.

---

## 7. React Component Design

### 7.1 `AppShell` changes

```typescript
const [view, setView] = useState<NexusView>({ screen: 'projects' });

const navigate = useCallback((next: NexusView) => {
  if (next.screen !== 'project-detail') setActiveProjectName(null);
  setView(next);
}, []);
```

The header gains a two-button nav between `Logo` and the project badge, each button `aria-current="page"` when active. Clicking Projects calls `navigate({ screen: 'projects' })`; Registry calls `navigate({ screen: 'registry' })`.

`AppShell` must not import from `src/lib/nexus-db.ts` (N-08).

### 7.2 `Dashboard` changes

One new branch rendering `<RegistryScreen />`. `RegistryScreen` takes no props: it needs neither `view` nor `navigate`, since the header owns screen switching.

### 7.3 `RegistryScreen`

**File:** `src/components/RegistryScreen/RegistryScreen.tsx`

Layout only. Renders a heading and two `RegistryPanel`s, one per descriptor. Owns no state.

### 7.4 `RegistryPanel`

**File:** `src/components/RegistryPanel/RegistryPanel.tsx`

```typescript
interface RegistryPanelProps {
  kind: RegistryKind;   // see 2.4
}
```

State: `entries`, `loading`, `error`, `showCreateForm`, `submitting`, `editingId`, `confirmDeleteId`, `busyId`, `projectUsage: Map<number, number>`.

Responsibilities: fetch via `kind.list()` on mount; render header with a count; toggle an inline create form; render one `RegistryCard` per entry; own every command call through the injected `kind` functions; enforce single-open edit and single-open delete confirmation, exactly as `TaskList` does.

For F-07 it also calls `listProjects()` and counts how many projects reference each entry as a default. This needs no new command. **Task usage is not counted**: there is no command that returns tasks across all projects, and adding one is out of scope here, so the confirmation wording covers tasks generically ("any task assigned to this agent will be unassigned").

### 7.5 `RegistryCard`

**File:** `src/components/RegistryCard/RegistryCard.tsx`

Presentational, following `TaskCard`: root is a `div`, not a button, because it contains buttons. Displays name, a type chip, executable path when set (dimmed, monospace, truncating from the left so the binary name stays visible), an enabled/disabled indicator, and created date. Actions are an enable/disable toggle, Edit, and Delete, with the inline confirmation row and a `children` slot for the edit form.

Disabled entries render at reduced emphasis but stay fully legible and fully interactive.

### 7.6 `RegistryForm`

**File:** `src/components/RegistryForm/RegistryForm.tsx`

Presentational, shared by create and edit, mirroring `ProjectForm` and `TaskForm`. Fields: name (required), type (required, plain text per 2.6), executable path (optional), enabled (a toggle). Submit reads "Register IDE" / "Register Agent" or "Save Changes", labels supplied by the descriptor. Never calls a command.

### 7.7 Assignment controls (Part B)

A new `.nexus-select` style is required in `globals.css`. `globals.css` resets `button` and `input` but not `select`, so a native select would arrive unstyled against the black and red theme. Unlike the task status control, a button group is not viable here: the number of registry entries is unbounded, and one of the options is "none".

- **`ProjectForm`** gains two selects, Default IDE and Default Agent, each with a "Not set" option mapping to `null`. It fetches enabled entries itself (3.2) and stays presentational with respect to *projects*: it still calls `onSubmit(values)` and never issues a project command.
- **`ProjectDetail`** view mode gains two `Field` rows resolving ids to names, falling back to "Not set", and to "Unknown (id N)" if an id somehow does not resolve.
- **`TaskCard`** gains an agent select in the row, disabled while `busy`, plus the assigned agent's name in the body when set. **`TaskList`** owns the `assignTaskAgent` call and fetches the enabled agent list once for all its cards.

**F-15 nuance worth stating:** the selects list enabled entries only, but a task or project may already reference a now-disabled entry. Each select therefore also includes the currently-selected entry even when disabled, marked as such, so opening the control never silently drops an existing assignment.

### 7.8 Styling

New CSS files for each new component. One `globals.css` addition: `.nexus-select`, plus a `.nexus-chip` for the type label if `.nexus-status-pill` does not fit unchanged. Existing tokens only, no new tokens, no theme changes.

---

## 8. Implementation Tasks

Rust tasks gate on `cargo check`; test tasks gate on `cargo test --lib`.

### Part A: the registry

| # | Task |
|---|------|
| T-01 | Create `db/ides.rs`: shared structs, `validate_entry`, column constant, row mapper, `get_by_id`. `cargo check`. |
| T-02 | Register `pub mod ides;` in `db/mod.rs`. `cargo check`. |
| T-03 | Implement `insert_ide`, `list_ides(enabled_only)`, `update_ide`, `delete_ide`. `cargo check`. |
| T-04 | Create `db/agents.rs` over `ai_agents`, re-exporting the structs from `ides.rs`. Register the module. `cargo check`. |
| T-05 | Add the `#[cfg(test)]` module to both, including the three cross-table SET NULL tests. `cargo test --lib`: all pass, existing 15 included. |
| T-06 | Add the eight Part A commands to `commands/mod.rs`. `cargo check`. |
| T-07 | Register them in `lib.rs` (19 entries). `cargo check`. |
| T-08 | Add TypeScript types and the eight wrappers. |
| T-09 | Add `.nexus-select` and any chip style to `globals.css`. Append only. |
| T-10 | Build `RegistryForm`. |
| T-11 | Build `RegistryCard`. |
| T-12 | Build `RegistryPanel`, `RegistryScreen`, the `NexusScreen` extension, the `AppShell` header nav, and the `Dashboard` branch. `pnpm build`. |

### Part B: assignment

| # | Task |
|---|------|
| T-13 | Extend `CreateProjectInput` / `UpdateProjectInput` and the two project statements. Extend the NEXUS-003 project test. `cargo test --lib`. |
| T-14 | Add `assign_task_agent` and `AssignTaskAgentInput` to `db/tasks.rs`, leaving `update_task` untouched. `cargo check`. |
| T-15 | Add the four Part B tests (two task, two project). `cargo test --lib`, including `update_task_preserves_external_id_and_agent` unmodified. |
| T-16 | Add `nexus_assign_task_agent`; register it (20 entries). `cargo check`. |
| T-17 | Add the TypeScript types and wrapper; extend the project input types. |
| T-18 | Add the two selects to `ProjectForm` and the two display rows to `ProjectDetail`. |
| T-19 | Add the agent select to `TaskCard` and the `assignTaskAgent` call to `TaskList`. |
| T-20 | `pnpm build`. Zero TypeScript errors. |

### Verification

| # | Task |
|---|------|
| T-21 | `cargo test --lib`: full suite green. |
| T-22 | `pnpm tauri build`: `.app` and ARM64 DMG. |
| T-23 | Manual functional pass, section 9, including restart and `sqlite3` SET NULL confirmation. |

---

## 9. Acceptance Criteria

**Build and structure**

- [ ] `pnpm build` and `pnpm tauri build` both clean.
- [ ] `cargo test --lib` green, including all 15 pre-existing tests.
- [ ] `update_task_preserves_external_id_and_agent` passes **unmodified**.
- [ ] No new dependencies; all four manifests unchanged.
- [ ] `migrations.rs` unchanged; live database still at migration level 1.
- [ ] `Logo`, `StatusBar`, `CommandBar`, `DbPanel` unchanged.
- [ ] `AppShell` does not import from `src/lib/nexus-db.ts`.
- [ ] No component imports `@tauri-apps/api`; no raw SQL under `src/`.
- [ ] `external_id` appears in no write path anywhere in the codebase.
- [ ] Exactly nine new commands; twenty registered.

**Part A**

- [ ] Both panels show an empty state on first visit.
- [ ] An IDE and an agent can each be registered with name and type; empty name or empty type cannot be submitted.
- [ ] Executable path is optional and stored as NULL when blank.
- [ ] New entries default to enabled.
- [ ] Every field can be edited; `updated_at` advances, `created_at` does not.
- [ ] Enable/disable round-trips and persists.
- [ ] Delete requires inline confirmation; cancel preserves, confirm removes.
- [ ] The confirmation names how many projects use the entry as a default.
- [ ] The header nav switches screens and marks the active one.
- [ ] Registry data survives restart.

**Part B**

- [ ] A project can be created and edited with a default IDE and default agent, and with either left unset.
- [ ] `ProjectDetail` shows both by name, or "Not set".
- [ ] A task can be assigned an agent and unassigned again from the task list.
- [ ] `TaskCard` shows the assigned agent's name.
- [ ] Disabled entries do not appear in assignment controls, but an already-assigned disabled entry still renders by name and is not silently dropped when its select is opened.
- [ ] Assignments survive restart.

**The SET NULL invariant**

- [ ] Deleting an IDE used as a project default leaves the project present with its name and tasks intact, and its default IDE now unset. Confirmed in the UI and via `sqlite3`.
- [ ] Deleting an agent used as a project default and as a task assignee leaves every project and task present, with both references cleared. Confirmed via `sqlite3`: project and task row counts are unchanged before and after.
- [ ] Deleting a project with tasks still cascades (the NEXUS-004 invariant, re-checked).

---

## 10. Explicitly Out of Scope

Deferred deliberately:

- **Launching an IDE, spawning a process, opening a repository, executing an agent.** `executable_path` is a recorded string. Nothing in NEXUS-005 reads the filesystem or runs anything.
- **Validating that `executable_path` exists, is executable, or is the thing it claims to be.**
- **Auto-detecting installed IDEs or agents.**
- **A closed vocabulary for `ide_type` / `agent_type`.** See 2.6.
- **Task counts per agent in the delete confirmation.** Needs an all-tasks query that does not exist. See 7.4.
- **Registry search, filtering, sorting, or reordering.**
- **Per-project IDE or agent overrides beyond the single default column each.**
- **Bulk enable/disable.**
- **Agent capability metadata, credentials, API keys, endpoints, or model selection.** No secret ever enters this database.

Also out of scope, per the standing NEXUS constraints:

- Jira, Claude, PlayerZero, Cursor, Grok, and ChatGPT integrations
- AI orchestration or execution of any kind
- Terminal execution, browser automation
- Settings management
- Voice recognition, text to speech
- News, weather, morning briefings, notifications
- Authentication, cloud sync, CI/CD, auto-update
- Custom title bar or window chrome
- Any routing library, state manager, UI library, form library, or ORM
- Any new Rust or frontend dependency
