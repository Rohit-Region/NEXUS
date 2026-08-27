# NEXUS-008: Settings & Preferences

## Overview

`settings` is the last table in migration 001 with no producer and no consumer. It has existed since NEXUS-002 and has never held a row. NEXUS-003 recorded "Settings management" as out of scope and no milestone since has claimed it.

Every preference NEXUS-007 introduces resets on relaunch, and the registry NEXUS-005 built pays off only on the project where the user remembers to pick from it.

NEXUS-008 closes both gaps: a typed settings layer over the key/value table, a Settings screen, and consumption at the four places where a stored preference changes what the user sees.

After this milestone every table in migration 001 has a producer, and every deferral recorded in NEXUS-003 through NEXUS-005 that is in scope for a local-first application has been discharged.

### The architectural constraint that shapes this milestone

NEXUS-005 shipped with an enforced acceptance criterion:

> `AppShell` must not import anything from `src/lib/nexus-db.ts`.

A persisted launch screen requires `AppShell`'s initial view to come from the database. Implemented naively, NEXUS-008 breaks a rule NEXUS-005 shipped with.

**The locked decision: prop-drill from `App.tsx`.** `App.tsx`, currently four lines, loads settings and passes them into `AppShell` as props. `AppShell` still imports nothing from `nexus-db.ts`, so the rule survives literally and in spirit.

**React Context is not introduced.** Launch-screen persistence is not dropped. The existing `AppShell` architecture is not relaxed. Section 7 specifies the prop path in full, and section 9.1 makes the boundary a verifiable grep.

### Locked decisions carried into this milestone

- No React Context, no global state manager.
- The `CommandBar` is not touched.
- No global cross-entity search.
- No new frontend testing dependency.

### Dependency on outstanding verification

NEXUS-004 and NEXUS-005 manual verification remains outstanding. Section 9.5 of this specification is the single canonical home for that carried-forward checklist; NEXUS-006 and NEXUS-007 reference it rather than duplicating it.

---

## 1. Existing State

### 1.1 Assumed baseline

This specification is written against the state after NEXUS-006 and NEXUS-007. Where it depends on either, the dependency is named explicitly in section 1.4.

### 1.2 The `settings` table as migration 001 defined it

```sql
CREATE TABLE IF NOT EXISTS settings (
    key        TEXT PRIMARY KEY,
    value      TEXT    NOT NULL,
    created_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
```

Verified: zero producers and zero consumers anywhere in `src-tauri/src/` or `src/`, outside the migration text itself and the unrendered `DbPanel`, which counts its rows and nothing more.

No foreign keys. No constraint on `key`. `value` is `TEXT NOT NULL`.

### 1.3 Established conventions this milestone follows

- Typed input structs with `#[serde(rename_all = "camelCase")]`, mirrored one-for-one in `src/types/db.ts`.
- `Result<T, String>` with messages naming the operation.
- `updated_at` written explicitly in every statement. No triggers.
- Validation in Rust with a typed error naming the accepted set, as `validate_status` and `validate_entry` do.
- Tolerance for values written outside the application: `TaskStatus` and `.nexus-status-pill` both fall back rather than crash on an unrecognised status.
- Forms are presentational and receive their options as props. `ProjectForm` receives `ides` and `agents`; it does not fetch them.
- Inline delete and reset confirmations. No modal library.
- `RegistrySelect` centralises the rule that a select offers enabled entries plus whatever is currently selected, even when disabled.

### 1.4 Dependencies on NEXUS-006 and NEXUS-007

| Setting                                              | Depends on                               | If that milestone is absent                                |
| ---------------------------------------------------- | ---------------------------------------- | ---------------------------------------------------------- |
| `launchScreen`                                       | NEXUS-006 for the `'overview'` value     | Only `'projects'` is valid and the preference is pointless |
| `projectSort`, `taskSort`, `registrySort`            | NEXUS-007 for the sort modes to exist    | The values have no consumer                                |
| `taskStatusFilter`                                   | NEXUS-007 for the status filter to exist | The value has no consumer                                  |
| `newProjectDefaultIdeId`, `newProjectDefaultAgentId` | NEXUS-005 only, already shipped          | No dependency                                              |

The settings layer itself, T-01 through T-11, depends on neither. Only the consumption tasks, T-14 through T-16, do. **NEXUS-008 should not be implemented before NEXUS-007**, or five of its seven settings will have nowhere to be consumed.

---

## 2. Requirements

### 2.1 Functional Requirements

| ID   | Requirement                                                                                                                                              |
| ---- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| F-01 | The application must persist user preferences in the existing `settings` table and reload them on launch.                                                |
| F-02 | The user must be able to choose which screen the application opens on, from Overview and Projects.                                                       |
| F-03 | The chosen launch screen must be honoured on the next launch.                                                                                            |
| F-04 | The user must be able to choose a default IDE and a default agent applied to newly created projects.                                                     |
| F-05 | New-project defaults must pre-fill `ProjectForm` in create mode and must remain overridable before submitting.                                           |
| F-06 | New-project defaults must not alter any existing project.                                                                                                |
| F-07 | The user must be able to choose a default sort mode for the project list, the task list, and the registry lists.                                         |
| F-08 | The user must be able to choose a default task status filter.                                                                                            |
| F-09 | Default sort and filter values must seed each list's initial control state on mount, and must remain overridable in the session.                         |
| F-10 | A setting referencing an IDE or agent that no longer exists must resolve to no selection, and must not corrupt the rest of the settings.                 |
| F-11 | A setting referencing a disabled IDE or agent must still render by name wherever it is displayed.                                                        |
| F-12 | A missing settings row must yield that setting's default, not an error.                                                                                  |
| F-13 | A malformed or unrecognised settings value must yield that setting's default, not an error and not a crash.                                              |
| F-14 | A key present in the table that this version does not recognise must be preserved across a save, not deleted.                                            |
| F-15 | The user must be able to reset all settings to their defaults.                                                                                           |
| F-16 | Reset must require an inline confirmation step.                                                                                                          |
| F-17 | Cancelling the reset confirmation must leave every setting unchanged.                                                                                    |
| F-18 | If settings cannot be read at startup, the application must still render using compile-time defaults, and must surface the failure without blocking use. |
| F-19 | The Settings screen must be reachable from the header navigation and must show loading, saving, and error states.                                        |

### 2.2 Non-Functional Requirements

| ID   | Requirement                                                                                                                                                                                                                |
| ---- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| N-01 | No new Rust or frontend dependencies, including no test framework.                                                                                                                                                         |
| N-02 | No routing library and **no React Context**. Navigation and settings both travel as props.                                                                                                                                 |
| N-03 | All new commands use the `nexus_` prefix and the `Result<T, String>` error pattern.                                                                                                                                        |
| N-04 | All new TypeScript types go in `src/types/`.                                                                                                                                                                               |
| N-05 | All new `invoke()` wrappers go in `src/lib/nexus-db.ts`. No component may import `@tauri-apps/api`.                                                                                                                        |
| N-06 | Settings database logic goes in a new `src-tauri/src/db/settings.rs`. Commands stay in `commands/mod.rs`. `lib.rs` stays thin.                                                                                             |
| N-07 | **`AppShell` must not import anything from `src/lib/nexus-db.ts`.** Settings reach it as props from `App.tsx`.                                                                                                             |
| N-08 | `db/projects.rs`, `db/tasks.rs`, `db/ides.rs`, `db/agents.rs`, `db/registry.rs`, and `db/stats.rs` are not modified.                                                                                                       |
| N-09 | `Logo`, `StatusBar`, `CommandBar`, `DbPanel`, `ProjectCard`, `ProjectDetail`, `TaskCard`, `TaskForm`, `RegistryCard`, `RegistryForm`, `RegistrySelect`, `StatTile`, `ListControls`, and `OverviewScreen` are not modified. |
| N-10 | `SettingsForm` is presentational. It receives `ides` and `agents` as props and calls no command, mirroring `ProjectForm` and `RegistryForm`.                                                                               |
| N-11 | The two registry selects on the Settings screen reuse `RegistrySelect`. The F-15 rule from NEXUS-005 is not reimplemented.                                                                                                 |
| N-12 | The task status vocabulary comes from `TASK_STATUSES` in `db/tasks.rs` on the Rust side and `TASK_STATUS_ORDER` on the frontend. Neither is re-declared.                                                                   |
| N-13 | Reading settings must never write to the database.                                                                                                                                                                         |
| N-14 | The `#[cfg(test)]` pattern extends to the new module. All existing tests must pass unmodified.                                                                                                                             |
| N-15 | No secret, credential, API key, token, endpoint, or password may be stored in `settings`, now or by any later extension of this layer.                                                                                     |
| N-16 | Local-first. No remote configuration, no sync, no profile.                                                                                                                                                                 |

