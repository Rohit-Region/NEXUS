# NEXUS-002: Local Persistence & Workspace Foundation

## Overview

Establish the local SQLite persistence layer and workspace data model for NEXUS. This milestone wires SQLite through the Tauri/Rust layer, defines the initial normalized schema, exposes typed Tauri commands to the React frontend, and adds a minimal development verification panel to confirm that persistence is working.

NEXUS-001 is complete and verified. This milestone extends it. Existing NEXUS-001 components are not redesigned or relocated unless integration requires it.

---

## 1. Existing State (from NEXUS-001)

Before proposing any changes, the relevant existing state is recorded here.

**Rust layer (`src-tauri/src/lib.rs`)**
- `tauri::Builder::default()` with a single `.setup()` closure.
- `tauri_plugin_log` registered only in debug builds.
- **Zero Tauri commands defined.** `.invoke_handler()` is not called.
- No SQL plugin, no persistent plugins in release builds.

**Rust dependencies (`src-tauri/Cargo.toml`)**
- `tauri = "2.11.3"` (features empty)
- `tauri-build = "2.6.3"`
- `tauri-plugin-log = "2"`
- `log = "0.4"`
- `serde` — **absent**
- `serde_json` — **absent**
- `tauri-plugin-sql` — **absent**

**Tauri capabilities (`src-tauri/capabilities/default.json`)**
- Only `core:default` is granted.
- No SQL plugin permission.

**Frontend (`package.json`)**
- `@tauri-apps/api` — **absent** (no JS-side `invoke()` calls exist anywhere).
- `@tauri-apps/plugin-sql` — **absent**.

**React**
- `App.tsx` → `AppShell` → `[Logo, StatusBar, Dashboard, CommandBar]`
- `Dashboard` is a static placeholder. It is the correct mounting point for the NEXUS-002 verification panel.
- `src/types/index.ts` holds `SystemStatus`, `PanelProps`, `CommandState`.

---

## 2. Requirements

### 2.1 Functional Requirements

| ID | Requirement |
|----|-------------|
| F-01 | SQLite database must be initialized automatically when NEXUS starts. |
| F-02 | The database file must be stored in the macOS application data directory (`$APP_DATA/nexus.db`). It must persist across restarts. |
| F-03 | The schema must be applied via sequential migrations on startup. |
| F-04 | The schema must define tables for: `projects`, `tasks`, `settings`, `ai_agents`, and `ides`. |
| F-05 | NEXUS must expose Tauri commands to: get database status, count records in each table, create a sample project, read projects, and delete a project by ID. |
| F-06 | The React frontend must be able to call these commands and display results. |
| F-07 | A development verification panel must be added to the existing Dashboard showing: DB status, project count, task count, AI agent count, IDE count. |
| F-08 | The verification panel must allow creating and deleting a test project to confirm round-trip persistence. |
| F-09 | Data must survive application restart. The verification panel must show counts that reflect data created in a previous session. |

### 2.2 Non-Functional Requirements

| ID | Requirement |
|----|-------------|
| N-01 | Database is local-only. No remote database, no backend server, no cloud sync. |
| N-02 | Raw SQL must never be exposed to or executed from the React layer. All database access goes through Tauri commands. |
| N-03 | Database logic must be isolated in a dedicated Rust module (`db/`). `lib.rs` must remain a thin orchestrator. |
| N-04 | Tauri commands must be defined in a dedicated module (`commands/`), not inline in `lib.rs`. |
| N-05 | The schema must be forward-compatible: migrations run in order and are never re-run on subsequent startups. |
| N-06 | The NEXUS-001 component structure must be preserved. No existing component files are moved or renamed. |
| N-07 | The verification panel is a development-only UI element. It must not become the final dashboard design. |
| N-08 | TypeScript types for all Tauri command payloads must be defined in `src/types/`. |
| N-09 | Only the minimum dependencies required to implement NEXUS-002 may be added. Each is justified in section 5. |
| N-10 | No authentication, no external API integrations, no remote services of any kind. |

### 2.3 Explicitly Out of Scope

The following must NOT be implemented in NEXUS-002:

