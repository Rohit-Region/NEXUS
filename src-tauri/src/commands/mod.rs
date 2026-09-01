use serde::{Deserialize, Serialize};
use tauri::{Emitter, State};

use crate::assistant::action::{ActionError, ActionOutcome, ActionRequest};
use crate::assistant::approval::ApprovalStore;
use crate::assistant::audit::{list_recent as list_audit, AuditEntry};
use crate::assistant::context::{assemble, AssistantContext};
use crate::assistant::converse::{respond_to, user_name, AssistantReply};
use crate::assistant::permission::{set_grant, Permission};
use crate::assistant::proactive::{
    briefing, preview, read_policy as read_proactive, record_accepted, set_policy as set_proactive,
    surface, Briefing, ProactivePolicy,
};
use crate::assistant::reasoning::{
    list_recent_use, read_policy, set_policy, AiAuditEntry, PrivacyPolicy,
};
use crate::assistant::referent::Resolution;
use crate::assistant::session::{AssistantSession, AssistantState, SessionSnapshot, TurnInput};
use crate::assistant::suggestions::{dismiss, generate, restore, Suggestion};
use crate::assistant::calendar;
use crate::assistant::notification_connector::{self, Arrival};
use crate::assistant::{
    execute_action, list_connectors, set_connector_config, set_connector_enabled, ConnectorView,
    EVENT_ASSISTANT_STATE,
};
use crate::db::{
    self,
    agents::{delete_agent, insert_agent, list_agents, update_agent},
    ides::{delete_ide, insert_ide, list_ides, update_ide},
    projects::{
        count_all_tables, insert_project, list_projects, update_project, CreateProjectInput,
        Project, UpdateProjectInput,
    },
    registry::{CreateRegistryEntryInput, RegistryEntry, UpdateRegistryEntryInput},
    search::{search_workspace, SearchResults},
    settings::{get_settings, reset_settings, update_settings, Settings},
    stats::{
        count_tasks_by_agent, count_tasks_by_project, list_recent_tasks, workspace_summary,
        AgentTaskCounts, ProjectTaskCounts, TaskWithProject, WorkspaceSummary,
    },
    tasks::{
        assign_task_agent, insert_task, list_tasks, update_task, update_task_status,
        AssignTaskAgentInput, CreateTaskInput, Task, UpdateTaskInput, UpdateTaskStatusInput,
    },
    DbState,
};
use crate::db::contacts::{
    create_contact, delete_contact, list_contacts, update_contact, Contact, ContactInput,
};
use crate::voice::intent::{resolve_voice_intent, VoiceCommandSpec, VoiceIntent};
use crate::voice::response::{response_for, VoiceOutcome};
use crate::voice::speech::{self, VoiceOption, VoiceSpeech};
use crate::voice::{self, VoiceStatus};

/// Returned by nexus_get_db_status.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DbStatus {
    pub initialized: bool,
    pub migration_level: i64,
    pub db_path: String,
}

/// Returned by nexus_get_db_counts.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DbCounts {
    pub projects: i64,
    pub tasks: i64,
    pub ai_agents: i64,
    pub ides: i64,
    pub settings: i64,
}

/// NEXUS-025: how loudly NEXUS announces an arrival.
///
/// NEXUS-021 argued that an assistant surfacing everything is one you learn
/// to ignore. The user chose `Immediate` after that argument was put to them,
/// so it is the default. The other two exist so changing their mind later is
/// a setting rather than a rewrite: building the dial is cheap, needing it
/// and not having it is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Aggression {
    /// Speak as soon as anything arrives.
    Immediate,
    /// Speak once about the group, rather than once per message.
    Batched,
    /// Never speak first. The panel still shows what came in.
    Silent,
}

const KEY_AGGRESSION: &str = "notification_aggression";

pub fn notification_aggression(conn: &rusqlite::Connection) -> Aggression {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        [KEY_AGGRESSION],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .and_then(|v| match v.as_str() {
        "batched" => Some(Aggression::Batched),
        "silent" => Some(Aggression::Silent),
        "immediate" => Some(Aggression::Immediate),
        _ => None,
    })
    .unwrap_or(Aggression::Immediate)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// What one poll found, and what to say about it.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationPoll {
    /// The sentence to speak, when there is one. `None` means stay quiet:
    /// nothing arrived, NEXUS is mid-turn, or announcements are off.
    pub announcement: Option<String>,
    pub arrivals: Vec<Arrival>,
}

// ── Commands ─────────────────────────────────────────────────────────────────

/// Return current DB status: initialized flag, migration level, file path.
#[tauri::command]
pub fn nexus_get_db_status(
    state: State<'_, DbState>,
    app: tauri::AppHandle,
) -> Result<DbStatus, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    let level = db::migration_level(&conn)?;

    use tauri::Manager;
    let db_path = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Path error: {e}"))?
        .join("nexus.db")
        .to_string_lossy()
        .into_owned();

    Ok(DbStatus {
        initialized: true,
        migration_level: level,
        db_path,
    })
}

/// Return record counts for all five tables.
#[tauri::command]
pub fn nexus_get_db_counts(state: State<'_, DbState>) -> Result<DbCounts, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    let counts = count_all_tables(&conn)?;
    Ok(DbCounts {
        projects: counts.projects,
        tasks: counts.tasks,
        ai_agents: counts.ai_agents,
        ides: counts.ides,
        settings: counts.settings,
    })
}

/// Insert a new project and return the full row.
#[tauri::command]
pub fn nexus_create_project(
    state: State<'_, DbState>,
    input: CreateProjectInput,
) -> Result<Project, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    insert_project(&conn, &input)
}

/// Update an existing project and return the full updated row.
#[tauri::command]
pub fn nexus_update_project(
    state: State<'_, DbState>,
    input: UpdateProjectInput,
) -> Result<Project, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    update_project(&conn, &input)
}

/// Return all projects ordered by creation date descending.
#[tauri::command]
pub fn nexus_list_projects(state: State<'_, DbState>) -> Result<Vec<Project>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    list_projects(&conn)
}

// -- Task commands -----------------------------------------------------------

/// Insert a new task for a project and return the full row.
#[tauri::command]
pub fn nexus_create_task(
    state: State<'_, DbState>,
    input: CreateTaskInput,
) -> Result<Task, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    insert_task(&conn, &input)
}

/// Return all tasks for one project, newest first.
#[tauri::command]
pub fn nexus_list_tasks(state: State<'_, DbState>, project_id: i64) -> Result<Vec<Task>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    list_tasks(&conn, project_id)
}

