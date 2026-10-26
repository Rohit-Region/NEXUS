pub mod news;
pub mod phone_calls;
pub mod portfolio_chat;
pub mod spending;
pub mod stocks;

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
#[tauri::command(async)]
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
#[tauri::command(async)]
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
#[tauri::command(async)]
pub fn nexus_create_project(
    state: State<'_, DbState>,
    input: CreateProjectInput,
) -> Result<Project, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    insert_project(&conn, &input)
}

/// Update an existing project and return the full updated row.
#[tauri::command(async)]
pub fn nexus_update_project(
    state: State<'_, DbState>,
    input: UpdateProjectInput,
) -> Result<Project, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    update_project(&conn, &input)
}

/// Return all projects ordered by creation date descending.
#[tauri::command(async)]
pub fn nexus_list_projects(state: State<'_, DbState>) -> Result<Vec<Project>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    list_projects(&conn)
}

// -- Task commands -----------------------------------------------------------

/// Insert a new task for a project and return the full row.
#[tauri::command(async)]
pub fn nexus_create_task(
    state: State<'_, DbState>,
    input: CreateTaskInput,
) -> Result<Task, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    insert_task(&conn, &input)
}

/// Return all tasks for one project, newest first.
#[tauri::command(async)]
pub fn nexus_list_tasks(state: State<'_, DbState>, project_id: i64) -> Result<Vec<Task>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    list_tasks(&conn, project_id)
}

/// Update a task's title, description and status, and return the full row.
#[tauri::command(async)]
pub fn nexus_update_task(
    state: State<'_, DbState>,
    input: UpdateTaskInput,
) -> Result<Task, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    update_task(&conn, &input)
}

/// Change a task's status only, and return the full row.
#[tauri::command(async)]
pub fn nexus_update_task_status(
    state: State<'_, DbState>,
    input: UpdateTaskStatusInput,
) -> Result<Task, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    update_task_status(&conn, &input)
}

// -- Registry commands -------------------------------------------------------

/// Register a new IDE and return the full row.
#[tauri::command(async)]
pub fn nexus_create_ide(
    state: State<'_, DbState>,
    input: CreateRegistryEntryInput,
) -> Result<RegistryEntry, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    insert_ide(&conn, &input)
}

/// Return registered IDEs. `enabled_only` filters out disabled entries.
#[tauri::command(async)]
pub fn nexus_list_ides(
    state: State<'_, DbState>,
    enabled_only: bool,
) -> Result<Vec<RegistryEntry>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    list_ides(&conn, enabled_only)
}

/// Update an IDE and return the full updated row.
#[tauri::command(async)]
pub fn nexus_update_ide(
    state: State<'_, DbState>,
    input: UpdateRegistryEntryInput,
) -> Result<RegistryEntry, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    update_ide(&conn, &input)
}

/// Delete an IDE by ID. Referring projects are blanked by ON DELETE SET NULL.
#[tauri::command(async)]
pub fn nexus_delete_ide(state: State<'_, DbState>, id: i64) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    delete_ide(&conn, id)
}

/// Register a new AI agent and return the full row.
#[tauri::command(async)]
pub fn nexus_create_agent(
    state: State<'_, DbState>,
    input: CreateRegistryEntryInput,
) -> Result<RegistryEntry, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    insert_agent(&conn, &input)
}

/// Return registered agents. `enabled_only` filters out disabled entries.
#[tauri::command(async)]
pub fn nexus_list_agents(
    state: State<'_, DbState>,
    enabled_only: bool,
) -> Result<Vec<RegistryEntry>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    list_agents(&conn, enabled_only)
}

/// Update an agent and return the full updated row.
#[tauri::command(async)]
pub fn nexus_update_agent(
    state: State<'_, DbState>,
    input: UpdateRegistryEntryInput,
) -> Result<RegistryEntry, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    update_agent(&conn, &input)
}

/// Delete an agent by ID. Referring projects and tasks are blanked by SET NULL.
#[tauri::command(async)]
pub fn nexus_delete_agent(state: State<'_, DbState>, id: i64) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    delete_agent(&conn, id)
}

/// Assign an agent to a task, or clear the assignment with a null agentId.
/// Narrow by design so `nexus_update_task` never writes this column (spec 2.5).
#[tauri::command(async)]
pub fn nexus_assign_task_agent(
    state: State<'_, DbState>,
    input: AssignTaskAgentInput,
) -> Result<Task, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    assign_task_agent(&conn, &input)
}

// -- Aggregate commands ------------------------------------------------------

/// Workspace-wide totals for the Overview screen.
#[tauri::command(async)]
pub fn nexus_get_workspace_summary(state: State<'_, DbState>) -> Result<WorkspaceSummary, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    workspace_summary(&conn)
}

/// Task counts per project, including projects with zero tasks.
#[tauri::command(async)]
pub fn nexus_count_tasks_by_project(
    state: State<'_, DbState>,
) -> Result<Vec<ProjectTaskCounts>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    count_tasks_by_project(&conn)
}

/// Assigned-task counts per agent, including agents with zero assigned tasks.
#[tauri::command(async)]
pub fn nexus_count_tasks_by_agent(
    state: State<'_, DbState>,
) -> Result<Vec<AgentTaskCounts>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    count_tasks_by_agent(&conn)
}

/// The most recently updated tasks across every project.
#[tauri::command(async)]
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
#[tauri::command(async)]
pub fn nexus_get_settings(state: State<'_, DbState>) -> Result<Settings, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    get_settings(&conn)
}

/// Validate and persist all preferences in one transaction, then re-read.
#[tauri::command(async)]
pub fn nexus_update_settings(
    state: State<'_, DbState>,
    input: Settings,
) -> Result<Settings, String> {
    let mut conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    update_settings(&mut conn, &input)
}

/// Delete only the known NEXUS keys and return the defaults.
#[tauri::command(async)]
pub fn nexus_reset_settings(state: State<'_, DbState>) -> Result<Settings, String> {
    let mut conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    reset_settings(&mut conn)
}

// -- Search command ----------------------------------------------------------

/// Search projects, tasks, IDEs and agents in one query.
///
/// Deterministic case-insensitive substring matching. No scoring, no fuzzy
/// matching, no inference. An empty query returns no results, not an error.
#[tauri::command(async)]
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
#[tauri::command(async)]
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
#[tauri::command(async)]
pub fn nexus_execute_action(
    state: State<'_, DbState>,
    approvals: State<'_, ApprovalStore>,
    session: State<'_, AssistantSession>,
    app: tauri::AppHandle,
    request: ActionRequest,
) -> Result<ActionOutcome, ActionError> {
    let original = request.clone();
    let (result, phone) = {
        let conn = state.0.lock().map_err(|e| ActionError::Failed {
            detail: format!("Lock error: {e}"),
        })?;
        (execute_action(&conn, &approvals, &session, request), phone_targets(&conn))
    };
    // The gate issued a token and is waiting: offer the same request on the
    // phone. Approving there redeems this very token, so it passes or fails
    // the same checks the app's own Approve button does.
    if let (Err(ActionError::NeedsApproval { token, summary, .. }), Some((alert, reply))) =
        (&result, phone)
    {
        let mut approved = original;
        approved.approval = Some(*token);
        ask_phone(
            alert,
            reply,
            crate::assistant::phone_approval::Pending::Action { request: approved },
            "NEXUS needs your approval",
            summary.clone(),
            crate::assistant::approval::APPROVAL_TTL,
            ("Approve", "Deny"),
        );
    }
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
#[tauri::command(async)]
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
#[tauri::command(async)]
pub fn nexus_list_connectors(state: State<'_, DbState>) -> Result<Vec<ConnectorView>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    list_connectors(&conn)
}

/// Grant or revoke one permission level for one connector.
///
/// Enforced in Rust the moment the next action is attempted, not by hiding a
/// button: the UI reflects the grant, it does not implement it.
#[tauri::command(async)]
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
#[tauri::command(async)]
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
#[tauri::command(async)]
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
#[tauri::command(async)]
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
#[tauri::command(async)]
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
#[tauri::command(async)]
pub fn nexus_assistant_resolve(session: State<'_, AssistantSession>, phrase: String) -> Resolution {
    session.resolve(&phrase)
}

/// Return to rest once a finished turn has been shown.
#[tauri::command(async)]
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
#[tauri::command(async)]
pub fn nexus_assistant_remember_list(
    session: State<'_, AssistantSession>,
    items: Vec<u64>,
) -> Option<u64> {
    session.remember_list(items)
}

/// Forget the conversation. The user's "start again".
#[tauri::command(async)]
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
#[tauri::command(async)]
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
#[tauri::command(async)]
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

#[tauri::command(async)]
pub fn nexus_list_contacts(state: State<'_, DbState>) -> Result<Vec<Contact>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    list_contacts(&conn)
}

#[tauri::command(async)]
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

#[tauri::command(async)]
pub fn nexus_delete_contact(
    state: State<'_, DbState>,
    id: i64,
) -> Result<Vec<Contact>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    delete_contact(&conn, id)?;
    list_contacts(&conn)
}

/// Connectors that may hold a secret, and the Keychain service each uses.
///
/// A fixed list rather than `format!("nexus-{connector_id}")`, so no caller
/// can write to an arbitrary Keychain service by naming one.
const SECRET_SERVICES: &[(&str, &str)] = &[
    ("jira", "nexus-jira"),
    ("github", "nexus-github"),
];

/// Store a credential for a connector, in the Keychain.
///
/// This existed in Rust and was never exposed, which is why setting Jira up
/// meant running `security add-generic-password` by hand. That is a bad
/// answer twice over: it asks the user to operate a credential tool they
/// should not have to know about, and `security`'s own prompt silently
/// truncates at 128 characters, which is exactly how a valid 192-character
/// token came to be stored as two thirds of itself.
///
/// `store_keychain_secret` reads the value back and compares, so a truncated
/// write is reported here rather than discovered as a mysterious 401 later.
///
/// The secret is never written to `nexus.db`: `set_connector_config` refuses
/// keys that look like secrets, and this is the channel that exists instead.
#[tauri::command(async)]
pub fn nexus_store_connector_secret(
    connector_id: String,
    account: String,
    secret: String,
) -> Result<(), String> {
    let service = SECRET_SERVICES
        .iter()
        .find(|(id, _)| *id == connector_id)
        .map(|(_, service)| *service)
        .ok_or_else(|| format!("{connector_id} does not hold a credential."))?;

    let account = account.trim();
    if account.is_empty() {
        return Err("Which account is this credential for?".to_string());
    }
    if secret.trim().is_empty() {
        return Err("There was no credential to store.".to_string());
    }

    crate::assistant::http::store_keychain_secret(service, account, secret.trim())?;
    // Same reason as in `nexus_set_connector_config`: a stored credential is
    // the event that makes a cached "not signed in" wrong.
    if connector_id == "github" {
        crate::assistant::github_connector::forget_ready();
    }
    Ok(())
}

/// Whether a connector already holds a credential for an account.
///
/// Returns only whether one is present, never the value. The Settings screen
/// needs to show "stored" without being a way to read it back out.
#[tauri::command(async)]
pub fn nexus_connector_secret_present(
    connector_id: String,
    account: String,
) -> Result<bool, String> {
    let service = SECRET_SERVICES
        .iter()
        .find(|(id, _)| *id == connector_id)
        .map(|(_, service)| *service)
        .ok_or_else(|| format!("{connector_id} does not hold a credential."))?;
    Ok(crate::assistant::http::keychain_secret(service, account.trim()).is_some())
}

// -- A manually rotated credential -------------------------------------------
//
// Distinct from the connector secrets above, which back a connector's own
// sign-in. This is a credential the user holds and refreshes on a schedule of
// their own (a service whose password rotates weekly, entered by hand), so
// what matters here that does not matter there is *when* it was last set. The
// value lives in the Keychain like any other; the timestamp lives in settings
// so the card can say "due for its weekly refresh" without ever reading the
// value back to check.

/// The one Keychain service every vault entry shares. The label is the
/// account, so several credentials coexist without a service each.
const VAULT_SERVICE: &str = "nexus-vault";

/// Labels the vault will accept. An allow-list for the same reason the
/// connector secrets have one: a command that will write any Keychain service
/// it is handed is a foothold, so the set of writable names is fixed in code.
const VAULT_LABELS: &[&str] = &["kai-connect"];

fn vault_timestamp_key(label: &str) -> String {
    format!("vault_updated_{label}")
}

/// Store or replace a vault credential, and stamp the time it happened.
///
/// The value is verified on the way in by `store_keychain_secret`, which reads
/// it back and refuses a truncated write: the same guard that caught the Jira
/// token losing 64 characters to the `security` prompt. Only once the value is
/// safely stored is the timestamp recorded, so a failed write never looks
/// fresh.
#[tauri::command(async)]
pub fn nexus_vault_set(
    state: State<'_, DbState>,
    label: String,
    secret: String,
) -> Result<(), String> {
    let label = label.trim();
    if !VAULT_LABELS.contains(&label) {
        return Err(format!("{label} is not a credential NEXUS holds."));
    }
    if secret.trim().is_empty() {
        return Err("There was no credential to store.".to_string());
    }

    crate::assistant::http::store_keychain_secret(VAULT_SERVICE, label, secret.trim())?;

    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, strftime('%s','now'))
         ON CONFLICT(key) DO UPDATE
           SET value = strftime('%s','now'),
               updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        rusqlite::params![vault_timestamp_key(label)],
    )
    .map_err(|e| format!("Stored the credential but could not record when: {e}"))?;
    Ok(())
}

/// Whether a vault credential is present, and when it was last set.
///
/// Never returns the value. The card needs to show "stored, updated 6 days
/// ago" and to nudge a refresh; it never needs, and is never given, the
/// secret itself.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultStatus {
    pub label: String,
    pub present: bool,
    /// Unix seconds of the last successful set, or null if never set.
    pub updated_at: Option<i64>,
}

#[tauri::command(async)]
pub fn nexus_vault_status(
    state: State<'_, DbState>,
    label: String,
) -> Result<VaultStatus, String> {
    let label = label.trim();
    if !VAULT_LABELS.contains(&label) {
        return Err(format!("{label} is not a credential NEXUS holds."));
    }
    let present = crate::assistant::http::keychain_secret(VAULT_SERVICE, label).is_some();

    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    let updated_at: Option<i64> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [vault_timestamp_key(label)],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .and_then(|v| v.parse().ok());

    Ok(VaultStatus {
        label: label.to_string(),
        present,
        // A timestamp with no credential behind it is stale bookkeeping from a
        // deleted entry, not a real "last set". Do not report it as one.
        updated_at: if present { updated_at } else { None },
    })
}

/// Store a connector's configuration: endpoints and account names.
///
/// Secrets are refused here and belong in the macOS Keychain. NEXUS never
/// writes a token to its database.
#[tauri::command(async)]
pub fn nexus_set_connector_config(
    state: State<'_, DbState>,
    connector_id: String,
    config: serde_json::Value,
) -> Result<Vec<ConnectorView>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    set_connector_config(&conn, &connector_id, &config)?;
    // GitHub's status is cached for a minute because asking costs a network
    // round trip. Changing its configuration is exactly the moment that
    // cache is wrong, and a user who has just fixed their account should not
    // have to wait out a TTL to see it.
    if connector_id == "github" {
        crate::assistant::github_connector::forget_ready();
    }
    list_connectors(&conn)
}

