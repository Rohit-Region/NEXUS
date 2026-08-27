# NEXUS-006: Workspace Overview & Aggregates

## Overview

NEXUS has no home. Every screen is a list of one entity type, and the application opens onto the project list with no sense of the workspace as a whole. Two earlier milestones deferred count features for want of an aggregate query layer that does not exist.

NEXUS-006 adds that layer and the screen that consumes it. It introduces a read-only aggregate module in Rust, four query commands, and an Overview screen that becomes the application's default destination.

Every number on the Overview comes from SQLite through the existing IPC boundary. No value is hardcoded, mocked, estimated, or computed from a stale client-side cache.

### What this milestone discharges

Two deferrals are recorded in earlier specs and are closed here:

- NEXUS-004 section 10: "Task counts on `ProjectCard`. Would need either a sixth query shape returning counts per project or an N+1 fetch from the list screen. Revisit when a project dashboard exists." NEXUS-006 is that dashboard.
- NEXUS-005 section 10 and 7.4: "Task counts per agent in the delete confirmation. Needs an all-tasks query that does not exist."

### Locked decisions carried into this milestone

- The `CommandBar` is not touched. It stays exactly as NEXUS-001 left it. Its placeholder is a future local command-palette capability, planned for NEXUS-009, not AI orchestration.
- No global cross-entity search. Deferred to NEXUS-009.
- No new frontend testing dependency. The no-new-dependencies rule remains in force.

### Dependency on outstanding verification

NEXUS-004 and NEXUS-005 are implemented, tested at the database layer, and building, but their manual UI verification is still outstanding. NEXUS-006 modifies `ProjectCard`, `ProjectList`, `RegistryPanel`, and `RegistryCard`, all of which are part of that unverified surface. Section 9 carries the outstanding scenarios forward explicitly. If the NEXUS-004 or NEXUS-005 pass turns up defects in those components, fix them before starting T-14.

---

## 1. Existing State (from NEXUS-001 through NEXUS-005)

### 1.1 Architecture in place

**Rust layer**

- `db/mod.rs`: `DbState`, `init()`, migration runner, `PRAGMA foreign_keys = ON` on every connection
- `db/migrations.rs`: one entry, migration 001; live database at level 1
- `db/projects.rs`: project CRUD, `count_projects`, `count_all_tables`, 7 tests
- `db/tasks.rs`: task CRUD, `TASK_STATUSES`, `validate_status`, `assign_task_agent`, 12 tests
- `db/registry.rs`: `RegistryEntry`, `CreateRegistryEntryInput`, `UpdateRegistryEntryInput`, `validate_entry`, `map_entry_row`
- `db/ides.rs`, `db/agents.rs`: registry CRUD over the two isomorphic tables, 9 and 10 tests
- `commands/mod.rs`: twenty registered commands
- `lib.rs`: thin orchestrator
- 38 tests total, all using in-memory connections seeded from the real `MIGRATIONS`

**Frontend**

- `src/lib/nexus-db.ts`: the only module importing `@tauri-apps/api`; twenty typed wrappers
- `src/types/db.ts`, `src/types/index.ts`
- `App.tsx` renders `AppShell` and nothing else
- `AppShell` owns `view` and `activeProjectName` only, and imports nothing from `nexus-db.ts`
- `NexusScreen` has three values: `'projects' | 'project-detail' | 'registry'`
- `Dashboard` switches between `ProjectList`, `ProjectDetail`, and `RegistryScreen`
- `ProjectList` / `ProjectCard` / `ProjectForm` / `ProjectDetail`
- `TaskList` / `TaskCard` / `TaskForm`, rendered inside `ProjectDetail` in view mode
- `RegistryScreen` / `RegistryPanel` / `RegistryCard` / `RegistryForm` / `RegistrySelect`
- Shared `.nexus-btn`, `.nexus-field`, `.nexus-select`, `.nexus-chip`, `.nexus-status-pill` in `globals.css`
- Shared helpers: `formatStamp` from `ProjectCard`, `StatusPill` / `TASK_STATUS_ORDER` / `formatStatus` / `nextStatus` from `TaskCard`, `RegistrySelect` from `RegistryPanel`
- `DbPanel` on disk, not rendered, tree-shaken out of the production bundle

### 1.2 Current default navigation behaviour

`AppShell` initialises with `useState<NexusView>({ screen: 'projects' })`. The header nav has two targets, Projects and Registry, each carrying `aria-current="page"` when active. Projects is treated as active for both `'projects'` and `'project-detail'`.

`navigate` clears `activeProjectName` for any screen other than `'project-detail'`.

### 1.3 What is missing

- **No aggregate query layer.** The only counting functions are `count_projects` (marked `#[allow(dead_code)]`) and `count_all_tables`, and the only command exposing them, `nexus_get_db_counts`, has no production consumer: it is called solely by `DbPanel`, which is not rendered.
- **No cross-project task query.** `list_tasks(conn, project_id)` is the only task read. Every task read is scoped to one project.
- **No per-project or per-agent counting.** `ProjectCard` shows no task count. `RegistryCard`'s delete confirmation names an exact project count but falls back to the generic phrase "Any task assigned to it will be unassigned" because the count is unavailable.
- **No home screen.** The application opens on a list.

---

## 2. Requirements

### 2.1 Functional Requirements

| ID   | Requirement                                                                                                                                                                                 |
| ---- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| F-01 | The application must provide an Overview screen reachable from the header navigation.                                                                                                       |
| F-02 | The Overview must be the screen the application opens on.                                                                                                                                   |
| F-03 | The Overview must display total counts for projects, tasks, IDEs, and AI agents, read from SQLite.                                                                                          |
| F-04 | The Overview must display a breakdown of tasks by status across all projects, covering every value in `TASK_STATUSES`.                                                                      |
| F-05 | The Overview must display how many IDEs and agents are enabled, distinct from how many exist.                                                                                               |
| F-06 | The Overview must display how many tasks have no assigned agent.                                                                                                                            |
| F-07 | The Overview must display a list of recently updated tasks across all projects, each showing its task title, its status, and the name of the project it belongs to.                         |
| F-08 | Recent tasks must be ordered by `updated_at` descending, with a deterministic tiebreak.                                                                                                     |
| F-09 | Every value displayed on the Overview must be derived from a database query performed in the Rust layer. No value may be hardcoded, mocked, or inferred client-side from a partial dataset. |
| F-10 | `ProjectCard` must display the number of tasks belonging to that project.                                                                                                                   |
| F-11 | A project with zero tasks must display `0`, not a blank, a dash, or an omitted badge.                                                                                                       |
| F-12 | The registry delete confirmation for an AI agent must state the exact number of tasks currently assigned to that agent.                                                                     |
| F-13 | An agent with zero assigned tasks must be described as such explicitly, not by omission.                                                                                                    |
| F-14 | On an empty database the Overview must render a meaningful empty state rather than a grid of zeroes with no explanation, and must prompt the user toward creating a first project.          |
| F-15 | The header navigation must indicate which of the three top-level screens is active.                                                                                                         |
| F-16 | The Overview must show a loading state while its queries are in flight and an error state if any query fails.                                                                               |