/// Update a task's title, description and status, and return the full row.
#[tauri::command]
pub fn nexus_update_task(
    state: State<'_, DbState>,
    input: UpdateTaskInput,
) -> Result<Task, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    update_task(&conn, &input)
}

/// Change a task's status only, and return the full row.
#[tauri::command]
pub fn nexus_update_task_status(
    state: State<'_, DbState>,
    input: UpdateTaskStatusInput,
) -> Result<Task, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    update_task_status(&conn, &input)
}

// -- Registry commands -------------------------------------------------------

/// Register a new IDE and return the full row.
#[tauri::command]
pub fn nexus_create_ide(
    state: State<'_, DbState>,
    input: CreateRegistryEntryInput,
) -> Result<RegistryEntry, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    insert_ide(&conn, &input)
}

/// Return registered IDEs. `enabled_only` filters out disabled entries.
#[tauri::command]
pub fn nexus_list_ides(
    state: State<'_, DbState>,
    enabled_only: bool,
) -> Result<Vec<RegistryEntry>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    list_ides(&conn, enabled_only)
}

/// Update an IDE and return the full updated row.
#[tauri::command]
pub fn nexus_update_ide(
    state: State<'_, DbState>,
    input: UpdateRegistryEntryInput,
) -> Result<RegistryEntry, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    update_ide(&conn, &input)
}

/// Delete an IDE by ID. Referring projects are blanked by ON DELETE SET NULL.
#[tauri::command]
pub fn nexus_delete_ide(state: State<'_, DbState>, id: i64) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    delete_ide(&conn, id)
}

/// Register a new AI agent and return the full row.
#[tauri::command]
pub fn nexus_create_agent(
    state: State<'_, DbState>,
    input: CreateRegistryEntryInput,
) -> Result<RegistryEntry, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    insert_agent(&conn, &input)
}

/// Return registered agents. `enabled_only` filters out disabled entries.
#[tauri::command]
pub fn nexus_list_agents(
    state: State<'_, DbState>,
    enabled_only: bool,
) -> Result<Vec<RegistryEntry>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    list_agents(&conn, enabled_only)
}

/// Update an agent and return the full updated row.
#[tauri::command]
pub fn nexus_update_agent(
    state: State<'_, DbState>,
    input: UpdateRegistryEntryInput,
) -> Result<RegistryEntry, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    update_agent(&conn, &input)
}

/// Delete an agent by ID. Referring projects and tasks are blanked by SET NULL.
#[tauri::command]
pub fn nexus_delete_agent(state: State<'_, DbState>, id: i64) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    delete_agent(&conn, id)
}

/// Assign an agent to a task, or clear the assignment with a null agentId.
/// Narrow by design so `nexus_update_task` never writes this column (spec 2.5).
#[tauri::command]
pub fn nexus_assign_task_agent(
    state: State<'_, DbState>,
    input: AssignTaskAgentInput,
) -> Result<Task, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    assign_task_agent(&conn, &input)
}

// -- Aggregate commands ------------------------------------------------------

/// Workspace-wide totals for the Overview screen.
#[tauri::command]
pub fn nexus_get_workspace_summary(state: State<'_, DbState>) -> Result<WorkspaceSummary, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    workspace_summary(&conn)
}

/// Task counts per project, including projects with zero tasks.
#[tauri::command]
pub fn nexus_count_tasks_by_project(
    state: State<'_, DbState>,
) -> Result<Vec<ProjectTaskCounts>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    count_tasks_by_project(&conn)
}

/// Assigned-task counts per agent, including agents with zero assigned tasks.
#[tauri::command]
pub fn nexus_count_tasks_by_agent(
    state: State<'_, DbState>,
) -> Result<Vec<AgentTaskCounts>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    count_tasks_by_agent(&conn)
}

/// The most recently updated tasks across every project.
#[tauri::command]
pub fn nexus_list_recent_tasks(
    state: State<'_, DbState>,
    limit: i64,
) -> Result<Vec<TaskWithProject>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    list_recent_tasks(&conn, limit)
}

// -- Settings commands -------------------------------------------------------

/// Read all preferences, fully populated.
///
/// Never errors on a data condition: missing, malformed and unrecognised
/// values become defaults, and registry ids that no longer exist resolve to
/// null (spec 008 2.4, 2.6).
#[tauri::command]
pub fn nexus_get_settings(state: State<'_, DbState>) -> Result<Settings, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    get_settings(&conn)
}

/// Validate and persist all preferences in one transaction, then re-read.
#[tauri::command]
pub fn nexus_update_settings(
    state: State<'_, DbState>,
    input: Settings,
) -> Result<Settings, String> {
    let mut conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    update_settings(&mut conn, &input)
}

/// Delete only the known NEXUS keys and return the defaults.
#[tauri::command]
pub fn nexus_reset_settings(state: State<'_, DbState>) -> Result<Settings, String> {
    let mut conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    reset_settings(&mut conn)
}

// -- Search command ----------------------------------------------------------

/// Search projects, tasks, IDEs and agents in one query.
///
/// Deterministic case-insensitive substring matching. No scoring, no fuzzy
/// matching, no inference. An empty query returns no results, not an error.
#[tauri::command]
pub fn nexus_search_workspace(
    state: State<'_, DbState>,
    query: String,
) -> Result<SearchResults, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    search_workspace(&conn, &query)
}

// -- Voice commands (NEXUS-010) ----------------------------------------------

/// Recognizer availability, on-device support and authorization state.
#[tauri::command]
pub fn nexus_voice_status(app: tauri::AppHandle) -> Result<VoiceStatus, String> {
    voice::status(&app)
}

/// Ask macOS for speech-recognition authorization. Only ever called from an
/// explicit user action in the UI, never on startup.
#[tauri::command]
pub fn nexus_voice_request_authorization(app: tauri::AppHandle) -> Result<(), String> {
    voice::request_authorization(&app)
}

/// Begin listening.
///
/// The `voice_enabled` preference is checked here, in Rust, so the microphone
/// cannot be started by any caller while voice is off (NEXUS-010 C-05, V-09).
/// Enforcing this only in the UI would leave the invariant unguarded.
#[tauri::command]
pub fn nexus_voice_start(state: State<'_, DbState>, app: tauri::AppHandle) -> Result<(), String> {
    let enabled = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        get_settings(&conn)?.voice_enabled
    };
    if !enabled {
        return Err("Voice is disabled. Enable it in Settings first.".to_string());
    }
    voice::start(&app)
}

/// Stop listening. Safe to call when idle.
#[tauri::command]
pub fn nexus_voice_stop(app: tauri::AppHandle) -> Result<(), String> {
    voice::stop(&app)
}

