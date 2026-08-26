use rusqlite::{Connection, Result as RusqliteResult};
use serde::{Deserialize, Serialize};

/// A project row returned to the frontend.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub repository_path: Option<String>,
    pub repository_url: Option<String>,
    pub default_ide_id: Option<i64>,
    pub default_agent_id: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

/// Input for creating a new project (from React).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProjectInput {
    pub name: String,
    pub description: Option<String>,
    pub repository_path: Option<String>,
    pub repository_url: Option<String>,
}

/// Insert a new project and return the full row.
pub fn insert_project(
    conn: &Connection,
    input: &CreateProjectInput,
) -> Result<Project, String> {
    conn.execute(
        "INSERT INTO projects (name, description, repository_path, repository_url)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![
            input.name,
            input.description,
            input.repository_path,
            input.repository_url,
        ],
    )
    .map_err(|e| format!("Failed to insert project: {e}"))?;

    let id = conn.last_insert_rowid();
    get_project_by_id(conn, id)
}

/// Return all projects ordered by creation date descending.
pub fn list_projects(conn: &Connection) -> Result<Vec<Project>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, name, description, repository_path, repository_url,
                    default_ide_id, default_agent_id, created_at, updated_at
             FROM projects
             ORDER BY created_at DESC",
        )
        .map_err(|e| format!("Failed to prepare list_projects: {e}"))?;

    let rows = stmt
        .query_map([], map_project_row)
        .map_err(|e| format!("Failed to query projects: {e}"))?;

    rows.collect::<RusqliteResult<Vec<_>>>()
        .map_err(|e| format!("Failed to collect projects: {e}"))
}

/// Delete a project by ID. Returns an error if the project does not exist.
pub fn delete_project(conn: &Connection, id: i64) -> Result<(), String> {
    let affected = conn
        .execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![id])
        .map_err(|e| format!("Failed to delete project {id}: {e}"))?;

    if affected == 0 {
        return Err(format!("Project {id} not found"));
    }

    Ok(())
}

/// Return the total number of project rows.
#[allow(dead_code)]
pub fn count_projects(conn: &Connection) -> Result<i64, String> {
    conn.query_row("SELECT COUNT(*) FROM projects", [], |row| {
        row.get::<_, i64>(0)
    })
    .map_err(|e| format!("Failed to count projects: {e}"))
}

/// Return counts for all tables.
pub struct TableCounts {
    pub projects: i64,
    pub tasks: i64,
    pub ai_agents: i64,
    pub ides: i64,
    pub settings: i64,
}

pub fn count_all_tables(conn: &Connection) -> Result<TableCounts, String> {
    let projects = conn
        .query_row("SELECT COUNT(*) FROM projects", [], |r| r.get::<_, i64>(0))
        .map_err(|e| format!("count projects: {e}"))?;
    let tasks = conn
        .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get::<_, i64>(0))
        .map_err(|e| format!("count tasks: {e}"))?;
    let ai_agents = conn
        .query_row("SELECT COUNT(*) FROM ai_agents", [], |r| r.get::<_, i64>(0))
        .map_err(|e| format!("count ai_agents: {e}"))?;
    let ides = conn
        .query_row("SELECT COUNT(*) FROM ides", [], |r| r.get::<_, i64>(0))
        .map_err(|e| format!("count ides: {e}"))?;
    let settings = conn
        .query_row("SELECT COUNT(*) FROM settings", [], |r| r.get::<_, i64>(0))
        .map_err(|e| format!("count settings: {e}"))?;

    Ok(TableCounts { projects, tasks, ai_agents, ides, settings })
}

// ── Private helpers ──────────────────────────────────────────────────────────

fn get_project_by_id(conn: &Connection, id: i64) -> Result<Project, String> {
    conn.query_row(
        "SELECT id, name, description, repository_path, repository_url,
                default_ide_id, default_agent_id, created_at, updated_at
         FROM projects WHERE id = ?1",
        rusqlite::params![id],
        map_project_row,
    )
    .map_err(|e| format!("Failed to fetch project {id}: {e}"))
}

fn map_project_row(row: &rusqlite::Row<'_>) -> RusqliteResult<Project> {
    Ok(Project {
        id:               row.get(0)?,
        name:             row.get(1)?,
        description:      row.get(2)?,
        repository_path:  row.get(3)?,
        repository_url:   row.get(4)?,
        default_ide_id:   row.get(5)?,
        default_agent_id: row.get(6)?,
        created_at:       row.get(7)?,
        updated_at:       row.get(8)?,
    })
}