### 2.3 Design Principle: typed at the boundary, key/value underneath

The table is `(key TEXT, value TEXT)`. Exposing that shape over IPC would push string parsing into React and make every consumer responsible for defaults, producing seven independent places to get the fallback wrong.

Instead a single typed `Settings` struct crosses the boundary. `get` reads every known key, applies defaults for anything missing or unparseable, and returns a fully populated struct. `update` takes the full struct and upserts one row per key, exactly as `UpdateProjectInput` and `UpdateRegistryEntryInput` take a whole entity.

The frontend never sees a key, a raw value, or a parse decision.

**Why not typed columns instead of key/value?** Seven typed columns would require migration 002 to alter or replace a table, and the migration array has stayed at one entry for five milestones. Key/value is what migration 001 chose, it is adequate, and using it keeps the schema untouched. The costs, listed honestly, are 2.4 and 2.5.

### 2.4 Design Principle: read tolerates everything, write validates strictly

The `settings` table has no `CHECK` constraint and no foreign key, so any value can be present. It is also the easiest table in the database to hand-edit through `sqlite3`.

**On read, nothing is an error.** Every one of these yields the compile-time default for that key and no error:

| Condition                                                    | Result                                       |
| ------------------------------------------------------------ | -------------------------------------------- |
| Key absent from the table                                    | Default                                      |
| `launch_screen = 'nonsense'`                                 | Default `'overview'`                         |
| `project_sort = ''`                                          | Default `'created-desc'`                     |
| `task_status_filter = 'open,archived,blocked'`               | `['open', 'blocked']`, unknown token dropped |
| `task_status_filter = 'archived'`                            | `[]`, meaning no filtering                   |
| `new_project_default_ide_id = 'abc'`                         | `None`                                       |
| `new_project_default_ide_id = '42'` where IDE 42 was deleted | `None`                                       |
| A key this version does not recognise                        | Ignored on read, preserved on write          |

**On write, invalid input is rejected** with a typed error naming the accepted set, matching `validate_status`:

- An unrecognised `launch_screen` or sort mode is an error.
- A status token outside `TASK_STATUSES` is an error.
- An `ide_id` or `agent_id` that does not exist in the registry is an error, mirroring the foreign-key rejection `update_project` already produces for `default_ide_id`.

The asymmetry is deliberate. A read must never leave the user unable to launch the application because of a value they may not have written. A write is the application's own output and has no excuse for being invalid.

### 2.5 Design Principle: never destroy a key you do not own

A save writes only the keys this version knows about. Any other row in the table is left exactly as it is.

Without this rule, an older build that saves settings silently deletes every key a newer build introduced. That is the `external_id` failure mode in different clothing: NEXUS-004 excluded `external_id` from `update_task` precisely so a struct-shaped write could not null out a column the UI does not render, and NEXUS-005 preserved that exclusion rather than weaken it.

`unknown_key_in_table_is_preserved` is a required test and the automated guard for this rule.

`reset` follows the same discipline: it deletes only the known keys, leaving anything else untouched.

### 2.6 Design Principle: a dangling id resolves on read and is never written back

`settings` has no foreign key, so deleting an IDE cannot cascade or set-null into it. A stored `new_project_default_ide_id` can outlive the IDE it names. This is the price of key/value storage and must be handled explicitly, not hoped away.

- **On read,** the id is looked up in `ides` or `ai_agents`. If absent, the field resolves to `None`.
- **The row is not deleted or rewritten during the read.** N-13: a read has no write side effect. The stale row is harmless because it never resolves, and the next save writes whatever the UI submitted, which will be `None` because that is what the user was shown.
- **A disabled entry is not dangling.** It exists, so it resolves to its id and renders by name with a disabled marker, per F-11 and the NEXUS-005 F-15 rule that `RegistrySelect` already implements.

### 2.7 Design Principle: settings must never prevent the application from rendering

If `nexus_get_settings` fails at startup, `App.tsx` falls back to the compile-time defaults, renders the shell normally, and surfaces a non-blocking notice. It does not render a blank window, a spinner that never resolves, or an error page.

A preferences failure is not a reason to make the application unusable. F-18 makes this observable, and section 9.5 scenario 9 verifies it by making the database unreadable.

---

## 3. Architecture

### 3.1 Component Tree and the settings prop path

```
App                                    (MODIFIED: loads settings, owns the state)
│   settings, onSettingsChange
└── AppShell                           (MODIFIED: props in, seeds view, 4th nav target)
    │   NOTE: imports nothing from src/lib/nexus-db.ts
    ├── header
    │   ├── Logo                                          (unchanged)
    │   ├── ScreenNav [Overview | Projects | Registry | Settings]
    │   ├── active project badge                          (unchanged)
    │   └── StatusBar                                     (unchanged)
    ├── Dashboard(view, navigate, settings, onSettingsChange)   (MODIFIED)
    │   ├── [overview]       -> OverviewScreen            (unchanged)
    │   ├── [projects]       -> ProjectList(settings)     (MODIFIED)
    │   ├── [project-detail] -> ProjectDetail(settings)   (MODIFIED, pass-through)
    │   │                          └── TaskList(settings) (MODIFIED)
    │   ├── [registry]       -> RegistryScreen(settings)  (MODIFIED, pass-through)
    │   │                          └── RegistryPanel(settings)  (MODIFIED)
    │   └── [settings]       -> SettingsScreen            (NEW)
    │                              └── SettingsForm       (NEW, presentational)
    └── CommandBar                                        (unchanged)
```

`AppShell` receives settings and passes them on. It never fetches them. The boundary is verifiable: `grep -n "nexus-db" src/components/AppShell/AppShell.tsx` must return nothing.

`ProjectDetail` and `RegistryScreen` are pass-through only: they accept `settings` and hand it to `TaskList` and `RegistryPanel` respectively, gaining no settings logic of their own.

### 3.2 Ownership

| Concern                                 | Owner                                                              |
| --------------------------------------- | ------------------------------------------------------------------ |
| Loading settings at startup             | `App.tsx`                                                          |
| Holding the current settings            | `App.tsx`                                                          |
| Which screen is showing                 | `AppShell`, seeded from `settings.launchScreen`                    |
| Reading, saving, and resetting settings | `SettingsScreen`                                                   |
| Notifying the app of a change           | `SettingsScreen` calls `onSettingsChange`, threaded from `App.tsx` |
| Rendering the settings fields           | `SettingsForm`, presentational                                     |
| Applying a default sort or filter       | Each list component, on mount                                      |
| Applying new-project defaults           | `ProjectList`, when building the create form's initial values      |

### 3.3 Why the whole `Settings` object travels rather than individual props

Each consumer needs two or three fields. Passing them individually would mean five to seven props on `Dashboard` and a signature change every time a setting is added.

