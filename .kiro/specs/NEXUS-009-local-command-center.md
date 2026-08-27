# NEXUS-009: Local Command Center

## Overview

The `CommandBar` has been decorative since NEXUS-001. It holds a text input whose value is stored in state and consumed by nothing, and a microphone button whose click handler is an explicit empty function. Eight milestones have shipped around it.

NEXUS-009 activates it as a **local command palette**: keyboard-driven navigation, creation, and search across the workspace, parsed entirely on-device.

It also discharges the last deferral carried from NEXUS-007: unified cross-entity search, which was deliberately postponed to this milestone so it could share the palette's infrastructure rather than be built twice.

### Phase relationship

This milestone was originally scoped as NEXUS-009A, with voice interaction as NEXUS-009B. The two are now separate documents:

- **NEXUS-009 (this document):** local keyboard and text Command Center. No AI, no network, no voice.
- **NEXUS-010:** voice interaction. **Not to be implemented.** It is gated behind an explicit decision between WebKit `SpeechRecognition` and a local speech-to-text engine, and behind a technical spike that may remove one of those options entirely.

NEXUS-010 is designed to feed transcribed text into the command grammar this milestone defines. Voice becomes another input to the same palette, never a parallel system. Nothing in NEXUS-009 assumes voice will arrive, and nothing here makes it harder or easier.

### Locked constraints

- **No AI.** No model, no inference, no orchestration, no assistant behaviour. Commands are matched against a static registry by plain text.
- **No network.** Not one request. The palette never leaves the machine.
- **No voice.** The microphone button is not wired up here.
- **No new dependencies**, Rust or frontend, including no test runner and no keybinding library.

### Dependency on outstanding verification

NEXUS-004 through NEXUS-008 are implemented but their manual UI validation is still in progress, and that tree is frozen. **NEXUS-009 must not begin until that validation completes and any defects it surfaces are fixed.** This milestone modifies `AppShell`, `ProjectList`, and `TaskList`, all of which sit in the surface under validation.

---

## 1. Existing State (from NEXUS-001 through NEXUS-008)

### 1.1 Architecture in place

**Rust layer**

- `db/`: `mod.rs`, `migrations.rs` (one entry, level 1), `projects.rs`, `tasks.rs`, `registry.rs`, `ides.rs`, `agents.rs`, `stats.rs`, `settings.rs`
- `commands/mod.rs`: twenty-seven registered commands
- `lib.rs`: thin orchestrator
- 76 tests, all using in-memory connections seeded from the real `MIGRATIONS`

**Frontend**

- `App.tsx` owns settings and prop-drills them into `AppShell`
- `AppShell` owns `view` and `activeProjectName`, and imports nothing from `src/lib/nexus-db.ts`
- `NexusScreen` has five values: `'overview' | 'projects' | 'project-detail' | 'registry' | 'settings'`
- `Dashboard` switches between five screens
- `src/lib/list-filters.ts`: pure `normalizeQuery`, `matchesQuery`, `compareText`, `compareStamp`, `compareStatus`, `sortWithIdTiebreak`, plus per-entity comparators and sort-option lists
- `ListControls` provides per-list search, sort and filter
- Shared `.nexus-btn`, `.nexus-field`, `.nexus-select`, `.nexus-chip`, `.nexus-status-pill`, `.nexus-notice`, `.nexus-filter-bar`, `.nexus-no-results`

### 1.2 The CommandBar as it stands

```tsx
const [state, setState] = useState<CommandState>({
  value: '',
  isListening: false, // always false in NEXUS-001
});

// Mic button is UI-only - no speech recognition in NEXUS-001
function handleMicClick() {
  // No-op: voice recognition is out of scope for NEXUS-001
}
```

The input carries `placeholder="Ask Nexus anything..."`. The button carries `aria-label="Voice input (not available)"` and `title="Voice input - coming soon"`.

Verified across the codebase: no `SpeechRecognition`, `webkitSpeechRecognition`, `getUserMedia`, `MediaRecorder`, `AudioContext`, or `speechSynthesis` reference anywhere, and no microphone entitlement in `tauri.conf.json` or `capabilities/default.json`.

### 1.3 What is missing

- **No cross-entity search.** `list_projects`, `list_ides` and `list_agents` return everything, but tasks can only be read per project or through `list_recent_tasks`, whose limit is clamped to 100 and whose ordering is recency, not relevance. There is no way to find a task by title across the workspace.
- **No keyboard entry point.** Every action requires the mouse to reach a screen first.
- **No way to open a create form directly.** Creating a task means navigating to Projects, opening a project, then clicking New Task.

---

## 2. Requirements

### 2.1 Functional Requirements

**Palette**

| ID   | Requirement                                                                                                |
| ---- | ---------------------------------------------------------------------------------------------------------- |
| F-01 | Pressing Command-K anywhere in the application must open the command palette.                              |
| F-02 | Pressing Escape, or clicking outside the palette, must close it without performing any action.             |
| F-03 | The palette must take keyboard focus on open and restore focus to the previously focused element on close. |
| F-04 | The palette must be dismissible with the mouse as well as the keyboard.                                    |
| F-05 | Opening the palette must not change the current screen.                                                    |
| F-06 | The palette must open with an empty query and a default list of available commands.                        |

**Commands**

