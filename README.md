# NEXUS

A local-first, macOS-native personal developer command center. It is a Tauri 2
desktop application: a React/TypeScript window on top of a Rust core, with
SQLite on disk and no backend, no account, and no cloud service behind it.

NEXUS holds your projects, tasks, editors and agents in one place, and it can
act on your behalf across the applications you already use: Chrome, your IDE,
GitHub, Jira, Teams, Outlook, WhatsApp, macOS itself. It listens and speaks on
device. It can consult a reasoning model, locally or in the cloud, but a model
is a resource it asks, never the thing that decides what runs.

The single fact that shapes the whole codebase:

> **Every action goes through one function, `assistant::execute_action`.**
> Voice, the command palette, a dashboard button, a proactive suggestion and a
> model's proposed plan all converge there. There is no second path. That is
> what makes "voice cannot bypass permissions" a structural property rather
> than a promise.

---

## 1. Current state

|             |                                                                      |
| ----------- | -------------------------------------------------------------------- |
| Platform    | macOS only (Apple frameworks via `objc2`, AppleScript, Keychain)     |
| Shell       | Tauri 2.11, Rust edition 2021                                        |
| Frontend    | React 18, TypeScript 5, Vite 6, `lucide-react` icons, pnpm           |
| Storage     | SQLite (`rusqlite`, bundled), 13 tables, forward-only migrations     |
| IPC surface | 76 Tauri commands                                                    |
| Connectors  | 11                                                                   |
| Actions     | 69                                                                   |
| Tests       | 762 Rust unit tests, 761 passing, 1 ignored (`cargo test`, verified) |
| Branch      | `feat/assistant-core-012-022b`                                       |

Milestones NEXUS-001 through NEXUS-028 are implemented. Built and verified are
not the same thing: several connectors are written, typed and tested but have
never made a live call because the grant or the credential does not exist on
this machine. Section 5 says which is which for each one.

---

## 2. Architecture

### 2.1 Layers

```
+---------------------------------------------------------------+
| React window (src/)                                        |
| AppShell, CommandPalette, VoiceController, AssistantPanel, |
| PermissionsPanel, ApprovalPrompt, NotificationsPanel ...   |
| Imports no business logic. Renders state, sends intent.    |
+-------------------------------+-------------------------------+
                                | Tauri IPC (76 commands)
+-------------------------------v-------------------------------+
| commands/mod.rs      the only IPC boundary |
+-------------------------------+-------------------------------+
                                |
+-------------------------------v-------------------------------+
| assistant/  THE ASSISTANT CORE                                |
|                                                               |
| converse.rs   escalation ladder: local answer -> known        |
| command -> parametric phrase -> matcher ->                    |
| reasoning provider -> honest decline                          |
| session.rs    what NEXUS is doing, in memory, gone on restart |
| referent.rs   "the PR", "the first one", "him"                |
| context.rs    per-request, budgeted, never an ambient blob    |
| reasoning.rs  provider trait + plan validator + AI audit      |
| suggestions/  what is worth saying                            |
| proactive.rs  whether now is the time to say it               |
|                                                               |
| ============ mod.rs :: execute_action ============            |
| THE GATE. identity -> enabled -> grant -> confirmation ->     |
| audit row -> dispatch -> audit close -> observe/follow-up     |
| =================================================             |
+-------------------------------+-------------------------------+
                                |
+-------------------------------v-------------------------------+
| Connectors (trait objects, one line each in `connectors()`) |
| nexus  browser  ide  github  jira  teams  whatsapp          |
| outlook  weather  system  notifications                     |
+-------------------------------+-------------------------------+
                                |
+-------------------------------v-------------------------------+
| Transports                                                   |
| db/ (SQLite)   shell.rs (fixed argv, hard deadline)          |
| http.rs (curl, https only, creds via stdin)                  |
| voice/ (SFSpeechRecognizer + AVSpeechSynthesizer, on device) |
| msgraph.rs (device code OAuth)   Keychain (secrets)          |
+---------------------------------------------------------------+
```

### 2.2 The gate

`assistant::execute_action` is seven steps, in this order, and nothing skips
one:

1. **Identity.** An unknown action id is rejected outright. NEXUS never picks a
   nearest match, because a plausible wrong action is worse than none.