// -- NEXUS-019: the reasoning layer ------------------------------------------

/// What reasoning NEXUS may do, and what it has done.
#[tauri::command(async)]
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
#[tauri::command(async)]
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
#[tauri::command(async)]
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
#[tauri::command(async)]
pub fn nexus_list_suggestions(state: State<'_, DbState>) -> Result<Vec<Suggestion>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    Ok(generate(&conn))
}

/// Stop raising one.
#[tauri::command(async)]
pub fn nexus_dismiss_suggestion(
    state: State<'_, DbState>,
    key: String,
) -> Result<Vec<Suggestion>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    dismiss(&conn, &key)?;
    Ok(generate(&conn))
}

/// Start raising it again.
#[tauri::command(async)]
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
#[tauri::command(async)]
pub fn nexus_surface_suggestions(state: State<'_, DbState>) -> Result<Vec<Suggestion>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    Ok(surface(&conn))
}

/// The same list, without starting anyone's cooldown. For polling.
#[tauri::command(async)]
pub fn nexus_preview_suggestions(state: State<'_, DbState>) -> Result<Vec<Suggestion>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    Ok(preview(&conn))
}

/// Where things stand, from local data only.
#[tauri::command(async)]
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

#[tauri::command(async)]
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

#[tauri::command(async)]
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
#[tauri::command(async)]
pub fn nexus_list_commitments(
    state: State<'_, DbState>,
    open_only: bool,
) -> Result<Vec<crate::db::commitments::Commitment>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    crate::db::commitments::list(&conn, open_only)
}

#[tauri::command(async)]
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
#[tauri::command(async)]
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

/// Push a reminder out to a new time from the panel.
///
/// Reuses `defer`, which also clears `raised_at` and reopens the row, so this
/// serves both cases the user means by "extend": an armed reminder getting
/// more runway, and one already told that they want brought back. Either way
/// the new time has to be in the future, for the same reason `create` checks:
/// a time in the past is a row that can never fire.
#[tauri::command(async)]
pub fn nexus_defer_commitment(
    state: State<'_, DbState>,
    id: i64,
    due_at: i64,
) -> Result<crate::db::commitments::Commitment, String> {
    if due_at <= unix_now() {
        return Err("That time has already passed.".to_string());
    }
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    crate::db::commitments::defer(&conn, id, due_at)?;
    crate::db::commitments::get(&conn, id)
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

#[tauri::command(async)]
pub fn nexus_notifications_poll(
    state: State<'_, DbState>,
    session: State<'_, AssistantSession>,
    approvals: State<'_, ApprovalStore>,
) -> Result<NotificationPoll, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    poll_notifications(&conn, &session, &approvals)
}

/// Tell NEXUS a suggestion was acted on, which resets how tired of it it is.
#[tauri::command(async)]
pub fn nexus_accept_suggestion(state: State<'_, DbState>, key: String) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    record_accepted(&conn, &key)
}

#[tauri::command(async)]
pub fn nexus_proactive_policy(state: State<'_, DbState>) -> Result<ProactivePolicy, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    Ok(read_proactive(&conn))
}

#[tauri::command(async)]
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
#[tauri::command(async)]
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
#[tauri::command(async)]
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
#[tauri::command(async)]
pub fn nexus_set_user_name(
    state: State<'_, DbState>,
    name: String,
) -> Result<Option<String>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    crate::assistant::converse::set_user_name(&conn, &name)?;
    Ok(crate::assistant::converse::user_name(&conn))
}

#[tauri::command(async)]
pub fn nexus_user_name(state: State<'_, DbState>) -> Result<Option<String>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    Ok(crate::assistant::converse::user_name(&conn))
}

/// The result of one stock check.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StockState {
    /// `Some(true)` buyable, `Some(false)` out of stock, `None` could not tell
    /// (a bot wall, a redirect, an error page). The caller must not treat
    /// unknown as a change in either direction.
    pub available: Option<bool>,
    /// The product's name, when the page gave one, for the alert to say.
    pub title: Option<String>,
}

/// Check whether a product URL is in stock.
///
/// A plain command, not an action through the gate, for the same reason the
/// reminder composer is: the user set this watch up in the UI, so the check it
/// asked for needs no second permission. It only ever reads a public page.
///
/// Fetched with a real browser user-agent, because a default `curl` UA is the
/// first thing a bot wall turns away. A non-product response (too small, or
/// missing the buy box and the unavailable text both) comes back as unknown
/// rather than as a confident answer, so the watcher never announces a restock
/// that is really just Amazon blocking the request.
/// Read a product's availability from an open Chrome tab, if one is showing it.
///
/// On a network that drops this app's own fetch, a corporate proxy that steers
/// by application, the browser is the only path to the page: Chrome's traffic
/// is allowed where NEXUS's is not. It also reads the rendered DOM rather than
/// raw HTML, so the buy box is read after the page's own scripts have settled.
///
/// Returns `None` when no matching tab is open or Chrome cannot be reached, so
/// the caller falls back to a direct fetch.
fn read_stock_from_chrome(url: &str) -> Option<StockState> {
    use crate::assistant::{browser_connector, shell::osascript, stock_watch};

    let asin = stock_watch::asin_of(url);
    let tabs = osascript(&browser_connector::list_tabs_script(), &[]).ok()?;
    if !tabs.success {
        return None;
    }
    let tab = browser_connector::parse_tab_listing(&tabs.stdout)
        .into_iter()
        .find(|t| match asin.as_deref() {
            Some(a) => t.url.contains(a),
            None => t.url == url,
        })?;

    // One expression, single-quoted throughout so it needs no escaping inside
    // the AppleScript string. Emits flag<TAB>title.
    let js = "(function(){\
        var b=!!document.getElementById('add-to-cart-button')||!!document.getElementById('buy-now-button');\
        var t=document.getElementById('productTitle');var ti=t?t.innerText.trim():'';\
        var u=(document.body.innerText||'').indexOf('Currently unavailable')>=0;\
        return (b?'A':(u?'U':'?'))+String.fromCharCode(9)+ti;})()";
    let script = format!(
        "tell application \"Google Chrome\" to execute tab {} of window {} javascript \"{}\"",
        tab.tab_index, tab.window_index, js
    );
    let out = osascript(&[&script], &[]).ok()?;
    if !out.success {
        log::warn!("check_stock: chrome read failed: {}", out.stderr.trim());
        return None;
    }

    let (flag, title) = out.stdout.split_once('\t').unwrap_or((out.stdout.trim(), ""));
    let available = stock_watch::availability_from_flag(flag);
    log::info!("check_stock: read from Chrome tab, availability {available:?}");
    Some(StockState {
        available,
        title: {
            let t = title.trim();
            (!t.is_empty()).then(|| t.to_string())
        },
    })
}

#[tauri::command(async)]
pub fn nexus_check_stock(url: String) -> Result<StockState, String> {
    const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
        AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
    log::info!("check_stock: called for {} chars", url.len());
    // Only http(s), and no argument injection: the scheme check keeps a
    // `file://` or a `-` option from reaching curl as a URL.
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        log::warn!("check_stock: rejected non-http url");
        return Err("That is not a web address.".to_string());
    }

    // The open tab first. On the user's network the app's own fetch is
    // dropped, so the browser is the reliable reader; and even where the fetch
    // works, a tab already on the page is the faster answer.
    if let Some(state) = read_stock_from_chrome(&url) {
        if state.available.is_some() {
            return Ok(state);
        }
    }

    // Amazon returns a bot-check stub to some automated fetches and the real
    // page to others, intermittently. A single fetch therefore reads "unknown"
    // at random. Retrying a couple of times when the answer is unknown turns a
    // flaky check into a reliable one without hammering: it stops the moment a
    // fetch comes back readable.
    let mut last: Option<bool> = None;
    let mut title: Option<String> = None;
    for attempt in 0..2 {
        let out = crate::assistant::shell::run(
            "/usr/bin/curl",
            &[
                "-sL",
                // Short, because on a network that drops this app's traffic the
                // connection hangs rather than refuses, and a long timeout just
                // makes the failure slow. Eight seconds is plenty for a network
                // that is not blocking it.
                "--max-time",
                "8",
                "-A",
                UA,
                "-H",
                "Accept-Language: en-IN,en;q=0.9",
                "--",
                &url,
            ],
            std::time::Duration::from_secs(10),
        );
        let out = match out {
            Ok(o) => o,
            Err(e) => {
                // Log and keep going rather than returning: a curl that could
                // not even run (blocked, hung, killed) on one attempt is worth
                // recording, and the retry might still succeed.
                log::warn!("check_stock: curl error on attempt {attempt}: {e}");
                continue;
            }
        };

        if !out.success {
            log::warn!(
                "check_stock: curl exited non-zero on attempt {attempt}: {}",
                out.stderr.chars().take(120).collect::<String>()
            );
            continue;
        }

        last = crate::assistant::stock_watch::availability(&out.stdout);
        if title.is_none() {
            title = crate::assistant::stock_watch::product_title(&out.stdout);
        }
        log::info!(
            "check_stock: attempt {attempt}, {} bytes, availability {:?}",
            out.stdout.len(),
            last
        );
        if last.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(600));
    }

    Ok(StockState {
        available: last,
        title,
    })
}

/// The single key under which the one auto-send number lives.
const ALERT_NUMBER_KEY: &str = "whatsapp_alert_number";

/// Save the number NEXUS may send alerts to, or clear it with an empty string.
///
/// There is exactly one, and it is the whole of the safety story for
/// unattended sending: `nexus_whatsapp_send_alert` reads this and nothing
/// else, so an alert can never reach a number the user did not set here
/// themselves. Validated the same way a composed message's recipient is, so a
/// malformed number is refused at the door rather than handed to `open`.
#[tauri::command(async)]
pub fn nexus_set_whatsapp_alert_number(
    state: State<'_, DbState>,
    phone: String,
) -> Result<Option<String>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    let trimmed = phone.trim();
    if trimmed.is_empty() {
        conn.execute(
            "DELETE FROM settings WHERE key = ?1",
            rusqlite::params![ALERT_NUMBER_KEY],
        )
        .map_err(|e| format!("Failed to clear the alert number: {e}"))?;
        return Ok(None);
    }
    let normalised = crate::assistant::whatsapp_connector::valid_phone(trimmed)
        .ok_or_else(|| "That is not an international phone number.".to_string())?;
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE
           SET value = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        rusqlite::params![ALERT_NUMBER_KEY, normalised],
    )
    .map_err(|e| format!("Failed to save the alert number: {e}"))?;
    Ok(Some(normalised))
}

/// The saved alert number, if one is set.
#[tauri::command(async)]
pub fn nexus_whatsapp_alert_number(state: State<'_, DbState>) -> Result<Option<String>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    Ok(conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            rusqlite::params![ALERT_NUMBER_KEY],
            |r| r.get::<_, String>(0),
        )
        .ok())
}

/// Send an alert message to the saved number, unattended.
///
/// No phone parameter by design: the destination is read from the one saved
/// number, so this command structurally cannot send anywhere else. Called by
/// the stock watcher when an item it is watching comes back in stock. Returns
/// an error the card can show if no number has been set, rather than silently
/// doing nothing.
#[tauri::command(async)]
pub fn nexus_whatsapp_send_alert(state: State<'_, DbState>, message: String) -> Result<(), String> {
    let number = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        conn.query_row(
            "SELECT value FROM settings WHERE key = ?1",
            rusqlite::params![ALERT_NUMBER_KEY],
            |r| r.get::<_, String>(0),
        )
        .ok()
    };
    // The lock is released before the send: `send_now` sleeps nearly three
    // seconds waiting for WhatsApp, and holding the database for that would
    // freeze every other command.
    let number = number.ok_or_else(|| {
        log::warn!("whatsapp_send_alert: no saved number");
        "No WhatsApp alert number is set. Add one in the Stock Watch card first.".to_string()
    })?;
    log::info!("whatsapp_send_alert: sending to saved number");
    crate::assistant::whatsapp_connector::send_now(&number, &message)
}

/// The key under which the one ntfy topic lives.
const NTFY_TOPIC_KEY: &str = "ntfy_topic";

/// Save the ntfy topic NEXUS publishes alerts to, or clear it with an empty
/// string.
///
/// Like the WhatsApp number, there is exactly one, and it is the whole of where
/// an alert can go: `nexus_ntfy_send_alert` reads this and nothing else.
/// Validated to ntfy's own character set here so a malformed topic is refused
/// before it is ever pasted into a URL.
#[tauri::command(async)]
pub fn nexus_set_ntfy_topic(
    state: State<'_, DbState>,
    topic: String,
) -> Result<Option<String>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    let trimmed = topic.trim();
    if trimmed.is_empty() {
        conn.execute(
            "DELETE FROM settings WHERE key = ?1",
            rusqlite::params![NTFY_TOPIC_KEY],
        )
        .map_err(|e| format!("Failed to clear the ntfy topic: {e}"))?;
        return Ok(None);
    }
    let normalised = crate::assistant::ntfy_connector::valid_topic(trimmed).ok_or_else(|| {
        "That is not a valid ntfy topic. Use letters, numbers, - and _ only.".to_string()
    })?;
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE
           SET value = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        rusqlite::params![NTFY_TOPIC_KEY, normalised],
    )
    .map_err(|e| format!("Failed to save the ntfy topic: {e}"))?;
    Ok(Some(normalised))
}

/// The saved ntfy topic, if one is set.
#[tauri::command(async)]
pub fn nexus_ntfy_topic(state: State<'_, DbState>) -> Result<Option<String>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    Ok(conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            rusqlite::params![NTFY_TOPIC_KEY],
            |r| r.get::<_, String>(0),
        )
        .ok())
}

/// Publish an alert to the saved ntfy topic.
///
/// No topic parameter by design, the same shape as the WhatsApp alert: the
/// destination is the one saved topic, so this command structurally cannot
/// publish anywhere else. Called by the stock watcher alongside the WhatsApp
/// send; each channel is independent so one failing does not stop the other.
#[tauri::command(async)]
pub fn nexus_ntfy_send_alert(
    state: State<'_, DbState>,
    title: String,
    message: String,
) -> Result<(), String> {
    let topic = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        conn.query_row(
            "SELECT value FROM settings WHERE key = ?1",
            rusqlite::params![NTFY_TOPIC_KEY],
            |r| r.get::<_, String>(0),
        )
        .ok()
    };
    // Lock released before the network call: a slow POST must not freeze the
    // database for every other command.
    let topic = topic.ok_or_else(|| {
        log::warn!("ntfy_send_alert: no saved topic");
        "No ntfy topic is set. Add one in the Stock Watch card first.".to_string()
    })?;
    log::info!("ntfy_send_alert: publishing to saved topic");
    crate::assistant::ntfy_connector::send_alert(&topic, &title, &message)
}

/// The whole personal profile, as a key to value map.
#[tauri::command(async)]
pub fn nexus_get_profile(
    state: State<'_, DbState>,
) -> Result<std::collections::BTreeMap<String, String>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    Ok(crate::assistant::profile::get_all(&conn))
}

/// Set one profile field, or clear it with a blank value. Returns the stored
/// value so the field reflects exactly what was kept.
#[tauri::command(async)]
pub fn nexus_set_profile_field(
    state: State<'_, DbState>,
    key: String,
    value: String,
) -> Result<String, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    crate::assistant::profile::set_field(&conn, &key, &value)
}

// -- Money: portfolio ---------------------------------------------------------