| ID   | Requirement                                                                                                              |
| ---- | ------------------------------------------------------------------------------------------------------------------------ |
| F-07 | Typing must filter the command list by case-insensitive substring against each command's label and its keywords.         |
| F-08 | Navigation commands must exist for every top-level screen: Overview, Projects, Registry, Settings.                       |
| F-09 | A command must exist to create a new project, which navigates to Projects and opens the create form.                     |
| F-10 | A command must exist to create a new task in a named project, which navigates to that project and opens its create form. |
| F-11 | Commands whose preconditions are unmet must not be offered. Creating a task requires at least one project to exist.      |
| F-12 | Selecting a command with Enter must perform it and close the palette.                                                    |

**Search**

| ID   | Requirement                                                                                                                                                           |
| ---- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| F-13 | Typing must search projects, tasks, IDEs and agents in one query.                                                                                                     |
| F-14 | Results must state which kind each match is, and must carry enough context to be unambiguous: a task must show its project name, a registry entry must show its type. |
| F-15 | Selecting a project result must open that project's detail screen.                                                                                                    |
| F-16 | Selecting a task result must open the detail screen of the project owning that task.                                                                                  |
| F-17 | Selecting an IDE or agent result must open the Registry screen.                                                                                                       |
| F-18 | Search must be case-insensitive and substring-based, consistent with NEXUS-007.                                                                                       |
| F-19 | Results must be capped and the cap must be visible when it is reached, never silently truncating.                                                                     |

**Navigation within the palette**

| ID   | Requirement                                                                                                        |
| ---- | ------------------------------------------------------------------------------------------------------------------ |
| F-20 | Arrow Up and Arrow Down must move the selection, wrapping at both ends.                                            |
| F-21 | The selected entry must be visually distinct and scrolled into view.                                               |
| F-22 | Commands and search results must be visually grouped and separately labelled.                                      |
| F-23 | A query matching nothing must show an explicit empty-result state, not a blank panel.                              |
| F-24 | The palette must show a loading state while a search is in flight and an error state if it fails, without closing. |

### 2.2 Non-Functional Requirements

| ID   | Requirement                                                                                                                                            |
| ---- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| N-01 | No new Rust or frontend dependencies, including no keybinding or dialog library.                                                                       |
| N-02 | No network request of any kind. No AI, no model, no inference.                                                                                         |
| N-03 | No voice. The microphone button is not wired up in this milestone.                                                                                     |
| N-04 | No routing library and no global state manager. No React Context.                                                                                      |
| N-05 | All new commands use the `nexus_` prefix and the `Result<T, String>` error pattern.                                                                    |
| N-06 | Search database logic goes in a new `src-tauri/src/db/search.rs`. Commands stay in `commands/mod.rs`. `lib.rs` stays thin.                             |
| N-07 | `db/projects.rs`, `db/tasks.rs`, `db/ides.rs`, `db/agents.rs`, `db/registry.rs`, `db/stats.rs`, `db/settings.rs` and `migrations.rs` are not modified. |
| N-08 | **`AppShell` must not import anything from `src/lib/nexus-db.ts`.** It owns palette open state only; the palette fetches its own data.                 |
| N-09 | The command registry is a pure module with no React import and no database import, mirroring `src/lib/list-filters.ts`.                                |
| N-10 | Matching reuses `normalizeQuery` and `matchesQuery` from `src/lib/list-filters.ts`. The rule is not reimplemented.                                     |
| N-11 | `Logo`, `StatusBar`, `DbPanel`, and every card and form component are not modified.                                                                    |
| N-12 | `CommandBar` is modified only as far as F-25 requires (see 2.5). Its microphone button and handler are untouched.                                      |
| N-13 | The `#[cfg(test)]` pattern extends to the new module. All 76 existing tests must pass unmodified.                                                      |
| N-14 | Local-first. No telemetry, no analytics, no command history leaving the machine.                                                                       |

### 2.3 Design Principle: a registry, not a parser

A command palette invites a grammar: verbs, arguments, quoting, autocomplete of parameters. NEXUS-009 does not build one.

Commands are **static entries in a registry**, each with a label, a set of keywords, a precondition, and an action. Typing filters that list by substring. There is no tokeniser, no argument parser, and no partial-input state machine.

Parameterised commands are handled by **expansion, not parsing**: "New task in ..." is not one command with a project argument; it is one entry per existing project, generated from the project list when the palette opens. Ten projects produce ten entries, each with the project name in its label and keywords, so typing part of a project name narrows to it naturally.

This costs a little memory and removes an entire category of defect. It also keeps the door open for NEXUS-010: a speech transcript is just a query string, matched the same way, with no grammar to teach a recogniser.

### 2.4 Design Principle: search belongs in SQL, matching belongs in one place

NEXUS-007 filtered lists client-side because each list already held its complete dataset. The palette does not: there is no cross-project task array in memory anywhere, and building one would mean fetching every task in the workspace on every keystroke.

NEXUS-009 therefore adds **one** command, `nexus_search_workspace`, which queries all four tables and returns a flat, ranked, capped result set.

The matching rule stays consistent regardless: the SQL uses `LIKE` with an escaped pattern, and the palette's client-side command filtering uses `matchesQuery` from `list-filters.ts`. Both are case-insensitive substring matches. Where the frontend filters, it reuses the existing function rather than writing a second definition (N-10).