### 2.2 Non-Functional Requirements

| ID   | Requirement                                                                                                                                                                                                                              |
| ---- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| N-01 | No new Rust or frontend dependencies.                                                                                                                                                                                                    |
| N-02 | No routing library and no global state manager. Navigation stays view state in `AppShell`.                                                                                                                                               |
| N-03 | All new commands use the `nexus_` prefix and the `Result<T, String>` error pattern.                                                                                                                                                      |
| N-04 | All new TypeScript types go in `src/types/`.                                                                                                                                                                                             |
| N-05 | All new `invoke()` wrappers go in `src/lib/nexus-db.ts`. No component may import `@tauri-apps/api`.                                                                                                                                      |
| N-06 | Aggregate database logic goes in a new `src-tauri/src/db/stats.rs`. Commands stay in `commands/mod.rs`. `lib.rs` stays thin.                                                                                                             |
| N-07 | `db/projects.rs`, `db/tasks.rs`, `db/ides.rs`, `db/agents.rs`, and `db/registry.rs` are not modified. The aggregate module reads across tables and owns no writes.                                                                       |
| N-08 | `AppShell` must not import anything from `src/lib/nexus-db.ts`. It gains one nav target and a changed initial screen constant, nothing more.                                                                                             |
| N-09 | `Logo`, `StatusBar`, `CommandBar`, `DbPanel`, `ProjectForm`, `ProjectDetail`, `TaskList`, `TaskCard`, `TaskForm`, `RegistryForm`, `RegistrySelect`, and `RegistryScreen` are not modified beyond what T-15 requires of `RegistryScreen`. |
| N-10 | `nexus_get_db_status` and `nexus_get_db_counts` are left exactly as they are. NEXUS-006 does not repurpose, extend, or remove them.                                                                                                      |
| N-11 | Status buckets are derived from `TASK_STATUSES` in `db/tasks.rs`. The vocabulary is not re-declared in the aggregate module.                                                                                                             |
| N-12 | The `#[cfg(test)]` pattern extends to the new module. All 38 existing tests must continue to pass unmodified.                                                                                                                            |
| N-13 | Local-first. No remote backend, no telemetry, no analytics.                                                                                                                                                                              |

### 2.3 Design Principle: aggregate in SQL, not in React

Every count could be computed in the frontend by fetching all rows and reducing over them. NEXUS-006 does not do that.

Counting in SQL keeps the database as the single source of truth, keeps the aggregate logic testable in the Rust test suite where the project already has 38 tests and where the frontend has none, and avoids a class of bug where two screens disagree because one of them reduced over a stale array.

It also keeps F-09 enforceable: if the Overview reduces over client arrays, "no hardcoded or mocked values" becomes a matter of inspection rather than a property of the design.

### 2.4 Design Principle: the zero-row rule

The single most likely defect in this milestone is an aggregate that silently omits rows with a zero count.

```sql
-- WRONG: a project with no tasks disappears from the result entirely
SELECT p.id, COUNT(t.id) FROM projects p
  INNER JOIN tasks t ON t.project_id = p.id GROUP BY p.id;

-- RIGHT: every project appears, zero-task projects included
SELECT p.id, COUNT(t.id) FROM projects p
  LEFT JOIN tasks t ON t.project_id = p.id GROUP BY p.id;
```

The wrong version compiles, runs, returns plausible-looking data, and is wrong in a way no type checker can see. It is exactly the class of defect that produced the `ide_type`-against-`ai_agents` bug during NEXUS-005 implementation, where a table name inside a SQL string literal was wrong and `cargo check` reported success.

F-11 and F-13 exist to make this observable in the UI. Two tests, `counts_by_project_includes_zero_task_projects` and `counts_by_agent_includes_zero_task_agents`, exist to make it observable in the suite. Neither is optional.

The same rule applies to the frontend: a project id absent from the counts result must render as `0`, never as blank. The lookup must be `counts.get(id) ?? 0`, not a conditional render.

### 2.5 Design Principle: `TaskWithProject` nests rather than flattens

Recent tasks need the project name alongside the task. Two shapes were considered:

- **Flatten:** a new struct repeating all nine `Task` fields plus `project_name`. Duplicates the NEXUS-004 serde contract, and the copy is the one that drifts.
- **Nest:** `TaskWithProject { task: Task, project_name: String }`.

NEXUS-006 nests. The existing `Task` struct and its TypeScript interface are reused verbatim, so `Task` has exactly one definition on each side of the boundary and the NEXUS-004 contract cannot diverge.

### 2.6 Design Principle: an unknown status counts, but buckets nowhere

`tasks.status` has no `CHECK` constraint by deliberate decision in NEXUS-004 section 4.2. A row hand-edited through `sqlite3` can therefore carry a value outside `TASK_STATUSES`.

`WorkspaceSummary.tasks` counts every task row. The four status buckets count only rows matching a known status. On a database containing an unknown status, the buckets will therefore sum to less than `tasks`.

This asymmetry is deliberate and must be documented on the type itself rather than hidden. The Overview must not present the buckets as if they partition the total, and must not attempt to reconcile the difference with an invented "other" bucket that has no schema basis.

---

## 3. Architecture

### 3.1 Component Tree

```
App                                                  (unchanged)
└── AppShell                       (MODIFIED: 4 screens, 3rd nav target, new default)
    ├── header
    │   ├── Logo                                     (unchanged)
    │   ├── ScreenNav [Overview | Projects | Registry]   (one target added)
    │   ├── active project badge (conditional)       (unchanged)
    │   └── StatusBar                                (unchanged)
    ├── Dashboard(view, navigate)                    (MODIFIED: one new branch)
    │   ├── [view = 'overview']       -> OverviewScreen          (NEW)
    │   │     ├── StatTile x n                                   (NEW)
    │   │     ├── status breakdown row                           (reuses StatusPill)
    │   │     └── recent tasks list                              (NEW, inline)
    │   ├── [view = 'projects']       -> ProjectList             (MODIFIED)
    │   │     └── ProjectCard                                    (MODIFIED: count badge)
    │   ├── [view = 'project-detail'] -> ProjectDetail           (unchanged)
    │   └── [view = 'registry']       -> RegistryScreen          (descriptor field added)
    │         └── RegistryPanel                                  (MODIFIED: agent counts)
    │               └── RegistryCard                             (MODIFIED: exact warning)
    └── CommandBar                                   (unchanged)
```