/// One holding with its live value filled in.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HoldingView {
    pub id: i64,
    pub symbol: String,
    pub label: String,
    pub quantity: f64,
    /// "stock" | "mf" | "manual", so the card can show units vs a flat value.
    pub kind: String,
    pub price: Option<f64>,
    pub prev_close: Option<f64>,
    pub currency: String,
    /// quantity * price, when a price was fetched (or the entered value, manual).
    pub value: Option<f64>,
    /// Today's change in value: quantity * (price - prevClose).
    pub day_change: Option<f64>,
    /// Why this holding has no price, if it has none.
    pub error: Option<String>,
    /// Where it is held: "ICICI Direct", "Zerodha", a ULIP policy; "" if unset.
    pub broker: String,
    /// What was paid for it in total, when known.
    pub invested: Option<f64>,
    /// For a price the user entered (a ULIP NAV): the date it is from.
    pub price_as_of: Option<String>,
}

/// Holdings in one place, totalled.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortfolioGroup {
    pub name: String,
    pub value: f64,
    /// Money put in: the group's own (ULIP premiums), else the sum of its
    /// holdings' when every one is known; `None` rather than a partial sum.
    pub invested: Option<f64>,
    pub gain: Option<f64>,
}

/// The whole portfolio, priced live.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Portfolio {
    pub holdings: Vec<HoldingView>,
    pub total_value: f64,
    pub total_day_change: f64,
    pub currency: String,
    pub groups: Vec<PortfolioGroup>,
}

/// Add a holding of a given kind, validating the identifier for that kind so a
/// typo is caught at the door rather than as a blank row later.
///
/// - `stock`: `symbol` is a market ticker (Yahoo), `quantity` is units.
/// - `mf`: `symbol` is an AMFI scheme code, `quantity` is units.
/// - `manual`: `symbol` is ignored, `quantity` is the rupee value (ULIP, cash,
///   anything with no feed). `label` names it.
#[tauri::command(async)]
pub fn nexus_add_holding(
    state: State<'_, DbState>,
    symbol: String,
    label: String,
    quantity: f64,
    kind: Option<String>,
) -> Result<i64, String> {
    let kind = kind.unwrap_or_else(|| "stock".to_string());
    if !(quantity.is_finite() && quantity > 0.0) {
        return Err("Enter a positive amount.".to_string());
    }

    // Store a cleaned identifier appropriate to the kind.
    let stored_symbol = match kind.as_str() {
        "stock" => crate::assistant::market_connector::valid_symbol(&symbol)
            .ok_or_else(|| format!("\"{symbol}\" is not a valid market symbol."))?,
        "mf" => crate::assistant::mf_connector::valid_code(&symbol)
            .ok_or_else(|| format!("\"{symbol}\" is not a valid fund scheme code."))?,
        "manual" => String::new(),
        other => return Err(format!("Unknown holding kind \"{other}\".")),
    };
    if kind == "manual" && label.trim().is_empty() {
        return Err("A manual holding needs a label.".to_string());
    }

    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    conn.execute(
        "INSERT INTO holdings (symbol, label, quantity, kind) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![stored_symbol, label.trim(), quantity, kind],
    )
    .map_err(|e| format!("Could not add the holding: {e}"))?;
    Ok(conn.last_insert_rowid())
}

/// Search Indian mutual funds by name, for picking one to add.
#[tauri::command(async)]
pub fn nexus_search_funds(
    query: String,
) -> Result<Vec<crate::assistant::mf_connector::FundMatch>, String> {
    crate::assistant::mf_connector::search(&query)
}

// -- Money: goals -------------------------------------------------------------

/// Every goal, with progress and the monthly saving needed worked out against
/// today's date. Computed, never stored, so the numbers are never stale.
#[tauri::command(async)]
pub fn nexus_list_goals(
    state: State<'_, DbState>,
) -> Result<Vec<crate::assistant::finance::GoalProgress>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let mut stmt = conn
        .prepare(
            "SELECT id, name, target_amount, current_amount, target_date, expected_return
             FROM goals ORDER BY target_date",
        )
        .map_err(|e| format!("Query error: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(crate::assistant::finance::goal_progress(
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, f64>(2)?,
                r.get::<_, f64>(3)?,
                &r.get::<_, String>(4)?,
                r.get::<_, f64>(5)?,
                now,
            ))
        })
        .map_err(|e| format!("Query error: {e}"))?;
    Ok(rows.filter_map(Result::ok).collect())
}

/// Add a goal. Returns the new id.
#[tauri::command(async)]
pub fn nexus_add_goal(
    state: State<'_, DbState>,
    name: String,
    target_amount: f64,
    target_date: String,
    expected_return: f64,
) -> Result<i64, String> {
    if name.trim().is_empty() {
        return Err("A goal needs a name.".to_string());
    }
    if !(target_amount.is_finite() && target_amount > 0.0) {
        return Err("The target amount must be positive.".to_string());
    }
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    conn.execute(
        "INSERT INTO goals (name, target_amount, target_date, expected_return)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![name.trim(), target_amount, target_date.trim(), expected_return],
    )
    .map_err(|e| format!("Could not add the goal: {e}"))?;
    Ok(conn.last_insert_rowid())
}

/// Update how much is saved toward a goal.
#[tauri::command(async)]
pub fn nexus_update_goal_amount(
    state: State<'_, DbState>,
    id: i64,
    current_amount: f64,
) -> Result<(), String> {
    if !(current_amount.is_finite() && current_amount >= 0.0) {
        return Err("The saved amount cannot be negative.".to_string());
    }
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    conn.execute(
        "UPDATE goals SET current_amount = ?2 WHERE id = ?1",
        rusqlite::params![id, current_amount],
    )
    .map_err(|e| format!("Could not update the goal: {e}"))?;
    Ok(())
}

/// Remove a goal.
#[tauri::command(async)]
pub fn nexus_remove_goal(state: State<'_, DbState>, id: i64) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    conn.execute("DELETE FROM goals WHERE id = ?1", rusqlite::params![id])
        .map_err(|e| format!("Could not remove the goal: {e}"))?;
    Ok(())
}

/// Project a retirement outcome from the supplied assumptions. Pure math over
/// the inputs; nothing is stored here (the inputs live in the profile).
#[tauri::command(async)]
pub fn nexus_retirement_projection(
    input: crate::assistant::finance::RetirementInput,
) -> Result<crate::assistant::finance::RetirementProjection, String> {
    Ok(crate::assistant::finance::project_retirement(&input))
}

// -- Money: bills & tax dates -------------------------------------------------

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Every recurring obligation with its next due date and days remaining,
/// soonest first.
#[tauri::command(async)]
pub fn nexus_list_bills(
    state: State<'_, DbState>,
) -> Result<Vec<crate::assistant::bills::BillView>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    let now = now_unix();
    let mut stmt = conn
        .prepare("SELECT id, name, kind, amount, recurrence, anchor_date FROM bills")
        .map_err(|e| format!("Query error: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<f64>>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
            ))
        })
        .map_err(|e| format!("Query error: {e}"))?;
    let mut out: Vec<_> = rows
        .filter_map(Result::ok)
        .filter_map(|(id, name, kind, amount, recurrence, anchor)| {
            crate::assistant::bills::bill_view(id, name, kind, amount, &recurrence, &anchor, now)
        })
        .collect();
    out.sort_by_key(|b| b.days_until);
    Ok(out)
}

/// Add a recurring obligation.
#[tauri::command(async)]
pub fn nexus_add_bill(
    state: State<'_, DbState>,
    name: String,
    kind: String,
    amount: Option<f64>,
    recurrence: String,
    anchor_date: String,
) -> Result<i64, String> {
    if name.trim().is_empty() {
        return Err("A reminder needs a name.".to_string());
    }
    if anchor_date.trim().is_empty() {
        return Err("Pick the due date.".to_string());
    }
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    conn.execute(
        "INSERT INTO bills (name, kind, amount, recurrence, anchor_date)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![name.trim(), kind.trim(), amount, recurrence.trim(), anchor_date.trim()],
    )
    .map_err(|e| format!("Could not add the reminder: {e}"))?;
    Ok(conn.last_insert_rowid())
}

/// Remove a reminder.
#[tauri::command(async)]
pub fn nexus_remove_bill(state: State<'_, DbState>, id: i64) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    conn.execute("DELETE FROM bills WHERE id = ?1", rusqlite::params![id])
        .map_err(|e| format!("Could not remove the reminder: {e}"))?;
    Ok(())
}

/// The upcoming India statutory tax dates (computed, not stored).
#[tauri::command(async)]
pub fn nexus_tax_dates() -> Result<Vec<crate::assistant::bills::TaxDate>, String> {
    Ok(crate::assistant::bills::upcoming_tax_dates(now_unix(), 5))
}

// -- Learn --------------------------------------------------------------------

/// Ask the local model to write study material, with the user's profile as
/// context.
///
/// Goes through the content-writing path, not the action planner: under the
/// planner's prompt a request to "write a plan" or "create flashcards" comes
/// back as invented actions instead of text. The profile is prepended so the
/// model writes for this person (their role, their focus). The model name and
/// profile are read under the lock, which is released before the call: a local
/// model takes seconds and must not freeze the database.
fn ask_local_model(state: &State<'_, DbState>, request: String) -> Result<String, String> {
    let (model, profile) = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        (
            crate::assistant::ollama_provider::local_model(&conn),
            crate::assistant::profile::as_context_lines(&conn),
        )
    };
    let prompt = if profile.is_empty() {
        request
    } else {
        format!("About the learner:\n{}\n\n{request}", profile.join("\n"))
    };
    let text = crate::assistant::ollama_provider::generate_text(&model, &prompt)?;
    if text.trim().is_empty() {
        return Err("The model returned nothing. Try again.".to_string());
    }
    Ok(text)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSummary {
    pub id: i64,
    pub name: String,
    pub topics_total: i64,
    pub topics_done: i64,
    pub cards_total: i64,
    pub cards_due: i64,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicView {
    pub id: i64,
    pub title: String,
    pub done: bool,
    pub cards: i64,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CardView {
    pub id: i64,
    pub skill_id: i64,
    pub skill_name: String,
    pub front: String,
    pub back: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LearnStats {
    pub streak: i64,
    pub due_today: i64,
    pub reviewed_today: i64,
}

fn today_day() -> i64 {
    crate::assistant::learn::day_of(now_unix())
}

/// Every skill with its plan progress and how many cards are due.
#[tauri::command(async)]
pub fn nexus_list_skills(state: State<'_, DbState>) -> Result<Vec<SkillSummary>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    let today = today_day();
    let mut stmt = conn
        .prepare(
            "SELECT s.id, s.name,
                (SELECT COUNT(*) FROM skill_topics t WHERE t.skill_id = s.id),
                (SELECT COUNT(*) FROM skill_topics t WHERE t.skill_id = s.id AND t.done = 1),
                (SELECT COUNT(*) FROM cards c WHERE c.skill_id = s.id),
                (SELECT COUNT(*) FROM cards c WHERE c.skill_id = s.id AND c.due_day <= ?1)
             FROM skills s ORDER BY s.id",
        )
        .map_err(|e| format!("Query error: {e}"))?;
    let rows = stmt
        .query_map([today], |r| {
            Ok(SkillSummary {
                id: r.get(0)?,
                name: r.get(1)?,
                topics_total: r.get(2)?,
                topics_done: r.get(3)?,
                cards_total: r.get(4)?,
                cards_due: r.get(5)?,
            })
        })
        .map_err(|e| format!("Query error: {e}"))?;
    Ok(rows.filter_map(Result::ok).collect())
}

/// Add a skill. The plan is drafted by a separate call so the UI can show the
/// skill immediately while the model works.
#[tauri::command(async)]
pub fn nexus_add_skill(state: State<'_, DbState>, name: String) -> Result<i64, String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 80 {
        return Err("Name the skill in a few words.".to_string());
    }
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    conn.execute("INSERT INTO skills (name) VALUES (?1)", rusqlite::params![name])
        .map_err(|e| format!("Could not add the skill: {e}"))?;
    Ok(conn.last_insert_rowid())
}

/// Remove a skill and everything under it. Deleted explicitly rather than by
/// cascade, so it holds whether or not foreign keys are enforced.
#[tauri::command(async)]
pub fn nexus_remove_skill(state: State<'_, DbState>, id: i64) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    conn.execute(
        "DELETE FROM card_reviews WHERE card_id IN (SELECT id FROM cards WHERE skill_id = ?1)",
        [id],
    )
    .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM cards WHERE skill_id = ?1", [id])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM skill_topics WHERE skill_id = ?1", [id])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM skills WHERE id = ?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn read_topics(conn: &rusqlite::Connection, skill_id: i64) -> Result<Vec<TopicView>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT t.id, t.title, t.done,
                (SELECT COUNT(*) FROM cards c WHERE c.topic_id = t.id)
             FROM skill_topics t WHERE t.skill_id = ?1 ORDER BY t.position, t.id",
        )
        .map_err(|e| format!("Query error: {e}"))?;
    let rows = stmt
        .query_map([skill_id], |r| {
            Ok(TopicView {
                id: r.get(0)?,
                title: r.get(1)?,
                done: r.get::<_, i64>(2)? != 0,
                cards: r.get(3)?,
            })
        })
        .map_err(|e| format!("Query error: {e}"))?;
    Ok(rows.filter_map(Result::ok).collect())
}

/// A skill's plan, in order.
#[tauri::command(async)]
pub fn nexus_skill_topics(
    state: State<'_, DbState>,
    skill_id: i64,
) -> Result<Vec<TopicView>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    read_topics(&conn, skill_id)
}

fn skill_name(conn: &rusqlite::Connection, skill_id: i64) -> Result<String, String> {
    conn.query_row("SELECT name FROM skills WHERE id = ?1", [skill_id], |r| r.get(0))
        .map_err(|_| "That skill no longer exists.".to_string())
}

/// Draft (or redraft) a skill's plan with the local model.
///
/// Replaces the existing topics. Cards keep their content but lose their topic
/// link, so redrafting a plan never deletes study material.
#[tauri::command(async)]
pub fn nexus_generate_plan(
    state: State<'_, DbState>,
    skill_id: i64,
) -> Result<Vec<TopicView>, String> {
    let name = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        skill_name(&conn, skill_id)?
    };

    let text = ask_local_model(
        &state,
        format!(
            "List the topics to learn for the skill \"{name}\", for the learner \
             described above, in the order they should be learned, from \
             fundamentals to advanced. Give 6 to 10 topics. One topic per line, a few \
             words each, no numbering, no explanations, nothing else."
        ),
    )?;
    let topics = crate::assistant::learn::parse_topics(&text, 12);
    if topics.is_empty() {
        return Err("The model's plan could not be read. Try again.".to_string());
    }

    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    conn.execute("UPDATE cards SET topic_id = NULL WHERE skill_id = ?1", [skill_id])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM skill_topics WHERE skill_id = ?1", [skill_id])
        .map_err(|e| e.to_string())?;
    for (pos, title) in topics.iter().enumerate() {
        conn.execute(
            "INSERT INTO skill_topics (skill_id, title, position) VALUES (?1, ?2, ?3)",
            rusqlite::params![skill_id, title, pos as i64],
        )
        .map_err(|e| e.to_string())?;
    }
    read_topics(&conn, skill_id)
}