/// Bring always-listening into line with the stored preferences.
///
/// The runtime flag is never set directly by the UI. It is derived here from
/// `voice_enabled` and `always_listening` together, so there is no ordering
/// in which turning voice off leaves the microphone held open.
#[tauri::command]
pub fn nexus_voice_sync_always_listening(
    state: State<'_, DbState>,
    app: tauri::AppHandle,
) -> Result<bool, String> {
    let wanted = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        let settings = get_settings(&conn)?;
        settings.voice_enabled && settings.always_listening
    };
    if wanted == voice::always_listening() {
        return Ok(wanted);
    }
    voice::set_always_listening(&app, wanted)?;
    Ok(wanted)
}

/// What a transcript heard in always-listening mode amounts to.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeOutcome {
    /// False for everything not addressed to NEXUS, which is most of what a
    /// permanently open microphone hears. The caller must then do nothing.
    pub woke: bool,
    /// A command spoken in the same breath as the wake word.
    pub command: Option<String>,
    /// The acknowledgement to speak, when the wake word came alone.
    pub reply: Option<String>,
}

/// Decide whether an utterance was addressed to NEXUS.
///
/// This runs before any matching or execution. A transcript that does not
/// wake NEXUS is discarded here and goes no further: it is not resolved, not
/// logged, and not stored.
#[tauri::command]
pub fn nexus_voice_wake(
    state: State<'_, DbState>,
    transcript: String,
) -> Result<WakeOutcome, String> {
    match voice::wake::detect(&transcript) {
        None => Ok(WakeOutcome {
            woke: false,
            command: None,
            reply: None,
        }),
        Some(voice::wake::Wake::WithCommand(command)) => Ok(WakeOutcome {
            woke: true,
            command: Some(command),
            reply: None,
        }),
        Some(voice::wake::Wake::Bare) => {
            let replies = {
                let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
                get_settings(&conn)?.wake_replies
            };
            Ok(WakeOutcome {
                woke: true,
                command: None,
                reply: Some(voice::next_wake_reply(&replies)),
            })
        }
    }
}

/// Resolve a spoken transcript to candidate commands (NEXUS-010 defect D-3).
///
/// Pure and deterministic. The command registry is passed in from
/// src/lib/commands.ts, which remains the single source of truth: nothing is
/// duplicated here. This returns candidate ids only; the palette shows them
/// and the user confirms. It never executes anything, and it is never called
/// on the keyboard path, so NEXUS-009 matching is unaffected.
#[tauri::command]
pub fn nexus_resolve_voice_intent(
    transcript: String,
    commands: Vec<VoiceCommandSpec>,
    project_names: Vec<String>,
) -> Result<VoiceIntent, String> {
    Ok(resolve_voice_intent(&transcript, &commands, &project_names))
}

// -- NEXUS-011: spoken responses ---------------------------------------------

/// Speak the response for an outcome.
///
/// The caller reports *what happened*, never what was heard: the wording is
/// chosen here by a deterministic template keyed on the executed command id
/// (`voice::response`). There is no channel through which a transcript could
/// reach the synthesizer.
///
/// Two suppressions are normal rather than errors, and both are enforced in
/// Rust so no caller can bypass them:
///
/// - Voice disabled in Settings. The same preference that gates the
///   microphone gates the speaker, so turning voice off silences NEXUS
///   completely (NEXUS-010 C-05).
/// - The microphone is open. Checked again inside `speech::speak`, at the
///   last moment before audio, because the two can race.
#[tauri::command]
pub fn nexus_voice_speak(
    state: State<'_, DbState>,
    app: tauri::AppHandle,
    outcome: VoiceOutcome,
) -> Result<VoiceSpeech, String> {
    let (enabled, voice_name) = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        let settings = get_settings(&conn)?;
        (settings.voice_enabled, settings.voice_name)
    };

    let text = response_for(&outcome);
    if !enabled {
        return Ok(VoiceSpeech {
            spoken: false,
            text,
            voice: None,
        });
    }

    speech::speak(&app, text, voice_name)
}

/// Silence the synthesizer. Safe to call when nothing is being spoken.
#[tauri::command]
pub fn nexus_voice_stop_speaking(app: tauri::AppHandle) -> Result<(), String> {
    speech::stop_speaking(&app)
}

/// English voices installed on this machine, for the Settings picker.
///
/// Machine-specific by nature: enhanced and premium voices are downloaded by
/// the user, so this is read at display time rather than stored.
#[tauri::command]
pub fn nexus_voice_list_voices(app: tauri::AppHandle) -> Result<Vec<VoiceOption>, String> {
    speech::list_voices(&app)
}

// -- NEXUS-012: the assistant action layer -----------------------------------

/// Perform one action.
///
/// The only IPC route to `execute_action`, which is itself the only route to
/// a connector. Everything the user can ask NEXUS to do arrives here: the
/// palette, and later voice and a reasoning provider's plan.
///
/// A `NeedsApproval` error is not a failure. It means NEXUS has understood
/// the request, refuses to act unattended, and is waiting: call again with
/// the token once the user has said yes.
#[tauri::command]
pub fn nexus_execute_action(
    state: State<'_, DbState>,
    approvals: State<'_, ApprovalStore>,
    session: State<'_, AssistantSession>,
    app: tauri::AppHandle,
    request: ActionRequest,
) -> Result<ActionOutcome, ActionError> {
    let result = {
        let conn = state.0.lock().map_err(|e| ActionError::Failed {
            detail: format!("Lock error: {e}"),
        })?;
        execute_action(&conn, &approvals, &session, request)
    };
    // Emitted after the lock is released, so a listener cannot re-enter the
    // command while the database is held.
    let _ = app.emit(
        EVENT_ASSISTANT_STATE,
        session.snapshot(approvals.pending_count()),
    );
    result
}

/// Withdraw a pending approval. Declining is not an error, and leaves no
/// audit row: nothing was attempted.
///
/// Returns how many requests are still waiting, so the UI can clear its
/// prompt without a second round trip.
#[tauri::command]
pub fn nexus_cancel_approval(
    approvals: State<'_, ApprovalStore>,
    session: State<'_, AssistantSession>,
    app: tauri::AppHandle,
    token: u64,
) -> usize {
    approvals.cancel(token);
    session.cancel();
    let remaining = approvals.pending_count();
    let _ = app.emit(EVENT_ASSISTANT_STATE, session.snapshot(remaining));
    remaining
}

/// Every connector, its status, its actions and its standing grants.
#[tauri::command]
pub fn nexus_list_connectors(state: State<'_, DbState>) -> Result<Vec<ConnectorView>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    list_connectors(&conn)
}