### 3.2 Ownership

| Concern                            | Owner                                                                 |
| ---------------------------------- | --------------------------------------------------------------------- |
| Which screen is showing            | `AppShell` (unchanged responsibility)                                 |
| Workspace summary and recent tasks | `OverviewScreen`                                                      |
| Per-project task counts            | `ProjectList`, passed to each `ProjectCard` as a prop                 |
| Per-agent task counts              | `RegistryPanel` (agent kind only), passed to `RegistryCard` as a prop |
| Rendering a single statistic       | `StatTile` (presentational)                                           |

`StatTile`, `ProjectCard`, and `RegistryCard` remain presentational. Only `OverviewScreen`, `ProjectList`, and `RegistryPanel` call commands, consistent with the established rule that list and screen components own data while cards and forms do not.

### 3.3 Rust Module Structure

```
src-tauri/src/
├── main.rs             (unchanged)
├── lib.rs              (add four commands to invoke_handler, 24 total)
├── db/
│   ├── mod.rs          (add `pub mod stats;`, one line)
│   ├── migrations.rs   (UNCHANGED, see 4.5)
│   ├── projects.rs     (UNCHANGED)
│   ├── tasks.rs        (UNCHANGED, read by stats.rs for TASK_STATUSES and Task)
│   ├── registry.rs     (UNCHANGED)
│   ├── ides.rs         (UNCHANGED)
│   ├── agents.rs       (UNCHANGED)
│   └── stats.rs        (NEW)
└── commands/
    └── mod.rs          (add four commands)
```

---

## 4. Database Schema Assessment

### 4.1 Sufficiency

Every value NEXUS-006 needs is derivable from migration 001 by query. No new table, column, index, constraint, or foreign key.

### 4.2 Tables read

| Table       | Read for                                                                                        |
| ----------- | ----------------------------------------------------------------------------------------------- |
| `projects`  | Project total; per-project grouping; project name on recent tasks                               |
| `tasks`     | Task total; status buckets; per-project counts; per-agent counts; unassigned count; recent list |
| `ides`      | Total and enabled counts                                                                        |
| `ai_agents` | Total and enabled counts; per-agent grouping                                                    |
| `settings`  | Not read. Remains without a producer until NEXUS-008                                            |

No table is written. NEXUS-006 adds no `INSERT`, `UPDATE`, or `DELETE` statement anywhere.

### 4.3 Foreign keys

Unchanged and unmodified, but relied upon implicitly: `tasks.project_id` is `NOT NULL REFERENCES projects(id) ON DELETE CASCADE`, so every task row joins to exactly one project and the recent-tasks join can never produce a null project name. `tasks.assigned_agent` is nullable `REFERENCES ai_agents(id) ON DELETE SET NULL`, which is why the unassigned count is meaningful and why per-agent counts must exclude nulls.

NEXUS-006 changes no foreign key and adds none.

### 4.4 `updated_at` and ordering

Recent tasks order by `updated_at DESC, id DESC`. The id tiebreak is required: `strftime('%Y-%m-%dT%H:%M:%fZ', 'now')` has millisecond resolution and two tasks updated inside the same millisecond would otherwise return in an unspecified order, making F-08 unverifiable and the list unstable across refreshes.

This matches the ordering already used by `list_tasks`, `list_ides`, and `list_agents`.

### 4.5 Migration 002: still not required

No schema change. `MIGRATIONS` stays at one entry and the live database stays at migration level 1, for the fourth milestone running.

Indexes on `tasks(project_id)` and `tasks(assigned_agent)` were considered for the two `GROUP BY` queries and rejected, for the same reasons given in NEXUS-004 section 4.5 and NEXUS-005 section 4.4: a local single-user command center will not accumulate enough rows for a sequential scan to be measurable, and `idx_tasks_project_external` already leads with `project_id`. Add an index only on profiling evidence, and when that happens it is migration 002 with its own milestone, not an ad-hoc `CREATE INDEX`.

---

## 5. Rust / Tauri Command Design

### 5.1 New commands

Four added, bringing the total to twenty-four. All twenty existing commands are retained without modification.

### 5.2 Structs in `db/stats.rs`

```rust
/// Workspace-wide totals.
///
/// `tasks` counts every task row. The four status buckets count only rows
/// whose status is in TASK_STATUSES, so on a database containing a status
/// written outside the application the buckets sum to less than `tasks`.
/// This is deliberate: see spec 2.6.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSummary {
    pub projects: i64,
    pub tasks: i64,
    pub tasks_open: i64,
    pub tasks_in_progress: i64,
    pub tasks_blocked: i64,
    pub tasks_done: i64,
    pub tasks_unassigned: i64,
    pub ides_total: i64,
    pub ides_enabled: i64,
    pub agents_total: i64,
    pub agents_enabled: i64,
}

/// Per-project task counts. One entry per project, including projects
/// with zero tasks (spec 2.4).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTaskCounts {
    pub project_id: i64,
    pub total: i64,
    pub open: i64,
    pub in_progress: i64,
    pub blocked: i64,
    pub done: i64,
}

/// Per-agent assigned-task counts. One entry per agent, including agents
/// with zero assigned tasks (spec 2.4).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTaskCounts {
    pub agent_id: i64,
    pub task_count: i64,
}

/// A task with the name of the project it belongs to.
/// Nests the NEXUS-004 Task rather than flattening it (spec 2.5).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskWithProject {
    pub task: Task,
    pub project_name: String,
}
```

### 5.3 Query shapes

`workspace_summary` is a single statement returning one row, built from scalar subqueries so it cannot partially fail:

```sql
SELECT
  (SELECT COUNT(*) FROM projects),
  (SELECT COUNT(*) FROM tasks),
  (SELECT COUNT(*) FROM tasks WHERE status = ?1),
  (SELECT COUNT(*) FROM tasks WHERE status = ?2),
  (SELECT COUNT(*) FROM tasks WHERE status = ?3),
  (SELECT COUNT(*) FROM tasks WHERE status = ?4),
  (SELECT COUNT(*) FROM tasks WHERE assigned_agent IS NULL),
  (SELECT COUNT(*) FROM ides),
  (SELECT COUNT(*) FROM ides WHERE enabled = 1),
  (SELECT COUNT(*) FROM ai_agents),
  (SELECT COUNT(*) FROM ai_agents WHERE enabled = 1)
```

The four status values are bound from `TASK_STATUSES` rather than typed into the SQL, satisfying N-11.

`count_tasks_by_project` and `count_tasks_by_agent` both use `LEFT JOIN ... GROUP BY` per 2.4.