/// Mark a topic done or not.
#[tauri::command(async)]
pub fn nexus_set_topic_done(state: State<'_, DbState>, id: i64, done: bool) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    conn.execute(
        "UPDATE skill_topics SET done = ?2 WHERE id = ?1",
        rusqlite::params![id, done as i64],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn topic_with_skill(
    conn: &rusqlite::Connection,
    topic_id: i64,
) -> Result<(i64, String, String), String> {
    conn.query_row(
        "SELECT t.skill_id, t.title, s.name FROM skill_topics t
         JOIN skills s ON s.id = t.skill_id WHERE t.id = ?1",
        [topic_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .map_err(|_| "That topic no longer exists.".to_string())
}

/// A short explanation of a topic, written by the local model for this user.
#[tauri::command(async)]
pub fn nexus_explain_topic(state: State<'_, DbState>, topic_id: i64) -> Result<String, String> {
    let (_, title, skill) = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        topic_with_skill(&conn, topic_id)?
    };
    ask_local_model(
        &state,
        format!(
            "Explain \"{title}\" (part of learning {skill}) to the learner described \
             above. Plain language, about 150 words, one short concrete example. If you \
             are not sure of something, say so rather than guessing."
        ),
    )
}

/// Generate flashcards for a topic with the local model and add them to the
/// deck, due immediately. Returns how many were added.
#[tauri::command(async)]
pub fn nexus_generate_cards(state: State<'_, DbState>, topic_id: i64) -> Result<usize, String> {
    let (skill_id, title, skill) = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        topic_with_skill(&conn, topic_id)?
    };
    let text = ask_local_model(
        &state,
        format!(
            "Write 5 flashcards to test understanding of \"{title}\" (part of learning \
             {skill}). Each card is two lines: a line starting \"Q:\" with a short \
             question, then a line starting \"A:\" with a short, correct answer. Leave a \
             blank line between cards. Nothing else."
        ),
    )?;
    let drafts = crate::assistant::learn::parse_cards(&text, 10);
    if drafts.is_empty() {
        return Err("The model's cards could not be read. Try again.".to_string());
    }
    let today = today_day();
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    for d in &drafts {
        conn.execute(
            "INSERT INTO cards (skill_id, topic_id, front, back, due_day)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![skill_id, topic_id, d.front, d.back, today],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(drafts.len())
}

/// Cards due now, oldest-due first, optionally for one skill.
#[tauri::command(async)]
pub fn nexus_due_cards(
    state: State<'_, DbState>,
    skill_id: Option<i64>,
    limit: Option<i64>,
) -> Result<Vec<CardView>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    let limit = limit.unwrap_or(20).clamp(1, 100);
    let mut stmt = conn
        .prepare(
            "SELECT c.id, c.skill_id, s.name, c.front, c.back FROM cards c
             JOIN skills s ON s.id = c.skill_id
             WHERE c.due_day <= ?1 AND (?2 IS NULL OR c.skill_id = ?2)
             ORDER BY c.due_day, c.id LIMIT ?3",
        )
        .map_err(|e| format!("Query error: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params![today_day(), skill_id, limit], |r| {
            Ok(CardView {
                id: r.get(0)?,
                skill_id: r.get(1)?,
                skill_name: r.get(2)?,
                front: r.get(3)?,
                back: r.get(4)?,
            })
        })
        .map_err(|e| format!("Query error: {e}"))?;
    Ok(rows.filter_map(Result::ok).collect())
}

/// Record a review and reschedule the card.
#[tauri::command(async)]
pub fn nexus_review_card(state: State<'_, DbState>, id: i64, grade: String) -> Result<(), String> {
    use crate::assistant::learn::{review, Grade, Schedule};
    let grade = Grade::parse(&grade).ok_or("Grade must be again, good, or easy.")?;
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    let prev: Schedule = conn
        .query_row(
            "SELECT ease, interval, reps FROM cards WHERE id = ?1",
            [id],
            |r| {
                Ok(Schedule {
                    ease: r.get(0)?,
                    interval: r.get(1)?,
                    reps: r.get(2)?,
                })
            },
        )
        .map_err(|_| "That card no longer exists.".to_string())?;
    let next = review(prev, grade);
    let today = today_day();
    conn.execute(
        "UPDATE cards SET ease = ?2, interval = ?3, reps = ?4, due_day = ?5 WHERE id = ?1",
        rusqlite::params![id, next.ease, next.interval, next.reps, today + next.interval],
    )
    .map_err(|e| e.to_string())?;
    let label = match grade {
        Grade::Again => "again",
        Grade::Good => "good",
        Grade::Easy => "easy",
    };
    conn.execute(
        "INSERT INTO card_reviews (card_id, grade, day) VALUES (?1, ?2, ?3)",
        rusqlite::params![id, label, today],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// -- Career -------------------------------------------------------------------

const APPLICATION_STATUSES: [&str; 5] = ["saved", "applied", "interviewing", "offer", "rejected"];

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Application {
    pub id: i64,
    pub company: String,
    pub role: String,
    pub url: String,
    pub source: String,
    pub status: String,
    pub next_action: String,
    pub next_action_date: Option<String>,
    pub notes: String,
    pub updated_at: String,
    /// The job description tailoring works against.
    pub description: String,
    pub tailored_resume: String,
    pub cover_letter: String,
}

/// Find roles from the permitted sources and rank them against the profile.
///
/// The profile is read under the lock and released before the network: this
/// makes three requests and must not hold the database while it waits. Capped
/// at fifty, best first.
#[tauri::command(async)]
pub fn nexus_discover_jobs(
    state: State<'_, DbState>,
    query: String,
) -> Result<Vec<crate::assistant::career::ScoredJob>, String> {
    use crate::assistant::career::{fetch_jobs, rank, Seeker};
    let seeker = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        let profile = crate::assistant::profile::get_all(&conn);
        Seeker::from_profile(
            profile.get("skills").map(String::as_str),
            profile.get("role_target").map(String::as_str),
        )
    };
    let jobs = fetch_jobs(&query);
    if jobs.is_empty() {
        return Err("No listings could be read right now. Check the connection and try again."
            .to_string());
    }
    let mut ranked = rank(jobs, &seeker);
    ranked.truncate(50);
    Ok(ranked)
}

/// Every tracked application, most recently changed first.
#[tauri::command(async)]
pub fn nexus_list_applications(state: State<'_, DbState>) -> Result<Vec<Application>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    let mut stmt = conn
        .prepare(
            "SELECT id, company, role, url, source, status, next_action, next_action_date,
                    notes, updated_at, description, tailored_resume, cover_letter
             FROM job_applications ORDER BY updated_at DESC",
        )
        .map_err(|e| format!("Query error: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Application {
                id: r.get(0)?,
                company: r.get(1)?,
                role: r.get(2)?,
                url: r.get(3)?,
                source: r.get(4)?,
                status: r.get(5)?,
                next_action: r.get(6)?,
                next_action_date: r.get(7)?,
                notes: r.get(8)?,
                updated_at: r.get(9)?,
                description: r.get(10)?,
                tailored_resume: r.get(11)?,
                cover_letter: r.get(12)?,
            })
        })
        .map_err(|e| format!("Query error: {e}"))?;
    Ok(rows.filter_map(Result::ok).collect())
}

/// Track a role. A role already tracked by the same link is not duplicated; its
/// existing id is returned instead.
#[tauri::command(async)]
pub fn nexus_add_application(
    state: State<'_, DbState>,
    company: String,
    role: String,
    url: String,
    source: String,
    status: String,
    description: Option<String>,
) -> Result<i64, String> {
    if role.trim().is_empty() {
        return Err("Name the role.".to_string());
    }
    let status = if APPLICATION_STATUSES.contains(&status.as_str()) {
        status
    } else {
        "saved".to_string()
    };
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    let url = url.trim();
    if !url.is_empty() {
        if let Ok(existing) = conn.query_row(
            "SELECT id FROM job_applications WHERE url = ?1",
            [url],
            |r| r.get::<_, i64>(0),
        ) {
            return Ok(existing);
        }
    }
    conn.execute(
        "INSERT INTO job_applications (company, role, url, source, status, description)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            company.trim(),
            role.trim(),
            url,
            source.trim(),
            status,
            description.unwrap_or_default().trim()
        ],
    )
    .map_err(|e| format!("Could not track the role: {e}"))?;
    Ok(conn.last_insert_rowid())
}

/// Update an application's stage, next step, and notes.
#[tauri::command(async)]
pub fn nexus_update_application(
    state: State<'_, DbState>,
    id: i64,
    status: String,
    next_action: String,
    next_action_date: Option<String>,
    notes: String,
) -> Result<(), String> {
    if !APPLICATION_STATUSES.contains(&status.as_str()) {
        return Err(format!("Unknown stage \"{status}\"."));
    }
    let date = next_action_date.filter(|d| !d.trim().is_empty());
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    conn.execute(
        "UPDATE job_applications
            SET status = ?2, next_action = ?3, next_action_date = ?4, notes = ?5,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
          WHERE id = ?1",
        rusqlite::params![id, status, next_action.trim(), date, notes.trim()],
    )
    .map_err(|e| format!("Could not update: {e}"))?;
    Ok(())
}

/// Longest document NEXUS keeps (a resume is a page or two of text).
const MAX_DOCUMENT: usize = 20_000;

/// A stored long-form document (currently: the base resume), or "" if unset.
#[tauri::command(async)]
pub fn nexus_get_document(state: State<'_, DbState>, key: String) -> Result<String, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    Ok(conn
        .query_row("SELECT content FROM documents WHERE key = ?1", [key.trim()], |r| r.get(0))
        .unwrap_or_default())
}