The whole object travels instead. `Dashboard` gains one prop, and adding a setting in a later milestone changes only the producer and the one consumer that reads it.

This is prop drilling by explicit decision, not by accident. The depth is three levels at most and every hop is visible in the type signature, which is the property Context would remove.

### 3.4 Rust Module Structure

```
src-tauri/src/
├── main.rs             (unchanged)
├── lib.rs              (add three commands to invoke_handler, 27 total)
├── db/
│   ├── mod.rs          (add `pub mod settings;`, one line)
│   ├── migrations.rs   (UNCHANGED, see 4.5)
│   ├── projects.rs     (UNCHANGED)
│   ├── tasks.rs        (UNCHANGED, read for TASK_STATUSES)
│   ├── registry.rs     (UNCHANGED)
│   ├── ides.rs         (UNCHANGED, read for existence checks)
│   ├── agents.rs       (UNCHANGED, read for existence checks)
│   ├── stats.rs        (UNCHANGED)
│   └── settings.rs     (NEW)
└── commands/
    └── mod.rs          (add three commands)
```

---

## 4. Database Schema Assessment

### 4.1 Sufficiency

The `settings` table is sufficient as migration 001 defined it. Seven keys, seven rows at most, all values expressible as `TEXT`.

### 4.2 Key names and value encoding

| Key                            | Value encoding                                         | Default        |
| ------------------------------ | ------------------------------------------------------ | -------------- |
| `launch_screen`                | `overview` or `projects`                               | `overview`     |
| `project_sort`                 | a `ProjectSortMode` token                              | `created-desc` |
| `task_sort`                    | a `TaskSortMode` token                                 | `created-desc` |
| `registry_sort`                | a `RegistrySortMode` token                             | `created-desc` |
| `task_status_filter`           | comma-separated status tokens; empty string means none | empty          |
| `new_project_default_ide_id`   | decimal integer, or empty string for none              | empty          |
| `new_project_default_agent_id` | decimal integer, or empty string for none              | empty          |

Key names are declared once as `&'static str` constants in `db/settings.rs` and never typed inline.

**Why comma-separated rather than JSON for the status filter.** `serde_json` is already a dependency, so JSON would cost nothing in packages. Comma-separated is chosen because the tokens are a small fixed vocabulary that cannot contain a comma, and because someone opening `nexus.db` in `sqlite3` sees `open,blocked` rather than `["open","blocked"]`. A settings table is the most likely table to be inspected and hand-edited, and the encoding should favour that reader. Parsing splits on comma, trims, drops empties, and drops unknown tokens per 2.4.

### 4.3 Writes

One upsert per known key:

```sql
INSERT INTO settings (key, value) VALUES (?1, ?2)
ON CONFLICT(key) DO UPDATE
  SET value = ?2,
      updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
```

`created_at` is written once by the schema default on first insert and never touched again. `updated_at` is set explicitly, matching `update_project`, `update_task`, `update_ide`, and `update_agent`. No trigger.

All seven upserts run inside a single transaction, so a validation failure or an I/O error leaves the previous settings wholly intact rather than half-written. This is the first place in the codebase where a transaction is genuinely required: every earlier write was a single statement.

### 4.4 Foreign keys

`settings` has none and gains none. Adding `REFERENCES ides(id)` is impossible against a `TEXT` column holding heterogeneous values, and adding typed columns to gain the reference would require migration 002 for a benefit 2.6 already handles at read time.

Existing foreign-key semantics elsewhere are untouched: `tasks.project_id` remains `ON DELETE CASCADE`; `projects.default_ide_id`, `projects.default_agent_id`, and `tasks.assigned_agent` remain `ON DELETE SET NULL`.

### 4.5 Migration 002: still not required

No table, column, index, constraint, or foreign key is added, altered, or removed. `MIGRATIONS` stays at one entry and the live database stays at migration level 1, for the sixth milestone running.

`settings` is the only table whose row count changes as a result of this milestone, and that is data, not schema.

---

## 5. Rust / Tauri Command Design

### 5.1 New commands

Three added, bringing the total to twenty-seven. All twenty-four existing commands are retained without modification.

### 5.2 The `Settings` struct

```rust
/// All application preferences, fully populated.
///
/// Every field is guaranteed present: `get_settings` substitutes the
/// compile-time default for any key that is missing, unparseable, or
/// unrecognised (spec 2.4). The two registry ids are resolved against the
/// registry on read, so a deleted entry yields None (spec 2.6).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub launch_screen: String,
    pub project_sort: String,
    pub task_sort: String,
    pub registry_sort: String,
    pub task_status_filter: Vec<String>,
    pub new_project_default_ide_id: Option<i64>,
    pub new_project_default_agent_id: Option<i64>,
}
```

`Settings` is used for both directions. There is no separate `UpdateSettingsInput`: the shape is identical, every field is always present, and a second struct would be two contracts to keep aligned for no gain.

### 5.3 Accepted value sets

```rust
pub const LAUNCH_SCREENS: [&str; 2] = ["overview", "projects"];

pub const PROJECT_SORTS: [&str; 5] =
    ["created-desc", "created-asc", "updated-desc", "name-asc", "name-desc"];
pub const TASK_SORTS: [&str; 5] =
    ["created-desc", "created-asc", "updated-desc", "title-asc", "status"];
pub const REGISTRY_SORTS: [&str; 4] =
    ["created-desc", "created-asc", "name-asc", "type-asc"];
```

These mirror the TypeScript unions in `src/types/index.ts` introduced by NEXUS-007. Status tokens are validated against `TASK_STATUSES` imported from `db/tasks.rs`, never re-declared (N-12).

### 5.4 Read behaviour

`get_settings` performs one `SELECT key, value FROM settings`, builds a map, and constructs `Settings` field by field. Each field:

1. Looks up its key. Absent yields the default.
2. Parses the value. A parse failure yields the default.
3. Validates against the accepted set. A miss yields the default.
4. For the two ids, checks existence in `ides` or `ai_agents`. Absent yields `None`.

No branch returns `Err` for a data condition. The only error `get_settings` can produce is a genuine database failure, phrased `"Failed to read settings: {e}"`.

Keys in the map that no field claims are ignored and never removed (2.5).

### 5.5 Write behaviour

`update_settings` validates the whole struct before opening a transaction, so a rejection writes nothing:

- `launch_screen` in `LAUNCH_SCREENS`, else `Err("Invalid launch screen: {v}. Expected one of: overview, projects")`
- each sort in its set, else `Err("Invalid {which} sort: {v}. Expected one of: ...")`
- every status token in `TASK_STATUSES`, else `Err("Invalid task status: {v}. Expected one of: ...")`
- duplicate status tokens are collapsed rather than rejected; order is not significant
- `new_project_default_ide_id`, when `Some(id)`, must exist in `ides`, else `Err("IDE {id} not found")`
- `new_project_default_agent_id`, when `Some(id)`, must exist in `ai_agents`, else `Err("Agent {id} not found")`

The two existence checks mirror the foreign-key rejection `update_project` already produces for a dangling `default_ide_id`, keeping the behaviour consistent across the codebase. A `None` is always valid and clears the preference.

Then, in one transaction, seven upserts per 4.3. Finally `get_settings` is called and its result returned, so the caller receives exactly what was persisted rather than what was submitted.

### 5.6 Reset behaviour

`reset_settings` deletes only the seven known keys inside a transaction, then returns `get_settings()`, which by construction yields the defaults. Unknown keys survive (2.5).

Deleting rather than writing defaults keeps the table's meaning clean: absent means default, and reset returns the table to the state it had before the application ever wrote to it.

### 5.7 IPC contracts