`list_recent_tasks` joins `tasks` to `projects` on the `NOT NULL` foreign key, selects the same task columns `db/tasks.rs` selects plus `projects.name`, and orders by `updated_at DESC, id DESC`.

### 5.4 Validation and error handling

- `limit` on `list_recent_tasks` is clamped in Rust to `1..=100`. A caller passing `0`, a negative number, or `9999` receives a clamped result, not an error and not an unbounded query. Clamping rather than rejecting keeps the command total: there is no input for which the Overview must handle a failure it caused itself.
- Every function returns `Result<T, String>` with a message naming the operation, matching the established convention.
- No function panics on an empty database. Every count returns `0`; every list returns an empty vector.
- A status value outside `TASK_STATUSES` is counted in totals and in no bucket, per 2.6. It is never an error.

### 5.5 IPC contracts

```
COMMAND:  nexus_get_workspace_summary
INPUT:    (none)
OUTPUT:   WorkspaceSummary
ERRORS:   "Lock error: {e}"
          "Failed to compute workspace summary: {e}"
REGISTERED: src-tauri/src/lib.rs, generate_handler! (entry 21 of 24)
INVOKED:  src/components/OverviewScreen/OverviewScreen.tsx
          via getWorkspaceSummary() in src/lib/nexus-db.ts

COMMAND:  nexus_count_tasks_by_project
INPUT:    (none)
OUTPUT:   Vec<ProjectTaskCounts>
          One entry per project row, including projects with zero tasks.
          Order is unspecified; consumers key by projectId.
ERRORS:   "Lock error: {e}"
          "Failed to count tasks by project: {e}"
REGISTERED: src-tauri/src/lib.rs, generate_handler! (entry 22 of 24)
INVOKED:  src/components/ProjectList/ProjectList.tsx
          via countTasksByProject() in src/lib/nexus-db.ts

COMMAND:  nexus_count_tasks_by_agent
INPUT:    (none)
OUTPUT:   Vec<AgentTaskCounts>
          One entry per ai_agents row, including agents with zero assigned
          tasks. Tasks with a NULL assigned_agent are counted for no agent.
ERRORS:   "Lock error: {e}"
          "Failed to count tasks by agent: {e}"
REGISTERED: src-tauri/src/lib.rs, generate_handler! (entry 23 of 24)
INVOKED:  src/components/RegistryPanel/RegistryPanel.tsx, agent kind only
          via countTasksByAgent() in src/lib/nexus-db.ts

COMMAND:  nexus_list_recent_tasks
INPUT:    limit: i64          clamped to 1..=100 in Rust
OUTPUT:   Vec<TaskWithProject>
          Ordered by task.updated_at DESC, task.id DESC.
ERRORS:   "Lock error: {e}"
          "Failed to list recent tasks: {e}"
REGISTERED: src-tauri/src/lib.rs, generate_handler! (entry 24 of 24)
INVOKED:  src/components/OverviewScreen/OverviewScreen.tsx
          via listRecentTasks(limit) in src/lib/nexus-db.ts
```

`limit` is passed as a bare argument in camelCase, matching how `deleteProject` passes `id` and `nexus_list_tasks` passes `projectId`.

### 5.6 Command registration

`lib.rs` `invoke_handler` grows from twenty entries to twenty-four. The four new entries are appended after `nexus_assign_task_agent`. No existing entry is reordered or removed.

### 5.7 Tests in `db/stats.rs`

The `#[cfg(test)]` module uses the established pattern: open `:memory:`, `PRAGMA foreign_keys = ON`, apply the real `MIGRATIONS`.

**Summary tests**

| Test                                         | Asserts                                                                                                                                                     |
| -------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `summary_on_empty_database`                  | Every one of the eleven fields is `0`. No panic, no error.                                                                                                  |
| `summary_counts_totals_and_status_buckets`   | With a known fixture, all eleven fields match hand-computed values; the four buckets sum to `tasks`.                                                        |
| `summary_counts_enabled_registry_separately` | With one enabled and one disabled entry per registry table, `ides_enabled` is 1 and `ides_total` is 2; likewise for agents.                                 |
| `summary_counts_unassigned_tasks`            | Tasks with `assigned_agent IS NULL` are counted in `tasks_unassigned`; assigned tasks are not.                                                              |
| `summary_tolerates_unknown_status`           | A task inserted with `status = 'archived'` counts in `tasks`, in none of the four buckets, and the buckets consequently sum to less than `tasks`. No error. |

**Per-project count tests**

| Test                                            | Asserts                                                                                                                                                  |
| ----------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `counts_by_project_includes_zero_task_projects` | The zero-row rule (2.4). A project with no tasks appears in the result with `total: 0` and all four buckets `0`. Result length equals the project count. |
| `counts_by_project_are_scoped`                  | Project A's counts exclude project B's tasks; per-status buckets are scoped too.                                                                         |
| `counts_by_project_on_empty_database`           | Empty vector, no error.                                                                                                                                  |

**Per-agent count tests**

| Test                                        | Asserts                                                                                                                                                                |
| ------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `counts_by_agent_includes_zero_task_agents` | The zero-row rule. An agent with no assigned tasks appears with `task_count: 0`. Result length equals the agent count.                                                 |
| `counts_by_agent_excludes_unassigned_tasks` | Tasks with `assigned_agent IS NULL` are counted for no agent; the sum of `task_count` equals the number of assigned tasks.                                             |
| `counts_by_agent_after_agent_delete`        | After deleting an agent that held tasks, those tasks are unassigned by `ON DELETE SET NULL`, appear in no agent's count, and no orphan agent id appears in the result. |

**Recent-task tests**

| Test                                          | Asserts                                                                                             |
| --------------------------------------------- | --------------------------------------------------------------------------------------------------- |
| `recent_tasks_are_ordered_by_updated_at_desc` | Updating an older task moves it to the front.                                                       |
| `recent_tasks_tiebreak_is_deterministic`      | Two tasks sharing an `updated_at` return in `id DESC` order, and the same order on a repeated call. |
| `recent_tasks_respect_limit`                  | A limit of 2 over 5 tasks returns exactly 2, and they are the 2 most recent.                        |
| `recent_tasks_clamp_limit`                    | `0`, `-5`, and `9999` all return a clamped, non-erroring result.                                    |
| `recent_tasks_carry_correct_project_name`     | Tasks from two projects each report their own project's name.                                       |
| `recent_tasks_on_empty_database`              | Empty vector, no error.                                                                             |

**Regression gate:** all 38 pre-existing tests must pass unmodified, `update_task_preserves_external_id_and_agent` included.

---

## 6. TypeScript Types