/// Save a long-form document. Blank clears it.
#[tauri::command(async)]
pub fn nexus_set_document(
    state: State<'_, DbState>,
    key: String,
    content: String,
) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() || key.len() > 40 {
        return Err("Invalid document name.".to_string());
    }
    if content.len() > MAX_DOCUMENT {
        return Err(format!("That is longer than {MAX_DOCUMENT} characters."));
    }
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    if content.trim().is_empty() {
        conn.execute("DELETE FROM documents WHERE key = ?1", [key])
            .map_err(|e| e.to_string())?;
    } else {
        conn.execute(
            "INSERT INTO documents (key, content) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE
               SET content = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            rusqlite::params![key, content],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Set the job description an application is tailored against.
#[tauri::command(async)]
pub fn nexus_set_application_description(
    state: State<'_, DbState>,
    id: i64,
    description: String,
) -> Result<(), String> {
    if description.len() > MAX_DOCUMENT {
        return Err(format!("That is longer than {MAX_DOCUMENT} characters."));
    }
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    conn.execute(
        "UPDATE job_applications SET description = ?2 WHERE id = ?1",
        rusqlite::params![id, description.trim()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// The inputs tailoring needs: the base resume and the application's company,
/// role and job description. Errors say which one is missing.
fn tailoring_inputs(
    conn: &rusqlite::Connection,
    id: i64,
) -> Result<(String, String, String, String), String> {
    let resume: String = conn
        .query_row("SELECT content FROM documents WHERE key = 'resume'", [], |r| r.get(0))
        .unwrap_or_default();
    if resume.trim().is_empty() {
        return Err("Add your base resume first (Career, Resume tab).".to_string());
    }
    let (company, role, description): (String, String, String) = conn
        .query_row(
            "SELECT company, role, description FROM job_applications WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|_| "That application no longer exists.".to_string())?;
    if description.trim().is_empty() {
        return Err("Paste the job description for this role first.".to_string());
    }
    Ok((resume, company, role, description))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TailoredResume {
    /// Your resume, reordered for the role. Every line is yours.
    pub reordered: String,
    /// A short model-written summary, if the model produced one.
    pub summary: Option<String>,
    /// Summary claims the resume does not back up, to check before sending.
    pub warnings: Vec<String>,
    /// Bullets that moved up.
    pub moved: Vec<String>,
    /// Job words found in the resume.
    pub matched: Vec<String>,
    /// Why there is no summary, when there isn't one.
    pub note: Option<String>,
}

/// Tailor the base resume to an application.
///
/// The resume itself is only reordered (see `tailor`): nothing is added or
/// reworded, because a small model rewriting it invented a title. The one
/// generated part is a short summary, checked for claims the resume does not
/// support. If the model is unavailable the reordered resume still comes back.
#[tauri::command(async)]
pub fn nexus_tailor_resume(state: State<'_, DbState>, id: i64) -> Result<TailoredResume, String> {
    let (resume, model, (company, role, description)) = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        let (resume, company, role, description) = tailoring_inputs(&conn, id)?;
        (
            resume,
            crate::assistant::ollama_provider::local_model(&conn),
            (company, role, description),
        )
    };

    let reordered = crate::assistant::tailor::reorder(&resume, &description);

    let prompt = format!(
        "Write a 2-sentence professional summary for the top of this resume, aimed at the \
         {role} role at {company}. Use ONLY facts stated in the resume. Use the candidate's \
         actual current job title from the resume, not the title in the job ad. Do not \
         invent numbers, skills, or experience.\n\nRESUME:\n{resume}\n\nJOB:\n{description}"
    );
    let (summary, warnings, note) =
        match crate::assistant::ollama_provider::generate_text(&model, &prompt) {
            Ok(text) => {
                let warnings = crate::assistant::tailor::unsupported(
                    &text,
                    &resume,
                    &description,
                    &[role.as_str(), company.as_str()],
                );
                (Some(text), warnings, None)
            }
            Err(e) => (None, Vec::new(), Some(format!("No summary: {e}"))),
        };

    let saved = match &summary {
        Some(s) => format!("{s}\n\n{}", reordered.text),
        None => reordered.text.clone(),
    };
    {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        conn.execute(
            "UPDATE job_applications SET tailored_resume = ?2 WHERE id = ?1",
            rusqlite::params![id, saved],
        )
        .map_err(|e| e.to_string())?;
    }

    Ok(TailoredResume {
        reordered: reordered.text,
        summary,
        warnings,
        moved: reordered.moved,
        matched: reordered.matched,
        note,
    })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverLetter {
    pub text: String,
    /// Claims the resume does not back up, to check before sending.
    pub warnings: Vec<String>,
}

/// Draft a cover letter for an application, checked for unsupported claims.
#[tauri::command(async)]
pub fn nexus_cover_letter(state: State<'_, DbState>, id: i64) -> Result<CoverLetter, String> {
    let (resume, model, company, role, description, name) = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        let (resume, company, role, description) = tailoring_inputs(&conn, id)?;
        let name = crate::assistant::profile::get_all(&conn)
            .get("display_name")
            .cloned()
            .unwrap_or_default();
        (
            resume,
            crate::assistant::ollama_provider::local_model(&conn),
            company,
            role,
            description,
            name,
        )
    };

    let company_ref = if company.trim().is_empty() { "the company".to_string() } else { company };
    let sign_off = if name.trim().is_empty() { String::new() } else { format!(" Sign it {name}.") };
    let prompt = format!(
        "Write a cover letter for the {role} role at {company_ref}, from the candidate whose \
         resume is below. About 200 words, 3 short paragraphs, warm and direct, no clichés. \
         Connect 2 or 3 specific things from the resume to what the job asks for. Use ONLY \
         facts from the resume; do not claim any skill, title, or number the resume does not \
         state. If the job asks for something the resume lacks, do not mention it.{sign_off}\
         \n\nRESUME:\n{resume}\n\nJOB:\n{description}"
    );
    let text = crate::assistant::ollama_provider::generate_text(&model, &prompt)?;
    let warnings = crate::assistant::tailor::unsupported(
        &text,
        &resume,
        &description,
        &[role.as_str(), company_ref.as_str()],
    );

    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    conn.execute(
        "UPDATE job_applications SET cover_letter = ?2 WHERE id = ?1",
        rusqlite::params![id, text],
    )
    .map_err(|e| e.to_string())?;
    Ok(CoverLetter { text, warnings })
}

/// Save an application's tailored resume or cover letter to Downloads as a
/// text file and reveal it in Finder, ready to attach to an application form.
///
/// NEXUS writes the file; the user attaches it. It never fills in or submits a
/// form. An existing file is never overwritten: a numbered name is used.
#[tauri::command(async)]
pub fn nexus_export_application_doc(
    state: State<'_, DbState>,
    id: i64,
    kind: String,
) -> Result<String, String> {
    let (label, column) = match kind.as_str() {
        "resume" => ("Resume", "tailored_resume"),
        "cover" => ("Cover letter", "cover_letter"),
        other => return Err(format!("Unknown document \"{other}\".")),
    };
    let (company, role, content): (String, String, String) = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        // `column` is one of two fixed names above, never user input.
        conn.query_row(
            &format!("SELECT company, role, {column} FROM job_applications WHERE id = ?1"),
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|_| "That application no longer exists.".to_string())?
    };
    if content.trim().is_empty() {
        return Err(format!("There is no {} yet. Create it in the Tailor tab first.", label.to_lowercase()));
    }

    let home = std::env::var("HOME").map_err(|_| "Could not find your home folder.".to_string())?;
    let dir = std::path::Path::new(&home).join("Downloads");
    let stem = crate::assistant::career::safe_filename(&format!("{label} - {company} - {role}"));
    let path = crate::assistant::career::unused_path(&dir, &stem);
    std::fs::write(&path, content).map_err(|e| format!("Could not save the file: {e}"))?;

    let shown = path.to_string_lossy().to_string();
    // Reveal it; failing to open Finder is not a failure to save.
    let _ = crate::assistant::shell::run(
        "/usr/bin/open",
        &["-R", &shown],
        crate::assistant::shell::DEFAULT_TIMEOUT,
    );
    Ok(shown)
}

// -- Today --------------------------------------------------------------------

/// The user's day, assembled from their own data, with a one-line summary.
///
/// Local time comes from SQLite's `localtime`, as elsewhere in NEXUS, so
/// "today" is the user's day rather than UTC's. Every count is a plain query;
/// the only thing that can be unknown is the calendar, and then the summary
/// says nothing about meetings rather than claiming there are none.
#[tauri::command(async)]
pub fn nexus_today(state: State<'_, DbState>) -> Result<crate::assistant::today::Today, String> {
    use crate::assistant::today::{compose_line, part_of_day, BillDue, CalendarSummary, Today};

    let now = now_unix();
    let mut today = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        let q = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0) };

        let hour = q("SELECT CAST(strftime('%H','now','localtime') AS INTEGER)");
        let day_end = q(
            "SELECT CAST(strftime('%s', date('now','localtime','+1 day'), 'utc') AS INTEGER)",
        );
        let local_date: String = conn
            .query_row("SELECT date('now','localtime')", [], |r| r.get(0))
            .unwrap_or_default();

        let name = crate::assistant::profile::get_all(&conn)
            .get("display_name")
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty());

        let reminders_due: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM commitments
                 WHERE state = 'open' AND due_at IS NOT NULL AND due_at < ?1",
                [day_end],
                |r| r.get(0),
            )
            .unwrap_or(0);

        // Bills within a week (overdue one-offs included), soonest first.
        let mut bills_due: Vec<BillDue> = Vec::new();
        if let Ok(mut stmt) =
            conn.prepare("SELECT id, name, kind, amount, recurrence, anchor_date FROM bills")
        {
            if let Ok(rows) = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<f64>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                ))
            }) {
                for (id, name, kind, amount, recurrence, anchor) in rows.filter_map(Result::ok) {
                    if let Some(b) = crate::assistant::bills::bill_view(
                        id, name, kind, amount, &recurrence, &anchor, now,
                    ) {
                        if b.days_until <= 7 {
                            bills_due.push(BillDue {
                                name: b.name,
                                days_until: b.days_until,
                                amount: b.amount,
                            });
                        }
                    }
                }
            }
        }
        bills_due.sort_by_key(|b| b.days_until);

        let tax_soon = crate::assistant::bills::upcoming_tax_dates(now, 1)
            .into_iter()
            .find(|t| t.days_until <= 30);

        let follow_ups_due: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM job_applications
                 WHERE status != 'rejected' AND next_action_date IS NOT NULL
                   AND next_action_date != '' AND next_action_date <= ?1",
                [&local_date],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let interviewing = q("SELECT COUNT(*) FROM job_applications WHERE status = 'interviewing'");

        let study_day = crate::assistant::learn::day_of(now);
        let cards_due: i64 = conn
            .query_row("SELECT COUNT(*) FROM cards WHERE due_day <= ?1", [study_day], |r| r.get(0))
            .unwrap_or(0);
        let streak = {
            let days: Vec<i64> = conn
                .prepare("SELECT DISTINCT day FROM card_reviews")
                .and_then(|mut s| {
                    s.query_map([], |r| r.get::<_, i64>(0))
                        .map(|rows| rows.filter_map(Result::ok).collect())
                })
                .unwrap_or_default();
            crate::assistant::learn::streak(&days, study_day)
        };

        Today {
            name,
            part_of_day: part_of_day(hour),
            calendar: None,
            reminders_due,
            bills_due,
            tax_soon,
            follow_ups_due,
            interviewing,
            cards_due,
            streak,
            line: String::new(),
        }
    };

    // The calendar is read after the lock is released. It never prompts:
    // without permission it returns an error, and the day is "unknown".
    let (day_start, offset) = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        let start: i64 = conn
            .query_row(
                "SELECT CAST(strftime('%s', date('now','localtime'), 'utc') AS INTEGER)",
                [],
                |r| r.get(0),
            )
            .unwrap_or(now);
        let offset: i64 = conn
            .query_row(
                "SELECT CAST(strftime('%s','now','localtime') AS INTEGER)
                      - CAST(strftime('%s','now') AS INTEGER)",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        (start, offset)
    };
    if let Ok(events) = crate::assistant::calendar_store::events_between(day_start, day_start + 86_400)
    {
        let timed: Vec<_> = events.into_iter().filter(|e| !e.all_day).collect();
        let next = timed.iter().find(|e| e.start >= now);
        today.calendar = Some(CalendarSummary {
            count: timed.len(),
            next_title: next.map(|e| e.title.clone()),
            next_at: next.map(|e| {
                let local = (e.start + offset).rem_euclid(86_400);
                format!("{:02}:{:02}", local / 3600, (local % 3600) / 60)
            }),
        });
    }

    today.line = compose_line(&today);
    Ok(today)
}

// -- Phone approvals ----------------------------------------------------------

const PHONE_APPROVALS_KEY: &str = "phone_approvals";
const REPLY_TOPIC_KEY: &str = "ntfy_reply_topic";
/// Where approval requests go. Its own random topic, never the stock-alert
/// one: on ntfy.sh anyone who knows a topic name can read it, a request
/// carries its one-time code, and the alert topic is a name a person chose and
/// has likely shown somewhere. 80 random bits cannot be guessed.
const APPROVE_TOPIC_KEY: &str = "ntfy_approve_topic";

/// A topic name with 80 random bits after the prefix.
fn random_topic(prefix: &str) -> Result<String, String> {
    let code = crate::assistant::phone_approval::new_code()?;
    Ok(format!("{prefix}-{}", &code[..20]))
}

const CALENDAR_KEY: &str = "calendar_enabled";

/// Apply the stored calendar switch. At startup, before anything reads the
/// calendar; absent means on.
pub fn load_calendar_switch(conn: &rusqlite::Connection) {
    crate::assistant::calendar_store::set_enabled(setting(conn, CALENDAR_KEY).as_deref() != Some("0"));
}

#[tauri::command(async)]
pub fn nexus_calendar_enabled() -> bool {
    crate::assistant::calendar_store::enabled()
}

#[tauri::command(async)]
pub fn nexus_set_calendar_enabled(state: State<'_, DbState>, enabled: bool) -> Result<bool, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    put_setting(&conn, CALENDAR_KEY, if enabled { "1" } else { "0" })?;
    crate::assistant::calendar_store::set_enabled(enabled);
    Ok(enabled)
}

fn setting(conn: &rusqlite::Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get::<_, String>(0))
        .ok()
        .filter(|v| !v.trim().is_empty())
}

fn put_setting(conn: &rusqlite::Connection, key: &str, value: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE
           SET value = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        rusqlite::params![key, value],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// (approval topic, reply topic) when phone approvals are on and set up.
pub fn phone_targets(conn: &rusqlite::Connection) -> Option<(String, String)> {
    if setting(conn, PHONE_APPROVALS_KEY).as_deref() != Some("1") {
        return None;
    }
    Some((setting(conn, APPROVE_TOPIC_KEY)?, setting(conn, REPLY_TOPIC_KEY)?))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneApprovalStatus {
    pub enabled: bool,
    /// The topic to subscribe to in the ntfy app, once generated.
    pub approve_topic: Option<String>,
}

fn phone_status(conn: &rusqlite::Connection) -> PhoneApprovalStatus {
    PhoneApprovalStatus {
        enabled: setting(conn, PHONE_APPROVALS_KEY).as_deref() == Some("1"),
        approve_topic: setting(conn, APPROVE_TOPIC_KEY),
    }
}

#[tauri::command(async)]
pub fn nexus_phone_approvals(state: State<'_, DbState>) -> Result<PhoneApprovalStatus, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    Ok(phone_status(&conn))
}

/// Turn phone approvals on or off. The first time on, NEXUS makes its two
/// private topics: one the phone subscribes to for requests, one it posts
/// taps back to.
#[tauri::command(async)]
pub fn nexus_set_phone_approvals(
    state: State<'_, DbState>,
    enabled: bool,
) -> Result<PhoneApprovalStatus, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    if enabled {
        if setting(&conn, APPROVE_TOPIC_KEY).is_none() {
            put_setting(&conn, APPROVE_TOPIC_KEY, &random_topic("nexus-approve")?)?;
        }
        if setting(&conn, REPLY_TOPIC_KEY).is_none() {
            put_setting(&conn, REPLY_TOPIC_KEY, &random_topic("nexus-reply")?)?;
        }
    }
    put_setting(&conn, PHONE_APPROVALS_KEY, if enabled { "1" } else { "0" })?;
    Ok(phone_status(&conn))
}

/// Send a request to the phone with Approve and Deny buttons, off the calling
/// thread: it is a network call, and the caller is often the UI.
fn ask_phone(
    alert: String,
    reply: String,
    pending: crate::assistant::phone_approval::Pending,
    title: &'static str,
    message: String,
    ttl: std::time::Duration,
    labels: (&'static str, &'static str),
) {
    std::thread::spawn(move || {
        use crate::assistant::ntfy_connector::{publish_with_buttons, Button};
        let code = match crate::assistant::phone_approval::register(pending, &message, ttl) {
            Ok(code) => code,
            Err(e) => {
                log::warn!("phone approval: {e}");
                return;
            }
        };
        let buttons = [
            Button { label: labels.0, body: format!("approve {code}") },
            Button { label: labels.1, body: format!("deny {code}") },
        ];
        if let Err(e) = publish_with_buttons(&alert, title, &message, &buttons, &reply) {
            log::warn!("phone approval not sent: {e}");
        }
    });
}

/// Send a harmless request, to check the whole round trip from the phone.
#[tauri::command(async)]
pub fn nexus_phone_approval_test(state: State<'_, DbState>) -> Result<(), String> {
    let (alert, reply) = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        phone_targets(&conn).ok_or("Turn phone approvals on first.")?
    };
    ask_phone(
        alert,
        reply,
        crate::assistant::phone_approval::Pending::Test,
        "NEXUS test approval",
        "Tap Approve or Deny. NEXUS will confirm it heard you.".to_string(),
        std::time::Duration::from_secs(600),
        ("Approve", "Deny"),
    );
    Ok(())
}

/// Act on one answer from the phone. Every path goes through the same code
/// the app's own buttons use.
fn handle_phone(
    app: &tauri::AppHandle,
    db: &DbState,
    verdict: crate::assistant::phone_approval::Verdict,
    pending: crate::assistant::phone_approval::Pending,
    summary: &str,
) -> String {
    use crate::assistant::phone_approval::{Pending, Verdict};
    match (pending, verdict) {
        (Pending::Test, Verdict::Approve) => "Approve received. Phone approvals work.".to_string(),
        (Pending::Test, Verdict::Deny) => "Deny received. Phone approvals work.".to_string(),
        (Pending::Action { request }, verdict) => {
            let (Some(approvals), Some(session)) = (
                tauri::Manager::try_state::<ApprovalStore>(app),
                tauri::Manager::try_state::<AssistantSession>(app),
            ) else {
                return "NEXUS is shutting down; nothing ran.".to_string();
            };
            let message = if verdict == Verdict::Approve {
                // The same token the app would redeem: single use, same action,
                // same input, unexpired, or the gate refuses.
                let result = match db.0.lock() {
                    Ok(conn) => execute_action(&conn, &approvals, &session, request),
                    Err(_) => return "NEXUS is busy; nothing ran. Try again.".to_string(),
                };
                match result {
                    Ok(_) => format!("Done: {summary}"),
                    Err(e) => format!("Didn't run ({summary}): {e}"),
                }
            } else {
                if let Some(token) = request.approval {
                    approvals.cancel(token);
                }
                session.cancel();
                format!("Cancelled: {summary}")
            };
            let _ = app.emit(EVENT_ASSISTANT_STATE, session.snapshot(approvals.pending_count()));
            message
        }
        (Pending::CloneApply { run_id }, Verdict::Approve) => match clone_apply(db, run_id) {
            Ok(msg) => msg,
            Err(e) => format!("Not applied: {e}"),
        },
        (Pending::CloneApply { run_id }, Verdict::Deny) => match clone_discard(db, run_id) {
            Ok(()) => "Discarded. Your repository was not touched.".to_string(),
            Err(e) => format!("Could not discard: {e}"),
        },
    }
}

/// One read of the reply topic, acting on each new answer. Called by the
/// background reader only while something is waiting. `cursor` is the last
/// reply seen (or the time the reader started), so nothing is acted on twice.
pub fn phone_poll_once(app: &tauri::AppHandle, db: &DbState, cursor: &mut String) {
    use crate::assistant::{ntfy_connector, phone_approval};
    let targets = match db.0.lock() {
        Ok(conn) => phone_targets(&conn),
        Err(_) => return,
    };
    let Some((alert, reply)) = targets else { return };
    let Ok(replies) = ntfy_connector::poll_replies(&reply, cursor) else { return };
    for r in replies {
        *cursor = r.id.clone();
        let Some((verdict, code)) = phone_approval::parse_reply(&r.message) else { continue };
        let outcome = match phone_approval::take(&code) {
            Some((pending, summary)) => handle_phone(app, db, verdict, pending, &summary),
            None => "Nothing to approve: it expired, was already answered, or NEXUS restarted. \
                     Nothing ran."
                .to_string(),
        };
        if let Err(e) = ntfy_connector::send_note(&alert, "NEXUS", &outcome) {
            log::warn!("phone confirmation not sent: {e}");
        }
    }
}