```
COMMAND:  nexus_get_settings
INPUT:    (none)
OUTPUT:   Settings
          Fully populated. Missing, malformed, or unrecognised values are
          replaced by defaults. Registry ids that no longer exist resolve
          to null. Never errors on a data condition.
ERRORS:   "Lock error: {e}"
          "Failed to read settings: {e}"        (database failure only)
REGISTERED: src-tauri/src/lib.rs, generate_handler! (entry 25 of 27)
INVOKED:  src/App.tsx via getSettings()
          src/components/SettingsScreen/SettingsScreen.tsx via getSettings()

COMMAND:  nexus_update_settings
INPUT:    input: Settings
OUTPUT:   Settings          re-read after the write
ERRORS:   "Lock error: {e}"
          "Invalid launch screen: {v}. Expected one of: overview, projects"
          "Invalid project sort: {v}. Expected one of: ..."
          "Invalid task sort: {v}. Expected one of: ..."
          "Invalid registry sort: {v}. Expected one of: ..."
          "Invalid task status: {v}. Expected one of: open, in_progress, blocked, done"
          "IDE {id} not found"
          "Agent {id} not found"
          "Failed to write settings: {e}"
REGISTERED: src-tauri/src/lib.rs, generate_handler! (entry 26 of 27)
INVOKED:  src/components/SettingsScreen/SettingsScreen.tsx via updateSettings()

COMMAND:  nexus_reset_settings
INPUT:    (none)
OUTPUT:   Settings          the defaults
ERRORS:   "Lock error: {e}"
          "Failed to reset settings: {e}"
REGISTERED: src-tauri/src/lib.rs, generate_handler! (entry 27 of 27)
INVOKED:  src/components/SettingsScreen/SettingsScreen.tsx via resetSettings()
```

### 5.8 Tests in `db/settings.rs`

Established in-memory pattern: open `:memory:`, `PRAGMA foreign_keys = ON`, apply the real `MIGRATIONS`.

**Defaults and round-trip**

| Test                                              | Asserts                                                                                                    |
| ------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- |
| `get_returns_defaults_on_empty_table`             | All seven fields equal their compile-time defaults. No error. Table still has zero rows afterwards (N-13). |
| `update_then_get_round_trips`                     | All seven fields persist and read back identically, including a multi-token status filter and both ids.    |
| `update_sets_updated_at_and_preserves_created_at` | A second save advances `updated_at` and leaves `created_at` unchanged.                                     |
| `update_upserts_without_duplicating_rows`         | Two saves leave exactly one row per known key.                                                             |

**Tolerance on read**

| Test                                            | Asserts                                                                                 |
| ----------------------------------------------- | --------------------------------------------------------------------------------------- |
| `invalid_launch_screen_falls_back_to_default`   | `launch_screen = 'nonsense'` yields `overview`, no error.                               |
| `invalid_sort_value_falls_back_to_default`      | Each of the three sort keys, set to garbage, yields its default.                        |
| `unknown_status_tokens_are_dropped_from_filter` | `'open,archived,blocked'` yields `["open", "blocked"]`. `'archived'` alone yields `[]`. |
| `non_integer_id_value_falls_back_to_none`       | `new_project_default_ide_id = 'abc'` yields `None`, no error.                           |
| `empty_string_id_value_is_none`                 | An empty value yields `None`.                                                           |

**The two load-bearing guards**

| Test                                       | Asserts                                                                                                                                                                                    |
| ------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **`unknown_key_in_table_is_preserved`**    | A hand-inserted `nexus_future_key` survives a full `update_settings` and a `reset_settings`, with its value unchanged (2.5).                                                               |
| **`deleted_default_ide_resolves_to_none`** | Set an IDE default, delete the IDE, read: the field is `None`, every other field is unchanged, and the stale row is still present in the table because the read did not write (2.6, N-13). |
| `deleted_default_agent_resolves_to_none`   | The same for agents.                                                                                                                                                                       |
| `disabled_default_still_resolves`          | A default pointing at a disabled entry resolves to its id, not `None`. Disabled is not dangling (F-11).                                                                                    |

**Validation on write**

| Test                                            | Asserts                                                                                                                |
| ----------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| `update_rejects_invalid_launch_screen`          | Typed error naming the accepted set; no row written.                                                                   |
| `update_rejects_invalid_sort_mode`              | Same, for each of the three sort keys.                                                                                 |
| `update_rejects_invalid_status_token`           | Same, naming the `TASK_STATUSES` vocabulary.                                                                           |
| `update_rejects_unknown_ide`                    | `Err("IDE {id} not found")`; no row written.                                                                           |
| `update_rejects_unknown_agent`                  | `Err("Agent {id} not found")`; no row written.                                                                         |
| `failed_update_leaves_previous_settings_intact` | After a successful save, a rejected save leaves every previously saved value readable and unchanged (4.3 transaction). |

**Reset**

| Test                            | Asserts                                                           |
| ------------------------------- | ----------------------------------------------------------------- |
| `reset_restores_defaults`       | After saving non-default values, reset yields all seven defaults. |
| `reset_deletes_only_known_keys` | An unknown key present before reset is present after.             |

**Regression gate:** every pre-existing test passes unmodified, `update_task_preserves_external_id_and_agent` included.

---

## 6. TypeScript Types

### 6.1 Additions to `src/types/db.ts`

```typescript
export interface Settings {
  launchScreen: 'overview' | 'projects';
  projectSort: ProjectSortMode;
  taskSort: TaskSortMode;
  registrySort: RegistrySortMode;
  taskStatusFilter: TaskStatus[];
  newProjectDefaultIdeId: number | null;
  newProjectDefaultAgentId: number | null;
}
```

The sort fields are typed as the NEXUS-007 unions rather than `string`. Rust rejects anything outside the accepted set on write and substitutes a default on read, so every value that can cross the boundary is in the union. A row hand-edited in `sqlite3` cannot violate this, because the Rust read layer has already normalised it.

### 6.2 Additions to `src/types/index.ts`

```typescript
export type NexusScreen =
  | 'overview'
  | 'projects'
  | 'project-detail'
  | 'registry'
  | 'settings';
```

`NexusView` keeps its shape.

### 6.3 Additions to `src/lib/nexus-db.ts`

```typescript
export function getSettings(): Promise<Settings> {
  return invoke<Settings>('nexus_get_settings');
}

export function updateSettings(input: Settings): Promise<Settings> {
  return invoke<Settings>('nexus_update_settings', { input });
}

export function resetSettings(): Promise<Settings> {
  return invoke<Settings>('nexus_reset_settings');
}
```

### 6.4 Compile-time defaults on the frontend

```typescript
// src/types/index.ts
export const DEFAULT_SETTINGS: Settings = {
  launchScreen: 'overview',
  projectSort: 'created-desc',
  taskSort: 'created-desc',
  registrySort: 'created-desc',
  taskStatusFilter: [],
  newProjectDefaultIdeId: null,
  newProjectDefaultAgentId: null,
};
```

Used solely by `App.tsx` for the F-18 startup fallback. It must match the Rust defaults exactly; a mismatch would mean the application behaves differently when settings fail to load than when they load empty. Section 9.1 makes this a review item.

---

## 7. React Component Design

### 7.1 `App.tsx`

Currently:

```tsx
export function App() {
  return <AppShell />;
}
```

Becomes the settings owner. State: `settings: Settings`, `loading: boolean`, `loadError: string | null`.

On mount it calls `getSettings()`. On success it stores the result. On failure it stores `DEFAULT_SETTINGS` and records `loadError` (F-18, 2.7).

While loading it renders a minimal placeholder rather than `AppShell`, because `AppShell` seeds its initial view from `settings.launchScreen` and must not mount with a value it will immediately contradict. The window is one IPC round trip against a local SQLite file.

It passes `settings` and a `handleSettingsChange` callback into `AppShell`. When `loadError` is non-null it also passes it, so a non-blocking notice can be shown; the application is fully usable in that state.