### 2.5 Design Principle: the CommandBar becomes an affordance, not the palette

The palette is an overlay, not an inline expansion of the existing bar. Reasons:

- The bar sits at the bottom of the shell. A results list growing upward from it fights the layout and the scroll container.
- An overlay can trap focus and restore it, which F-03 requires.
- The bar is present on every screen, including inside `ProjectDetail`'s scroll area. An overlay is unaffected by that.

`CommandBar` therefore changes minimally: its input becomes a **button-like affordance** that opens the palette on focus or click, rather than accepting free text that nothing consumes. This is the single change permitted by N-12.

**The placeholder text and the microphone button are deliberately left alone in this milestone.** Both are pending an explicit decision after NEXUS-004 to NEXUS-008 manual validation completes, between leaving the microphone as-is, disabling it visibly, or replacing the placeholder with a Command Palette hint. Section 10 records that decision as open. Do not pre-empt it.

### 2.6 Design Principle: intent travels with the view

F-09 and F-10 require navigating to a screen *and* opening a form there. `NexusView` currently carries only a screen and an optional project id.

It gains one optional field:

```typescript
export interface NexusView {
  screen: NexusScreen;
  projectId?: number;
  /** One-shot instruction consumed by the destination on mount. */
  intent?: 'create-project' | 'create-task';
}
```

The destination consumes it **as a `useState` initializer**, exactly as NEXUS-008 seeds sort and filter defaults. That makes it naturally one-shot: it applies on mount and never again, so it cannot re-fire on a later render, and no explicit clearing step is needed in `AppShell`.

`ProjectDetail` must pass `intent` through to `TaskList` for `'create-task'`, as it already passes `settings`.

---

## 3. Architecture

### 3.1 Component Tree

```
App                                              (unchanged)
└── AppShell                    (MODIFIED: Command-K listener, palette state)
    │   NOTE: still imports nothing from src/lib/nexus-db.ts
    ├── header                                   (unchanged)
    ├── Dashboard                                (MODIFIED: forwards intent)
    │   ├── [overview]       -> OverviewScreen   (unchanged)
    │   ├── [projects]       -> ProjectList      (MODIFIED: consumes intent)
    │   ├── [project-detail] -> ProjectDetail    (MODIFIED: forwards intent)
    │   │                          └── TaskList  (MODIFIED: consumes intent)
    │   ├── [registry]       -> RegistryScreen   (unchanged)
    │   └── [settings]       -> SettingsScreen   (unchanged)
    ├── CommandBar                               (MODIFIED: opens the palette)
    └── CommandPalette                           (NEW, overlay, owns its data)
          ├── query input
          ├── Commands group
          └── Search results group
```

### 3.2 Ownership

| Concern                                 | Owner                                                    |
| --------------------------------------- | -------------------------------------------------------- |
| Whether the palette is open             | `AppShell`                                               |
| The Command-K and Escape key handling   | `AppShell`                                               |
| Command registry definitions            | `src/lib/commands.ts`, pure                              |
| Palette query, selection index, results | `CommandPalette`                                         |
| Fetching projects and search results    | `CommandPalette`                                         |
| Performing an action                    | `CommandPalette`, by calling the `navigate` it was given |
| Opening a create form on arrival        | `ProjectList` and `TaskList`, from `view.intent`         |

`AppShell` holds a boolean and a keyboard listener. It fetches nothing and imports no command wrapper, preserving the NEXUS-005 boundary that NEXUS-008 already had to work around.

### 3.3 Rust Module Structure

```
src-tauri/src/
├── main.rs             (unchanged)
├── lib.rs              (add one command to invoke_handler, 28 total)
├── db/
│   ├── mod.rs          (add `pub mod search;`, one line)
│   ├── migrations.rs   (UNCHANGED, see 4.4)
│   ├── ... all other modules UNCHANGED ...
│   └── search.rs       (NEW)
└── commands/
    └── mod.rs          (add one command)
```

---

## 4. Database Schema Assessment

### 4.1 Sufficiency

Every column the search needs exists. No new table, column, index, constraint, or foreign key.

### 4.2 Tables read

| Table       | Matched against                                                   |
| ----------- | ----------------------------------------------------------------- |
| `projects`  | `name`, `description`, `repository_path`, `repository_url`        |
| `tasks`     | `title`, `description`; joined to `projects` for the project name |
| `ides`      | `name`, `ide_type`, `executable_path`                             |
| `ai_agents` | `name`, `agent_type`, `executable_path`                           |
| `settings`  | Not read. The palette exposes no preference.                      |

No table is written. NEXUS-009 adds no `INSERT`, `UPDATE`, or `DELETE`.

**`tasks.external_id` is deliberately not searched.** No milestone produces it, so every row holds `NULL` and searching it would be dead functionality. This is the same reasoning NEXUS-007 recorded.

### 4.3 Matching and escaping

Matching is `LOWER(column) LIKE '%' || LOWER(?1) || '%' ESCAPE '\'`.