/// Grant or revoke one permission level for one connector.
///
/// Enforced in Rust the moment the next action is attempted, not by hiding a
/// button: the UI reflects the grant, it does not implement it.
#[tauri::command]
pub fn nexus_set_permission_grant(
    state: State<'_, DbState>,
    connector_id: String,
    level: String,
    granted: bool,
) -> Result<Vec<ConnectorView>, String> {
    let parsed = Permission::parse(&level).ok_or_else(|| {
        let accepted: Vec<&str> = Permission::all().iter().map(|p| p.as_str()).collect();
        format!(
            "Unknown permission level: {level}. Expected one of: {}",
            accepted.join(", ")
        )
    })?;
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    set_grant(&conn, &connector_id, parsed, granted)?;
    list_connectors(&conn)
}

/// Turn a whole connector on or off.
#[tauri::command]
pub fn nexus_set_connector_enabled(
    state: State<'_, DbState>,
    connector_id: String,
    enabled: bool,
) -> Result<Vec<ConnectorView>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    set_connector_enabled(&conn, &connector_id, enabled)?;
    list_connectors(&conn)
}

/// The audit trail, newest first.
#[tauri::command]
pub fn nexus_list_audit(state: State<'_, DbState>, limit: i64) -> Result<Vec<AuditEntry>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    list_audit(&conn, limit)
}

// -- NEXUS-013: assistant state and context ----------------------------------

/// What NEXUS is doing and what it has been talking about.
///
/// Read on demand rather than pushed, with the state event as the hint to
/// re-read. `listening` is not stored anywhere: the session derives it from
/// the microphone's own flag, so there is one source of truth.
#[tauri::command]
pub fn nexus_assistant_snapshot(
    session: State<'_, AssistantSession>,
    approvals: State<'_, ApprovalStore>,
) -> SessionSnapshot {
    session.snapshot(approvals.pending_count())
}

/// The full context: conversation, work context and recent actions.
///
/// Assembled per call and to a budget. There is no ambient context object
/// growing in the background, because the thing this eventually becomes is a
/// prompt, and prompts have to be small on purpose.
#[tauri::command]
pub fn nexus_assistant_context(
    state: State<'_, DbState>,
    session: State<'_, AssistantSession>,
    approvals: State<'_, ApprovalStore>,
) -> Result<AssistantContext, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    assemble(&conn, &session, approvals.pending_count())
}

/// Resolve a phrase like "the PR" or "the first one" against the conversation.
///
/// Deterministic and provider-free. Returns `ambiguous` with the candidates
/// named rather than picking one, so the caller can ask instead of guessing.
#[tauri::command]
pub fn nexus_assistant_resolve(session: State<'_, AssistantSession>, phrase: String) -> Resolution {
    session.resolve(&phrase)
}

/// Return to rest once a finished turn has been shown.
#[tauri::command]
pub fn nexus_assistant_settle(
    session: State<'_, AssistantSession>,
    approvals: State<'_, ApprovalStore>,
    app: tauri::AppHandle,
) -> SessionSnapshot {
    session.settle();
    let snapshot = session.snapshot(approvals.pending_count());
    let _ = app.emit(EVENT_ASSISTANT_STATE, snapshot.clone());
    snapshot
}

/// Record that NEXUS rendered a list, in the order the user saw it.
///
/// The only thing an ordinal may index into: "the first one" must mean a
/// position in something actually shown, never an inference over hidden
/// state. Ids come from a snapshot, so a caller can only compose a list out
/// of referents NEXUS itself created.
///
/// Returns the list id, or null when the list was empty: counting through
/// nothing is not something the user can mean.
#[tauri::command]
pub fn nexus_assistant_remember_list(
    session: State<'_, AssistantSession>,
    items: Vec<u64>,
) -> Option<u64> {
    session.remember_list(items)
}

/// Forget the conversation. The user's "start again".
#[tauri::command]
pub fn nexus_assistant_clear(
    session: State<'_, AssistantSession>,
    approvals: State<'_, ApprovalStore>,
    app: tauri::AppHandle,
) -> SessionSnapshot {
    session.clear();
    let snapshot = session.snapshot(approvals.pending_count());
    let _ = app.emit(EVENT_ASSISTANT_STATE, snapshot.clone());
    snapshot
}

// -- NEXUS-014: the conversation ---------------------------------------------