`App.tsx` is the only component outside `SettingsScreen` that calls a settings command.

### 7.2 `AppShell` changes

New props:

```typescript
interface AppShellProps {
  settings: Settings;
  onSettingsChange: (next: Settings) => void;
  settingsError?: string | null;
}
```

Changes:

1. Initial view is seeded from the prop: `useState<NexusView>({ screen: settings.launchScreen })`. Seeded once on mount; changing the preference later does not navigate the user away mid-session.
2. The nav gains a fourth target, Settings, placed last.
3. `settings` and `onSettingsChange` are passed to `Dashboard`.
4. If `settingsError` is set, a single dismissible line renders under the header.

`AppShell` imports nothing from `src/lib/nexus-db.ts` (N-07). It gains no fetch, no effect, and no settings logic beyond seeding and forwarding.

### 7.3 `Dashboard` changes

Two new props, `settings` and `onSettingsChange`, forwarded to the branches that need them, plus one new branch:

```tsx
{view.screen === 'settings' && (
  <SettingsScreen settings={settings} onSettingsChange={onSettingsChange} />
)}
```

`ProjectList`, `ProjectDetail`, and `RegistryScreen` receive `settings`. `OverviewScreen` does not: it consumes no preference.

### 7.4 `SettingsScreen`

**File:** `src/components/SettingsScreen/SettingsScreen.tsx`

```typescript
interface SettingsScreenProps {
  settings: Settings;
  onSettingsChange: (next: Settings) => void;
}
```

State: `ides: RegistryEntry[]`, `agents: RegistryEntry[]`, `loading`, `saving`, `resetting`, `confirmReset: boolean`, `error: string | null`, `savedAt: string | null`.

On mount it fetches `listIdes(false)` and `listAgents(false)`. `enabledOnly` is `false` because a currently-selected default may be disabled and must still render by name (F-11), the same reasoning `ProjectDetail` uses.

It also re-fetches settings on mount rather than trusting the prop, so opening the screen shows what is actually stored. The prop remains the source for the rest of the application until a save reports a new value.

Save calls `updateSettings`, then `onSettingsChange(result)` with the returned struct, so the whole application sees exactly what was persisted.

Reset uses the established inline pattern: a Reset button sets `confirmReset`, an inline row appears reading "Reset all settings to their defaults? This cannot be undone.", Cancel clears the flag and changes nothing (F-17), Confirm calls `resetSettings`, then `onSettingsChange(result)`.

States: loading text while the registry lists are in flight; a disabled form with "Saving..." while saving; `role="alert"` on error; a brief confirmation after a successful save.

### 7.5 `SettingsForm`

**File:** `src/components/SettingsForm/SettingsForm.tsx`

```typescript
interface SettingsFormProps {
  values: Settings;
  ides: RegistryEntry[];
  agents: RegistryEntry[];
  onSubmit: (values: Settings) => Promise<void>;
  onCancel: () => void;
  submitting: boolean;
}
```

Presentational. Receives `ides` and `agents` as props and calls no command (N-10), mirroring `ProjectForm` after the NEXUS-005 amendment.

Fields:

| Field                      | Control                                                                       |
| -------------------------- | ----------------------------------------------------------------------------- |
| Launch screen              | Two `.nexus-btn` toggles, Overview and Projects                               |
| Default project sort       | `.nexus-select` over the `ProjectSortMode` options                            |
| Default task sort          | `.nexus-select` over the `TaskSortMode` options                               |
| Default registry sort      | `.nexus-select` over the `RegistrySortMode` options                           |
| Default task status filter | Status pill toggles over `TASK_STATUS_ORDER`, none selected meaning no filter |
| New-project default IDE    | `RegistrySelect`                                                              |
| New-project default agent  | `RegistrySelect`                                                              |

The two registry pickers use `RegistrySelect` unchanged (N-11), inheriting the rule that enabled entries are offered plus whatever is currently selected even when disabled. That rule is not reimplemented here.

### 7.6 Consumption in the list components

| Component       | Reads                                                               | Applies                                                                                        |
| --------------- | ------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| `ProjectList`   | `projectSort`, `newProjectDefaultIdeId`, `newProjectDefaultAgentId` | Seeds its sort state on mount; pre-fills the create form's `defaultIdeId` and `defaultAgentId` |
| `TaskList`      | `taskSort`, `taskStatusFilter`                                      | Seeds sort and status filter state on mount                                                    |
| `RegistryPanel` | `registrySort`                                                      | Seeds sort state on mount                                                                      |

**Seeded, not bound.** Each list uses the setting as the initial value of its `useState` and never again. Changing a default in Settings does not retroactively re-sort a list the user has already adjusted in this session. The new default applies the next time that list mounts, which given 2.6 of NEXUS-007 means the next time the user navigates to it.

**New-project defaults apply to create only.** `ProjectList` uses them to build `initialValues` for `ProjectForm` in create mode. `ProjectDetail`'s edit mode continues to use the project's own stored values. No existing project is altered (F-06), and the pre-filled values remain overridable before submitting (F-05).

If a default id resolves to `None` because the entry was deleted, the create form opens with that field unset, which is correct and requires no special handling in the component: the Rust read layer has already resolved it.

### 7.7 Styling

New CSS files: `SettingsScreen.css`, `SettingsForm.css`. Any shared additions to `globals.css` are append-only. Existing tokens only, no new tokens, no theme change.

Reuses `.nexus-btn`, `.nexus-field`, `.nexus-select`, `.nexus-chip`, `.nexus-status-pill`, and `RegistrySelect`.

---

## 8. Implementation Tasks

Rust tasks gate on `cargo check`. Test tasks gate on `cargo test --lib`. Frontend tasks gate on `npx tsc --noEmit`.

---

### T-01: Create `db/settings.rs` with the model and defaults

**Objective.** The struct, key constants, accepted-value sets, and compile-time defaults.

**Files.** `src-tauri/src/db/settings.rs` (new).

**Dependencies.** None.

**Implementation details.** Declare `Settings` per 5.2 with the doc comment recording the tolerance and resolution rules. Declare the seven key-name constants and `LAUNCH_SCREENS`, `PROJECT_SORTS`, `TASK_SORTS`, `REGISTRY_SORTS` per 5.3. Add a `Settings::defaults()` constructor. Import `TASK_STATUSES` from `super::tasks`.

**Acceptance criteria.** No key name appears as an inline string literal outside its constant. No status token is declared in this file. File compiles once registered.

**Tests.** None yet.

---

### T-02: Register the module

**Objective.** Make `settings.rs` part of the crate.

**Files.** `src-tauri/src/db/mod.rs`.

**Dependencies.** T-01.

**Implementation details.** Add `pub mod settings;` in alphabetical position.

**Acceptance criteria.** `cargo check` exits 0. Unused-item warnings expected until T-08.

**Tests.** None.

---

### T-03: Implement value parsing and the tolerance rules

**Objective.** Every read-side fallback in one place, before any query uses it.

**Files.** `src-tauri/src/db/settings.rs`.

**Dependencies.** T-02.

**Implementation details.** Private helpers: `parse_enum(value, accepted, default) -> String`; `parse_status_filter(value) -> Vec<String>` splitting on comma, trimming, dropping empties and unknown tokens, collapsing duplicates; `parse_id(value) -> Option<i64>` returning `None` for an empty or non-integer value. None of these returns `Result`.

**Acceptance criteria.** No helper in this group can return an error. `parse_status_filter("open,archived,blocked")` yields `["open", "blocked"]`. `parse_id("abc")` yields `None`.

**Tests.** Covered by the read tests in T-05.

---

### T-04: Implement `get_settings()`

**Objective.** Read with defaults and dangling-id resolution, without writing.