**The query must be escaped before binding.** `%`, `_` and `\` are wildcards or the escape character in a `LIKE` pattern. A user typing `100%` or `snake_case` in the palette would otherwise get silently wrong results: `_` matches any single character, so `snake_case` would match `snakeXcase`. A private `escape_like` helper prefixes each of the three with a backslash, and `escape_like_is_applied` is a required test.

This is the kind of defect that produces plausible output and no error, so it is called out rather than left to the implementer.

### 4.4 Migration 002: still not required

No schema change. `MIGRATIONS` stays at one entry and the live database stays at level 1, for the seventh milestone running.

A full-text search index using SQLite FTS5 was considered and rejected. It would require migration 002, a virtual table, trigger-based synchronisation with four source tables, and a rebuild path, in exchange for performance that a local single-user workspace will not notice. `LIKE` over a few hundred rows is adequate. Revisit only on measured evidence.

---

## 5. Rust / Tauri Command Design

### 5.1 New command

One added, bringing the total to twenty-eight. All twenty-seven existing commands are retained without modification.

### 5.2 Result shape

```rust
/// One search hit, flattened deliberately.
///
/// A serde-tagged enum would carry a different payload per kind and force the
/// frontend to narrow before it can render a row. Every consumer needs the
/// same four things, so the shape is flat and `kind` is a plain string
/// matching the TypeScript union.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    /// 'project' | 'task' | 'ide' | 'agent'
    pub kind: String,
    pub id: i64,
    /// Primary label: project name, task title, or registry entry name.
    pub title: String,
    /// Disambiguating context: the owning project for a task, the type for a
    /// registry entry, the repository path for a project. None when absent.
    pub subtitle: Option<String>,
    /// Where selecting this result navigates. Set for projects and tasks.
    pub project_id: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResults {
    pub results: Vec<SearchResult>,
    /// True when the cap was reached, so the UI can say so (F-19).
    pub truncated: bool,
}
```

### 5.3 Query behaviour

- An empty or whitespace-only query returns an empty result set and `truncated: false`, without touching the database.
- Each of the four tables is queried separately and the results concatenated in a fixed kind order: projects, tasks, IDEs, agents. Ordering within a kind is `name`/`title` ascending, case-insensitive, with an `id` tiebreak, matching the NEXUS-007 total-ordering rule.
- The overall result is capped at `SEARCH_RESULT_CAP` (50). When more matches exist, the set is truncated and `truncated` is set. Silent truncation is forbidden by F-19.
- Kind order is fixed rather than relevance-scored. Scoring invites tuning and has no obvious right answer at this scale; a stable, explainable order is more useful than a clever one.

### 5.4 Validation and error handling

- No input can produce an error. An empty query is a valid no-op; a query of only wildcard characters is escaped and matches literally.
- Every function returns `Result<T, String>` with a message naming the operation, matching the established convention.
- No function panics on an empty database.

### 5.5 IPC contract

```
COMMAND:  nexus_search_workspace
INPUT:    query: String
OUTPUT:   SearchResults
          Empty results for an empty or whitespace-only query.
          Capped at 50 entries with `truncated` set when the cap is reached.
          Kind order: project, task, ide, agent.
ERRORS:   "Lock error: {e}"
          "Failed to search workspace: {e}"
REGISTERED: src-tauri/src/lib.rs, generate_handler! (entry 28 of 28)
INVOKED:  src/components/CommandPalette/CommandPalette.tsx
          via searchWorkspace(query) in src/lib/nexus-db.ts
```

### 5.6 Tests in `db/search.rs`

Established in-memory pattern: open `:memory:`, `PRAGMA foreign_keys = ON`, apply the real `MIGRATIONS`.

| Test                                         | Asserts                                                                                                                                                      |
| -------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `search_on_empty_database`                   | Empty results, `truncated: false`, no error.                                                                                                                 |
| `search_empty_query_returns_nothing`         | An empty query and a whitespace-only query both return no results.                                                                                           |
| `search_is_case_insensitive`                 | A lowercase query matches uppercase content and the reverse.                                                                                                 |
| `search_matches_substring`                   | A query matching the middle of a word is a hit.                                                                                                              |
| `search_matches_every_project_field`         | Distinct markers in name, description, path and URL each match.                                                                                              |
| `search_matches_task_title_and_description`  | Both fields match.                                                                                                                                           |
| `search_matches_registry_name_type_and_path` | All three fields match, for both kinds.                                                                                                                      |
| `search_task_carries_project_name_and_id`    | A task hit has the owning project's name as `subtitle` and its id as `projectId`.                                                                            |
| `search_does_not_match_external_id`          | A task whose only match is in `external_id` is not returned (4.2).                                                                                           |
| **`escape_like_is_applied`**                 | A project literally named `snake_case` is matched by the query `snake_case`, and a project named `snakeXcase` is **not**. Likewise `100%` matches literally. |
| `search_orders_by_kind_then_name`            | Projects precede tasks precede IDEs precede agents; within a kind, case-insensitive ascending with an id tiebreak.                                           |
| `search_caps_results_and_reports_truncation` | With more than 50 matches, exactly 50 are returned and `truncated` is true.                                                                                  |
| `search_below_cap_reports_not_truncated`     | With fewer matches, `truncated` is false.                                                                                                                    |
| `search_ignores_null_fields`                 | A row with null description or path does not match and does not error.                                                                                       |

**Regression gate:** all 76 pre-existing tests pass unmodified, `update_task_preserves_external_id_and_agent` included.

---

## 6. TypeScript Types

### 6.1 Additions to `src/types/db.ts`

```typescript
export type SearchResultKind = 'project' | 'task' | 'ide' | 'agent';