/// Ask NEXUS something, in text or by voice.
///
/// Opens a turn, then walks the deterministic escalation ladder: a local
/// answer first, the NEXUS-010 command matcher second, a plain refusal third.
/// No reasoning provider is involved, and none is reachable from the resolver.
///
/// An `action` reply is deliberately *not* executed here. It names a registry
/// id; the caller maps it through the same bridge the palette uses and runs
/// it through the gate, so there is still exactly one path to a connector.
/// The turn is left open so that execution continues it rather than starting
/// a second one.
#[tauri::command]
pub fn nexus_assistant_ask(
    state: State<'_, DbState>,
    session: State<'_, AssistantSession>,
    approvals: State<'_, ApprovalStore>,
    app: tauri::AppHandle,
    text: String,
    spoken: bool,
    commands: Vec<VoiceCommandSpec>,
    project_names: Vec<String>,
) -> Result<AssistantReply, String> {
    // **The wake word is stripped here, whatever the caller did.**
    //
    // A surface is supposed to hand over the command rather than the whole
    // utterance, and the voice controller does, except inside the window it
    // opens after asking a question: there it forwarded the transcript
    // verbatim, wake word and all. The resolver is not indifferent to those
    // extra tokens. "hey nexus check notifications" happened to survive them;
    // "hey nexus sign in to microsoft" did not, and resolved to nothing at
    // all. A defect that depends on whether a phrase happens to tolerate two
    // stray words is not one anybody can reason about.
    //
    // Doing it here as well as in the controller is deliberate belt and
    // braces. Detection is idempotent: a phrase with no wake word comes back
    // unchanged, so this can never take anything away from a real command.
    let woken = crate::voice::wake::detect(&text);
    let text = match woken {
        Some(crate::voice::wake::Wake::WithCommand(ref command)) => command.clone(),
        // Bare wake word, or none at all. Either way there is nothing to
        // strip, and the phrase goes on exactly as it arrived.
        _ => text,
    };

    // Was this clearly meant for NEXUS?
    //
    // Typing is always deliberate. Speech is deliberate when it carried the
    // wake word; without one it arrived in the window NEXUS opens after
    // answering, which also catches whatever else is said in the room. Such
    // an utterance still walks the deterministic ladder, because "the first
    // one" is a real follow-up, but it never reaches a reasoning provider.
    let deliberate = !spoken || woken.is_some();

    session.begin_turn(if spoken {
        TurnInput::Voice { text: text.clone() }
    } else {
        TurnInput::Text { text: text.clone() }
    });

    let response = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        let snapshot = session.snapshot(approvals.pending_count());
        respond_to(
            &text,
            &conn,
            &snapshot,
            &commands,
            &project_names,
            deliberate,
        )
    };

    // File what the answer named, in the order the user reads it. Without
    // this, "do the first one" has nothing to count through.
    let mut filed = Vec::new();
    for draft in &response.referents {
        filed.push(session.remember(
            draft.kind,
            &draft.display_name,
            "nexus",
            draft.metadata.clone(),
        ));
    }
    if response.rendered_as_list {
        session.remember_list(filed);
    }

    // An offer lives for one exchange. It survives only where the turn is
    // still about it: the follow-up itself, which `execute_action` will
    // settle, or a re-ask after a mishearing. Anything else means the user
    // moved on, and an offer left standing is how a "yes" ten minutes later
    // sends a message nobody was still looking at.
    if !response.holds_follow_up {
        session.clear_follow_up();
    }

    // An action the resolver chained onto its own answer: the greeting plus
    // the ticket list the user actually meant by "good morning".
    //
    // Run here rather than handed to the surface, so the two arrive as one
    // spoken sentence instead of a greeting followed a second later by an
    // unrelated-sounding list. It still goes through `execute_action`, so it
    // is gated, audited, and refused if the grant was revoked since the
    // resolver looked.
    //
    // Failure is absorbed on purpose. A greeting that turns into a
    // connector error report because a token expired overnight is worse than
    // a greeting that is merely shorter than usual.
    let reply = match &response.reply {
        AssistantReply::Answer { text, cited } if response.then.is_some() || response.tail.is_some() => {
            let chained = response.then.and_then(|(action_id, input)| {
                let conn = state.0.lock().ok()?;
                execute_action(
                    &conn,
                    &approvals,
                    &session,
                    ActionRequest {
                        action_id,
                        input,
                        approval: None,
                    },
                )
                .ok()
                .and_then(|outcome| outcome.detail)
            });

            // Greeting, then what the connector found, then what is planned.
            // Each part is skipped when absent rather than leaving a gap, so
            // a failed connector shortens the sentence instead of breaking
            // its punctuation.
            let joined = [Some(text.clone()), chained, response.tail]
                .into_iter()
                .flatten()
                .filter(|part| !part.trim().is_empty())
                .collect::<Vec<String>>()
                .join(" ");

            AssistantReply::Answer {
                text: joined,
                cited: cited.clone(),
            }
        }
        _ => response.reply,
    };

    match &reply {
        AssistantReply::Answer { text, .. } => {
            session.advance(AssistantState::Completed, Some(text.clone()), None)
        }
        // Left in Thinking: the caller is about to run it, and that execution
        // should continue this turn rather than open another.
        AssistantReply::Action { summary, .. } => {
            session.advance(AssistantState::Thinking, Some(summary.clone()), None)
        }
        AssistantReply::Choices { .. } => session.advance(
            AssistantState::Thinking,
            Some("Waiting for you to choose".to_string()),
            None,
        ),
        // Something NEXUS never made sense of is not a failed turn. With the
        // microphone held open it is usually a fragment of the room, and
        // recording it as a failure fills the session log with other
        // people's conversation.
        AssistantReply::Unresolved {
            reason,
            understood: true,
        } => session.advance(AssistantState::Failed, None, Some(reason.clone())),
        AssistantReply::Unresolved {
            understood: false, ..
        } => session.advance(AssistantState::Idle, None, None),
        // Left awaiting the user: a proposal is an offer, and every step in
        // it still has to pass the gate individually.
        AssistantReply::Proposal { steps, .. } => session.advance(
            AssistantState::AwaitingConfirmation,
            Some(format!(
                "Proposed {} step{}",
                steps.len(),
                if steps.len() == 1 { "" } else { "s" }
            )),
            None,
        ),
    }

    let _ = app.emit(
        EVENT_ASSISTANT_STATE,
        session.snapshot(approvals.pending_count()),
    );
    Ok(reply)
}

/// Resolve a phrase and abandon the turn if it names nothing.
///
/// Used by the conversation surface for follow-ups like "open the PR" before
/// the request is treated as a fresh instruction.
#[tauri::command]
pub fn nexus_assistant_cancel_turn(
    session: State<'_, AssistantSession>,
    approvals: State<'_, ApprovalStore>,
    app: tauri::AppHandle,
) -> SessionSnapshot {
    session.cancel();
    let snapshot = session.snapshot(approvals.pending_count());
    let _ = app.emit(EVENT_ASSISTANT_STATE, snapshot.clone());
    snapshot
}

// -- Contacts ----------------------------------------------------------------
//
// The only people NEXUS can message. Typed by the user; the macOS address
// book is deliberately not read.

#[tauri::command]
pub fn nexus_list_contacts(state: State<'_, DbState>) -> Result<Vec<Contact>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    list_contacts(&conn)
}

#[tauri::command]
pub fn nexus_save_contact(
    state: State<'_, DbState>,
    contact: ContactInput,
) -> Result<Vec<Contact>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    if contact.id.is_some() {
        update_contact(&conn, &contact)?;
    } else {
        create_contact(&conn, &contact)?;
    }
    list_contacts(&conn)
}

#[tauri::command]
pub fn nexus_delete_contact(
    state: State<'_, DbState>,
    id: i64,
) -> Result<Vec<Contact>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    delete_contact(&conn, id)?;
    list_contacts(&conn)
}

/// Store a connector's configuration: endpoints and account names.
///
/// Secrets are refused here and belong in the macOS Keychain. NEXUS never
/// writes a token to its database.
#[tauri::command]
pub fn nexus_set_connector_config(
    state: State<'_, DbState>,
    connector_id: String,
    config: serde_json::Value,
) -> Result<Vec<ConnectorView>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    set_connector_config(&conn, &connector_id, &config)?;
    list_connectors(&conn)
}

// -- NEXUS-019: the reasoning layer ------------------------------------------