### 6.1 Additions to `src/types/db.ts`

```typescript
export interface WorkspaceSummary {
  projects: number;
  tasks: number;
  tasksOpen: number;
  tasksInProgress: number;
  tasksBlocked: number;
  tasksDone: number;
  tasksUnassigned: number;
  idesTotal: number;
  idesEnabled: number;
  agentsTotal: number;
  agentsEnabled: number;
}

export interface ProjectTaskCounts {
  projectId: number;
  total: number;
  open: number;
  inProgress: number;
  blocked: number;
  done: number;
}

export interface AgentTaskCounts {
  agentId: number;
  taskCount: number;
}

export interface TaskWithProject {
  task: Task;
  projectName: string;
}
```

`TaskWithProject.task` reuses the existing `Task` interface unchanged.

### 6.2 Additions to `src/types/index.ts`

```typescript
export type NexusScreen =
  | 'overview'
  | 'projects'
  | 'project-detail'
  | 'registry';
```

`NexusView` keeps its shape. `projectId` remains optional and is unset on `'overview'`.

### 6.3 Additions to `src/lib/nexus-db.ts`

```typescript
export function getWorkspaceSummary(): Promise<WorkspaceSummary> {
  return invoke<WorkspaceSummary>('nexus_get_workspace_summary');
}

export function countTasksByProject(): Promise<ProjectTaskCounts[]> {
  return invoke<ProjectTaskCounts[]>('nexus_count_tasks_by_project');
}

export function countTasksByAgent(): Promise<AgentTaskCounts[]> {
  return invoke<AgentTaskCounts[]>('nexus_count_tasks_by_agent');
}

export function listRecentTasks(limit: number): Promise<TaskWithProject[]> {
  return invoke<TaskWithProject[]>('nexus_list_recent_tasks', { limit });
}
```

---

## 7. React Component Design

### 7.1 `AppShell` changes

Two changes only.

1. The initial view constant becomes `{ screen: 'overview' }`.
2. The header nav gains a third button, Overview, placed first. `aria-current="page"` logic becomes: Overview active when `view.screen === 'overview'`; Projects active when `view.screen` is `'projects'` or `'project-detail'`; Registry active when `view.screen === 'registry'`.

`navigate` is unchanged: it already clears `activeProjectName` for any screen other than `'project-detail'`, which covers `'overview'` without modification.

`AppShell` gains no data fetching and no import from `src/lib/nexus-db.ts` (N-08). NEXUS-008 will make the launch screen a persisted preference; until then it is a constant.

### 7.2 `Dashboard` changes

One new branch:

```tsx
{view.screen === 'overview' && <OverviewScreen navigate={navigate} />}
```

`OverviewScreen` receives `navigate` so its empty state can send the user to the projects screen and its recent-task entries can open the owning project. It does not receive `view`.

### 7.3 `OverviewScreen`

**File:** `src/components/OverviewScreen/OverviewScreen.tsx`

```typescript
interface OverviewScreenProps {
  navigate: (view: NexusView) => void;
}
```

State: `summary: WorkspaceSummary | null`, `recent: TaskWithProject[]`, `loading: boolean`, `error: string | null`.

Fetches both commands in one `Promise.all` inside a `useCallback`, invoked from a `useEffect` on mount. `RECENT_TASK_LIMIT` is a module constant of `8`.

Layout, top to bottom:

1. Heading and a one-line subtitle
2. A tile grid: Projects, Tasks, IDEs, AI Agents. Each tile shows a total; the IDE and agent tiles show `enabled` as a secondary line
3. A task status row: one `StatusPill` per entry in `TASK_STATUS_ORDER` with its count, plus an Unassigned count. Statuses with a count of zero are shown, not hidden, because a workspace with no blocked tasks is information
4. Recent activity: up to eight `TaskWithProject` rows, each showing status pill, title, project name, and an absolute timestamp via the existing `formatStamp`. Clicking a row calls `navigate({ screen: 'project-detail', projectId: entry.task.projectId })`

**Empty state (F-14).** When `summary.projects === 0` and `summary.tasks === 0`, the tile grid and recent list are replaced by a single empty panel: an icon, "Workspace is empty", one sentence of explanation, and a button that calls `navigate({ screen: 'projects' })`. The registry tiles are still shown if either registry table is non-empty, since a user may register tools before creating a project.

**Loading state.** While `loading` is true and `summary` is null, a single line of text in the established style. Tiles are not rendered with placeholder zeroes, because a zero that later becomes a five is indistinguishable from a real zero and would violate the spirit of F-09.

**Error state.** If either command rejects, an error paragraph with `role="alert"` in `--color-accent`, and no tiles. The Overview does not render partial data from a failed fetch.

### 7.4 `StatTile`

**File:** `src/components/StatTile/StatTile.tsx`

```typescript
interface StatTileProps {
  label: string;
  value: number;
  detail?: string;
  accent?: boolean;
}
```

Presentational. Renders a label, a large monospace value, and an optional dimmed detail line. No state, no command calls, no conditional data logic.

### 7.5 `ProjectCard` changes

`ProjectCard` gains one required prop:

```typescript
  taskCount: number;
```

Rendered as a chip in the existing `project-card__aside`, before the date, using `.nexus-chip`. The label reads `{n} tasks`, or `1 task` for exactly one, or `0 tasks` for zero.

`taskCount` is required rather than optional. An optional prop defaulting to zero would make a missing count indistinguishable from a real zero, which is precisely the confusion F-11 exists to prevent.

`ProjectCard` remains presentational and remains a `<button>` root, unchanged in that respect.

### 7.6 `ProjectList` changes

`refresh` fetches `listProjects()` and `countTasksByProject()` in the existing `Promise.all`, and builds a `Map<number, number>` from `projectId` to `total`.

Each `ProjectCard` receives `taskCount={counts.get(project.id) ?? 0}`. The `?? 0` is required by 2.4: a project absent from the counts result renders as zero, never as blank.

Nothing else in `ProjectList` changes. Its create flow, error handling, and empty state are untouched.

### 7.7 `RegistryPanel`, `RegistryScreen`, and `RegistryCard` changes

`RegistryKind` gains one field:

```typescript
  /** Whether this kind can hold task assignments. Agents can; IDEs cannot. */
  countsTasks: boolean;
```

`RegistryScreen` sets it `false` on the IDE descriptor and `true` on the agent descriptor. This is `RegistryScreen`'s only change.

When `countsTasks` is true, `RegistryPanel.refresh` also calls `countTasksByAgent()` and builds a `Map<number, number>`. When false, the map is empty and no call is made. This keeps the IDE panel from issuing a query that has no meaning for it, without branching on `kind.key` inside the panel body.