export interface SearchResult {
  kind: SearchResultKind;
  id: number;
  title: string;
  subtitle: string | null;
  projectId: number | null;
}

export interface SearchResults {
  results: SearchResult[];
  truncated: boolean;
}
```

### 6.2 Additions to `src/types/index.ts`

```typescript
/** One-shot instruction consumed by the destination screen on mount. */
export type NexusIntent = 'create-project' | 'create-task';

export interface NexusView {
  screen: NexusScreen;
  projectId?: number;
  intent?: NexusIntent;
}

/** A palette entry. Actions receive navigate; they never call a command. */
export interface PaletteCommand {
  id: string;
  label: string;
  /** Extra terms matched alongside the label. */
  keywords: string[];
  group: 'Navigate' | 'Create';
  run: (navigate: (view: NexusView) => void) => void;
}
```

`NexusScreen` is unchanged. The palette is an overlay, not a screen.

### 6.3 Additions to `src/lib/nexus-db.ts`

```typescript
export function searchWorkspace(query: string): Promise<SearchResults> {
  return invoke<SearchResults>('nexus_search_workspace', { query });
}
```

---

## 7. React Component Design

### 7.1 `src/lib/commands.ts`

**File:** `src/lib/commands.ts` (new)

Pure. No React import, no import from `nexus-db.ts`, mirroring `list-filters.ts`.

```typescript
/** Static navigation commands, always available. */
export function navigationCommands(): PaletteCommand[];

/**
 * Create commands. Task creation is expanded to one entry per project
 * (spec 2.3), so `projects` is required and an empty list yields only the
 * project-creation command (F-11).
 */
export function createCommands(projects: Project[]): PaletteCommand[];

/** Filters by label and keywords using matchesQuery from list-filters. */
export function filterCommands(
  commands: PaletteCommand[],
  normalized: string,
): PaletteCommand[];
```

`filterCommands` calls `matchesQuery(normalized, [command.label, ...command.keywords])`, reusing the NEXUS-007 rule rather than restating it (N-10).

### 7.2 `CommandPalette`

**File:** `src/components/CommandPalette/CommandPalette.tsx`

```typescript
interface CommandPaletteProps {
  open: boolean;
  onClose: () => void;
  navigate: (view: NexusView) => void;
}
```

State: `query`, `projects`, `results`, `truncated`, `selectedIndex`, `loading`, `error`.

Behaviour:

- On open, fetches `listProjects()` once to build the expanded create commands. Closing discards it; reopening refetches, so a project created since last open is present.
- Search runs on query change. Results and commands are combined into one flat, ordered array of selectable entries, with commands first, so `selectedIndex` addresses a single list and arrow keys cross the group boundary naturally.
- Arrow Up and Down move `selectedIndex` with wraparound (F-20). Enter runs the selected entry. Escape closes.
- Query changes reset `selectedIndex` to 0, so the highlight never points at a stale row.
- The selected row is scrolled into view with `scrollIntoView({ block: 'nearest' })`.
- Loading, error, and empty-result states render inside the palette without closing it (F-23, F-24).
- When `truncated` is true, a line states that more matches exist and the query should be narrowed (F-19).

Accessibility: the input carries `role="combobox"`, `aria-expanded`, and `aria-activedescendant` pointing at the selected row's id. The list carries `role="listbox"`, each row `role="option"` with `aria-selected`. The overlay carries `role="dialog"` and `aria-modal="true"`.

**Focus handling (F-03).** On open, the palette records `document.activeElement`, focuses its input, and on close returns focus to the recorded element if it is still in the document. A simple focus trap keeps Tab within the palette while open.

**The palette never calls a mutation command.** Selecting a create command navigates with an intent; the destination screen performs the work through its existing handlers. That keeps every write on exactly one path.

### 7.3 `AppShell` changes

Adds one state value and one effect:

```typescript
const [paletteOpen, setPaletteOpen] = useState(false);
```

A `useEffect` registers a `keydown` listener on `window` for Command-K (`event.metaKey && event.key === 'k'`), calling `preventDefault()` and toggling `paletteOpen`. Cleanup removes the listener. Escape is handled inside the palette, which owns focus while open.

`AppShell` renders `<CommandPalette open={paletteOpen} onClose={...} navigate={navigate} />` as the last child, and passes `onOpenPalette` to `CommandBar`.

`AppShell` gains no fetch and no import from `src/lib/nexus-db.ts` (N-08).

**Control-K is not bound.** On macOS, Control-K is the Emacs kill-line binding that text inputs honour, and NEXUS is macOS-first. Only Command-K is registered.

### 7.4 `CommandBar` changes

One change, permitted by N-12 and justified in 2.5: the input becomes a palette affordance rather than a free-text field nothing consumes.

```typescript
interface CommandBarProps {
  onOpenPalette: () => void;
}
```

The input becomes `readOnly`, and `onFocus` and `onClick` both call `onOpenPalette`. Its local `value` state is removed, since nothing consumed it. `CommandState.isListening`, the microphone button, `handleMicClick`, the `placeholder`, and every microphone-related label are **left exactly as they are**, pending the decision recorded in section 10.

### 7.5 Intent consumption

`Dashboard` forwards `view.intent` to `ProjectList` and `ProjectDetail`. `ProjectDetail` forwards it to `TaskList`, as it already forwards `settings`.

Each destination consumes it as a `useState` initializer:

```typescript
const [showCreateForm, setShowCreateForm] = useState(intent === 'create-project');
```

One-shot by construction: it seeds mount state and is never read again, so it cannot re-fire on a later render (2.6).

`Dashboard` must key `ProjectDetail` on `view.projectId` so that navigating from one project to another remounts it and the intent applies. Without that, a `'create-task'` intent aimed at a second project would be ignored because the component never unmounted.

### 7.6 Styling

New CSS files: `CommandPalette.css`, plus small additions to `CommandBar.css` for the affordance cursor. Any shared additions to `globals.css` are append-only.

Existing tokens only. No new tokens, no theme change. The overlay uses a scrim over `--color-bg-primary` at reduced opacity and the panel uses `--color-bg-secondary` with the established border and radius tokens.

---

## 8. Implementation Tasks

Rust tasks gate on `cargo check`. Test tasks gate on `cargo test --lib`. Frontend tasks gate on `npx tsc --noEmit`.

---

### T-01: Create `db/search.rs` with types and escaping

**Objective.** The result shapes and the `LIKE` escaping helper, before any query.

**Files.** `src-tauri/src/db/search.rs` (new).

**Dependencies.** None.

**Implementation details.** Declare `SearchResult` and `SearchResults` per 5.2. Add `SEARCH_RESULT_CAP: usize = 50`. Add a private `escape_like(query: &str) -> String` prefixing `\`, `%` and `_` with a backslash, in that order so the escape character is handled first.

**Acceptance criteria.** `escape_like("100%")` yields `100\%`. `escape_like("a_b")` yields `a\_b`. `escape_like("c\\d")` yields `c\\\\d`. Backslash is escaped before the wildcards.

**Tests.** Covered by `escape_like_is_applied` in T-04.

---

### T-02: Register the module

**Objective.** Make `search.rs` part of the crate.

**Files.** `src-tauri/src/db/mod.rs`.

**Dependencies.** T-01.

**Implementation details.** Add `pub mod search;` in alphabetical position.

**Acceptance criteria.** `cargo check` exits 0. Unused-item warnings expected until T-05.

**Tests.** None.

---

### T-03: Implement `search_workspace`

**Objective.** The four queries, concatenated, ordered and capped.

**Files.** `src-tauri/src/db/search.rs`.

**Dependencies.** T-02.

**Implementation details.** Return empty immediately for an empty or whitespace-only query, without querying. Four statements per 4.2 and 4.3, each using `LOWER(col) LIKE '%' || LOWER(?1) || '%' ESCAPE '\'` over the escaped query. Tasks join `projects` for `subtitle` and `projectId`. Order within each kind by lower(name/title) then id. Concatenate in kind order, then truncate to the cap and set `truncated`.