/// What reasoning NEXUS may do, and what it has done.
#[tauri::command]
pub fn nexus_reasoning_status(state: State<'_, DbState>) -> Result<serde_json::Value, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    let policy = read_policy(&conn);
    let providers: Vec<serde_json::Value> = crate::assistant::reasoning::providers(&conn)
        .iter()
        .map(|p| {
            serde_json::json!({
                "id": p.id(),
                "model": p.model(),
                "reach": p.reach(),
                "available": p.available(),
            })
        })
        .collect();

    // Which one would actually answer right now, and why none would if none
    // would. The UI shows this rather than making the user infer it.
    let active = crate::assistant::reasoning::best_provider(&conn);

    Ok(serde_json::json!({
        "providers": providers,
        "cloudVendors": crate::assistant::cloud_provider::describe(&conn),
        "activeProvider": active.as_ref().ok().map(|p| p.id()),
        // Checkable from outside rather than only asserted in a test: the
        // local provider's exemption from the external switch depends on it.
        "localProviderIsLoopback": crate::assistant::ollama_provider::base_url_is_loopback(),
        "localTimeoutSeconds":
            crate::assistant::ollama_provider::reason_timeout().as_secs(),
        "unavailableReason": active.err().map(|e| e.to_string()),
        "externalReasoningAllowed": policy.external_reasoning_allowed,
        "contentSharingAllowed": policy.content_sharing_allowed,
    }))
}

/// Turn external reasoning, and content sharing, on or off.
///
/// Two switches rather than one: allowing NEXUS to consult a cloud model is a
/// different decision from allowing it to send the contents of your messages.
#[tauri::command]
pub fn nexus_set_reasoning_policy(
    state: State<'_, DbState>,
    external_reasoning_allowed: bool,
    content_sharing_allowed: bool,
) -> Result<serde_json::Value, String> {
    {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        set_policy(
            &conn,
            PrivacyPolicy {
                external_reasoning_allowed,
                content_sharing_allowed,
            },
        )?;
    }
    nexus_reasoning_status(state)
}

/// Why NEXUS contacted a reasoning provider. Categories, never contents.
#[tauri::command]
pub fn nexus_list_ai_audit(
    state: State<'_, DbState>,
    limit: i64,
) -> Result<Vec<AiAuditEntry>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    list_recent_use(&conn, limit)
}

// -- NEXUS-020: suggestions ---------------------------------------------------

/// What NEXUS thinks might be worth doing, derived from current data.
///
/// Nothing here runs. Each suggestion names a proposed action; accepting one
/// sends it through `nexus_execute_action` like any other, so permission and
/// confirmation are unchanged.
#[tauri::command]
pub fn nexus_list_suggestions(state: State<'_, DbState>) -> Result<Vec<Suggestion>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    Ok(generate(&conn))
}

/// Stop raising one.
#[tauri::command]
pub fn nexus_dismiss_suggestion(
    state: State<'_, DbState>,
    key: String,
) -> Result<Vec<Suggestion>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    dismiss(&conn, &key)?;
    Ok(generate(&conn))
}

/// Start raising it again.
#[tauri::command]
pub fn nexus_restore_suggestion(
    state: State<'_, DbState>,
    key: String,
) -> Result<Vec<Suggestion>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    restore(&conn, &key)?;
    Ok(generate(&conn))
}

// -- NEXUS-021: proactive surfacing -------------------------------------------

/// What NEXUS wants to raise now. Starts each suggestion's cooldown.
#[tauri::command]
pub fn nexus_surface_suggestions(state: State<'_, DbState>) -> Result<Vec<Suggestion>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    Ok(surface(&conn))
}

/// The same list, without starting anyone's cooldown. For polling.
#[tauri::command]
pub fn nexus_preview_suggestions(state: State<'_, DbState>) -> Result<Vec<Suggestion>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    Ok(preview(&conn))
}

/// Where things stand, from local data only.
#[tauri::command]
pub fn nexus_briefing(state: State<'_, DbState>) -> Result<Briefing, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    Ok(briefing(&conn))
}

/// Emitted when the watcher has something to say. Payload: [`NotificationPoll`].
pub const EVENT_NOTIFICATION: &str = "nexus://notification";

/// Which applications NEXUS may read notifications from, and how loudly it
/// says so.
///
/// Separate from `Settings` on purpose. That struct is the user's workspace
/// preferences; this is the privacy boundary for a Full Disk Access grant,
/// and it deserves to be read and written somewhere a reviewer can find it
/// rather than as two more fields among twenty.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationPolicy {
    /// Applications by name, as they appear in a bundle identifier's last
    /// segment: `WhatsApp`, `Teams`. Empty means nothing is read at all.
    pub apps: Vec<String>,
    pub aggression: Aggression,
    /// Whether the notification store can actually be read right now, and
    /// why not if it cannot. Distinct from `apps` being empty: one is a
    /// permission the user has not granted, the other is a choice they have
    /// not made, and the UI must not conflate them.
    pub blocked: Option<String>,
}

#[tauri::command]
pub fn nexus_notification_policy(
    state: State<'_, DbState>,
) -> Result<NotificationPolicy, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    Ok(NotificationPolicy {
        apps: notification_connector::opted_in_apps(&conn),
        aggression: notification_aggression(&conn),
        blocked: notification_connector::available().err().map(|e| e.to_string()),
    })
}

#[tauri::command]
pub fn nexus_set_notification_policy(
    state: State<'_, DbState>,
    apps: Vec<String>,
    aggression: Aggression,
) -> Result<NotificationPolicy, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    let cleaned: Vec<String> = apps
        .iter()
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty())
        .collect();

    for (key, value) in [
        (notification_connector::KEY_APPS, cleaned.join(",")),
        (
            KEY_AGGRESSION,
            match aggression {
                Aggression::Immediate => "immediate".to_string(),
                Aggression::Batched => "batched".to_string(),
                Aggression::Silent => "silent".to_string(),
            },
        ),
    ] {
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, value],
        )
        .map_err(|e| format!("Failed to save: {e}"))?;
    }

    Ok(NotificationPolicy {
        apps: notification_connector::opted_in_apps(&conn),
        aggression: notification_aggression(&conn),
        blocked: notification_connector::available().err().map(|e| e.to_string()),
    })
}

/// NEXUS-028: what the user said they would do, and settling one.
#[tauri::command]
pub fn nexus_list_commitments(
    state: State<'_, DbState>,
    open_only: bool,
) -> Result<Vec<crate::db::commitments::Commitment>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    crate::db::commitments::list(&conn, open_only)
}

#[tauri::command]
pub fn nexus_delete_commitment(state: State<'_, DbState>, id: i64) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    crate::db::commitments::delete(&conn, id)
}