// -- Supervised task clone ----------------------------------------------------

const GIT: &str = "/usr/bin/git";
/// How long Claude Code may work on one task.
const CLONE_EDIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20 * 60);
/// How long the project's test suite may run.
const CLONE_TEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15 * 60);
/// Largest diff kept for review. Bigger than this is not a reviewable change.
const MAX_CLONE_DIFF: usize = 2 * 1024 * 1024;
/// States in which a worker is (or should be) running.
const CLONE_LIVE: [&str; 4] = ["queued", "preparing", "editing", "testing"];

/// Runs with a worker thread alive in this process. A run in a live state
/// that is not in here was interrupted by a restart.
static ACTIVE_CLONES: std::sync::Mutex<Vec<i64>> = std::sync::Mutex::new(Vec::new());

fn clones_dir() -> Result<std::path::PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "Could not find your home folder.".to_string())?;
    Ok(std::path::Path::new(&home)
        .join("Library/Application Support/com.nexus.desktop/clones"))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloneProject {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub allowed: bool,
    /// Whether the path is a git repository NEXUS can make a worktree from.
    pub is_git: bool,
    /// The test command NEXUS would run, if one was found.
    pub test_label: Option<String>,
}

/// Projects with a local folder, and whether each is allowed to run tasks.
#[tauri::command(async)]
pub fn nexus_clone_projects(state: State<'_, DbState>) -> Result<Vec<CloneProject>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    let mut stmt = conn
        .prepare(
            "SELECT id, name, repository_path, clone_allowed FROM projects
             WHERE repository_path IS NOT NULL AND repository_path != '' ORDER BY name",
        )
        .map_err(|e| format!("Query error: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?))
        })
        .map_err(|e| format!("Query error: {e}"))?;
    Ok(rows
        .filter_map(Result::ok)
        .map(|(id, name, path, allowed)| {
            let p = std::path::Path::new(&path);
            CloneProject {
                id,
                name,
                is_git: p.join(".git").exists(),
                test_label: crate::assistant::clone::detect_tests(p).map(|t| t.label),
                path,
                allowed: allowed != 0,
            }
        })
        .collect())
}

/// Allow or disallow tasks for a project. The consent for its code to be read
/// by Claude Code.
#[tauri::command(async)]
pub fn nexus_set_clone_allowed(
    state: State<'_, DbState>,
    project_id: i64,
    allowed: bool,
) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    conn.execute(
        "UPDATE projects SET clone_allowed = ?2 WHERE id = ?1",
        rusqlite::params![project_id, allowed as i64],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloneRun {
    pub id: i64,
    pub project_id: i64,
    pub project_name: String,
    pub task: String,
    pub status: String,
    pub summary: String,
    pub diff: String,
    pub diff_stat: String,
    pub test_label: String,
    pub test_passed: Option<bool>,
    pub test_output: String,
    pub error: String,
    pub created_at: String,
}

/// Every run, newest first. Runs left in a live state by a restart are marked
/// interrupted here, so nothing shows as running that is not.
#[tauri::command(async)]
pub fn nexus_clone_runs(state: State<'_, DbState>) -> Result<Vec<CloneRun>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    let active = ACTIVE_CLONES.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let stale: Vec<i64> = {
        let mut stmt = conn
            .prepare("SELECT id, status FROM clone_runs")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        rows.filter_map(Result::ok)
            .filter(|(id, status)| CLONE_LIVE.contains(&status.as_str()) && !active.contains(id))
            .map(|(id, _)| id)
            .collect()
    };
    for id in stale {
        let _ = conn.execute(
            "UPDATE clone_runs SET status = 'failed',
                error = 'Interrupted: NEXUS was closed while this was running.',
                finished_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
            [id],
        );
    }
    let mut stmt = conn
        .prepare(
            "SELECT r.id, r.project_id, COALESCE(p.name, '(deleted project)'), r.task, r.status,
                    r.summary, r.diff, r.diff_stat, r.test_label, r.test_passed, r.test_output,
                    r.error, r.created_at
             FROM clone_runs r LEFT JOIN projects p ON p.id = r.project_id
             ORDER BY r.id DESC LIMIT 30",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(CloneRun {
                id: r.get(0)?,
                project_id: r.get(1)?,
                project_name: r.get(2)?,
                task: r.get(3)?,
                status: r.get(4)?,
                summary: r.get(5)?,
                diff: r.get(6)?,
                diff_stat: r.get(7)?,
                test_label: r.get(8)?,
                test_passed: r.get::<_, Option<i64>>(9)?.map(|v| v != 0),
                test_output: r.get(10)?,
                error: r.get(11)?,
                created_at: r.get(12)?,
            })
        })
        .map_err(|e| e.to_string())?;
    Ok(rows.filter_map(Result::ok).collect())
}

/// Start a task. Refused unless the project is allowed, has a git folder, and
/// has no task already running. The work happens on a thread; poll the runs.
#[tauri::command(async)]
pub fn nexus_clone_start(
    app: tauri::AppHandle,
    state: State<'_, DbState>,
    project_id: i64,
    task: String,
) -> Result<i64, String> {
    let task = task.trim().to_string();
    if task.len() < 10 {
        return Err("Describe the task in a sentence or two.".to_string());
    }
    if task.len() > 4_000 {
        return Err("Keep the task under 4,000 characters; split bigger work into steps.".to_string());
    }
    if crate::assistant::claude_provider::binary().is_none() {
        return Err("Claude Code is not installed on this Mac.".to_string());
    }
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    let (path, allowed): (Option<String>, i64) = conn
        .query_row(
            "SELECT repository_path, clone_allowed FROM projects WHERE id = ?1",
            [project_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| "That project no longer exists.".to_string())?;
    if allowed == 0 {
        return Err("Tasks are not allowed for this project yet. Switch it on first.".to_string());
    }
    if !path.is_some_and(|p| std::path::Path::new(&p).join(".git").exists()) {
        return Err(
            "This project has no local git folder set. Add its folder path in Projects.".to_string(),
        );
    }
    // Busy means a live run for *this* project whose worker is still alive. A
    // live-looking row left by a restart does not block a new task.
    let live: Vec<i64> = conn
        .prepare(
            "SELECT id FROM clone_runs WHERE project_id = ?1
               AND status IN ('queued','preparing','editing','testing')",
        )
        .and_then(|mut s| {
            s.query_map([project_id], |r| r.get::<_, i64>(0))
                .map(|rows| rows.filter_map(Result::ok).collect())
        })
        .unwrap_or_default();
    let active = ACTIVE_CLONES.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if live.iter().any(|id| active.contains(id)) {
        return Err("A task is already running for this project. Let it finish first.".to_string());
    }
    conn.execute(
        "INSERT INTO clone_runs (project_id, task) VALUES (?1, ?2)",
        rusqlite::params![project_id, task],
    )
    .map_err(|e| e.to_string())?;
    let id = conn.last_insert_rowid();
    drop(conn);

    ACTIVE_CLONES.lock().unwrap_or_else(|e| e.into_inner()).push(id);
    std::thread::spawn(move || {
        if let Err(e) = clone_steps(&app, id) {
            let mut phone = None;
            if let Some(db) = tauri::Manager::try_state::<DbState>(&app) {
                if let Ok(conn) = db.0.lock() {
                    let _ = conn.execute(
                        "UPDATE clone_runs SET status = 'failed', error = ?2,
                            finished_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
                        rusqlite::params![id, e],
                    );
                    phone = phone_targets(&conn);
                }
            }
            if let Some((alert, _)) = phone {
                let _ = crate::assistant::ntfy_connector::send_note(
                    &alert,
                    "NEXUS: task failed",
                    &crate::assistant::clone::tail(&e, 300),
                );
            }
        }
        ACTIVE_CLONES.lock().unwrap_or_else(|e| e.into_inner()).retain(|x| *x != id);
    });
    Ok(id)
}

fn set_clone_status(app: &tauri::AppHandle, id: i64, status: &str) {
    set_clone(app, id, "UPDATE clone_runs SET status = ?2 WHERE id = ?1", status);
}

fn set_clone(app: &tauri::AppHandle, id: i64, sql: &str, value: &str) {
    if let Some(db) = tauri::Manager::try_state::<DbState>(app) {
        if let Ok(conn) = db.0.lock() {
            let _ = conn.execute(sql, rusqlite::params![id, value]);
        }
    }
}

fn git(args: &[&str]) -> Result<crate::assistant::shell::Output, String> {
    let out = crate::assistant::shell::run(GIT, args, crate::assistant::shell::DEFAULT_TIMEOUT)
        .map_err(|e| e.to_string())?;
    if out.success {
        Ok(out)
    } else {
        Err(format!("git {}: {}", args.get(2).unwrap_or(&""), out.stderr.trim()))
    }
}

/// The work of one run. Each stage records itself before it starts, so the
/// UI always says what is happening; any error ends the run as failed.
fn clone_steps(app: &tauri::AppHandle, id: i64) -> Result<(), String> {
    use crate::assistant::clone::{build_prompt, child_path, claude_args, detect_tests, resolve_program, tail};

    let (task, path) = {
        let db = tauri::Manager::try_state::<DbState>(app).ok_or("NEXUS is shutting down.")?;
        let conn = db.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        conn.query_row(
            "SELECT r.task, p.repository_path FROM clone_runs r JOIN projects p ON p.id = r.project_id
             WHERE r.id = ?1",
            [id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .map_err(|_| "The run or its project no longer exists.".to_string())?
    };
    set_clone_status(app, id, "preparing");

    // A detached worktree at the current commit, outside the repository.
    let top = git(&["-C", &path, "rev-parse", "--show-toplevel"])?.stdout.trim().to_string();
    let base = git(&["-C", &top, "rev-parse", "HEAD"])?.stdout.trim().to_string();
    let dir = clones_dir()?.join(format!("run-{id}"));
    if dir.exists() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    std::fs::create_dir_all(dir.parent().unwrap_or(&dir)).map_err(|e| e.to_string())?;
    let dir_s = dir.to_string_lossy().to_string();
    git(&["-C", &top, "worktree", "add", "--detach", &dir_s, &base])?;
    set_clone(app, id, "UPDATE clone_runs SET worktree = ?2 WHERE id = ?1", &dir_s);
    set_clone(app, id, "UPDATE clone_runs SET base_commit = ?2 WHERE id = ?1", &base);

    // Share installed dependencies rather than reinstalling them: the test
    // run needs them, and the worktree is a fresh checkout without them.
    let deps = std::path::Path::new(&top).join("node_modules");
    if dir.join("package.json").exists() && deps.exists() && !dir.join("node_modules").exists() {
        let _ = std::os::unix::fs::symlink(&deps, dir.join("node_modules"));
    }

    // Claude Code edits, inside the sandbox described in `clone`.
    set_clone_status(app, id, "editing");
    let claude = crate::assistant::claude_provider::binary().ok_or("Claude Code is not installed.")?;
    let claude_path = child_path(std::path::Path::new(claude));
    let out = crate::assistant::shell::run_in_dir(
        claude,
        &claude_args(),
        &dir,
        &[("PATH", claude_path.as_str())],
        Some(&build_prompt(&task)),
        CLONE_EDIT_TIMEOUT,
    )
    .map_err(|e| format!("Claude Code: {e}"))?;
    let summary = tail(out.stdout.trim(), 6_000);
    set_clone(app, id, "UPDATE clone_runs SET summary = ?2 WHERE id = ?1", &summary);
    if !out.success {
        return Err(format!("Claude Code stopped: {}", tail(out.stderr.trim(), 1_000)));
    }

    // The change, read as a diff before any test runs, so build and test
    // artefacts can never end up in it. Shared dependencies are excluded.
    git(&["-C", &dir_s, "add", "-A", "--", ".", ":(exclude)node_modules"])?;
    let stat = git(&["-C", &dir_s, "diff", "--cached", "--stat"])?.stdout;
    let diff_out = git(&["-C", &dir_s, "diff", "--cached", "--binary"])?;
    // The raw bytes: a patch must end exactly as git wrote it to apply.
    let diff = String::from_utf8_lossy(&diff_out.raw_stdout).to_string();
    if diff.trim().is_empty() {
        return Err("Claude Code finished without changing any files. Its summary may say why.".to_string());
    }
    if diff.len() > MAX_CLONE_DIFF {
        return Err("The change is too large to review here. Split the task into smaller steps.".to_string());
    }
    set_clone(app, id, "UPDATE clone_runs SET diff = ?2 WHERE id = ?1", &diff);
    set_clone(app, id, "UPDATE clone_runs SET diff_stat = ?2 WHERE id = ?1", stat.trim());

    // NEXUS runs the tests itself; the result shown is the one it observed.
    let mut test_note = "no test command found".to_string();
    match detect_tests(&dir) {
        Some(plan) => {
            set_clone(app, id, "UPDATE clone_runs SET test_label = ?2 WHERE id = ?1", &plan.label);
            set_clone_status(app, id, "testing");
            match resolve_program(plan.program) {
                Some(program) => {
                    let path_env = child_path(&program);
                    let out = crate::assistant::shell::run_in_dir(
                        &program.to_string_lossy(),
                        &plan.args,
                        &dir,
                        &[("PATH", path_env.as_str()), ("CI", "1")],
                        None,
                        CLONE_TEST_TIMEOUT,
                    );
                    let (passed, log) = match out {
                        Ok(o) => (o.success, format!("{}\n{}", o.stdout, o.stderr)),
                        Err(e) => (false, e.to_string()),
                    };
                    set_clone(app, id, "UPDATE clone_runs SET test_output = ?2 WHERE id = ?1", &tail(log.trim(), 8_000));
                    set_clone(app, id, "UPDATE clone_runs SET test_passed = CAST(?2 AS INTEGER) WHERE id = ?1", if passed { "1" } else { "0" });
                    test_note = format!("{} {}", plan.label, if passed { "passed" } else { "FAILED" });
                }
                None => {
                    test_note = "tests not run".to_string();
                    set_clone(
                        app,
                        id,
                        "UPDATE clone_runs SET test_output = ?2 WHERE id = ?1",
                        &format!("Could not find `{}` on this Mac, so the tests were not run.", plan.program),
                    );
                }
            }
        }
        None => set_clone(app, id, "UPDATE clone_runs SET test_label = ?2 WHERE id = ?1", "no test command found"),
    }

    if let Some(db) = tauri::Manager::try_state::<DbState>(app) {
        if let Ok(conn) = db.0.lock() {
            let _ = conn.execute(
                "UPDATE clone_runs SET status = 'ready',
                    finished_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
                [id],
            );
        }
    }

    // Ready for review: offer Apply / Discard on the phone. Apply from there
    // runs the same checked apply as the app (uncommitted, conflicts touch
    // nothing). Twelve hours, because reviewing later is normal; the apply
    // re-checks against the working copy as it is then.
    let targets = tauri::Manager::try_state::<DbState>(app)
        .and_then(|db| db.0.lock().ok().and_then(|conn| phone_targets(&conn)));
    if let Some((alert, reply)) = targets {
        let short: String = task.chars().take(120).collect();
        let files = stat.trim().lines().last().unwrap_or("").trim().to_string();
        ask_phone(
            alert,
            reply,
            crate::assistant::phone_approval::Pending::CloneApply { run_id: id },
            "NEXUS: task ready for review",
            format!("{short} · {files} · {test_note}"),
            std::time::Duration::from_secs(12 * 3600),
            ("Apply", "Discard"),
        );
    }
    Ok(())
}

/// The run's repository top level and worktree, for apply and discard.
fn clone_paths(conn: &rusqlite::Connection, id: i64) -> Result<(String, String, String, String), String> {
    conn.query_row(
        "SELECT p.repository_path, r.worktree, r.status, r.diff FROM clone_runs r
         JOIN projects p ON p.id = r.project_id WHERE r.id = ?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
    .map_err(|_| "That run no longer exists.".to_string())
}

/// Remove a run's worktree. Leaves the user's repository as it was.
fn remove_worktree(repo: &str, worktree: &str) {
    if worktree.is_empty() {
        return;
    }
    let _ = crate::assistant::shell::run(
        GIT,
        &["-C", repo, "worktree", "remove", "--force", worktree],
        crate::assistant::shell::DEFAULT_TIMEOUT,
    );
    if std::path::Path::new(worktree).exists() {
        let _ = std::fs::remove_dir_all(worktree);
    }
    let _ = crate::assistant::shell::run(GIT, &["-C", repo, "worktree", "prune"], crate::assistant::shell::DEFAULT_TIMEOUT);
}

/// Apply a finished run's diff to the user's working copy, as uncommitted
/// changes. Checked first: if it would conflict with their own uncommitted
/// work, nothing is touched and they are told why. NEXUS never commits.
#[tauri::command(async)]
pub fn nexus_clone_apply(state: State<'_, DbState>, id: i64) -> Result<String, String> {
    clone_apply(state.inner(), id)
}

/// The apply itself, shared by the app's button and a phone approval so both
/// go through exactly the same checks.
pub fn clone_apply(state: &DbState, id: i64) -> Result<String, String> {
    let (repo, worktree, status, diff) = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        clone_paths(&conn, id)?
    };
    if status != "ready" {
        return Err(format!("Only a finished run can be applied (this one is {status})."));
    }
    let top = git(&["-C", &repo, "rev-parse", "--show-toplevel"])?.stdout.trim().to_string();
    let dir = std::path::Path::new(&top);
    let check = crate::assistant::shell::run_in_dir(
        GIT,
        &["apply", "--check", "--whitespace=nowarn"],
        dir,
        &[],
        Some(&diff),
        crate::assistant::shell::DEFAULT_TIMEOUT,
    )
    .map_err(|e| e.to_string())?;
    if !check.success {
        return Err(format!(
            "This change conflicts with the current state of your working copy, so nothing was \
             applied. Commit or stash your own changes (or pull), then apply again.\n{}",
            tail_err(&check.stderr)
        ));
    }
    let applied = crate::assistant::shell::run_in_dir(
        GIT,
        &["apply", "--whitespace=nowarn"],
        dir,
        &[],
        Some(&diff),
        crate::assistant::shell::DEFAULT_TIMEOUT,
    )
    .map_err(|e| e.to_string())?;
    if !applied.success {
        return Err(format!("git could not apply the change: {}", tail_err(&applied.stderr)));
    }
    remove_worktree(&top, &worktree);
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    conn.execute("UPDATE clone_runs SET status = 'applied' WHERE id = ?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "Applied to {top} as uncommitted changes. Review them in your editor and commit when you're happy."
    ))
}

fn tail_err(s: &str) -> String {
    crate::assistant::clone::tail(s.trim(), 1_500)
}

/// Throw a run away: remove its worktree; the user's repository is untouched.
#[tauri::command(async)]
pub fn nexus_clone_discard(state: State<'_, DbState>, id: i64) -> Result<(), String> {
    clone_discard(state.inner(), id)
}

/// Discard, shared by the app's button and a phone "Discard".
pub fn clone_discard(state: &DbState, id: i64) -> Result<(), String> {
    let (repo, worktree, status, _) = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        clone_paths(&conn, id)?
    };
    if CLONE_LIVE.contains(&status.as_str())
        && ACTIVE_CLONES.lock().unwrap_or_else(|e| e.into_inner()).contains(&id)
    {
        return Err("This run is still working. Wait for it to finish, then discard it.".to_string());
    }
    if let Ok(out) = git(&["-C", &repo, "rev-parse", "--show-toplevel"]) {
        remove_worktree(out.stdout.trim(), &worktree);
    }
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    conn.execute("UPDATE clone_runs SET status = 'discarded' WHERE id = ?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// -- Work prep ----------------------------------------------------------------

/// Compose the standup draft from data the frontend fetched through the
/// gated Jira and GitHub actions. Pure: it formats, it fetches nothing, so
/// what it can say is limited to what those actions returned.
#[tauri::command(async)]
pub fn nexus_compose_standup(
    input: crate::assistant::standup::StandupInput,
) -> Result<crate::assistant::standup::Standup, String> {
    Ok(crate::assistant::standup::compose(&input))
}

// -- Wellbeing ----------------------------------------------------------------

const WELLBEING_KEY: &str = "wellbeing";

/// The wellbeing settings, or the defaults if never saved. In the settings
/// table, not the profile: these are preferences, not facts about the user,
/// and the profile is what model prompts read.
pub fn load_wellbeing(conn: &rusqlite::Connection) -> crate::assistant::wellbeing::Settings {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [WELLBEING_KEY], |r| {
        r.get::<_, String>(0)
    })
    .ok()
    .and_then(|json| serde_json::from_str(&json).ok())
    .unwrap_or_default()
}

#[tauri::command(async)]
pub fn nexus_wellbeing_settings(
    state: State<'_, DbState>,
) -> Result<crate::assistant::wellbeing::Settings, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    Ok(load_wellbeing(&conn))
}

/// Save wellbeing settings, clamped to sane values; returns what was stored.
#[tauri::command(async)]
pub fn nexus_set_wellbeing_settings(
    state: State<'_, DbState>,
    settings: crate::assistant::wellbeing::Settings,
) -> Result<crate::assistant::wellbeing::Settings, String> {
    let clean = settings.sanitised();
    let json = serde_json::to_string(&clean).map_err(|e| e.to_string())?;
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE
           SET value = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        rusqlite::params![WELLBEING_KEY, json],
    )
    .map_err(|e| format!("Could not save: {e}"))?;
    Ok(clean)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WellbeingStatus {
    pub active_minutes: i64,
    pub next_break_in: Option<i64>,
    pub enabled: bool,
}

/// How long the current stretch has been, and when the next break nudge is.
#[tauri::command(async)]
pub fn nexus_wellbeing_status(state: State<'_, DbState>) -> Result<WellbeingStatus, String> {
    let settings = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        load_wellbeing(&conn)
    };
    let tracker = crate::assistant::wellbeing::TRACKER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default();
    Ok(WellbeingStatus {
        active_minutes: tracker.active_secs / 60,
        next_break_in: tracker.next_break_in(&settings),
        enabled: settings.any_enabled(),
    })
}