**Acceptance criteria.** An empty query performs no query and returns `truncated: false`. `external_id` appears in no `WHERE` clause. A null column never matches and never errors.

**Tests.** All fourteen from 5.6.

---

### T-04: Add the `#[cfg(test)]` module

**Objective.** Lock the search behaviour, escaping included.

**Files.** `src-tauri/src/db/search.rs`.

**Dependencies.** T-03.

**Implementation details.** All fourteen tests from 5.6. `search_caps_results_and_reports_truncation` seeds 60 matching projects.

**Acceptance criteria.** `cargo test --lib` passes with 76 pre-existing plus 14 new, zero failures.

**Tests.** As above.

---

### T-05: Add and register the command

**Objective.** Twenty-eight entries.

**Files.** `src-tauri/src/commands/mod.rs`, `src-tauri/src/lib.rs`.

**Dependencies.** T-03.

**Implementation details.** One `#[tauri::command]` in the existing lock-then-delegate shape. Append the registration; do not reorder existing entries.

**Acceptance criteria.** `sed -n '/invoke_handler/,/])/p' src-tauri/src/lib.rs | grep -c 'commands::'` returns 28. `cargo check` exits 0 with zero warnings.

**Tests.** IPC contract check.

---

### T-06: TypeScript types and wrapper

**Objective.** Mirror the Rust contract and add the intent field.

**Files.** `src/types/db.ts`, `src/types/index.ts`, `src/lib/nexus-db.ts`.

**Dependencies.** T-05.

**Implementation details.** Add the types of 6.1 and 6.2 and the wrapper of 6.3. `NexusView` gains `intent?: NexusIntent`; `NexusScreen` is unchanged.

**Acceptance criteria.** `npx tsc --noEmit` exits 0. Contract check reports 28 registered, 28 invoked, 28 defined, zero mismatches, and camelCase parity for `SearchResult` and `SearchResults`. Existing `NexusView` call sites still compile, since `intent` is optional.

**Tests.** IPC contract check.

---

### T-07: Build `src/lib/commands.ts`

**Objective.** The command registry, pure.

**Files.** `src/lib/commands.ts` (new).

**Dependencies.** T-06.

**Implementation details.** Per 7.1. Four navigation commands. `createCommands` returns the project-creation command always, and one task-creation command per project (2.3). `filterCommands` delegates to `matchesQuery`.

**Acceptance criteria.** `grep -E "react|nexus-db" src/lib/commands.ts` returns nothing. With an empty project list, `createCommands` returns exactly one command (F-11). Each task command's keywords include its project name.

**Tests.** None automated, no runner. Written pure so it is testable when one exists.