`RegistryCard` gains one prop:

```typescript
  taskUsage: number | null;   // null when the kind does not hold task assignments
```

`usageWarning` is extended. Its current text is:

> Delete this {kind}? {N projects use it as a default and will be cleared}. Any task assigned to it will be unassigned. Nothing else is deleted.

The generic task clause is replaced when `taskUsage` is a number:

- `0`: "No task is assigned to it."
- `1`: "1 task is assigned to it and will be unassigned."
- `n`: "{n} tasks are assigned to it and will be unassigned."

When `taskUsage` is null, the task clause is omitted entirely rather than kept generic, because an IDE cannot hold task assignments and the sentence would be false.

The project clause is unchanged. The closing "Nothing else is deleted." is unchanged and remains accurate: `ON DELETE SET NULL` blanks references without removing rows.

### 7.8 Styling

New CSS files: `OverviewScreen.css`, `StatTile.css`. Any shared tile styling goes into `globals.css` append-only as `.nexus-stat-tile`; if `StatTile.css` suffices, `globals.css` is not modified at all.

Existing tokens only. No new tokens, no theme change. Reuses `.nexus-btn`, `.nexus-chip`, `.nexus-status-pill`, and `formatStamp`.

---

## 8. Implementation Tasks

Rust tasks gate on `cargo check`. Test tasks gate on `cargo test --lib`. Frontend tasks gate on `npx tsc --noEmit`.

---

### T-01: Create `db/stats.rs` with types and helpers

**Objective.** Establish the module with its four structs and private helpers, before any query logic.

**Files.** `src-tauri/src/db/stats.rs` (new).

**Dependencies.** None.

**Implementation details.** Declare `WorkspaceSummary`, `ProjectTaskCounts`, `AgentTaskCounts`, and `TaskWithProject` with `#[serde(rename_all = "camelCase")]`. Import `Task` and `TASK_STATUSES` from `super::tasks`. Add the doc comment on `WorkspaceSummary` recording the 2.6 bucket asymmetry. Add a private `clamp_limit(limit: i64) -> i64` returning `limit.clamp(1, 100)`.

**Acceptance criteria.** File compiles once registered. No struct re-declares `Task`'s fields. No status literal is typed as a string in this file outside a test.

**Tests.** None yet.

---

### T-02: Register the module

**Objective.** Make `stats.rs` part of the crate.

**Files.** `src-tauri/src/db/mod.rs`.

**Dependencies.** T-01.

**Implementation details.** Add `pub mod stats;` in alphabetical position among the existing `pub mod` lines.

**Acceptance criteria.** `cargo check` exits 0. Warnings for unused items are expected until T-07.

**Tests.** None.

---

### T-03: Implement `workspace_summary()`

**Objective.** One statement returning all eleven totals.

**Files.** `src-tauri/src/db/stats.rs`.

**Dependencies.** T-02.

**Implementation details.** Build the scalar-subquery statement of 5.3. Bind the four status values from `TASK_STATUSES` rather than typing literals. Return `Err(format!("Failed to compute workspace summary: {e}"))` on query failure.

**Acceptance criteria.** On an empty database returns eleven zeroes. On a seeded database every field matches a hand-computed value. `cargo check` exits 0.

**Tests.** `summary_on_empty_database`, `summary_counts_totals_and_status_buckets`, `summary_counts_enabled_registry_separately`, `summary_counts_unassigned_tasks`, `summary_tolerates_unknown_status`.

---

### T-04: Implement `count_tasks_by_project()`

**Objective.** Per-project totals and status buckets, including zero-task projects.

**Files.** `src-tauri/src/db/stats.rs`.

**Dependencies.** T-02.

**Implementation details.** `LEFT JOIN tasks ON tasks.project_id = projects.id`, `GROUP BY projects.id`. Buckets via `SUM(CASE WHEN status = ?N THEN 1 ELSE 0 END)`, statuses bound from `TASK_STATUSES`. `total` via `COUNT(tasks.id)`, which correctly yields 0 for a project with no matching rows, unlike `COUNT(*)`.

**Acceptance criteria.** Result length equals the row count of `projects`. A project with no tasks is present with `total: 0`. Counts for one project never include another project's tasks.

**Tests.** `counts_by_project_includes_zero_task_projects`, `counts_by_project_are_scoped`, `counts_by_project_on_empty_database`.

---

### T-05: Implement `count_tasks_by_agent()`

**Objective.** Per-agent assigned-task counts, including zero-task agents.

**Files.** `src-tauri/src/db/stats.rs`.

**Dependencies.** T-02.

**Implementation details.** `LEFT JOIN tasks ON tasks.assigned_agent = ai_agents.id`, `GROUP BY ai_agents.id`, `COUNT(tasks.id)`. Tasks with `assigned_agent IS NULL` join to no agent and are therefore counted nowhere, which is correct.

**Acceptance criteria.** Result length equals the row count of `ai_agents`. An agent with no assigned tasks is present with `task_count: 0`. The sum of `task_count` equals the number of tasks with a non-null `assigned_agent`.

**Tests.** `counts_by_agent_includes_zero_task_agents`, `counts_by_agent_excludes_unassigned_tasks`, `counts_by_agent_after_agent_delete`.

---

### T-06: Implement `list_recent_tasks(limit)`

**Objective.** The first cross-project task read.

**Files.** `src-tauri/src/db/stats.rs`.

**Dependencies.** T-02.

**Implementation details.** Select the `tasks` columns in the same order `db/tasks.rs` uses, plus `projects.name`. `INNER JOIN projects ON projects.id = tasks.project_id` is correct here and is not a violation of 2.4: `tasks.project_id` is `NOT NULL`, so no task can be dropped. Order by `updated_at DESC, id DESC`. Apply `clamp_limit`. Map the task columns with the same field order as `map_task_row`.

**Acceptance criteria.** Ordering is `updated_at DESC` with an `id DESC` tiebreak and is stable across repeated calls. `limit` is clamped to `1..=100`. Each entry's `projectName` matches the owning project.

**Tests.** `recent_tasks_are_ordered_by_updated_at_desc`, `recent_tasks_tiebreak_is_deterministic`, `recent_tasks_respect_limit`, `recent_tasks_clamp_limit`, `recent_tasks_carry_correct_project_name`, `recent_tasks_on_empty_database`.

---

### T-07: Add the four commands

**Objective.** Expose the aggregate layer over IPC.

**Files.** `src-tauri/src/commands/mod.rs`.

**Dependencies.** T-03, T-04, T-05, T-06.

**Implementation details.** Four `#[tauri::command]` functions following the existing lock-then-delegate shape. Import the four structs and four functions from `crate::db::stats`. No logic in the command bodies beyond acquiring the lock.