/// Create a reminder from the panel.
///
/// A plain command rather than an action through the gate, and consistently
/// so: the UI already creates projects, tasks and contacts this way. The
/// gate exists for things NEXUS does *on the user's behalf* after
/// interpreting a sentence; a person filling in a form and pressing a button
/// has already given every confirmation a prompt could ask for.
///
/// `due_at` is unix seconds, or absent for something to keep on the list
/// without a reminder. The connector says plainly when it will not fire, and
/// so does the panel.
#[tauri::command]
pub fn nexus_create_commitment(
    state: State<'_, DbState>,
    what: String,
    due_at: Option<i64>,
) -> Result<crate::db::commitments::Commitment, String> {
    let text = what.trim();
    if text.is_empty() {
        return Err("A reminder needs something to say.".to_string());
    }
    // A reminder in the past can never be raised: `due_now` only looks
    // forward, so accepting one would put a row on the list that quietly
    // does nothing.
    if let Some(at) = due_at {
        if at <= unix_now() {
            return Err("That time has already passed.".to_string());
        }
    }
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    crate::db::commitments::create(&conn, text, due_at)
}

/// Refresh the cached schedule, if it is old enough to be worth a call.
///
/// Runs through `execute_action` like anything else, so the calendar read is
/// permission-checked and audited exactly as it would be if the user had
/// asked for it aloud. A failure is swallowed: the cache stays as it was, the
/// state stays `Unknown`, and NEXUS fails open rather than going mute.
fn refresh_schedule(conn: &rusqlite::Connection, approvals: &ApprovalStore) {
    if !calendar::stale() {
        return;
    }
    let request = ActionRequest {
        action_id: "outlook.today_schedule".to_string(),
        input: serde_json::Value::Null,
        approval: None,
    };
    // **A scratch session, and that is the point.** `execute_action` records
    // a turn and advances the assistant's state, which is right for something
    // the user asked for and wrong for housekeeping. Sharing the real session
    // put "Check today's calendar and meetings / Not signed in to Microsoft"
    // into the conversation as though it were the answer to whatever the user
    // had just said, every few minutes, unprompted.
    //
    // The audit row is still written, and the permission gate still applies:
    // what is thrown away is the conversational record, not the accounting.
    let scratch = AssistantSession::default();
    match execute_action(conn, approvals, &scratch, request) {
        Ok(outcome) => calendar::remember(calendar::parse_schedule(&outcome.output)),
        // Stamped, so an unreachable calendar is not retried on every tick,
        // but still recorded as unknown so NEXUS fails open rather than
        // treating "could not look" as "nothing on".
        Err(_) => calendar::remember_failure(),
    }
}

/// NEXUS-025: what arrived, and the question to ask about it.
///
/// One command rather than a read plus a separate offer, because the two must
/// not drift: an announcement the user can answer with "yes" is only safe if
/// the offer that "yes" resolves against was created in the same breath.
///
/// **The user's name is used deliberately.** "You have a message" is a
/// notification; "Hey Rohit, you have a message from Priya" is somebody
/// talking to you, and the difference is the whole milestone.
/// The body of the poll, over plain references.
///
/// Pulled out of the command so the watcher thread can call it too. The
/// command stays as the way a surface asks "anything now?"; the thread is
/// what makes NEXUS speak without being asked, which is the whole milestone.
pub fn poll_notifications(
    conn: &rusqlite::Connection,
    session: &AssistantSession,
    approvals: &ApprovalStore,
) -> Result<NotificationPoll, String> {
        // Debug-level, and deliberately here rather than in a surface: the
    // watcher runs whether or not anything is on screen, so this is the only
    // place a wiring problem would ever be visible.
    log::debug!("notification poll");

    let aggression = notification_aggression(conn);
    if aggression == Aggression::Silent {
        return Ok(NotificationPoll::default());
    }

    // Never speak over a command in flight. The recogniser would hear NEXUS,
    // and NEXUS answering itself is the loop that makes always-listening
    // unusable. Checked before anything else, so a busy turn suppresses every
    // kind of announcement rather than only one of them.
    if session.state().is_busy() {
        return Ok(NotificationPoll::default());
    }

    // Only once nothing is in flight. `execute_action` clears the pending
    // follow-up on every success, so refreshing the calendar while the user
    // still owes an answer to "shall I read it?" would silently throw that
    // question away and leave their "yes" resolving against nothing.
    if session.pending_follow_up().is_none() {
        refresh_schedule(conn, approvals);
    }

    let name = user_name(conn).unwrap_or_default();
    let greeting = if name.is_empty() {
        String::new()
    } else {
        format!("Hey {name}, ")
    };

    // A reminder that has come due.
    //
    // Checked before the calendar, not after: the point of a reminder is that
    // it arrives at the time the user chose, and a meeting starting in the
    // same tick must not swallow it. `due_now` marks the row raised as it
    // returns it, so the same reminder cannot be announced twice.
    //
    // This is the piece that was missing. `due_now` existed, was tested, and
    // had no caller outside its own tests, so every reminder ever recorded
    // sat in the table and nothing ever said it aloud.
    if let Some(due) = crate::db::commitments::due_now(conn, unix_now()) {
        return Ok(NotificationPoll {
            announcement: Some(format!("{greeting}you asked me to remind you: {}.", due.what)),
            ..Default::default()
        });
    }

    // NEXUS-027. A meeting is the worst possible moment for an assistant that
    // speaks first, and this is what makes the immediate setting liveable.
    //
    // **Unknown is not Clear, and is deliberately treated as Clear here.**
    // With Outlook unreachable NEXUS does not know, and staying silent
    // whenever the calendar cannot be read would mean one expired token
    // turns the whole assistant mute with no sign of why. Failing open is
    // the choice; the two states are kept distinct so it stays a choice.
    let clock = calendar::local_minutes(conn).unwrap_or(0);
    match calendar::state_at(clock) {
        calendar::Now::InMeeting => return Ok(NotificationPoll::default()),
        calendar::Now::StartingSoon(subject) => {
            return Ok(NotificationPoll {
                announcement: Some(format!(
                    "{greeting}{subject} starts in about {} minutes.",
                    calendar::WARN_MINUTES
                )),
                ..Default::default()
            })
        }
        calendar::Now::Clear | calendar::Now::Unknown => {}
    }

    // NEXUS-028 next: a commitment is due at a time the user chose, and a
    // message is due whenever somebody else felt like sending one. Between
    // the two, the thing the user asked to be told about wins.
    //
    // `due_now` marks it raised as it hands it over, so this is once and
    // then never again however long it stays open.
    if let Some(owed) = crate::db::commitments::due_now(conn, unix_now()) {
        session.offer_follow_up(
            "nexus.settle_commitment",
            serde_json::json!({ "id": owed.id, "state": "done" }),
            "Say yes if that is done, or no to leave it open.",
        );
        return Ok(NotificationPoll {
            // The user's own words back to them, not a paraphrase.
            announcement: Some(format!(
                "{greeting}you said you would {}. Did you?",
                owed.what
            )),
            ..Default::default()
        });
    }

    // A read failure here is not surfaced as an error. Polling runs on a
    // timer, and a missing Full Disk Access grant would otherwise raise the
    // same complaint every few seconds for as long as it went ungranted.
    // `notifications.status` is where that question gets a real answer.
    let arrivals = match notification_connector::since_cursor(conn, unix_now()) {
        Ok(found) => found,
        Err(_) => return Ok(NotificationPoll::default()),
    };
    if arrivals.is_empty() {
        return Ok(NotificationPoll::default());
    }

    let announcement = match (aggression, arrivals.len()) {
        // Batched, or several at once: say what is waiting and offer to read
        // the newest. Reading four messages aloud unprompted is how an
        // assistant becomes something you talk over.
        (Aggression::Batched, _) | (_, 2..) => {
            let who: Vec<&str> = arrivals.iter().map(|a| a.from.as_str()).collect();
            format!(
                "{greeting}you have {} messages, from {}. Shall I read the latest?",
                arrivals.len(),
                who.join(", ")
            )
        }
        _ => {
            let one = &arrivals[0];
            format!(
                "{greeting}you got a message from {} on {}. Shall I read it, or skip it?",
                one.from, one.app
            )
        }
    };

    // The offer the announcement's "yes" resolves against. It carries the
    // message text, because by the time the user answers NEXUS has kept
    // nothing: the session is memory only and the offer expires on its TTL.
    // **Every arrival, not just the newest.** The announcement names all of
    // them, so "yes" has to read all of them; carrying only the last one made
    // NEXUS say "you have four messages" and then read one.
    let messages: Vec<serde_json::Value> = arrivals
        .iter()
        .map(|a| serde_json::json!({ "from": a.from, "preview": a.preview }))
        .collect();
    session.offer_follow_up(
        "notifications.read_aloud",
        serde_json::json!({ "messages": messages }),
        "Say yes to hear them, or no to leave them.",
    );

    Ok(NotificationPoll {
        announcement: Some(announcement),
        arrivals,
    })
}