---

### T-08: Build `CommandPalette` shell and keyboard model

**Objective.** Overlay, input, selection, focus handling.

**Files.** `src/components/CommandPalette/CommandPalette.tsx`, `.css` (both new).

**Dependencies.** T-07.

**Implementation details.** Per 7.2, excluding search. Renders commands only. Arrow wraparound, Enter, Escape, focus capture and restore, focus trap, ARIA per 7.2.

**Acceptance criteria.** Arrow Down from the last entry selects the first. Escape closes without navigating. Focus returns to the previously focused element. Changing the query resets the selection to 0. The overlay carries `role="dialog"` and `aria-modal="true"`.

**Tests.** Manual scenarios 1 through 6.

---

### T-09: Add search to the palette

**Objective.** Wire `searchWorkspace` and the results group.

**Files.** `src/components/CommandPalette/CommandPalette.tsx`, `.css`.

**Dependencies.** T-08.

**Implementation details.** Per 7.2. Commands and results in one flat selectable array, commands first, each group separately labelled. Loading, error, empty and truncated states. Result selection navigates per F-15 to F-17.

**Acceptance criteria.** A task result shows its project name. Selecting it opens that project. A truncated result set states so. An empty query shows commands and no results group. A failing search shows an error and leaves the palette open.

**Tests.** Manual scenarios 7 through 12.

---

### T-10: Wire Command-K into `AppShell`

**Objective.** The global entry point.

**Files.** `src/components/AppShell/AppShell.tsx`, `src/components/AppShell/AppShell.css`.

**Dependencies.** T-09.

**Implementation details.** Per 7.3. `keydown` listener on `window`, cleaned up on unmount. Only Command-K. Palette rendered as the last child.

**Acceptance criteria.** `grep -n "nexus-db" src/components/AppShell/AppShell.tsx` returns nothing. Command-K opens the palette from every screen. The listener is removed on unmount. Control-K in a text field still behaves natively.

**Tests.** Manual scenarios 1 and 13.

---

### T-11: `CommandBar` affordance

**Objective.** Make the dead input open the palette.

**Files.** `src/components/CommandBar/CommandBar.tsx`, `.css`.

**Dependencies.** T-10.

**Implementation details.** Per 7.4. Input becomes `readOnly` with `onFocus` and `onClick` opening the palette. Remove the unused `value` state. **Do not touch** the microphone button, `handleMicClick`, `isListening`, the placeholder, or any microphone label.

**Acceptance criteria.** `git diff src/components/CommandBar/CommandBar.tsx` shows no change to the `<button>` element, `handleMicClick`, or the `placeholder` attribute. Clicking the input opens the palette. Typing into it directly is impossible.

**Tests.** Manual scenario 14.

---

### T-12: Intent consumption

**Objective.** Create commands land on an open form.

**Files.** `src/components/Dashboard/Dashboard.tsx`, `src/components/ProjectList/ProjectList.tsx`, `src/components/ProjectDetail/ProjectDetail.tsx`, `src/components/TaskList/TaskList.tsx`.

**Dependencies.** T-06, T-09.

**Implementation details.** Per 7.5. Consume as a `useState` initializer. Key `ProjectDetail` on `view.projectId`.

**Acceptance criteria.** "New project" opens Projects with the create form already open. "New task in UI-TEST-WORK" opens that project with its task form already open. Navigating from one project's create-task intent to another's opens the second form, proving the remount key works. Navigating without an intent opens no form.

**Tests.** Manual scenarios 15 through 17.

---

### T-13: Full verification

**Objective.** Prove the milestone.

**Files.** None.

**Dependencies.** All preceding tasks.

**Implementation details.** Run `cargo test --lib`, `pnpm build`, `pnpm tauri build`, the IPC contract check, and the structural greps of 9.1. Perform the manual checklist of 9.3.

**Acceptance criteria.** Section 9 in full.

**Tests.** The complete suite plus the manual checklist.

---

## 9. Acceptance Criteria

### 9.1 Build and structure

- [ ] `pnpm build` completes with zero TypeScript and zero Vite errors.
- [ ] `pnpm tauri build` produces `NEXUS.app` and the ARM64 DMG.
- [ ] `cargo test --lib` passes with the 76 pre-existing tests plus the 14 added, zero failures.
- [ ] `update_task_preserves_external_id_and_agent` passes unmodified.
- [ ] `git diff --stat package.json pnpm-lock.yaml src-tauri/Cargo.toml src-tauri/Cargo.lock` produces no output.
- [ ] `git diff --stat src-tauri/src/db/migrations.rs` produces no output, and `SELECT MAX(id) FROM _migrations` returns 1.
- [ ] `git diff --stat` shows no change to `db/projects.rs`, `db/tasks.rs`, `db/ides.rs`, `db/agents.rs`, `db/registry.rs`, `db/stats.rs`, `db/settings.rs`, or `main.rs`.
- [ ] `grep -n "nexus-db" src/components/AppShell/AppShell.tsx` returns nothing.
- [ ] `grep -rn "createContext\|useContext" src/` returns nothing.
- [ ] `grep -rl "@tauri-apps/api" src/` returns exactly `src/lib/nexus-db.ts`.
- [ ] `grep -E "react|nexus-db" src/lib/commands.ts` returns nothing.
- [ ] No raw SQL under `src/`.
- [ ] `external_id` appears in no write statement and in no search `WHERE` clause.
- [ ] Registered commands equals 28, invoked equals 28, defined equals 28, zero mismatches.
- [ ] `SearchResult` and `SearchResults` camelCase field lists match their TypeScript interfaces exactly.
- [ ] `NexusScreen` still has exactly five values.
- [ ] **No network API is referenced anywhere**: `grep -rniE "fetch\(|XMLHttpRequest|WebSocket|axios|https?://" src/` returns only the repository-URL placeholder strings.
- [ ] **No speech API is referenced anywhere**: `grep -rniE "SpeechRecognition|getUserMedia|MediaRecorder|speechSynthesis" src/ src-tauri/src/` returns nothing.
- [ ] `git diff src/components/CommandBar/CommandBar.tsx` shows no change to the microphone button, `handleMicClick`, or the placeholder text.