**Acceptance criteria.** `cargo check` exits 0 with zero warnings. No command body contains a SQL string or a conditional.

**Tests.** None directly; covered by T-03 through T-06.

---

### T-08: Register the commands

**Objective.** Twenty-four entries in `generate_handler!`.

**Files.** `src-tauri/src/lib.rs`.

**Dependencies.** T-07.

**Implementation details.** Append the four entries after `nexus_assign_task_agent`. Do not reorder existing entries.

**Acceptance criteria.** `sed -n '/invoke_handler/,/])/p' src-tauri/src/lib.rs | grep -c 'commands::'` returns 24. `cargo check` exits 0 with zero warnings.

**Tests.** IPC contract check: registered equals defined.

---

### T-09: TypeScript types and wrappers

**Objective.** Mirror the Rust contract on the frontend.

**Files.** `src/types/db.ts`, `src/lib/nexus-db.ts`.

**Dependencies.** T-08.

**Implementation details.** Add the four interfaces of 6.1 and the four wrappers of 6.3. `TaskWithProject` references the existing `Task` interface; do not restate its fields.

**Acceptance criteria.** `npx tsc --noEmit` exits 0. Contract check reports 24 registered, 24 invoked, 24 defined, zero mismatches, and camelCase field parity for `WorkspaceSummary`, `ProjectTaskCounts`, and `AgentTaskCounts`.

**Tests.** IPC contract check.

---

### T-10: Extend `NexusScreen`

**Objective.** Add the fourth screen value before anything renders it.

**Files.** `src/types/index.ts`.

**Dependencies.** None. May run in parallel with T-01 through T-09.

**Implementation details.** Add `'overview'` as the first union member. Leave `NexusView` unchanged.

**Acceptance criteria.** `npx tsc --noEmit` exits 0. `NexusScreen` has exactly four values.

**Tests.** None.

---

### T-11: Build `StatTile`

**Objective.** The presentational statistic tile.

**Files.** `src/components/StatTile/StatTile.tsx`, `src/components/StatTile/StatTile.css` (both new).

**Dependencies.** T-10.

**Implementation details.** Props per 7.4. No state, no effects, no imports from `src/lib/nexus-db.ts`. Existing design tokens only.

**Acceptance criteria.** `StatTile.tsx` contains no `useState`, no `useEffect`, and no import from `../../lib/nexus-db`. Renders `0` as the character `0`, not as a falsy blank.

**Tests.** Manual, as part of the Overview scenarios.

---

### T-12: Build `OverviewScreen`

**Objective.** The screen itself, with its four states.

**Files.** `src/components/OverviewScreen/OverviewScreen.tsx`, `src/components/OverviewScreen/OverviewScreen.css` (both new).

**Dependencies.** T-09, T-11.

**Implementation details.** Per 7.3. Both commands in one `Promise.all`. `RECENT_TASK_LIMIT = 8`. The status row iterates `TASK_STATUS_ORDER` imported from `TaskCard`, not a local list. Recent rows call `navigate` with the task's `projectId`.

**Acceptance criteria.** Loading renders text and no tiles. Error renders an element with `role="alert"` and no tiles. An empty database renders the empty panel and a button that navigates to `'projects'`. A populated database renders every tile with a value taken from the summary. No numeric literal appears in the render output except `RECENT_TASK_LIMIT`.

**Tests.** Manual scenarios 1, 2, 6, 7.

---

### T-13: Wire the Overview into the shell

**Objective.** Make the screen reachable and make it the default.

**Files.** `src/components/AppShell/AppShell.tsx`, `src/components/AppShell/AppShell.css`, `src/components/Dashboard/Dashboard.tsx`.

**Dependencies.** T-12.

**Implementation details.** Per 7.1 and 7.2. Initial state becomes `{ screen: 'overview' }`. Nav gains an Overview button, placed first. Active-state logic per 7.1. `Dashboard` gains one branch.

**Acceptance criteria.** `grep -n "nexus-db" src/components/AppShell/AppShell.tsx` returns nothing. The application opens on the Overview. Exactly one nav button carries `aria-current="page"` at any time. Opening a project detail keeps Projects marked active. `pnpm build` exits 0.

**Tests.** Manual scenario 6.

---

### T-14: Task count on `ProjectCard`

**Objective.** Close the NEXUS-004 deferral.

**Files.** `src/components/ProjectCard/ProjectCard.tsx`, `src/components/ProjectCard/ProjectCard.css`, `src/components/ProjectList/ProjectList.tsx`.

**Dependencies.** T-09.

**Implementation details.** Per 7.5 and 7.6. `taskCount` is a required prop. `ProjectList` builds the map and passes `counts.get(project.id) ?? 0`.

**Acceptance criteria.** A project with zero tasks renders the text `0 tasks`. A project with one task renders `1 task`. `ProjectCard` remains presentational, with no import from `../../lib/nexus-db`. `npx tsc --noEmit` exits 0, and omitting `taskCount` is a compile error.

**Tests.** Manual scenario 3.

---

### T-15: Exact task count in the registry delete confirmation

**Objective.** Close the NEXUS-005 section 7.4 deferral.

**Files.** `src/components/RegistryPanel/RegistryPanel.tsx`, `src/components/RegistryCard/RegistryCard.tsx`, `src/components/RegistryScreen/RegistryScreen.tsx`.

**Dependencies.** T-09.

**Implementation details.** Per 7.7. `RegistryKind` gains `countsTasks`; `RegistryScreen` sets it `false` for the IDE descriptor and `true` for the agent descriptor. `RegistryCard` gains `taskUsage: number | null`. `usageWarning` gains the three-way task clause and omits it entirely when `taskUsage` is null.

**Acceptance criteria.** Deleting an agent with two assigned tasks shows a confirmation containing "2 tasks are assigned to it and will be unassigned." Deleting an agent with none shows "No task is assigned to it." Deleting an IDE shows no clause about tasks. The IDE panel issues no call to `countTasksByAgent`.

**Tests.** Manual scenario 5.

---

### T-16: Full verification

**Objective.** Prove the milestone.

**Files.** None.

**Dependencies.** All preceding tasks.

**Implementation details.** Run `cargo test --lib`, `pnpm build`, `pnpm tauri build`. Run the IPC contract check. Run the structural greps of section 9.1. Perform the manual checklist of section 9.5, including the carried-forward NEXUS-004 and NEXUS-005 scenarios.

**Acceptance criteria.** Section 9 in full.

**Tests.** The complete suite plus the manual checklist.

---

## 9. Acceptance Criteria

### 9.1 Build and structure