2. **Connector enabled.** A connector the user switched off refuses.
3. **Standing grant.** The connector must hold the action's permission level.
   A missing row is a denial; there is no tri-state.
4. **Per-invocation confirmation.** Required if the action's own policy says
   `Always`, or if its level is Write or above. A spec can tighten the rule and
   can never loosen it, and a test enforces that no registered action tries.
5. **Audit row opened before dispatch,** so an action that hangs or panics
   still leaves a trace.
6. **Dispatch.** The connector deserialises the JSON into its own typed input
   struct with `deny_unknown_fields`. That deserialisation is the validation.
7. **Audit row closed** either way, with the reason on failure. On success the
   connector may register referents ("the PR"), describe the result for
   speaking aloud, and offer a follow-up. On failure it may offer a remedy,
   which is itself an ordinary action through the gate.

### 2.3 Connectors

A connector answers *how* a specific application does something. The Assistant
Core never names one. The trait requires `actions()` (static), `capabilities()`
and `status()` (both evaluated at runtime, because an IDE may not be installed
and a CLI may not be authenticated), `summarize()` (renders the approval
sentence before the user is asked, so the prompt says "Delete project Atlas"
rather than "Delete project 41"), and `dispatch()`.

Optional hooks: `observe`, `describe_result`, `follow_up`, `remedy`,
`validate_input`, `zero_input_actions`.

Adding a connector is one line in `connectors()`. It arrives with **no grants
at all** and is inert until the user allows it in Settings.

### 2.4 From a sentence to an action

`converse::resolve` is a deterministic ladder. Each rung either matches exactly
or declines. No model is reachable from any of them.

| Rung  | What it handles                                                                                                                                                                                                                           |
| ----- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 0     | An answer to a question NEXUS just asked ("yes", "haan bhej do"). Reached only while an offer is live, which is what makes the permissive vocabulary safe. Declines are checked before affirmations, so "no, don't send it" is a refusal. |
| 1     | Dictation into an editor: a verb plus "claude". First, because a dictated sentence is arbitrary text and every later rung would find something in it.                                                                                     |
| 1b-1d | "remind me to X in twenty minutes", "what do I owe", "any new messages".                                                                                                                                                                  |
| 2-4   | Time and date, what NEXUS can do here, sign-offs, greetings. All local, all instant.                                                                                                                                                      |
| 5     | Local data. A question a query answers never travels.                                                                                                                                                                                     |
| 6-6a  | "Open Slack", a connector named on its own.                                                                                                                                                                                               |
| 7     | Parametric phrases: a named project in a named editor, a URL, a search. Resolved against the registry and the database, never a hardcoded list.                                                                                           |
| 8     | The NEXUS-010 voice matcher, reused verbatim over the command registry.                                                                                                                                                                   |
| 9     | `escalate`: a reasoning provider, if one is available and allowed.                                                                                                                                                                        |
| -     | Otherwise an honest decline, flagged `understood: true/false` so a fragment of overheard speech is dropped silently and a real refusal is spoken.                                                                                         |

### 2.5 The reasoning layer

Three rules give "a provider is not the security boundary" teeth:

1. A provider returns an **answer or a plan, never an effect**. Separate
   variants of separate types, so prose cannot become executable by containing
   action-shaped words.
2. A plan is **validated against the action registry before the user sees it**.
   Unknown action id: rejected. Input that does not deserialise into that
   action's struct: rejected. NEXUS does not improvise a near match.
3. Every validated step **still goes through the gate**, with its own
   permission, confirmation and audit row.

Providers, in preference order: Ollama on `127.0.0.1` first (nothing leaves the
machine, so the privacy question does not arise), then any configured cloud
provider. Malformed or inventive model output degrades to an answer, never to a
plan.

### 2.6 Speaking first

A Rust thread started in `lib.rs` polls every 8 seconds and emits an event.
Deliberately not a `useEffect` in the frontend: NEXUS speaking first cannot
depend on a component being mounted or a panel being open. The database lock is
taken and released inside each tick, never held across the sleep.

`proactive.rs` keeps cooldown, fatigue backoff, a cap of six and silence as a
valid answer. Message announcements override it at an aggression level held in
settings (`immediate` by default, with `batched` and `silent` already built).
`calendar.rs` keeps NEXUS quiet during meetings and fails **open**: with Outlook
unreachable it announces anyway, because one expired token should not silently
mute the assistant.