### 9.2 Behaviour

- [ ] Command-K opens the palette from all five screens.
- [ ] Escape closes it without navigating; the previously focused element regains focus.
- [ ] Clicking the scrim closes it.
- [ ] Opening the palette does not change the current screen.
- [ ] An empty query lists navigation and create commands, no results group.
- [ ] Typing filters commands case-insensitively by label and keywords.
- [ ] With zero projects, no task-creation command is offered.
- [ ] Arrow keys move the selection and wrap at both ends.
- [ ] The selected row is visible without manual scrolling.
- [ ] Enter performs the selected entry and closes the palette.
- [ ] Search returns hits from all four entity kinds, grouped and labelled by kind.
- [ ] A task result shows its owning project name.
- [ ] Selecting a project or task result opens the right project detail.
- [ ] Selecting an IDE or agent result opens Registry.
- [ ] A query matching nothing shows an explicit empty state.
- [ ] A result set at the cap states that more matches exist.
- [ ] A query containing `%` or `_` matches those characters literally.
- [ ] "New project" arrives on Projects with the create form open.
- [ ] "New task in X" arrives on X with the task form open.

### 9.3 Manual UI verification

Every scenario is manual; there is no frontend test runner.

1. Press Command-K on each of the five screens. The palette opens each time and the screen behind is unchanged.
2. Press Escape. The palette closes and focus returns to where it was.
3. Click the scrim. The palette closes.
4. Open the palette and press Arrow Down past the last entry. Selection wraps to the first. Arrow Up from the first wraps to the last.
5. Type a query, then clear it. The selection returns to the first entry each time the query changes.
6. Press Tab repeatedly while open. Focus stays inside the palette.
7. Type part of a project name. Both a create command for that project and a project search result appear, in separate labelled groups.
8. Select a task result. The owning project's detail screen opens.
9. Select an IDE result. The Registry screen opens.
10. Type a string matching nothing. An explicit empty state appears, not a blank panel.
11. Seed more than 50 matching rows through `sqlite3`, then search. Exactly 50 appear and the panel says more exist.
12. Create a project named `snake_case` and another named `snakeXcase`. Search `snake_case`. Only the first matches.
13. Focus a text input, press Control-K. Native behaviour, no palette.
14. Click the CommandBar input. The palette opens. The microphone button and its tooltip are unchanged.
15. Run "New project". Projects opens with the create form already expanded.
16. Run "New task in UI-TEST-WORK". That project opens with its task form expanded.
17. From that project, run "New task in UI-TEST-ZULU". The second project opens with its form expanded, proving the remount key.
18. Quit and relaunch. No palette state persists; nothing was written to the database.

---

## 10. Explicitly Out of Scope

Deferred deliberately:

- **Voice input of any kind.** See NEXUS-010, which is gated and not to be implemented.
- **AI, LLM, model, inference, or assistant behaviour.** The palette matches strings against a static registry. Nothing is generated.
- **Any network request.** Not one.
- **The microphone button, its handler, its labels, and the `"Ask Nexus anything..."` placeholder.** Left exactly as they are. **An explicit decision is pending** after NEXUS-004 to NEXUS-008 manual validation completes, between: leaving the microphone placeholder as-is, disabling it visibly, or replacing the placeholder with a Command Palette hint. Do not pre-empt it in this milestone.
- **A command grammar, argument parsing, quoting, or parameter autocomplete.** See 2.3.
- **Fuzzy matching, typo tolerance, or relevance scoring.** Substring and a fixed kind order.
- **Command history, recent commands, or favourites.** Nothing is persisted.
- **Custom or user-configurable keybindings.** Command-K only.
- **Commands that mutate directly.** Every write still goes through an existing screen handler.
- **Commands for editing, deleting, assigning, or toggling.** Navigate and create only.
- **Searching `external_id`.** No producer writes it.
- **FTS5 or any search index.** See 4.4.
- **Result previews, inline editing, or multi-step palette flows.**
- **A frontend test framework.**

Also out of scope, per the standing NEXUS constraints:

- Jira, Claude, PlayerZero, Cursor, Grok, and ChatGPT integrations
- AI orchestration or execution of any kind
- IDE launching, terminal execution, browser automation
- Text to speech
- News, weather, morning briefings, notifications
- Authentication, cloud sync, CI/CD, auto-update
- Custom title bar or window chrome
- Any routing library, state manager, UI library, form library, or ORM
- Any new Rust or frontend dependency
