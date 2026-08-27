use serde::{Deserialize, Serialize};
use tauri::{Emitter, State};

use crate::assistant::action::{ActionOutcome, ActionRequest, ActionError};
use crate::assistant::approval::ApprovalStore;
use crate::assistant::audit::{list_recent as list_audit, AuditEntry};
use crate::assistant::context::{assemble, AssistantContext};
use crate::assistant::converse::{respond, AssistantReply};
use crate::assistant::permission::{set_grant, Permission};
use crate::assistant::reasoning::{
    list_recent_use, read_policy, set_policy, AiAuditEntry, PrivacyPolicy,
};
use crate::assistant::proactive::{
    briefing, preview, read_policy as read_proactive, record_accepted,
    set_policy as set_proactive, surface, Briefing, ProactivePolicy,
};
use crate::assistant::referent::Resolution;
use crate::assistant::suggestions::{dismiss, generate, restore, Suggestion};
use crate::assistant::session::{AssistantSession, AssistantState, SessionSnapshot, TurnInput};
use crate::assistant::{
    execute_action, list_connectors, set_connector_config, set_connector_enabled,
    ConnectorView, EVENT_ASSISTANT_STATE,
};
use crate::voice::intent::{resolve_voice_intent, VoiceCommandSpec, VoiceIntent};
use crate::voice::response::{response_for, VoiceOutcome};
use crate::voice::speech::{self, VoiceOption, VoiceSpeech};
use crate::voice::{self, VoiceStatus};
use crate::db::{
    self,
    agents::{delete_agent, insert_agent, list_agents, update_agent},
    ides::{delete_ide, insert_ide, list_ides, update_ide},
    projects::{
        count_all_tables, insert_project, list_projects, update_project,
        CreateProjectInput, Project, UpdateProjectInput,
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
        projects:  counts.projects,
        tasks:     counts.tasks,
        ai_agents: counts.ai_agents,
        ides:      counts.ides,
        settings:  counts.settings,
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
pub fn nexus_list_tasks(
    state: State<'_, DbState>,
    project_id: i64,
) -> Result<Vec<Task>, String> {
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
pub fn nexus_get_workspace_summary(
    state: State<'_, DbState>,
) -> Result<WorkspaceSummary, String> {
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
pub fn nexus_voice_start(
    state: State<'_, DbState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
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
    let _ = app.emit(EVENT_ASSISTANT_STATE, session.snapshot(approvals.pending_count()));
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
pub fn nexus_list_audit(
    state: State<'_, DbState>,
    limit: i64,
) -> Result<Vec<AuditEntry>, String> {
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
pub fn nexus_assistant_resolve(
    session: State<'_, AssistantSession>,
    phrase: String,
) -> Resolution {
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
    session.begin_turn(if spoken {
        TurnInput::Voice { text: text.clone() }
    } else {
        TurnInput::Text { text: text.clone() }
    });

    let response = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        let snapshot = session.snapshot(approvals.pending_count());
        respond(&text, &conn, &snapshot, &commands, &project_names)
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

    let reply = response.reply;
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
        AssistantReply::Unresolved { reason } => {
            session.advance(AssistantState::Failed, None, Some(reason.clone()))
        }
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
pub fn nexus_reasoning_status(
    state: State<'_, DbState>,
) -> Result<serde_json::Value, String> {
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
pub fn nexus_list_suggestions(
    state: State<'_, DbState>,
) -> Result<Vec<Suggestion>, String> {
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
pub fn nexus_surface_suggestions(
    state: State<'_, DbState>,
) -> Result<Vec<Suggestion>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    Ok(surface(&conn))
}

/// The same list, without starting anyone's cooldown. For polling.
#[tauri::command]
pub fn nexus_preview_suggestions(
    state: State<'_, DbState>,
) -> Result<Vec<Suggestion>, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    Ok(preview(&conn))
}

/// Where things stand, from local data only.
#[tauri::command]
pub fn nexus_briefing(state: State<'_, DbState>) -> Result<Briefing, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    Ok(briefing(&conn))
}

/// Tell NEXUS a suggestion was acted on, which resets how tired of it it is.
#[tauri::command]
pub fn nexus_accept_suggestion(
    state: State<'_, DbState>,
    key: String,
) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    record_accepted(&conn, &key)
}

#[tauri::command]
pub fn nexus_proactive_policy(
    state: State<'_, DbState>,
) -> Result<ProactivePolicy, String> {
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