### 2.7 Persistence

SQLite at the Tauri app data directory, forward-only numbered migrations in
`db/migrations.rs`. Tables: `projects`, `tasks`, `ides`, `ai_agents`,
`settings`, `contacts`, `commitments`, `connectors`, `permission_grants`,
`action_audit`, `ai_audit`, `suggestion_dismissals`, `suggestion_activity`.

Deliberately **not** persisted: the conversation, pending approvals, speech
transcripts, notification previews, message bodies. A conversation surviving a
restart would be a transcript on disk, and a persisted approval queue would let
something approved last night fire this morning.

---

## 3. The permission model

### 3.1 Two things that are easy to conflate

- A **grant** is standing, per connector, per level. "May NEXUS ever do this
  kind of thing with this service?" Lives in `permission_grants`. A row is a
  yes; absence is a no; revoking is a `DELETE`, so a half-written table cannot
  produce an accidental yes.
- A **confirmation** is per invocation and expires in 5 minutes. "May NEXUS do
  this specific thing, right now?" Lives in memory in `approval.rs`, capped at
  32 pending, never written to disk.

A token binds to one action id **and** the exact input it was shown for. Editing
the input in the confirmation UI invalidates the token, so "approve one thing,
perform another" is not reachable.

### 3.2 The five levels

| Level         | Meaning                                              | Always confirms |
| ------------- | ---------------------------------------------------- | --------------- |
| `read`        | Observe without changing anything                    | no              |
| `interact`    | Move things on screen: open, focus, navigate, search | no              |
| `write`       | Create or modify data somewhere                      | **yes**         |
| `execute`     | Run a command, a build, or injected code             | **yes**         |
| `destructive` | Remove or irreversibly change something              | **yes**         |

Each action also declares `reach` (`LocalOnly` or `LeavesMachine`) and
`reversible`, both shown in the approval prompt, because "this cannot be undone"
changes the answer.

### 3.3 What is granted by default

Only the `nexus` connector, and only `read`, `interact`, `write`,
`destructive`. It acts on the user's own workspace inside the application they
opened, so it is authorised on arrival; the grants exist to be revocable, not to
be earned. `execute` is absent because no `nexus.*` action runs a command.

**Every other connector arrives with nothing.** Chrome, the IDE, GitHub, Jira,
Teams, WhatsApp, Outlook, the weather and notifications all do nothing at all
until the user allows them in Settings, per level.

### 3.3a What is actually granted on this machine

Read from `permission_grants` rather than described, because a document that
says what *should* be granted and a database that says what *is* granted will
disagree eventually, and the database wins.

| Connector       | Granted                                   | Missing                    |
| --------------- | ----------------------------------------- | -------------------------- |
| `nexus`         | read, interact, write, destructive        | (uses no `execute`)        |
| `browser`       | read, interact, write, execute            | complete                   |
| `ide`           | read, interact, write, execute            | complete                   |
| `teams`         | read, interact, write                     | complete                   |
| `outlook`       | read, interact, write                     | complete                   |
| `whatsapp`      | read, write                               | complete                   |
| `notifications` | read                                      | complete (read only by design) |
| `github`        | read, interact                            | (defines no write)         |
| `jira`          | read, interact                            | **write**, so transitions are refused |
| `system`        | read, interact                            | complete                   |
| `weather`       | read                                      | complete                   |

macOS side: **Full Disk Access and Accessibility are both granted.** Full Disk
Access follows the process that launched the binary, so in development it is
the terminal that must hold it, not `target/debug/nexus`. Granting the binary
directly does nothing and looks correct while doing nothing.

What that leaves blocked, and why none of it is a code defect:

| Not working                        | Reason                                                                                              |
| ---------------------------------- | --------------------------------------------------------------------------------------------------- |
| Mail, calendar, meeting warnings   | Microsoft Entra admin consent for `Mail.Read`, `Calendars.Read`, `Mail.Send` is submitted, not granted |
| Reading Teams chat history         | `Chat.Read` is admin-only in every tenant, and is not part of the pending request                     |
| Moving a Jira issue                | `jira` has no `write` grant                                                                          |
| Reading WhatsApp conversations     | No mechanism exists. See section 6.                                                                  |