**Files.** `src-tauri/src/db/settings.rs`.

**Dependencies.** T-03.

**Implementation details.** One `SELECT key, value FROM settings` into a map. Build each field per 5.4. Resolve the two ids with `SELECT 1 FROM ides WHERE id = ?1` and the agent equivalent. Ignore unclaimed keys. Only a database failure returns `Err`.

**Acceptance criteria.** On an empty table returns the defaults and leaves the table with zero rows. A garbage value in any key yields that key's default. A deleted registry id yields `None` while its row remains in the table. No `INSERT`, `UPDATE`, or `DELETE` appears anywhere in this function.

**Tests.** `get_returns_defaults_on_empty_table`, `invalid_launch_screen_falls_back_to_default`, `invalid_sort_value_falls_back_to_default`, `unknown_status_tokens_are_dropped_from_filter`, `non_integer_id_value_falls_back_to_none`, `empty_string_id_value_is_none`, `deleted_default_ide_resolves_to_none`, `deleted_default_agent_resolves_to_none`, `disabled_default_still_resolves`.

---

### T-05: Implement write validation

**Objective.** Reject invalid input before touching the database.

**Files.** `src-tauri/src/db/settings.rs`.

**Dependencies.** T-01.

**Implementation details.** A private `validate(conn, settings) -> Result<(), String>` per 5.5, including the two registry existence checks. Error strings name the accepted set. Duplicate status tokens are collapsed, not rejected.

**Acceptance criteria.** Every rejection path produces a message naming both the offending value and the accepted set. `None` for either id always validates.

**Tests.** `update_rejects_invalid_launch_screen`, `update_rejects_invalid_sort_mode`, `update_rejects_invalid_status_token`, `update_rejects_unknown_ide`, `update_rejects_unknown_agent`.

---

### T-06: Implement `update_settings()`

**Objective.** Transactional upsert of the seven known keys.

**Files.** `src-tauri/src/db/settings.rs`.

**Dependencies.** T-04, T-05.

**Implementation details.** Validate first. Then open a transaction, run seven upserts per 4.3, commit. Return `get_settings()`. Touch no key outside the seven (2.5).

**Acceptance criteria.** Two consecutive saves leave exactly one row per key. `created_at` is stable across saves; `updated_at` advances. A rejected save leaves the previously stored values readable and unchanged. An unknown key present before the save is present after with its value unchanged.

**Tests.** `update_then_get_round_trips`, `update_sets_updated_at_and_preserves_created_at`, `update_upserts_without_duplicating_rows`, `unknown_key_in_table_is_preserved`, `failed_update_leaves_previous_settings_intact`.

---

### T-07: Implement `reset_settings()`

**Objective.** Return to defaults without destroying foreign keys.

**Files.** `src-tauri/src/db/settings.rs`.

**Dependencies.** T-06.

**Implementation details.** In one transaction, `DELETE FROM settings WHERE key = ?1` for each of the seven constants. Then return `get_settings()`.

**Acceptance criteria.** After reset, all seven fields equal their defaults. An unknown key present before reset is present after. No `DELETE FROM settings` without a `WHERE key` clause appears anywhere.

**Tests.** `reset_restores_defaults`, `reset_deletes_only_known_keys`.

---

### T-08: Add the three commands

**Objective.** Expose the settings layer over IPC.

**Files.** `src-tauri/src/commands/mod.rs`.

**Dependencies.** T-04, T-06, T-07.

**Implementation details.** Three `#[tauri::command]` functions in the existing lock-then-delegate shape. Import `Settings` and the three functions from `crate::db::settings`.

**Acceptance criteria.** `cargo check` exits 0 with zero warnings. No command body contains SQL or a conditional.

**Tests.** Covered by T-04 through T-07.

---

### T-09: Register the commands

**Objective.** Twenty-seven entries.

**Files.** `src-tauri/src/lib.rs`.

**Dependencies.** T-08.

**Implementation details.** Append the three entries. Do not reorder existing entries.

**Acceptance criteria.** `sed -n '/invoke_handler/,/])/p' src-tauri/src/lib.rs | grep -c 'commands::'` returns 27. `cargo check` exits 0 with zero warnings.

**Tests.** IPC contract check.

---

### T-10: TypeScript types, defaults, and wrappers

**Objective.** Mirror the Rust contract and declare the startup fallback.

**Files.** `src/types/db.ts`, `src/types/index.ts`, `src/lib/nexus-db.ts`.

**Dependencies.** T-09.

**Implementation details.** Add `Settings` per 6.1, extend `NexusScreen` per 6.2, add `DEFAULT_SETTINGS` per 6.4, add the three wrappers per 6.3.

**Acceptance criteria.** `npx tsc --noEmit` exits 0. Contract check reports 27 registered, 27 invoked, 27 defined, zero mismatches, and camelCase parity for `Settings`. `DEFAULT_SETTINGS` matches `Settings::defaults()` field for field, verified by side-by-side review.

**Tests.** IPC contract check.

---

### T-11: Build `SettingsForm`

**Objective.** The presentational form.

**Files.** `src/components/SettingsForm/SettingsForm.tsx`, `src/components/SettingsForm/SettingsForm.css` (both new).

**Dependencies.** T-10.

**Implementation details.** Props and fields per 7.5. Uses `RegistrySelect` for both registry pickers. Uses `TASK_STATUS_ORDER` for the status toggles.

**Acceptance criteria.** `grep -n "nexus-db" src/components/SettingsForm/SettingsForm.tsx` returns nothing. The two registry pickers render via `RegistrySelect`, not a locally written `<select>`. Deselecting every status toggle produces an empty array, not a selection of all.

**Tests.** Manual scenarios 3, 4, 5.

---

### T-12: Build `SettingsScreen`

**Objective.** The screen, with save, reset, and its states.

**Files.** `src/components/SettingsScreen/SettingsScreen.tsx`, `src/components/SettingsScreen/SettingsScreen.css` (both new).

**Dependencies.** T-11.

**Implementation details.** Per 7.4. Fetches `listIdes(false)` and `listAgents(false)` and re-fetches settings on mount. Save calls `updateSettings` then `onSettingsChange`. Reset uses the inline confirmation pattern.

**Acceptance criteria.** Clicking Reset shows an inline confirmation and changes nothing until Confirm. Cancel leaves every value as it was. A save error renders `role="alert"` and leaves the form populated with the attempted values. Both registry selects list disabled entries that are currently selected.

**Tests.** Manual scenarios 6, 7, 8.

---

### T-13: Wire the Settings screen into the shell

**Objective.** Make it reachable.

**Files.** `src/components/AppShell/AppShell.tsx`, `src/components/AppShell/AppShell.css`, `src/components/Dashboard/Dashboard.tsx`.

**Dependencies.** T-12.

**Implementation details.** `AppShell` gains the props of 7.2 and a fourth nav target. `Dashboard` gains two props and one branch per 7.3.

**Acceptance criteria.** `grep -n "nexus-db" src/components/AppShell/AppShell.tsx` returns nothing. The header shows four nav targets. Exactly one carries `aria-current="page"`. `pnpm build` exits 0.

**Tests.** Manual scenario 2.

---

### T-14: Load settings in `App.tsx` and seed the launch screen

**Objective.** The prop path, and the F-18 fallback.

**Files.** `src/App.tsx`, `src/components/AppShell/AppShell.tsx`.

**Dependencies.** T-13.

**Implementation details.** Per 7.1. `App.tsx` fetches on mount, holds `settings`, falls back to `DEFAULT_SETTINGS` and records `loadError` on failure, renders a placeholder while loading, and passes `settings`, `onSettingsChange`, and `settingsError` down. `AppShell` seeds its initial view from `settings.launchScreen`.