- [ ] `pnpm build` completes with zero TypeScript and zero Vite errors.
- [ ] `pnpm tauri build` produces `NEXUS.app` and `NEXUS_0.1.0_aarch64.dmg`.
- [ ] `cargo test --lib` passes with the 38 pre-existing tests plus the 17 added by this milestone, zero failures.
- [ ] `update_task_preserves_external_id_and_agent` passes unmodified.
- [ ] `git diff --stat package.json pnpm-lock.yaml src-tauri/Cargo.toml src-tauri/Cargo.lock` produces no output.
- [ ] `git diff --stat src-tauri/src/db/migrations.rs` produces no output, and `SELECT MAX(id) FROM _migrations` returns 1.
- [ ] `git diff --stat` shows no change to `db/projects.rs`, `db/tasks.rs`, `db/ides.rs`, `db/agents.rs`, `db/registry.rs`, or `main.rs`.
- [ ] `git diff --stat` shows no change to `Logo`, `StatusBar`, `CommandBar`, `DbPanel`, `ProjectForm`, `ProjectDetail`, `TaskList`, `TaskCard`, `TaskForm`, `RegistryForm`, or `RegistrySelect`.
- [ ] `grep -n "nexus-db" src/components/AppShell/AppShell.tsx` returns nothing.
- [ ] `grep -rl "@tauri-apps/api" src/` returns exactly `src/lib/nexus-db.ts`.
- [ ] No raw SQL under `src/`.
- [ ] `external_id` appears in no write statement anywhere in the codebase.
- [ ] Registered commands equals 24, invoked equals 24, defined equals 24, with zero mismatches in either direction.
- [ ] `WorkspaceSummary`, `ProjectTaskCounts`, and `AgentTaskCounts` camelCase field lists match their TypeScript interfaces exactly.
- [ ] `NexusScreen` has exactly four values.
- [ ] `DbPanel.tsx` and `DbPanel.css` exist on disk, and the string `Persistence Verification` appears zero times in `dist/assets/*.js`.

### 9.2 Overview behaviour

- [ ] The application opens on the Overview screen with no user action.
- [ ] The Overview displays tiles for projects, tasks, IDEs, and agents.
- [ ] The IDE and agent tiles show enabled counts distinct from totals.
- [ ] A status row shows a count for each of `open`, `in_progress`, `blocked`, `done`, and a count for unassigned tasks.
- [ ] A status with a count of zero is displayed showing `0`, not hidden.
- [ ] The recent activity list shows at most 8 entries.
- [ ] Each recent entry shows the task title, its status, and the name of its owning project.
- [ ] Recent entries are ordered most-recently-updated first.
- [ ] Clicking a recent entry navigates to the detail screen of that entry's project.
- [ ] While queries are in flight, a loading line is shown and no tile is rendered.
- [ ] If a query fails, an element with `role="alert"` is shown and no tile is rendered.
- [ ] On a database with zero projects and zero tasks, the empty panel is shown instead of the tile grid, with a button that navigates to the projects screen.

### 9.3 Counts

- [ ] Every tile value equals the corresponding `sqlite3` count taken at the same moment.
- [ ] A project with zero tasks displays `0 tasks` on its card.
- [ ] A project with one task displays `1 task`.
- [ ] Creating a task and returning to the project list increases that project's badge by exactly one.
- [ ] The agent delete confirmation states the exact number of assigned tasks.
- [ ] An agent with zero assigned tasks is described with "No task is assigned to it."
- [ ] The IDE delete confirmation contains no clause about tasks.

### 9.4 Navigation

- [ ] The header shows three nav targets: Overview, Projects, Registry.
- [ ] Exactly one carries `aria-current="page"` at any moment.
- [ ] Opening a project detail leaves Projects marked active, not Overview.
- [ ] Navigating from a project detail to the Overview clears the active project badge.

### 9.5 Manual UI verification

**Carried forward and still owed.** Every NEXUS-004 and NEXUS-005 manual scenario remains outstanding at the time this specification is written. They must be completed before or alongside the NEXUS-006 pass, because NEXUS-006 modifies `ProjectCard`, `ProjectList`, `RegistryPanel`, and `RegistryCard`, all of which sit in that unverified surface. The full carried-forward list is maintained in the NEXUS-008 specification, section 9.5, so it exists in exactly one place.

**New NEXUS-006 scenarios**

1. **Overview renders.** Launch the application. The Overview appears without any navigation. Every tile shows a number.
2. **Every tile matches `sqlite3`.** With the application open, run the equivalent `COUNT(*)` for each tile against `nexus.db`. Every tile matches. Repeat after creating a project and a task, then refreshing.
3. **Zero-task project shows `0`.** Create a project and add no tasks. On the projects screen, its card reads `0 tasks`, not blank and not absent. Add one task; it reads `1 task`.
4. **Recent items have correct names and order.** Create tasks in two different projects. Each recent entry names its own project. Edit the oldest task; it moves to the top of the list.
5. **Delete confirmation has the exact task count.** Assign two tasks to an agent. Begin deleting that agent: the confirmation states two tasks. Cancel. Unassign both, retry: it states no task is assigned. Begin deleting an IDE: no task clause appears at all.
6. **Active navigation state.** Click each of the three nav targets in turn; the clicked one is highlighted and the others are not. Open a project detail; Projects stays highlighted. Return to Overview; the project badge disappears from the header.
7. **Empty database state.** With the application closed, move `nexus.db` aside. Launch. The Overview shows the empty panel, not a grid of zeroes. The button navigates to the projects screen. Restore the original database afterwards.

---

## 10. Explicitly Out of Scope

Deferred deliberately:

- **Charts, graphs, sparklines, gauges, or any visualisation beyond counts and lists.** The Overview is numbers and a list.
- **Historical or time-series data.** No table records history; trends would require schema.
- **Per-IDE task counts.** `tasks` has no IDE relationship.
- **Filtering, searching, or sorting the Overview.** That is NEXUS-007, and only for the entity lists.
- **Configurable launch screen.** The default is a constant here. NEXUS-008 makes it a preference.
- **Configurable recent-activity limit.** `RECENT_TASK_LIMIT` is a module constant.
- **Making `DbPanel` visible, toggleable, or reachable.** It stays on disk and out of the UI.
- **Repurposing, extending, or removing `nexus_get_db_status` or `nexus_get_db_counts`.** They remain as NEXUS-002 left them.
- **Indexes on `tasks(project_id)` or `tasks(assigned_agent)`.** Revisit only on profiling evidence, as migration 002 in its own milestone.
- **The `CommandBar`.** Untouched. See the NEXUS-009 roadmap note in the NEXUS-008 specification.
- **Global cross-entity search.** Deferred to NEXUS-009.

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
- Any new Rust or frontend dependency, including a frontend test framework