### 3.4 macOS permissions NEXUS asks the operating system for

| Grant                                                      | Needed for                                              | State                                                                                                                                                                                              |
| ---------------------------------------------------------- | ------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Microphone (`NSMicrophoneUsageDescription`)                | Voice input                                             | Declared in `Info.plist`. Voice is off by default.                                                                                                                                                 |
| Speech Recognition (`NSSpeechRecognitionUsageDescription`) | Transcription                                           | Declared. `requiresOnDeviceRecognition = true` unconditionally, and the session refuses to start if on-device recognition is unsupported. No remote fallback exists.                               |
| Automation (per target app)                                | AppleScript against Chrome, WhatsApp, editors           | Prompted by macOS on first use. `shell.rs` enforces a hard deadline so the prompt cannot freeze the app.                                                                                           |
| Accessibility                                              | Keystrokes for `ide.type_prompt`, `whatsapp.press_send` | Granted by hand in System Settings. Connectors name the exact pane on failure and can offer to open it.                                                                                            |
| Full Disk Access                                           | Reading the macOS notification store                    | Granted by hand. Not scoped: it grants Messages, Mail, Safari history and every document. Accepted knowingly. The opt-in application list in section 3.5 is the boundary that makes it defensible. |

Chrome's "Allow JavaScript from Apple Events" is a separate manual toggle inside
Chrome, required only for the `Execute`-level browser actions.

### 3.5 Privacy switches, all off by default

| Switch                       | Effect when off                                                                                                               |
| ---------------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| External reasoning allowed   | Any `LeavesMachine` provider refuses to be consulted. The local Ollama provider is exempt because nothing leaves the machine. |
| Content sharing allowed      | Message bodies, file contents and page text are stripped from any context assembled for a provider.                           |
| Notification app opt-in list | **Empty.** Empty means NEXUS reads no notifications at all, even with Full Disk Access granted.                               |
| `voice_enabled`              | The microphone never starts. Re-read in Rust before touching the microphone, not only in the UI.                              |
| `always_listening`           | The microphone opens only when asked, not continuously for a wake word.                                                       |

### 3.6 What travels to a reasoning provider

Built by subtraction in `reasoning::build_context`, and budgeted: the request
(max 2000 chars), up to 40 action ids and one-line summaries, up to 12 factual
workspace lines ("Project Atlas, 3 open, 1 blocked"), up to 8 lines of what
NEXUS said earlier. Categories travel with the context and are recorded.

Credentials never travel. Message bodies, file contents and page text require
the separate content grant. Every provider call writes an `ai_audit` row with
provider, model, reach, purpose, **categories** and outcome, so "why did NEXUS
contact an external service" has an answer that is not a guess.

### 3.7 Credentials

API keys and refresh tokens live in the **macOS Keychain**, never in
`nexus.db`, which is a plain file. `set_connector_config` refuses any key that
looks like a secret. Credentials never reach `argv`: `http.rs` writes them to
curl's stdin as a config file, because every process on the machine can read
another's command line. `argv` is always exactly `--config -` plus fixed flags,
and a test asserts it. Only `https://` is accepted.

GitHub is the exception worth naming: NEXUS never holds a GitHub token at all.
The `gh` CLI holds it, and NEXUS asks `gh` questions.

### 3.8 The audit trail

`action_audit` answers "what did NEXUS do on my behalf", not "what did I read".
It stores the rendered summary the user already saw, never raw input and never
anything NEXUS merely observed. Rows are written before dispatch and closed
after. Refusals are written too: "NEXUS was asked to do this and would not" is
exactly what an audit trail is for. Outcomes: `attempted`, `succeeded`,
`failed`, `refused`. A row stuck at `attempted` means NEXUS never came back,
which is itself worth seeing.

---

## 4. What NEXUS can do without any connector

These need no grant beyond the seeded `nexus` one.

- **Workspace.** Projects, tasks, IDE and agent registry, assignment, overview
  counts, recent activity, full-text search, list filter/sort/persist.
- **Command palette and command bar.** Registry in `src/lib/commands.ts`, the
  single source of truth, shared by the palette and the voice matcher.