#[tauri::command]
pub fn nexus_notifications_poll(
    state: State<'_, DbState>,
    session: State<'_, AssistantSession>,
    approvals: State<'_, ApprovalStore>,
) -> Result<NotificationPoll, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    poll_notifications(&conn, &session, &approvals)
}

/// Tell NEXUS a suggestion was acted on, which resets how tired of it it is.
#[tauri::command]
pub fn nexus_accept_suggestion(state: State<'_, DbState>, key: String) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    record_accepted(&conn, &key)
}

#[tauri::command]
pub fn nexus_proactive_policy(state: State<'_, DbState>) -> Result<ProactivePolicy, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    Ok(read_proactive(&conn))
}

#[tauri::command]
pub fn nexus_set_proactive_policy(
    state: State<'_, DbState>,
    enabled: bool,
    cooldown_minutes: i64,
) -> Result<ProactivePolicy, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    set_proactive(
        &conn,
        ProactivePolicy {
            enabled,
            cooldown_minutes,
        },
    )?;
    Ok(read_proactive(&conn))
}

/// Choose which model a reasoning provider uses.
///
/// Keys are never set here: they live in the Keychain, and the status payload
/// carries the exact command to add one.
#[tauri::command]
pub fn nexus_set_reasoning_model(
    state: State<'_, DbState>,
    provider: String,
    model: String,
) -> Result<serde_json::Value, String> {
    {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        match provider.as_str() {
            "ollama" => crate::assistant::ollama_provider::set_model(&conn, &model)?,
            "anthropic" => crate::assistant::cloud_provider::set_model(
                &conn,
                crate::assistant::cloud_provider::Vendor::Anthropic,
                &model,
            )?,
            "openai" => crate::assistant::cloud_provider::set_model(
                &conn,
                crate::assistant::cloud_provider::Vendor::OpenAi,
                &model,
            )?,
            other => return Err(format!("NEXUS has no reasoning provider called {other}.")),
        }
    }
    nexus_reasoning_status(state)
}

/// Models a local Ollama actually has, for the picker.
#[tauri::command]
pub fn nexus_local_models() -> Vec<String> {
    crate::assistant::ollama_provider::installed_models()
}

/// Speak a response NEXUS composed.
///
/// NEXUS-011 spoke only from fixed templates keyed by an executed action.
/// That rule existed to stop NEXUS reading a *transcript* back, which would
/// have put recognised speech into the audio path. An answer NEXUS built
/// from its own data is a different thing, and refusing to say it out loud
/// left the assistant mute.
///
/// The NEXUS-010 invariants still hold underneath: `speech::speak` refuses
/// while the microphone is open, and interrupt-and-replace means a newer
/// answer cancels a stale one.
#[tauri::command]
pub fn nexus_voice_say(
    state: State<'_, DbState>,
    app: tauri::AppHandle,
    text: String,
) -> Result<crate::voice::speech::VoiceSpeech, String> {
    let (enabled, voice_name) = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        let settings = get_settings(&conn)?;
        (settings.voice_enabled, settings.voice_name)
    };

    // The same preference that gates the microphone gates the speaker.
    if !enabled {
        return Ok(crate::voice::speech::VoiceSpeech {
            spoken: false,
            text,
            voice: None,
        });
    }

    // Bounded: a spoken answer that runs for a minute is one the user will
    // interrupt, and reading a long list aloud is not useful anyway.
    const SPOKEN_CAP: usize = 400;
    let trimmed = text.trim();
    let spoken: String = if trimmed.chars().count() > SPOKEN_CAP {
        trimmed.chars().take(SPOKEN_CAP).collect::<String>() + ", and more on screen"
    } else {
        trimmed.to_string()
    };

    if spoken.is_empty() {
        return Ok(crate::voice::speech::VoiceSpeech {
            spoken: false,
            text: spoken,
            voice: None,
        });
    }

    crate::voice::speech::speak(&app, spoken, voice_name)
}

/// What NEXUS should call you.
///
/// Blank means it greets without a name rather than guessing one from the
/// account, which is the safer default: a wrong name in a greeting is worse
/// than no name.
#[tauri::command]
pub fn nexus_set_user_name(
    state: State<'_, DbState>,
    name: String,
) -> Result<Option<String>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    crate::assistant::converse::set_user_name(&conn, &name)?;
    Ok(crate::assistant::converse::user_name(&conn))
}

#[tauri::command]
pub fn nexus_user_name(state: State<'_, DbState>) -> Result<Option<String>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    Ok(crate::assistant::converse::user_name(&conn))
}