- Jira, Claude, PlayerZero, Cursor, Grok, ChatGPT integrations
- AI agent orchestration or command execution
- Voice recognition / text-to-speech
- Browser automation
- Terminal / IDE launching
- News, weather, notifications, morning briefings
- Authentication or user accounts
- Cloud synchronization or remote APIs
- CI/CD, auto-update
- Full CRUD for all tables (only projects get full test CRUD; other tables get count-only reads)
- Final dashboard UI design

---

## 3. Architecture & Design

### 3.1 Data Flow

```
React (TypeScript)
    │  invoke("get_db_status") etc.
    │  @tauri-apps/api
    ▼
Tauri IPC boundary
    │
    ▼
src-tauri/src/commands/mod.rs   ← #[tauri::command] functions
    │
    ▼
src-tauri/src/db/              ← all SQL logic
    ├── mod.rs                 ← pool init, migration runner
    ├── migrations.rs          ← ordered SQL migration strings
    └── projects.rs            ← CRUD helpers for projects table
    │
    ▼
SQLite via rusqlite             ← local file: $APP_DATA/nexus.db
```

React never writes SQL. Rust never returns raw SQL to React. The command layer is a typed boundary.

### 3.2 Rust Module Structure

```
src-tauri/src/
├── main.rs             (unchanged — binary entry, calls app_lib::run())
├── lib.rs              (modified — registers plugin + commands, remains thin)
├── db/
│   ├── mod.rs          (pool/connection init, migration runner, app data path)
│   ├── migrations.rs   (ordered migration SQL strings)
│   └── projects.rs     (project CRUD helpers)
└── commands/
    └── mod.rs          (#[tauri::command] definitions — thin wrappers over db/)
```

**Why this structure:**
- `db/` owns all SQL knowledge. It is testable independently of Tauri.
- `commands/` owns the IPC boundary — it translates Tauri AppHandle/State into `db/` calls and serializes results.
- `lib.rs` registers both but contains no business logic.

### 3.3 State Management (Rust)

Tauri's managed state system (`tauri::Manager::manage()`) is used to share the database connection across commands. A `Mutex<rusqlite::Connection>` is managed as application state:

```rust
// in lib.rs setup closure:
let conn = db::init(&app.handle())?;
app.manage(DbState(Mutex::new(conn)));
```

Commands receive `State<'_, DbState>` as a parameter — the standard Tauri 2 pattern. This is safe and avoids a connection pool library (see dependency justification in section 5).

### 3.4 Migration Strategy

Migrations are applied using a hand-rolled migration runner. There is no ORM. The approach:

1. On startup, `db::init()` opens (or creates) `nexus.db`.
2. It creates a `_migrations` table if it does not exist.
3. It reads the highest applied migration number from `_migrations`.
4. It applies any migrations with a higher number in strict ascending order.
5. Each migration is a single `const &str` in `migrations.rs`, identified by a sequential integer.
6. Applied migrations are recorded with a timestamp. They are never re-applied.

This is sufficient for NEXUS-002 and avoids an ORM or migration framework dependency.

---

## 4. SQLite Schema

### 4.1 Schema Design Notes

The requested data model has been reviewed and two design decisions are proposed:

**Decision 1 — `default_ide` and `default_ai_agent` in `projects` table**

The spec lists `default_ide` and `default_ai_agent` as direct columns on `project`. These are stored as foreign keys (`INTEGER REFERENCES ides(id)` and `INTEGER REFERENCES ai_agents(id)`) rather than plain text strings. This keeps the model normalized and avoids duplicated strings across rows. Both columns are `NULL`-able since a project need not have defaults set in NEXUS-002.

**Decision 2 — `external_id` in `tasks`**

`external_id` is kept as `TEXT NULL` — it is the future home of a Jira issue key (e.g., `"PROJ-102"`). It is nullable because tasks created locally before Jira integration will have no external ID. A unique index is added scoped to `(project_id, external_id)` to prevent duplicate Jira imports per project.

**Decision 3 — `settings` primary key**

`settings` uses `key TEXT PRIMARY KEY` (not an auto-increment integer `id`). This matches the intent: settings are looked up by name, not by row number. `ON CONFLICT REPLACE` is used for upserts.

**Decision 4 — Timestamps**

