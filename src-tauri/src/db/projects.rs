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

/// Input for updating an existing project (from React).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProjectInput {
    pub id: i64,
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

/// Update an existing project and return the full updated row.
/// `updated_at` is set explicitly by the UPDATE statement; SQLite has no
/// ON UPDATE default and NEXUS deliberately uses no triggers.
pub fn update_project(
    conn: &Connection,
    input: &UpdateProjectInput,
) -> Result<Project, String> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err("Project name cannot be empty".to_string());
    }

    let affected = conn
        .execute(
            "UPDATE projects
                SET name            = ?1,
                    description     = ?2,
                    repository_path = ?3,
                    repository_url  = ?4,
                    updated_at      = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
              WHERE id = ?5",
            rusqlite::params![
                name,
                input.description,
                input.repository_path,
                input.repository_url,
                input.id,
            ],
        )
        .map_err(|e| format!("Failed to update project {}: {e}", input.id))?;

    if affected == 0 {
        return Err(format!("Project {} not found", input.id));
    }

    get_project_by_id(conn, input.id)
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

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrations::MIGRATIONS;

    /// In-memory DB with the real migration set and FK enforcement,
    /// matching what db::init() configures for the on-disk database.
    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .expect("enable foreign keys");
        for &(_, sql) in MIGRATIONS {
            conn.execute_batch(sql).expect("apply migration");
        }
        conn
    }

    fn seed_project(conn: &Connection, name: &str) -> Project {
        insert_project(
            conn,
            &CreateProjectInput {
                name: name.to_string(),
                description: Some("initial description".to_string()),
                repository_path: Some("/tmp/initial".to_string()),
                repository_url: None,
            },
        )
        .expect("insert project")
    }

    #[test]
    fn update_project_changes_fields_and_updated_at() {
        let conn = test_conn();
        let created = seed_project(&conn, "Original");

        // Force a distinct timestamp; strftime('%f') has millisecond resolution.
        std::thread::sleep(std::time::Duration::from_millis(5));

        let updated = update_project(
            &conn,
            &UpdateProjectInput {
                id: created.id,
                name: "Renamed".to_string(),
                description: Some("new description".to_string()),
                repository_path: None,
                repository_url: Some("https://example.com/repo".to_string()),
            },
        )
        .expect("update project");

        assert_eq!(updated.id, created.id);
        assert_eq!(updated.name, "Renamed");
        assert_eq!(updated.description.as_deref(), Some("new description"));
        assert_eq!(updated.repository_path, None);
        assert_eq!(
            updated.repository_url.as_deref(),
            Some("https://example.com/repo")
        );
        assert_eq!(
            updated.created_at, created.created_at,
            "created_at must not change on update"
        );
        assert_ne!(
            updated.updated_at, created.updated_at,
            "updated_at must advance on update"
        );

        // The change is persisted, not just returned.
        let reread = list_projects(&conn).expect("list projects");
        assert_eq!(reread.len(), 1);
        assert_eq!(reread[0].name, "Renamed");
        assert_eq!(reread[0].updated_at, updated.updated_at);
    }

    #[test]
    fn update_project_rejects_empty_name() {
        let conn = test_conn();
        let created = seed_project(&conn, "Keep Me");

        let err = update_project(
            &conn,
            &UpdateProjectInput {
                id: created.id,
                name: "   ".to_string(),
                description: None,
                repository_path: None,
                repository_url: None,
            },
        )
        .expect_err("empty name must be rejected");
        assert!(err.contains("cannot be empty"), "unexpected error: {err}");

        // The row is untouched.
        let reread = list_projects(&conn).expect("list projects");
        assert_eq!(reread[0].name, "Keep Me");
        assert_eq!(reread[0].updated_at, created.updated_at);
    }

    #[test]
    fn update_project_rejects_unknown_id() {
        let conn = test_conn();
        let err = update_project(
            &conn,
            &UpdateProjectInput {
                id: 4242,
                name: "Ghost".to_string(),
                description: None,
                repository_path: None,
                repository_url: None,
            },
        )
        .expect_err("unknown id must be rejected");
        assert!(err.contains("not found"), "unexpected error: {err}");
    }

    /// NEXUS-002 semantics: tasks.project_id ... ON DELETE CASCADE.
    /// NEXUS-003 must not change this.
    #[test]
    fn delete_project_cascades_to_tasks() {
        let conn = test_conn();
        let keep = seed_project(&conn, "Keep");
        let doomed = seed_project(&conn, "Doomed");

        for project_id in [keep.id, doomed.id] {
            conn.execute(
                "INSERT INTO tasks (title, project_id) VALUES (?1, ?2)",
                rusqlite::params![format!("task for {project_id}"), project_id],
            )
            .expect("insert task");
        }

        let count_tasks_for = |project_id: i64| -> i64 {
            conn.query_row(
                "SELECT COUNT(*) FROM tasks WHERE project_id = ?1",
                rusqlite::params![project_id],
                |r| r.get::<_, i64>(0),
            )
            .expect("count tasks")
        };

        assert_eq!(count_tasks_for(doomed.id), 1);

        delete_project(&conn, doomed.id).expect("delete project");

        assert_eq!(
            count_tasks_for(doomed.id),
            0,
            "child tasks must be cascade-deleted with their project"
        );
        assert_eq!(
            count_tasks_for(keep.id),
            1,
            "unrelated tasks must be untouched"
        );
        assert_eq!(count_projects(&conn).expect("count projects"), 1);
    }

    #[test]
    fn deleting_project_does_not_orphan_updated_project() {
        let conn = test_conn();
        let a = seed_project(&conn, "A");
        let b = seed_project(&conn, "B");

        update_project(
            &conn,
            &UpdateProjectInput {
                id: b.id,
                name: "B renamed".to_string(),
                description: None,
                repository_path: None,
                repository_url: None,
            },
        )
        .expect("update B");

        delete_project(&conn, a.id).expect("delete A");

        let remaining = list_projects(&conn).expect("list projects");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, b.id);
        assert_eq!(remaining[0].name, "B renamed");
    }
}