- **Voice, on device.** Wake word, transcription via `SFSpeechRecognizer`,
  spoken replies via `AVSpeechSynthesizer`. Audio never touches the filesystem;
  transcripts are emitted to the frontend and held nowhere. Response text is a
  `match` statement keyed by outcome, never by transcript, so a phrase the
  recogniser heard has no channel to reach the speaker.
- **Conversation.** Session state, referents ("open the PR", "do the first
  one"), follow-up offers with a TTL, ordinals over lists actually rendered.
- **Suggestions.** Rules over data NEXUS already holds: blocked tasks, tasks
  that have not moved in 14 days, projects with no repository path, projects
  with a repo worth checking for pull requests, registered editors whose
  executable has gone. Every suggestion carries a structured reason, nothing
  executes, and only dismissals persist.
- **Commitments.** "Remind me to call the dentist in twenty minutes." Only an
  explicit phrase is recorded; nothing is inferred from conversation. A
  commitment with no time says plainly that it will not remind you.
- **Briefing.** Today's meetings, unread mail, blocked tasks, PRs awaiting
  review, degrading to what it could reach.

---

## 5. What NEXUS can do with each connector

69 actions across 11 connectors. `R` = read, `I` = interact, `W` = write,
`E` = execute, `D` = destructive. Every `W`, `E` and `D` action is confirmed per
invocation, without exception.

### nexus (13 actions, local, seeded)

`open_overview` `open_projects` `open_registry` `open_settings` `open_project`
`new_project` `new_task` are all `I`, unconfirmed, and do not navigate: they
return a directive the shell applies, which is what lets a navigation be gated
and audited like anything else.

| Action                    | Lvl | Note                                  |
| ------------------------- | --- | ------------------------------------- |
| `nexus.remember`          | W   | Record a commitment with a due time   |
| `nexus.list_commitments`  | R   |                                       |
| `nexus.settle_commitment` | W   | Done, deferred or dropped             |
| `nexus.create_task`       | W   |                                       |
| `nexus.delete_task`       | D   | Not reversible                        |
| `nexus.delete_project`    | D   | Deletes its tasks too. Not reversible |

### browser (11 actions, Chrome)

Three tiers with deliberately different risk. Tier 1 is `/usr/bin/open` and
costs nothing. Tier 2 is AppleScript and costs a one-time macOS Automation
prompt. Tier 3 is `execute javascript` and needs a manual Chrome toggle, which
is why it is `Execute`.

| Action                                                                 | Lvl | Tier              |
| ---------------------------------------------------------------------- | --- | ----------------- |
| `browser.open_url` `browser.search`                                    | I   | 1                 |
| `browser.list_tabs`                                                    | R   | 2                 |
| `browser.activate_tab` `browser.focus_tab` `browser.navigate`          | I   | 2                 |
| `browser.close_tab`                                                    | W   | 2, not reversible |
| `browser.read_page` `browser.click` `browser.type` `browser.type_here` | E   | 3                 |

The DevTools Protocol is deliberately not used: `--remote-debugging-port` hands
any local process full control of the browser with no authentication. Every
argument reaches AppleScript as a value, never as script text.

### ide (8 actions)

The registry is the source of truth. NEXUS reads `ides.executable_path` and
launches whatever is there; the core never learns that IntelliJ exists.

| Action                                         | Lvl | Note                                                                   |
| ---------------------------------------------- | --- | ---------------------------------------------------------------------- |
| `ide.discover`                                 | R   | What is installed on this Mac worth registering                        |
| `ide.list` `ide.status`                        | R   |                                                                        |
| `ide.open_project` `ide.open_file` `ide.focus` | I   |                                                                        |
| `ide.type_prompt`                              | W   | Types a dictated sentence into Claude's prompt box and stops           |
| `ide.submit_prompt`                            | E   | Presses Return. Separate action, separate confirmation, not reversible |

There is no `ide.run_task` and there is no shell anywhere. Typing and submitting
are two actions so a misheard sentence is visible on screen before anything acts
on it. Where the Claude Code extension is missing, dictation is refused rather
than attempted, because typing into an open command palette and then confirming
Return would run whatever it matched.

### github (5 actions, via the `gh` CLI)

`github.status` `github.list_prs` `github.read_pr` `github.read_pr_comments`
are `R`; `github.open_pr` is `I`. Fixed argument vectors with `--json`, parsed
into typed structs. No `gh api` with a caller-supplied path. The repo comes from
`projects.repository_url`, so "the PR for this project" is a join, not a guess.

**Works today** where `gh` is installed and authenticated.

### jira (8 actions, REST API v3)

`jira.status` `jira.read_issue` `jira.read_comments` `jira.search`
`jira.find_for_task` `jira.list_transitions` are `R`; `jira.open_issue` is `I`;
`jira.transition_issue` is `W`, confirmed, and not reversible because plenty of
Jira workflows are one-way.

Needs a site and account in `connectors.config_json` and an API token in the
Keychain. Issue keys are validated against a strict shape before reaching a URL.
NEXUS reads the legal transitions first and posts an id it was handed, never a
status string it composed. Creating and commenting are deliberately absent:
both carry an ADF document body, and a connector writing half-formed ADF into
somebody's tracker is worse than one that does not write.

### teams (6 actions, two halves)

**Works today, no authorisation:** `teams.status` (`R`), `teams.open_chat`
(`I`), `teams.compose_message` (`W`, confirmed). The `msteams:` deep link opens
a chat with the message already in the compose box. The user presses send. NEXUS
never sends.

**Blocked on an organisation, not on code:** `teams.list_chats`,
`teams.read_messages` (`R`), `teams.send_message` (`W`). These need Microsoft
Graph, which needs `Chat.Read` / `Chat.ReadWrite`, which are not user-consentable
in a managed tenant. The Graph code path is written and typed and **has never
made a request**. Every one of those actions reports itself unavailable until
configuration and a token exist.

### outlook (7 actions, Microsoft Graph, device code sign-in)

| Action                                     | Lvl | Reach                                                    |
| ------------------------------------------ | --- | -------------------------------------------------------- |
| `outlook.status`                           | R   | local                                                    |
| `outlook.sign_in` `outlook.finish_sign_in` | I   | leaves machine                                           |
| `outlook.sign_out`                         | I   | local                                                    |
| `outlook.unread_mail`                      | R   | leaves machine                                           |
| `outlook.today_schedule`                   | R   | leaves machine                                           |
| `outlook.send_mail`                        | W   | confirmed, shows recipient and full body, not reversible |

Built before Teams deliberately: `Mail.Read`, `Calendars.Read` and `Mail.Send`
are user-consentable in a default tenant. Device code flow rather than a
localhost redirect, so NEXUS never listens on a port. The refresh token goes to
the Keychain over stdin; the access token is held in memory only. Message bodies
are never requested: `$select` asks for subjects, senders and times.

**Built, not verified.** The live read runs and fails correctly with "Not
signed in to Microsoft".

### whatsapp (3 actions)

`whatsapp.status` (`R`), `whatsapp.compose_message` (`W`, confirmed),
`whatsapp.press_send` (`W`, confirmed, needs Accessibility).

There is no personal-account API and this connector does not pretend otherwise.
The Business Platform does not reach personal chats; library automation of the
web client violates the terms and risks the number being banned. What is
supported is the `whatsapp://send` URL scheme: NEXUS drafts, the user approves
the wording, WhatsApp opens with it in the box.

**Reading incoming messages is not supported and is not deferred.** No action
claims it, because there is no supported mechanism to implement later.

Name-to-number lookup reads WhatsApp's own contact store read-only, returns
only the closest few matches, and never writes a number to NEXUS's database, the
audit log or a file. There is no call that returns the address book.

### notifications (3 actions, all `R`, all `LocalOnly`)

`notifications.status` `notifications.recent` `notifications.read_aloud`.

Reads the macOS notification store, which is what every application told the
system in order to draw a banner. One source for WhatsApp, Teams, Slack,
Calendar and everything else, without automating any of them.

- Read only. No action dismisses, clears or acts on a notification. The store is
  opened read-only.
- Requires Full Disk Access, and reports `Unavailable` **with the reason** when
  it is missing. "No messages" and "not allowed to look" never render the same.
- Only opted-in applications are read. **The default set is empty.**
- Nothing is persisted beyond a cursor. Previews are never written to the
  database, never logged, and never sent to a reasoning provider.

**Built, read unverified.** Decoding is tested against real plist fixtures; the
store read needs the grant. In development the grant must go to the dev binary,
not the bundled app, and a denied read looks identical to an empty table.

### system (3 actions)

`system.list_apps` (`R`), `system.open_app` (`I`), `system.open_settings_pane`
(`I`).

NEXUS only launches something it has already found on disk: a name is matched
against installed applications and the *discovered path* is executed. A name
that matches nothing is refused. There is no path by which a caller-supplied
string reaches the system as a command, which is why this is `Interact` and not
`Execute`.

### weather (2 actions)

`weather.current` (`R`, `LeavesMachine`), `weather.status` (`R`, local). Uses
`wttr.in`, no key, no account. With no city configured it infers location from
the request IP, and `weather.status` says so rather than leaving it to be
discovered. Deliberately not folded into the greeting, so a greeting keeps
working with the network off.

---

## 6. What NEXUS deliberately does not do

Stated so none of it reads as oversight.

- No arbitrary command execution. No `ide.run_task`, no action anywhere that
  takes a command line, and no shell: `shell.rs` builds fixed argument vectors
  so nothing a user or a model supplies can become syntax.
- No sending on WhatsApp or Teams. It drafts and hands over.
- No reading WhatsApp conversation history. Only what was posted as a
  notification while NEXUS was watching.
- No Jira create or comment.
- No OS-level notifications. NEXUS speaks and shows things in its own window.
- No message contents to a reasoning provider. A preview is spoken locally or
  not at all.
- No transcript, conversation or preview on disk.
- No remote speech fallback, no cloud sync, no accounts, no auto-update, no CI.
- No coordinate-based UI automation, and no Chrome DevTools Protocol.

---

## 7. Where things live

| Path                                                                           | What                                                                                  |
| ------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------- |
| [src-tauri/src/lib.rs](src-tauri/src/lib.rs)                                   | Tauri setup, managed state, the notification watcher thread, the command handler list |
| [src-tauri/src/commands/mod.rs](src-tauri/src/commands/mod.rs)                 | Every IPC command, and nothing else                                                   |
| [src-tauri/src/assistant/mod.rs](src-tauri/src/assistant/mod.rs)               | The connector registry and `execute_action`, the gate                                 |
| [src-tauri/src/assistant/permission.rs](src-tauri/src/assistant/permission.rs) | Levels, reach, confirm policy, standing grants                                        |
| [src-tauri/src/assistant/approval.rs](src-tauri/src/assistant/approval.rs)     | Per-invocation approvals, in memory, TTL 5 minutes                                    |
| [src-tauri/src/assistant/audit.rs](src-tauri/src/assistant/audit.rs)           | The action audit trail                                                                |
| [src-tauri/src/assistant/action.rs](src-tauri/src/assistant/action.rs)         | `ActionSpec`, `ActionRequest`, `ActionError`                                          |
| [src-tauri/src/assistant/connector.rs](src-tauri/src/assistant/connector.rs)   | The `Connector` trait                                                                 |
| [src-tauri/src/assistant/converse.rs](src-tauri/src/assistant/converse.rs)     | The escalation ladder                                                                 |
| [src-tauri/src/assistant/reasoning.rs](src-tauri/src/assistant/reasoning.rs)   | Provider trait, context builder, plan validator, AI audit                             |
| [src-tauri/src/voice/](src-tauri/src/voice/)                                   | On-device recognition, wake word, synthesis, response templates                       |
| [src-tauri/src/db/migrations.rs](src-tauri/src/db/migrations.rs)               | Forward-only schema                                                                   |
| [src/lib/commands.ts](src/lib/commands.ts)                                     | The command registry, single source of truth for palette and voice                    |
| [src/components/](src/components/)                                             | The window. Imports no IPC in `AppShell` by design                                    |
| [.kiro/specs/](.kiro/specs/)                                                   | Milestone specifications, NEXUS-001 through NEXUS-028                                 |

---

## 8. Running it

```bash
pnpm install
pnpm tauri dev          # dev build
pnpm tauri build        # bundle
cd src-tauri && cargo test
```

Grants are made in the application's Settings screen, per connector, per level.
Operating system grants are made in System Settings: Privacy & Security, under
Microphone, Speech Recognition, Accessibility, Automation and Full Disk Access.
NEXUS will not write a grant row on your behalf, because that would bypass the
gate the whole application is built around.