**Acceptance criteria.** `grep -n "nexus-db" src/components/AppShell/AppShell.tsx` still returns nothing. `grep -rn "createContext\|useContext" src/` returns nothing. Setting the launch screen to Projects, quitting, and relaunching opens on Projects. With the database made unreadable, the application still renders with default settings and shows a non-blocking notice.

**Tests.** Manual scenarios 1, 2, 9.

---

### T-15: Consume new-project defaults in `ProjectList`

**Objective.** Make the registry pay off on every new project.

**Files.** `src/components/ProjectList/ProjectList.tsx`.

**Dependencies.** T-14.

**Implementation details.** Accept `settings`. Seed the sort control from `settings.projectSort`. Build `initialValues` for the create-mode `ProjectForm` with `defaultIdeId` and `defaultAgentId` from settings.

**Acceptance criteria.** With a default IDE set, opening the create form shows it pre-selected. Changing it before submitting stores the changed value. Submitting without touching it stores the default. Editing an existing project is unaffected. A default whose entry has been deleted opens the form with the field unset and does not throw.

**Tests.** Manual scenarios 3, 4.

---

### T-16: Consume default sort and filter in `TaskList` and `RegistryPanel`

**Objective.** Seed the NEXUS-007 controls.

**Files.** `src/components/TaskList/TaskList.tsx`, `src/components/RegistryPanel/RegistryPanel.tsx`, `src/components/ProjectDetail/ProjectDetail.tsx`, `src/components/RegistryScreen/RegistryScreen.tsx`.

**Dependencies.** T-14.

**Implementation details.** `ProjectDetail` and `RegistryScreen` accept `settings` and pass it through, gaining no logic. `TaskList` seeds its sort from `settings.taskSort` and its status filter from `settings.taskStatusFilter`. `RegistryPanel` seeds its sort from `settings.registrySort`.

**Acceptance criteria.** Setting a default task sort and opening a project shows tasks in that order. Setting a default status filter and opening a project shows that filter already applied, with the NEXUS-007 `{visible} of {total}` header. Changing a control in the session overrides the default without altering the stored setting.

**Tests.** Manual scenario 5.

---

### T-17: Verify the architectural boundary

**Objective.** Prove the NEXUS-005 rule survived.

**Files.** None. Review and grep only.

**Dependencies.** T-14, T-15, T-16.

**Implementation details.** Confirm the only settings-command callers are `App.tsx` and `SettingsScreen`. Confirm no Context was introduced.

**Acceptance criteria.** `grep -n "nexus-db" src/components/AppShell/AppShell.tsx` returns nothing. `grep -rn "createContext\|useContext\|Provider" src/` returns nothing. `grep -rln "getSettings\|updateSettings\|resetSettings" src/` returns exactly `src/App.tsx`, `src/components/SettingsScreen/SettingsScreen.tsx`, and `src/lib/nexus-db.ts`.

**Tests.** Structural greps in 9.1.

---

### T-18: Full verification

**Objective.** Prove the milestone.

**Files.** None.

**Dependencies.** All preceding tasks.

**Implementation details.** Run `cargo test --lib`, `pnpm build`, `pnpm tauri build`, the IPC contract check, and the structural greps of 9.1. Perform the complete manual checklist of 9.5, including the carried-forward NEXUS-004 and NEXUS-005 scenarios.

**Acceptance criteria.** Section 9 in full.

**Tests.** The complete suite plus the manual checklist.

---

## 9. Acceptance Criteria

### 9.1 Build and structure

- [ ] `pnpm build` completes with zero TypeScript and zero Vite errors.
- [ ] `pnpm tauri build` produces `NEXUS.app` and `NEXUS_0.1.0_aarch64.dmg`.
- [ ] `cargo test --lib` passes with every pre-existing test plus the 20 added by this milestone, zero failures.
- [ ] `update_task_preserves_external_id_and_agent` passes unmodified.
- [ ] `git diff --stat package.json pnpm-lock.yaml src-tauri/Cargo.toml src-tauri/Cargo.lock` produces no output.
- [ ] `git diff --stat src-tauri/src/db/migrations.rs` produces no output, and `SELECT MAX(id) FROM _migrations` returns 1.
- [ ] `git diff --stat` shows no change to `db/projects.rs`, `db/tasks.rs`, `db/ides.rs`, `db/agents.rs`, `db/registry.rs`, `db/stats.rs`, or `main.rs`.
- [ ] **`grep -n "nexus-db" src/components/AppShell/AppShell.tsx` returns nothing.**
- [ ] **`grep -rn "createContext\|useContext" src/` returns nothing.**
- [ ] `grep -rln "getSettings\|updateSettings\|resetSettings" src/` returns exactly `src/App.tsx`, `src/components/SettingsScreen/SettingsScreen.tsx`, and `src/lib/nexus-db.ts`.
- [ ] `grep -rl "@tauri-apps/api" src/` returns exactly `src/lib/nexus-db.ts`.
- [ ] No raw SQL under `src/`.
- [ ] `external_id` appears in no write statement anywhere in the codebase.
- [ ] Registered commands equals 27, invoked equals 27, defined equals 27, with zero mismatches.
- [ ] `Settings` camelCase field list matches the TypeScript interface exactly.
- [ ] `DEFAULT_SETTINGS` matches `Settings::defaults()` field for field.
- [ ] `NexusScreen` has exactly five values.
- [ ] No `DELETE FROM settings` without a `WHERE key` clause exists in the codebase.
- [ ] `db/settings.rs`'s `get_settings` contains no `INSERT`, `UPDATE`, or `DELETE`.
- [ ] No secret, credential, key, token, or endpoint is stored in or read from `settings`.

### 9.2 Persistence

- [ ] Each of the seven settings, changed and saved, reads back with the changed value after a full application quit and relaunch.
- [ ] The `settings` table contains at most one row per key after any number of saves.
- [ ] `created_at` is unchanged and `updated_at` advances on a second save of the same key.
- [ ] Launch screen set to Projects opens the application on Projects; set to Overview opens on Overview.

### 9.3 Defaults, corruption, and dangling references

- [ ] With `settings` empty, every field reads as its documented default and no row is created by the read.
- [ ] `UPDATE settings SET value='nonsense' WHERE key='launch_screen'` followed by a relaunch opens on Overview with no error.
- [ ] `task_status_filter` set to `open,archived,blocked` yields a filter of open and blocked.
- [ ] `task_status_filter` set to `archived` alone yields no filtering.
- [ ] A non-integer value in either id key yields no selection and no error.
- [ ] A hand-inserted unknown key survives a save and a reset with its value unchanged.
- [ ] Deleting an IDE that is the new-project default leaves every other setting readable, the Settings screen functional, and that field unset.
- [ ] A default pointing at a disabled entry renders by name with a disabled marker and remains selected.
- [ ] With the database made unreadable, the application renders with default settings and shows a non-blocking notice rather than a blank window.

### 9.4 Behaviour

- [ ] New-project defaults pre-fill the create form and can be changed before submitting.
- [ ] Submitting without changing them stores the default values on the new project.
- [ ] No existing project changes when a new-project default changes.
- [ ] Default sort and filter seed each list on mount, and a session override does not alter the stored setting.
- [ ] Changing a default does not re-sort a list already open in the session.
- [ ] Reset shows an inline confirmation before changing anything.
- [ ] Cancelling the reset leaves every setting at its previous value, verified in `sqlite3`.
- [ ] Confirming the reset restores every default and removes only the seven known keys.
- [ ] A rejected save leaves the previously stored settings intact.
- [ ] The header shows four nav targets and exactly one is marked current.

### 9.5 Manual UI verification

This section is the canonical home for the carried-forward checklist. NEXUS-006 section 9.5 and NEXUS-007 section 9.5 reference it rather than duplicating it.

#### Part A: carried forward from NEXUS-004 and still owed

