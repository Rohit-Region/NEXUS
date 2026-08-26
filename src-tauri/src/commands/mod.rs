use serde::{Deserialize, Serialize};
use tauri::State;

use crate::db::{
    self,
    agents::{delete_agent, insert_agent, list_agents, update_agent},
    ides::{delete_ide, insert_ide, list_ides, update_ide},
    projects::{
        count_all_tables, delete_project, insert_project, list_projects, update_project,
        CreateProjectInput, Project, UpdateProjectInput,
    },
    registry::{CreateRegistryEntryInput, RegistryEntry, UpdateRegistryEntryInput},
    tasks::{
        assign_task_agent, delete_task, insert_task, list_tasks, update_task, update_task_status,
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

/// Delete a project by ID.
#[tauri::command]
pub fn nexus_delete_project(state: State<'_, DbState>, id: i64) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    delete_project(&conn, id)
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

/// Delete a task by ID.
#[tauri::command]
pub fn nexus_delete_task(state: State<'_, DbState>, id: i64) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    delete_task(&conn, id)
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