All tables use `TEXT` for `created_at` / `updated_at`, stored as ISO-8601 UTC strings (`DATETIME DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))`). SQLite has no native `DATETIME` type; this is the standard portable approach.

### 4.2 Migration 001 — Initial Schema

```sql
-- Migration 001: initial schema

CREATE TABLE IF NOT EXISTS ides (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    name            TEXT    NOT NULL,
    ide_type        TEXT    NOT NULL,         -- e.g. 'cursor', 'vscode', 'intellij'
    executable_path TEXT,
    enabled         INTEGER NOT NULL DEFAULT 1,
    created_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS ai_agents (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    name            TEXT    NOT NULL,
    agent_type      TEXT    NOT NULL,         -- e.g. 'claude', 'grok', 'chatgpt'
    enabled         INTEGER NOT NULL DEFAULT 1,
    executable_path TEXT,
    created_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS projects (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    name             TEXT    NOT NULL,
    description      TEXT,
    repository_path  TEXT,
    repository_url   TEXT,
    default_ide_id   INTEGER REFERENCES ides(id) ON DELETE SET NULL,
    default_agent_id INTEGER REFERENCES ai_agents(id) ON DELETE SET NULL,
    created_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS tasks (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    external_id    TEXT,                      -- future: Jira key e.g. PROJ-102
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

CREATE TABLE IF NOT EXISTS settings (
    key        TEXT PRIMARY KEY,
    value      TEXT    NOT NULL,
    created_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS _migrations (
    id         INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
```

**Column name note:** `type` is a reserved word in some contexts. The columns are named `ide_type` and `agent_type` to avoid ambiguity, which is a minor improvement over the spec's `type`.

---

## 5. Dependency Justification

The following new dependencies are required. No others are added.

### 5.1 Rust — `rusqlite`

**Why:** SQLite access in Rust. `rusqlite` is the standard, widely-used, well-maintained crate for SQLite access from Rust. It wraps the SQLite C library directly (bundled via the `bundled` feature so no system SQLite is required).

**Why not `tauri-plugin-sql`:** `tauri-plugin-sql` is a Tauri plugin that exposes raw SQL execution directly to the JavaScript frontend via `invoke`. This violates requirement N-02 (raw SQL must not be exposed to React). NEXUS-002 requires custom Tauri commands that return typed structs — for that, `rusqlite` in the Rust layer with hand-written commands is the correct approach.

**Why not an ORM (Diesel, SeaORM, sqlx):** NEXUS-002's schema is simple and stable. An ORM adds significant compile-time complexity (proc-macros, derive-heavy), longer build times, and more dependencies. The hand-rolled migration runner + `rusqlite` is sufficient and matches the "minimum dependencies" requirement.

**Cargo entry:**
```toml
rusqlite = { version = "0.32", features = ["bundled"] }
```
The `bundled` feature compiles SQLite from source — no system SQLite dependency, consistent across macOS versions.

### 5.2 Rust — `serde` and `serde_json`

**Why:** Any `#[tauri::command]` that takes or returns a non-primitive value requires `serde::Serialize` / `serde::Deserialize`. The `Project`, `DbStatus`, and `DbCounts` structs returned from commands must implement these. This is a strict requirement of the Tauri IPC system — not an optional convenience.

**Cargo entry:**
```toml
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

These were intentionally omitted from NEXUS-001 as not required then. They are strictly required now for Tauri command serialization.

### 5.3 Frontend — `@tauri-apps/api`

**Why:** The JS-side `invoke()` function that calls Tauri commands lives in this package. Without it there is no way to call Rust commands from React. It is the official Tauri 2 package for this purpose.

**Package entry:** `@tauri-apps/api` (dev or runtime — used at runtime in the webview, so a regular dependency).

**Why not `@tauri-apps/plugin-sql`:** Same reason as 5.1 — this package exposes raw SQL to JS, which violates N-02.

---

## 6. Tauri Command Design

All commands are prefixed `nexus_` to namespace them and avoid collisions.

| Command | Direction | Input | Output | Purpose |
|---------|-----------|-------|--------|---------|
| `nexus_get_db_status` | Rust → React | — | `DbStatus` | Returns DB version, migration level, file path |
| `nexus_get_db_counts` | Rust → React | — | `DbCounts` | Returns record count for each table |
| `nexus_create_project` | React → Rust | `CreateProjectInput` | `Project` | Inserts a new project row, returns it |
| `nexus_list_projects` | Rust → React | — | `Vec<Project>` | Returns all projects |
| `nexus_delete_project` | React → Rust | `id: i64` | `()` | Deletes a project by ID |

### 6.1 TypeScript Payload Shapes

```typescript
// src/types/db.ts