/// Whether a timed calendar event is under way right now. False when the
/// calendar cannot be read: an unknown calendar must not silence nudges
/// forever.
pub fn in_meeting_now(now: i64) -> bool {
    crate::assistant::calendar_store::events_between(now - 12 * 3600, now + 60)
        .map(|events| events.iter().any(|e| !e.all_day && e.start <= now && e.end > now))
        .unwrap_or(false)
}

/// One wellbeing tick: read the clock, the idle time and the calendar, advance
/// the tracker, and return any nudges due. Called by the background thread.
pub fn wellbeing_tick(conn_state: &DbState) -> Vec<crate::assistant::wellbeing::Nudge> {
    let (settings, local_minute, local_day) = {
        let Ok(conn) = conn_state.0.lock() else { return Vec::new() };
        let q = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0);
        (
            load_wellbeing(&conn),
            q("SELECT CAST(strftime('%H','now','localtime') AS INTEGER) * 60
                    + CAST(strftime('%M','now','localtime') AS INTEGER)"),
            q("SELECT CAST(julianday('now','localtime') AS INTEGER)"),
        )
    };
    if !settings.any_enabled() {
        return Vec::new();
    }
    // Without the idle time NEXUS cannot tell whether anyone is there, so it
    // says nothing rather than guess.
    let Some(idle) = crate::assistant::wellbeing::idle_seconds() else {
        return Vec::new();
    };
    let now = now_unix();
    let in_meeting = (settings.breaks || settings.hydration) && in_meeting_now(now);
    let mut guard = crate::assistant::wellbeing::TRACKER
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    guard
        .get_or_insert_with(Default::default)
        .tick(&settings, now, idle, local_minute, local_day, in_meeting)
}

// -- Career: interview prep ---------------------------------------------------

/// Good questions for a candidate to ask, whatever the company. Fixed rather
/// than generated: they are sound everywhere, and a generated list would add
/// nothing but the chance of an odd one.
const ASK_THEM: [&str; 5] = [
    "What does success look like in this role after the first 90 days?",
    "What is the biggest challenge the team is working through right now?",
    "How do you balance new features against performance, accessibility and quality work?",
    "Who owns frontend architecture and the design system, and how are those decisions made?",
    "What are the next steps in the process, and when should I expect to hear back?",
];

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleBrief {
    /// Requirements the resume covers.
    pub have: Vec<String>,
    /// Requirements the resume does not mention.
    pub gaps: Vec<String>,
    /// Whether a requirements list was found in the description at all.
    pub found: bool,
    pub ask_them: Vec<String>,
}

/// What the role asks for against what the resume shows.
///
/// Deliberately not a company brief. Asked about a hiring company, the local
/// model invented a founding year, a headcount, a funding round and a
/// valuation, with confidence, despite being told to say when unsure. Facts
/// about a company have to come from the user's own research; the UI links to
/// it. This brief is built only from the description and the resume, so it
/// cannot be wrong about either.
#[tauri::command(async)]
pub fn nexus_role_brief(state: State<'_, DbState>, id: i64) -> Result<RoleBrief, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    let (resume, _, _, description) = tailoring_inputs(&conn, id)?;
    let cov = crate::assistant::tailor::requirement_coverage(&description, &resume);
    Ok(RoleBrief {
        found: !(cov.have.is_empty() && cov.gaps.is_empty()),
        have: cov.have,
        gaps: cov.gaps,
        ask_them: ASK_THEM.iter().map(|q| q.to_string()).collect(),
    })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InterviewQuestion {
    pub id: i64,
    pub question: String,
    pub answer: String,
    pub feedback: String,
    /// Claims in the coach's suggested opening line that neither the resume
    /// nor the answer supports. Recomputed on read, never stored.
    pub warnings: Vec<String>,
}

/// The coach's opening line checked against the resume plus the user's own
/// answer: a fact the candidate stated aloud is theirs to use, even if it is
/// not on the resume.
fn opening_warnings(
    feedback: &str,
    answer: &str,
    resume: &str,
    description: &str,
    role: &str,
    company: &str,
) -> Vec<String> {
    match crate::assistant::tailor::opening_line(feedback) {
        Some(line) => crate::assistant::tailor::unsupported(
            &line,
            &format!("{resume}\n{answer}"),
            description,
            &[role, company],
        ),
        None => Vec::new(),
    }
}

fn read_questions(
    conn: &rusqlite::Connection,
    application_id: i64,
) -> Result<Vec<InterviewQuestion>, String> {
    let ctx = tailoring_inputs(conn, application_id).ok();
    let mut stmt = conn
        .prepare(
            "SELECT id, question, answer, feedback FROM interview_questions
             WHERE application_id = ?1 ORDER BY id",
        )
        .map_err(|e| format!("Query error: {e}"))?;
    let rows = stmt
        .query_map([application_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| format!("Query error: {e}"))?;
    Ok(rows
        .filter_map(Result::ok)
        .map(|(id, question, answer, feedback)| {
            let warnings = match &ctx {
                Some((resume, company, role, description)) if !feedback.is_empty() => {
                    opening_warnings(&feedback, &answer, resume, description, role, company)
                }
                _ => Vec::new(),
            };
            InterviewQuestion {
                id,
                question,
                answer,
                feedback,
                warnings,
            }
        })
        .collect())
}

/// The practice questions for an application.
#[tauri::command(async)]
pub fn nexus_interview_questions(
    state: State<'_, DbState>,
    id: i64,
) -> Result<Vec<InterviewQuestion>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    read_questions(&conn, id)
}

/// Generate practice questions from the job description and resume.
///
/// Replaces the questions not yet answered; answered ones are kept, so asking
/// for a fresh set never throws away practice already done.
#[tauri::command(async)]
pub fn nexus_generate_interview_questions(
    state: State<'_, DbState>,
    id: i64,
) -> Result<Vec<InterviewQuestion>, String> {
    let (resume, model, company, role, description) = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        let (resume, company, role, description) = tailoring_inputs(&conn, id)?;
        (
            resume,
            crate::assistant::ollama_provider::local_model(&conn),
            company,
            role,
            description,
        )
    };
    let at = if company.trim().is_empty() { String::new() } else { format!(" at {company}") };
    let prompt = format!(
        "You are interviewing a candidate for the {role} role{at}. Using the job description \
         and the candidate's resume below, write 6 interview questions: 3 technical questions \
         about the skills the job requires, 2 behavioural questions, and 1 question about a \
         specific item on the resume. One question per line, no numbering, no answers, \
         nothing else.\n\nJOB:\n{description}\n\nRESUME:\n{resume}"
    );
    let text = crate::assistant::ollama_provider::generate_text(&model, &prompt)?;
    let questions = crate::assistant::tailor::parse_questions(&text, 8);
    if questions.is_empty() {
        return Err("The model's questions could not be read. Try again.".to_string());
    }

    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    conn.execute(
        "DELETE FROM interview_questions WHERE application_id = ?1 AND answer = ''",
        [id],
    )
    .map_err(|e| e.to_string())?;
    for q in &questions {
        conn.execute(
            "INSERT INTO interview_questions (application_id, question) VALUES (?1, ?2)",
            rusqlite::params![id, q],
        )
        .map_err(|e| e.to_string())?;
    }
    read_questions(&conn, id)
}

/// Save an answer and get coaching feedback on it.
#[tauri::command(async)]
pub fn nexus_interview_feedback(
    state: State<'_, DbState>,
    question_id: i64,
    answer: String,
) -> Result<InterviewQuestion, String> {
    let answer = answer.trim().to_string();
    if answer.len() < 10 {
        return Err("Write an answer first, even a rough one.".to_string());
    }
    if answer.len() > MAX_DOCUMENT {
        return Err("That answer is too long.".to_string());
    }
    let (question, resume, model, company, role, description) = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        let (application_id, question): (i64, String) = conn
            .query_row(
                "SELECT application_id, question FROM interview_questions WHERE id = ?1",
                [question_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|_| "That question no longer exists.".to_string())?;
        let (resume, company, role, description) = tailoring_inputs(&conn, application_id)?;
        (
            question,
            resume,
            crate::assistant::ollama_provider::local_model(&conn),
            company,
            role,
            description,
        )
    };

    let prompt = format!(
        "You are a supportive but honest interview coach for the {role} role.\n\
         Question: {question}\n\
         Candidate's answer: {answer}\n\n\
         Their resume is below. You may point out resume facts their answer could have \
         used, but never invent experience, numbers or results they have not stated.\n\n\
         Give feedback in four short labelled parts:\n\
         Worked: what was good.\n\
         Missing: what a strong answer would include that this one lacks. Be specific.\n\
         Structure: how to organise it better (STAR for behavioural questions).\n\
         Try opening with: one first sentence the candidate could say, in first person \
         (\"I ...\"), using only facts from their answer or resume.\n\n\
         RESUME:\n{resume}"
    );
    let feedback = crate::assistant::ollama_provider::generate_text(&model, &prompt)?
        .replace("**", "");
    let warnings = opening_warnings(&feedback, &answer, &resume, &description, &role, &company);

    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    conn.execute(
        "UPDATE interview_questions SET answer = ?2, feedback = ?3 WHERE id = ?1",
        rusqlite::params![question_id, answer, feedback],
    )
    .map_err(|e| e.to_string())?;
    Ok(InterviewQuestion {
        id: question_id,
        question,
        answer,
        feedback,
        warnings,
    })
}