1. Create a task in a project; it appears in the list immediately.
2. Submitting a task with an empty title is blocked and shows a validation message.
3. Clicking the status pill advances the status, and advancing from `done` wraps to `open`.
4. Edit a task's title, description, and status inline; all three save.
5. Deleting a task shows an inline confirmation; Cancel preserves it; Confirm removes it.
6. Only one task can be in edit mode or awaiting delete confirmation at a time.
7. Tasks created in project A do not appear in project B.
8. Task data survives a full application restart, including edited titles and statuses.
9. `updated_at` changes after an edit or a status change; `created_at` does not.
10. Deleting a project with tasks removes it, and `sqlite3` confirms zero task rows remain with that `project_id`.
11. Seed a task with a non-null `external_id` via `sqlite3`, edit that task in the UI, then confirm in `sqlite3` that `external_id` is unchanged.
12. Every task created through the UI has `external_id` and `assigned_agent` NULL in `sqlite3`.

#### Part B: carried forward from NEXUS-005 and still owed

13. The Registry screen renders and both panels show their empty states on a fresh database.
14. An IDE can be registered with a name and type; blank name and blank type are each blocked.
15. An agent can be registered the same way.
16. The enable/disable toggle round-trips and shows a Disabled chip when off.
17. The delete confirmation names the correct number of projects using the entry as a default.
18. Header navigation switches screens and marks the active one.
19. Registry data survives a full restart.
20. A project can be created and edited with a default IDE and agent, and with either left unset.
21. `ProjectDetail` shows both by name, or "Not set".
22. A task can be assigned an agent and unassigned again from the task list.
23. `TaskCard` shows the assigned agent's name.
24. A disabled agent is not offered in the assignment select.
25. **The F-15 check.** Assign an agent to a task, then disable that agent. Reopen the task's agent select: the agent is still listed, marked disabled, and still selected. The assignment is not silently dropped.
26. **SET NULL through the UI.** Delete an IDE used as a project default. The project still exists with its name and tasks intact, and its Default IDE now reads "Not set". Confirm in `sqlite3` that the project row survives with `default_ide_id` NULL.
27. Delete an agent used as a project default and as a task assignee: every project and task survives with both references cleared, confirmed by equal row counts before and after in `sqlite3`.
28. Assignments survive a full restart.

#### Part C: new NEXUS-008 scenarios

1. **Every preference survives a full restart.** Change all seven settings to non-default values and save. Quit the application completely. Relaunch. Open Settings: every value is as saved. Confirm the same in `sqlite3`.
2. **Launch screen is honoured.** Set the launch screen to Projects, save, quit, relaunch: the application opens on Projects with Projects marked current. Set it back to Overview and repeat.
3. **New-project defaults pre-fill.** Register an IDE and an agent. Set both as new-project defaults and save. Go to Projects, click New Project: both selects show the defaults already chosen. Submit; open the project: both show by name.
4. **Defaults remain overridable.** With defaults set, open the create form, change the IDE to a different entry, and submit. The project stores the changed value, not the default. Open Settings: the default is unchanged.
5. **Default sort and filter seed the lists.** Set the default task sort to title A to Z and the default status filter to `blocked`, and save. Open a project with mixed tasks: the list is title-sorted with the blocked filter already applied and the header reads `{visible} of {total}`. Change the sort in the session, navigate away and back: the default is applied again, and Settings still shows the stored default.
6. **Disabled default renders by name.** Set a default agent, then disable that agent in the Registry. Return to Settings: the agent is still selected, listed, and marked disabled. Open the create form: the same.
7. **Deleting a default IDE keeps settings valid.** Set a default IDE, then delete that IDE in the Registry. Return to Settings: the field reads Not set, every other setting is unchanged, and the screen is fully usable. Confirm in `sqlite3` that the other six keys still hold their values.
8. **Reset confirmation and cancellation.** With non-default settings saved, click Reset. An inline confirmation appears and nothing has changed yet. Click Cancel: every value is as it was, verified in `sqlite3`. Click Reset again and Confirm: every field returns to its default. Before resetting, insert a row with key `nexus_future_key` via `sqlite3`; after the reset, confirm that row is still present with its value unchanged.
9. **Corrupted settings fall back safely.** With the application closed, run `UPDATE settings SET value='nonsense' WHERE key='launch_screen'` and set an id key to `abc`. Relaunch: the application opens on Overview, the affected id field reads Not set, no error dialog appears, and every other setting is intact. Then make the database file unreadable and relaunch: the application still renders with default settings and shows a non-blocking notice rather than a blank window. Restore the file afterwards.

---

## 10. Explicitly Out of Scope

Deferred deliberately:

- **Theme, colour, and font customisation.** The NEXUS-001 theme is fixed. Changing that is its own decision.
- **Window size and position persistence.** Tauri configuration, not application data.
- **Keyboard shortcut customisation.**
- **Import and export of settings.**
- **Per-project setting overrides.** Settings are workspace-wide.
- **Any secret, credential, API key, token, endpoint, model name, or password.** No secret enters this database, now or by later extension.
- **Making `DbPanel` visible or toggleable from Settings.** Tempting and wrong: it would resurrect a development artifact into the production UI.
- **A settings-driven feature-flag system.**
- **Persisting live filter and search state.** NEXUS-007 section 2.6 specifies control state as session-local. This milestone persists defaults that seed it, nothing more.
- **Migration 002.** Not required. If typed settings columns are ever wanted, that is a deliberate schema milestone.
- **React Context.** Explicitly decided against.
- **The `CommandBar`.** Untouched.
- **Global cross-entity search.**

Also out of scope, per the standing NEXUS constraints:

- Jira, Claude, PlayerZero, Cursor, Grok, and ChatGPT integrations
- AI orchestration or execution of any kind
- IDE launching, terminal execution, browser automation
- Voice recognition, text to speech
- News, weather, morning briefings, notifications
- Authentication, cloud sync, CI/CD, auto-update
- Custom title bar or window chrome
- Any routing library, state manager, UI library, form library, or ORM
- Any new Rust or frontend dependency, including a frontend test framework

---

## 11. Future Roadmap: NEXUS-009

Recorded here so the direction is not lost. **No specification exists for NEXUS-009 and none is to be written yet.**

After NEXUS-008, every table in migration 001 has a producer and every in-scope deferral from NEXUS-003 through NEXUS-005 is discharged. One thread remains open: the `CommandBar` has been decorative since NEXUS-001, carrying the placeholder "Ask Nexus anything...".

NEXUS-009 is the likely home for:

- **A local command palette** built on the existing `CommandBar`, activating a component that has been inert for eight milestones.
- **Unified cross-entity search** across projects, tasks, IDEs, and agents, deferred from NEXUS-007 specifically so it could share this infrastructure rather than be built twice.
- **Local command parsing**, entirely on-device, with no network call and no model.
- **Navigation commands** to jump to a screen, a project, or a project's tasks.
- **Create commands** to add a project or a task without leaving the keyboard.
- **Filter and search commands** reusing the pure functions in `src/lib/list-filters.ts`.

Two things must be settled before that specification is written:

1. **The placeholder implies an assistant.** "Ask Nexus anything" reads as AI. AI orchestration is on the standing out-of-scope list. The locked decision is to treat the `CommandBar` as a future *local command palette*, not AI orchestration. Whether the placeholder text changes to match is a product decision.
2. **Unified search probably needs one Rust command.** Searching across four entity types from one input cannot be done from data already in a single list component's state, unlike NEXUS-007's per-list filtering. That is a genuine backend addition and should be scoped deliberately.

**Potential future AI orchestration is explicitly gated.** It remains out of scope and requires explicit approval, including approval of any external dependency it would introduce. Nothing in NEXUS-006 through NEXUS-008 assumes it, prepares for it, or makes it easier or harder.