export interface DbStatus {
  initialized: boolean;
  migrationLevel: number;
  dbPath: string;
}

export interface DbCounts {
  projects: number;
  tasks: number;
  aiAgents: number;
  ides: number;
  settings: number;
}

export interface Project {
  id: number;
  name: string;
  description: string | null;
  repositoryPath: string | null;
  repositoryUrl: string | null;
  defaultIdeId: number | null;
  defaultAgentId: number | null;
  createdAt: string;
  updatedAt: string;
}

export interface CreateProjectInput {
  name: string;
  description?: string;
  repositoryPath?: string;
  repositoryUrl?: string;
}
```

### 6.2 Rust Command Signatures (target shape)

```rust
// src-tauri/src/commands/mod.rs

#[tauri::command]
pub fn nexus_get_db_status(state: State<'_, DbState>) -> Result<DbStatus, String> { … }

#[tauri::command]
pub fn nexus_get_db_counts(state: State<'_, DbState>) -> Result<DbCounts, String> { … }

#[tauri::command]
pub fn nexus_create_project(
    state: State<'_, DbState>,
    input: CreateProjectInput,
) -> Result<Project, String> { … }

#[tauri::command]
pub fn nexus_list_projects(state: State<'_, DbState>) -> Result<Vec<Project>, String> { … }

#[tauri::command]
pub fn nexus_delete_project(state: State<'_, DbState>, id: i64) -> Result<(), String> { … }
```

Errors are returned as `Result<T, String>` — the standard Tauri 2 command error pattern. Future milestones can introduce a typed error enum.

---

## 7. React Integration Design

### 7.1 Tauri API Wrapper

A thin module wraps all `invoke()` calls to keep Tauri IPC calls out of component files:

```
src/
  lib/
    nexus-db.ts     ← typed wrappers around invoke() for each DB command
```

Components import from `src/lib/nexus-db.ts`, not directly from `@tauri-apps/api`. This isolates the Tauri dependency to one file.

```typescript
// src/lib/nexus-db.ts
import { invoke } from '@tauri-apps/api/core';
import type { DbStatus, DbCounts, Project, CreateProjectInput } from '../types/db';

export const getDbStatus   = (): Promise<DbStatus>            => invoke('nexus_get_db_status');
export const getDbCounts   = (): Promise<DbCounts>            => invoke('nexus_get_db_counts');
export const createProject = (i: CreateProjectInput): Promise<Project> => invoke('nexus_create_project', { input: i });
export const listProjects  = (): Promise<Project[]>           => invoke('nexus_list_projects');
export const deleteProject = (id: number): Promise<void>      => invoke('nexus_delete_project', { id });
```

### 7.2 `DbPanel` Component

A new component `DbPanel` is added to the Dashboard. It does not replace the existing Dashboard content — it is composed inside Dashboard as an additional section.

```
src/components/DbPanel/
    DbPanel.tsx
    DbPanel.css
```

`DbPanel` is self-contained: it fetches its own data via `useEffect` on mount and on demand. It manages its own loading and error state. No global state manager is introduced.

**Rendered content:**
- Status row: `DB: INITIALIZED` or `DB: ERROR` with migration level and file path.
- Counts grid: Projects / Tasks / AI Agents / IDEs / Settings.
- A "Create Test Project" button that calls `nexus_create_project`.
- A project list showing created projects with a "Delete" button per row.
- A "Refresh" button to re-fetch counts and status.

### 7.3 Dashboard Integration

`Dashboard.tsx` is modified minimally: `DbPanel` is imported and rendered below the existing welcome content. The watermark and welcome text are unchanged.

---

## 8. Capability Update

`src-tauri/capabilities/default.json` must grant the IPC commands access. Since NEXUS-002 uses custom `#[tauri::command]` functions (not `tauri-plugin-sql`), only `core:default` is needed — custom commands are allowed by default in Tauri 2 when registered via `invoke_handler`. No additional capability entry is required.

---

## 9. Implementation Tasks

Tasks are ordered for sequential execution. Each task that modifies Rust must be followed by a `cargo check` verification.

| # | Task | Description |
|---|------|-------------|
| T-01 | **Add Rust dependencies** | Add `rusqlite` (bundled), `serde` (derive), `serde_json` to `src-tauri/Cargo.toml`. Run `cargo check` to verify resolution. |
| T-02 | **Add `@tauri-apps/api` frontend dependency** | Add `@tauri-apps/api` to `package.json` via pnpm. |
| T-03 | **Create `db/migrations.rs`** | Define the `_migrations` table DDL and migration 001 SQL string as ordered `const` values. |
| T-04 | **Create `db/mod.rs`** | Implement `DbState` struct (wrapping `Mutex<rusqlite::Connection>`), `init()` function that opens/creates `nexus.db` in `$APP_DATA`, creates `_migrations` table, runs pending migrations. |
| T-05 | **Create `db/projects.rs`** | Implement `insert_project()`, `list_projects()`, `delete_project()`, `count_projects()` using `rusqlite`. |
| T-06 | **Create `commands/mod.rs`** | Implement all five `#[tauri::command]` functions as thin wrappers over `db/`. |
| T-07 | **Wire `lib.rs`** | Register `DbState` via `app.manage()` in the setup closure. Add `.invoke_handler(tauri::generate_handler![...])` with all five commands. Run `cargo check`. |
| T-08 | **Add TypeScript types** | Create `src/types/db.ts` with `DbStatus`, `DbCounts`, `Project`, `CreateProjectInput`. |
| T-09 | **Create `src/lib/nexus-db.ts`** | Implement the typed `invoke()` wrappers for all five commands. |
| T-10 | **Build `DbPanel` component** | Implement `DbPanel.tsx` + `DbPanel.css` — status, counts, project list, create/delete buttons. |
| T-11 | **Integrate `DbPanel` into `Dashboard`** | Add `DbPanel` below existing Dashboard welcome content. Minimal change to `Dashboard.tsx`. |
| T-12 | **Verify frontend build** | Run `pnpm build`. Fix any TypeScript errors. |
| T-13 | **Verify full build and launch** | Run `pnpm tauri build`. Confirm bundle produces. Launch the app. Verify DB panel shows "INITIALIZED", correct migration level, all zero counts. Create a test project, confirm count becomes 1, delete it, confirm count returns to 0. Restart the app and confirm data from the previous session is reflected correctly (if not deleted). |

---

## 10. Acceptance Criteria

The milestone is complete when all of the following are true:

- [ ] `pnpm build` completes without TypeScript or Vite errors.
- [ ] `pnpm tauri build` produces a macOS `.app` bundle and `.dmg` without errors.
- [ ] The app launches and the NEXUS window opens correctly (NEXUS-001 UI intact).
- [ ] The DB panel displays `DB: INITIALIZED` on first launch.
- [ ] The DB panel displays the current migration level (≥ 1).
- [ ] The DB panel shows the path to `nexus.db` in the macOS app data directory.
- [ ] All five table counts are displayed (projects, tasks, AI agents, IDEs, settings).
- [ ] Clicking "Create Test Project" inserts a row; the project count increments to 1.
- [ ] The new project appears in the project list in the panel.
- [ ] Clicking "Delete" on the project removes it; the project count returns to 0.
- [ ] Restarting the app preserves any data that was not explicitly deleted.
- [ ] No raw SQL is written in any TypeScript/React file.
- [ ] `@tauri-apps/api` is the only new frontend dependency introduced.
- [ ] `rusqlite` (bundled), `serde` (derive), and `serde_json` are the only new Rust dependencies.
- [ ] All Rust DB logic is isolated in `src-tauri/src/db/`.
- [ ] All Tauri command definitions are isolated in `src-tauri/src/commands/`.
- [ ] `lib.rs` contains no SQL logic.
- [ ] `Dashboard.tsx` is modified only to add `DbPanel`; all existing content is unchanged.
- [ ] No out-of-scope features (Jira, AI, voice, cloud, auth, etc.) are present.