/// Remove a practice question.
#[tauri::command(async)]
pub fn nexus_delete_interview_question(state: State<'_, DbState>, id: i64) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    conn.execute("DELETE FROM interview_questions WHERE id = ?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Stop tracking an application.
#[tauri::command(async)]
pub fn nexus_remove_application(state: State<'_, DbState>, id: i64) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    // Its practice questions go with it rather than lingering unreachable.
    conn.execute("DELETE FROM interview_questions WHERE application_id = ?1", [id])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM job_applications WHERE id = ?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Streak, cards due today, and reviews done today.
#[tauri::command(async)]
pub fn nexus_learn_stats(state: State<'_, DbState>) -> Result<LearnStats, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    let today = today_day();
    let mut stmt = conn
        .prepare("SELECT DISTINCT day FROM card_reviews")
        .map_err(|e| e.to_string())?;
    let days: Vec<i64> = stmt
        .query_map([], |r| r.get(0))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    let due_today: i64 = conn
        .query_row("SELECT COUNT(*) FROM cards WHERE due_day <= ?1", [today], |r| r.get(0))
        .unwrap_or(0);
    let reviewed_today: i64 = conn
        .query_row("SELECT COUNT(*) FROM card_reviews WHERE day = ?1", [today], |r| r.get(0))
        .unwrap_or(0);
    Ok(LearnStats {
        streak: crate::assistant::learn::streak(&days, today),
        due_today,
        reviewed_today,
    })
}

/// Remove a holding by id.
#[tauri::command(async)]
pub fn nexus_remove_holding(state: State<'_, DbState>, id: i64) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    conn.execute("DELETE FROM holdings WHERE id = ?1", rusqlite::params![id])
        .map_err(|e| format!("Could not remove the holding: {e}"))?;
    Ok(())
}

/// The portfolio, priced live.
///
/// Rows are read under the lock, then the lock is dropped before any quote is
/// fetched: pricing is several network calls and must not freeze the database.
/// Each holding is priced independently, so one unreachable symbol shows its
/// error while the rest still total.
#[tauri::command(async)]
pub fn nexus_portfolio(state: State<'_, DbState>) -> Result<Portfolio, String> {
    // Read and released before the network.
    type Row = (i64, String, String, f64, String, String, Option<f64>, Option<f64>, Option<String>);
    let (rows, group_invested): (Vec<Row>, std::collections::HashMap<String, f64>) = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, symbol, label, quantity, kind, broker, invested, unit_price, price_as_of
                   FROM holdings ORDER BY broker, id",
            )
            .map_err(|e| format!("Query error: {e}"))?;
        let mapped = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, f64>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, Option<f64>>(6)?,
                    r.get::<_, Option<f64>>(7)?,
                    r.get::<_, Option<String>>(8)?,
                ))
            })
            .map_err(|e| format!("Query error: {e}"))?;
        let rows = mapped.filter_map(Result::ok).collect();
        let mut groups = std::collections::HashMap::new();
        let mut gstmt = conn
            .prepare("SELECT name, invested FROM portfolio_groups")
            .map_err(|e| format!("Query error: {e}"))?;
        for (name, invested) in gstmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?)))
            .map_err(|e| format!("Query error: {e}"))?
            .flatten()
        {
            groups.insert(name, invested);
        }
        (rows, groups)
    };

    let mut holdings = Vec::with_capacity(rows.len());
    let mut total_value = 0.0;
    let mut total_day_change = 0.0;
    let currency = "INR".to_string();

    for (id, symbol, label, quantity, kind, broker, invested, unit_price, price_as_of) in rows {
        let priced: Result<(f64, Option<f64>, Option<String>), String> = match kind.as_str() {
            // A ULIP fund: units at the NAV the user entered, which has no
            // public feed, so no day change.
            "ulip" => match unit_price {
                Some(nav) => {
                    let value = nav * quantity;
                    total_value += value;
                    holdings.push(HoldingView {
                        id,
                        symbol,
                        label,
                        quantity,
                        kind,
                        price: Some(nav),
                        prev_close: None,
                        currency: currency.clone(),
                        value: Some(value),
                        day_change: None,
                        error: None,
                        broker,
                        invested,
                        price_as_of,
                    });
                    continue;
                }
                None => Err("No NAV entered yet. Tell the chat box, e.g. \"MUP nav 9.5\".".to_string()),
            },
            // A fund: today's NAV by scheme code, day change vs yesterday's NAV.
            "mf" => crate::assistant::mf_connector::fetch_nav(&symbol)
                .map(|n| (n.nav, n.prev_nav, (!n.name.is_empty()).then_some(n.name))),
            // Manual (ULIP, cash): the entered value is the value, no fetch.
            "manual" => {
                let value = quantity;
                total_value += value;
                holdings.push(HoldingView {
                    id,
                    symbol,
                    label,
                    quantity,
                    kind,
                    price: None,
                    prev_close: None,
                    currency: currency.clone(),
                    value: Some(value),
                    day_change: None,
                    error: None,
                    broker: broker.clone(),
                    invested,
                    price_as_of: None,
                });
                continue;
            }
            // A stock: live quote by ticker.
            _ => crate::assistant::market_connector::fetch_quote(&symbol)
                .map(|q| (q.price, q.prev_close, q.name)),
        };

        match priced {
            Ok((price, prev, name)) => {
                let value = price * quantity;
                let day_change = prev.map(|pc| (price - pc) * quantity);
                total_value += value;
                total_day_change += day_change.unwrap_or(0.0);
                holdings.push(HoldingView {
                    id,
                    symbol,
                    label: if label.is_empty() {
                        name.unwrap_or_else(|| "(unnamed)".to_string())
                    } else {
                        label
                    },
                    quantity,
                    kind,
                    price: Some(price),
                    prev_close: prev,
                    currency: currency.clone(),
                    value: Some(value),
                    day_change,
                    error: None,
                    broker: broker.clone(),
                    invested,
                    price_as_of: None,
                });
            }
            Err(e) => holdings.push(HoldingView {
                id,
                symbol,
                label,
                quantity,
                kind,
                price: None,
                prev_close: None,
                currency: currency.clone(),
                value: None,
                day_change: None,
                error: Some(e),
                broker: broker.clone(),
                invested,
                price_as_of,
            }),
        }
    }

    // Groups in first-seen order (the rows are sorted by broker).
    let mut groups: Vec<PortfolioGroup> = Vec::new();
    for h in &holdings {
        let name = if h.broker.is_empty() { "Other".to_string() } else { h.broker.clone() };
        if !groups.iter().any(|g| g.name == name) {
            let members: Vec<&HoldingView> = holdings
                .iter()
                .filter(|x| (if x.broker.is_empty() { "Other" } else { x.broker.as_str() }) == name)
                .collect();
            let value: f64 = members.iter().filter_map(|m| m.value).sum();
            let invested = group_invested.get(&name).copied().filter(|v| *v > 0.0).or_else(|| {
                members.iter().map(|m| m.invested).sum::<Option<f64>>()
            });
            groups.push(PortfolioGroup { gain: invested.map(|i| value - i), name, value, invested });
        }
    }

    Ok(Portfolio {
        holdings,
        total_value,
        total_day_change,
        currency,
        groups,
    })
}

/// The daily news briefing: headlines plus a one-line, model-written heads-up.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsBriefing {
    /// The lead line shown in the hero. Written by the local model when one is
    /// reachable, otherwise the top headline so the hero is never empty.
    pub headline: String,
    /// Whether the model wrote the headline, so the card can mark it as
    /// generated rather than passing a model's words off as NEXUS's own.
    pub generated: bool,
    pub items: Vec<crate::assistant::news_connector::NewsItem>,
}

/// Assemble the briefing: fetch news, then ask the model to pick out what
/// matters.
///
/// The model is asked off the database lock: `reason` can wait up to ninety
/// seconds on the local model, and holding the connection for that would freeze
/// every other command. The provider is built while the lock is held (it copies
/// its settings and borrows nothing), then the lock is dropped before the slow
/// call. A missing or unreachable model is not an error here: the briefing
/// still shows the news with the top headline as its lead.
#[tauri::command(async)]
pub fn nexus_news_briefing(
    state: State<'_, DbState>,
    limit: Option<usize>,
) -> Result<NewsBriefing, String> {
    use crate::assistant::reasoning::{self, AiContext, Purpose, Reasoning};

    let n = limit.unwrap_or(10).clamp(1, 20);
    let items = crate::assistant::news_connector::fetch_all(n);

    let fallback = items
        .first()
        .map(|i| i.title.clone())
        .unwrap_or_else(|| "No news to report right now.".to_string());

    let headlines: Vec<String> = items
        .iter()
        .take(reasoning::MAX_WORKSPACE_LINES)
        .map(|i| format!("[{}] {}", i.source, i.title))
        .collect();

    if headlines.is_empty() {
        return Ok(NewsBriefing {
            headline: fallback,
            generated: false,
            items,
        });
    }

    let context = AiContext {
        request: "You are Rohit's personal assistant. From the headlines under \
                  'What NEXUS knows right now', write ONE short, friendly sentence \
                  (20 words max) telling Rohit what is most worth his attention today. \
                  No preamble, no list, just the sentence."
            .to_string(),
        actions: Vec::new(),
        workspace: headlines,
        conversation: Vec::new(),
        categories: Vec::new(),
    };

    // Build the provider under the lock, then release it before reasoning.
    let provider = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        reasoning::best_provider(&conn)
    };

    let (headline, generated) = match provider {
        Ok(provider) => match provider.reason(Purpose::Answer, &context) {
            Ok(Reasoning::Answer { text }) if !text.trim().is_empty() => {
                (text.trim().to_string(), true)
            }
            // A plan, an empty answer, or a failure all fall back to the news
            // itself rather than showing nothing.
            _ => (fallback, false),
        },
        Err(_) => (fallback, false),
    };

    Ok(NewsBriefing {
        headline,
        generated,
        items,
    })
}

/// The latest news headlines, merged from the default feeds.
///
/// Read-only and identity-free: it fetches public RSS over HTTPS and returns a
/// flat list for the briefing to render. `limit` is clamped so a caller cannot
/// ask for an unbounded fetch. Runs on the thread pool because it makes several
/// network calls and must not block the main thread.
#[tauri::command(async)]
pub fn nexus_fetch_news(limit: Option<usize>) -> Result<Vec<crate::assistant::news_connector::NewsItem>, String> {
    let n = limit.unwrap_or(12).clamp(1, 40);
    Ok(crate::assistant::news_connector::fetch_all(n))
}

/// Hold macOS awake while stock watches are running, or let it sleep again.
///
/// The stock watcher only checks while NEXUS is open and the Mac is awake, so
/// "watch while I am away" really means "do not let the Mac doze". `caffeinate`
/// is the system tool for exactly that; NEXUS spawns one child and keeps its
/// handle so the same toggle can stop it.
///
/// `-w <own pid>` is a failsafe: if NEXUS exits without cleaning up (a crash),
/// caffeinate sees its parent gone and exits too, so a stray process can never
/// hold the Mac awake forever. Idempotent: turning it on twice does not stack a
/// second caffeinate, and turning it off when already off is a no-op.
///
/// What it cannot do is override a **closed lid**: clamshell sleep is enforced
/// below this layer. So this keeps checks running with the lid up, not with the
/// Mac shut.
#[tauri::command(async)]
pub fn nexus_set_keep_awake(enabled: bool) -> Result<bool, String> {
    // Function-local so only this command can touch the handle. A poisoned
    // lock means a previous caller panicked mid-toggle; the child handle is
    // still valid, so recover it rather than refuse to ever toggle again.
    static KEEP_AWAKE: std::sync::Mutex<Option<std::process::Child>> =
        std::sync::Mutex::new(None);
    let mut guard = KEEP_AWAKE.lock().unwrap_or_else(|e| e.into_inner());

    if enabled {
        if guard.is_none() {
            let pid = std::process::id().to_string();
            // -i: no idle sleep. -s: no system sleep on AC (ignored on
            // battery, where -i still holds). Display sleep is left alone, so
            // the screen can still dim while NEXUS keeps checking.
            let child = std::process::Command::new("/usr/bin/caffeinate")
                .args(["-i", "-s", "-w", &pid])
                .spawn()
                .map_err(|e| format!("Could not start caffeinate: {e}"))?;
            log::info!("keep_awake: on (caffeinate pid {})", child.id());
            *guard = Some(child);
        }
    } else if let Some(mut child) = guard.take() {
        let _ = child.kill();
        let _ = child.wait();
        log::info!("keep_awake: off");
    }
    Ok(enabled)
}

/// Ask macOS for calendar access, raising the system prompt.
///
/// For the Schedule card's button: a first read raises the prompt on its own,
/// but once denied or dismissed the only way back is to ask again explicitly,
/// which is what this is. Returns the access state afterwards so the card can
/// stop showing the prompt once it is granted.
#[tauri::command(async)]
pub fn nexus_request_calendar_access() -> Result<String, String> {
    use crate::assistant::calendar_store::{access, enabled, request_access, Access};
    if !enabled() {
        return Ok("off".to_string());
    }
    request_access();
    // The grant is answered asynchronously by the user, so this reports the
    // state as it stands now; the card re-reads after the prompt is answered.
    Ok(match access() {
        Access::Granted => "granted",
        Access::Denied => "denied",
        Access::Restricted => "restricted",
        Access::WriteOnly => "writeOnly",
        Access::NotDetermined => "notDetermined",
    }
    .to_string())
}

/// Open a web address in the default browser.
///
/// For cards that need the user signed in through the browser (the versions
/// card, which reads an SSO page NEXUS cannot authenticate itself). Only
/// http(s), passed after `--`, so nothing in the string reaches `open` as an
/// option.
#[tauri::command(async)]
pub fn nexus_open_url(url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("That is not a web address.".to_string());
    }
    let out = crate::assistant::shell::run(
        "/usr/bin/open",
        &["--", &url],
        std::time::Duration::from_secs(10),
    )
    .map_err(|e| e.to_string())?;
    if !out.success {
        return Err("Could not open the page.".to_string());
    }
    Ok(())
}

/// Raise a macOS notification banner.
///
/// Through `osascript display notification`, which is a real Notification
/// Center banner and needs no new dependency. Used by the stock watcher so a
/// restock surfaces even when NEXUS is not the front window.
#[tauri::command(async)]
pub fn nexus_desktop_notification(title: String, body: String) -> Result<(), String> {
    // Passed as `argv` to `on run`, never interpolated, so a title or body
    // with a quote in it cannot break out of the script.
    crate::assistant::shell::osascript(
        &["display notification (item 1 of argv) with title (item 2 of argv)"],
        &[&body, &title],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
