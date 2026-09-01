use rusqlite::{Connection, Result as RusqliteResult};
use serde::{Deserialize, Serialize};

/// The task status vocabulary. Mirrored by TaskStatus in src/types/db.ts.
/// The database column has no CHECK constraint; this layer is the only writer
/// and therefore the only place the vocabulary is enforced.
pub const TASK_STATUSES: [&str; 4] = ["open", "in_progress", "blocked", "done"];

/// A task row returned to the frontend.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: i64,
    pub external_id: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
    pub project_id: i64,
    pub assigned_agent: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

/// Input for creating a new task (from React).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTaskInput {
    pub project_id: i64,
    pub title: String,
    pub description: Option<String>,
    /// None means the schema default, 'open'.
    pub status: Option<String>,
}

/// Input for updating an existing task (from React).
/// Deliberately carries no external_id or assigned_agent: see update_task.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTaskInput {
    pub id: i64,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
}

/// Input for an agent assignment change (from React).
/// `agent_id: None` clears the assignment.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssignTaskAgentInput {
    pub id: i64,
    pub agent_id: Option<i64>,
}

/// Input for a status-only change (from React).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTaskStatusInput {
    pub id: i64,
    pub status: String,
}

/// Reject any status outside the known vocabulary.
fn validate_status(status: &str) -> Result<(), String> {
    if TASK_STATUSES.contains(&status) {
        Ok(())
    } else {
        Err(format!(
            "Invalid task status: {status}. Expected one of: {}",
            TASK_STATUSES.join(", ")
        ))
    }
}

// -- Private helpers ---------------------------------------------------------

const TASK_COLUMNS: &str = "id, external_id, title, description, status,
                            project_id, assigned_agent, created_at, updated_at";

fn get_task_by_id(conn: &Connection, id: i64) -> Result<Task, String> {
    conn.query_row(
        &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1"),
        rusqlite::params![id],
        map_task_row,
    )
    .map_err(|e| format!("Failed to fetch task {id}: {e}"))
}

fn map_task_row(row: &rusqlite::Row<'_>) -> RusqliteResult<Task> {
    Ok(Task {
        id: row.get(0)?,
        external_id: row.get(1)?,
        title: row.get(2)?,
        description: row.get(3)?,
        status: row.get(4)?,
        project_id: row.get(5)?,
        assigned_agent: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

// -- Create / read -----------------------------------------------------------

/// Insert a new task for a project and return the full row.
///
/// Writes project_id, title, description and status only. external_id and
/// assigned_agent are left to their schema default of NULL; NEXUS-004 has no
/// producer for either column.
///
/// A project_id that does not exist is rejected by the foreign key, not by a
/// pre-check: the database is the authority.
pub fn insert_task(conn: &Connection, input: &CreateTaskInput) -> Result<Task, String> {
    let title = input.title.trim();
    if title.is_empty() {
        return Err("Task title cannot be empty".to_string());
    }

    match input.status.as_deref() {
        Some(status) => {
            validate_status(status)?;
            conn.execute(
                "INSERT INTO tasks (project_id, title, description, status)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![input.project_id, title, input.description, status],
            )
        }
        // Omit the column entirely so SQLite applies its own DEFAULT 'open'.
        None => conn.execute(
            "INSERT INTO tasks (project_id, title, description)
             VALUES (?1, ?2, ?3)",
            rusqlite::params![input.project_id, title, input.description],
        ),
    }
    .map_err(|e| format!("Failed to insert task: {e}"))?;

    get_task_by_id(conn, conn.last_insert_rowid())
}

/// Return all tasks for one project, newest first.
/// The id tiebreak keeps ordering stable for tasks created inside the same
/// millisecond, which strftime('%f') cannot separate.
pub fn list_tasks(conn: &Connection, project_id: i64) -> Result<Vec<Task>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {TASK_COLUMNS}
               FROM tasks
              WHERE project_id = ?1
              ORDER BY created_at DESC, id DESC"
        ))
        .map_err(|e| format!("Failed to prepare list_tasks: {e}"))?;

    let rows = stmt
        .query_map(rusqlite::params![project_id], map_task_row)
        .map_err(|e| format!("Failed to query tasks for project {project_id}: {e}"))?;

    rows.collect::<RusqliteResult<Vec<_>>>()
        .map_err(|e| format!("Failed to collect tasks: {e}"))
}

// -- Update / delete ---------------------------------------------------------

/// Shared tail for both update paths (spec 5.1): map the rusqlite result onto
/// a Task, or a typed error when no row matched.
fn finish_task_update(
    conn: &Connection,
    id: i64,
    result: RusqliteResult<usize>,
) -> Result<Task, String> {
    let affected = result.map_err(|e| format!("Failed to update task {id}: {e}"))?;

    if affected == 0 {
        return Err(format!("Task {id} not found"));
    }

    get_task_by_id(conn, id)
}

/// Update a task's title, description and status, and return the full row.
///
/// The column list is exactly title, description, status and updated_at.
/// external_id and assigned_agent must never appear here: a struct-shaped
/// UPDATE would null out data that a later integration milestone writes.
/// Guarded by update_task_preserves_external_id_and_agent.
pub fn update_task(conn: &Connection, input: &UpdateTaskInput) -> Result<Task, String> {
    let title = input.title.trim();
    if title.is_empty() {
        return Err("Task title cannot be empty".to_string());
    }
    validate_status(&input.status)?;

    let result = conn.execute(
        "UPDATE tasks
            SET title       = ?1,
                description = ?2,
                status      = ?3,
                updated_at  = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
          WHERE id = ?4",
        rusqlite::params![title, input.description, input.status, input.id],
    );

    finish_task_update(conn, input.id, result)
}

/// Change a task's status only, and return the full row.
/// Same column-list discipline as update_task.
pub fn update_task_status(
    conn: &Connection,
    input: &UpdateTaskStatusInput,
) -> Result<Task, String> {
    validate_status(&input.status)?;

    let result = conn.execute(
        "UPDATE tasks
            SET status     = ?1,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
          WHERE id = ?2",
        rusqlite::params![input.status, input.id],
    );

    finish_task_update(conn, input.id, result)
}

/// Assign an agent to a task, or clear the assignment, and return the full row.
///
/// NEXUS-005 adds this rather than widening `update_task`, so that
/// `update_task` keeps its guarantee of never writing `external_id` or
/// `assigned_agent` (spec 2.5, N-09). Same column-list discipline applies here:
/// `external_id` is not in this statement either.
pub fn assign_task_agent(conn: &Connection, input: &AssignTaskAgentInput) -> Result<Task, String> {
    let result = conn.execute(
        "UPDATE tasks
            SET assigned_agent = ?1,
                updated_at     = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
          WHERE id = ?2",
        rusqlite::params![input.agent_id, input.id],
    );

    finish_task_update(conn, input.id, result)
}

/// Delete a task by ID. Returns an error if the task does not exist.
pub fn delete_task(conn: &Connection, id: i64) -> Result<(), String> {
    let affected = conn
        .execute("DELETE FROM tasks WHERE id = ?1", rusqlite::params![id])
        .map_err(|e| format!("Failed to delete task {id}: {e}"))?;

    if affected == 0 {
        return Err(format!("Task {id} not found"));
    }

    Ok(())
}

// -- Tests -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrations::MIGRATIONS;
    use crate::db::projects::{delete_project, insert_project, CreateProjectInput};

    /// In-memory DB with the real migration set and FK enforcement, matching
    /// what db::init() configures for the on-disk database.
    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .expect("enable foreign keys");
        for &(_, sql) in MIGRATIONS {
            conn.execute_batch(sql).expect("apply migration");
        }
        conn
    }

    fn seed_project(conn: &Connection, name: &str) -> i64 {
        insert_project(
            conn,
            &CreateProjectInput {
                name: name.to_string(),
                description: None,
                repository_path: None,
                repository_url: None,
                default_ide_id: None,
                default_agent_id: None,
            },
        )
        .expect("insert project")
        .id
    }

    fn seed_task(conn: &Connection, project_id: i64, title: &str) -> Task {
        insert_task(
            conn,
            &CreateTaskInput {
                project_id,
                title: title.to_string(),
                description: Some("seeded".to_string()),
                status: None,
            },
        )
        .expect("insert task")
    }

    #[test]
    fn insert_task_defaults_to_open() {
        let conn = test_conn();
        let project_id = seed_project(&conn, "P");

        let task = insert_task(
            &conn,
            &CreateTaskInput {
                project_id,
                title: "  Untrimmed  ".to_string(),
                description: None,
                status: None,
            },
        )
        .expect("insert task");

        assert_eq!(
            task.status, "open",
            "omitted status must use the schema default"
        );
        assert_eq!(task.title, "Untrimmed", "title must be trimmed");
        assert_eq!(task.project_id, project_id);
        assert_eq!(
            task.external_id, None,
            "UI-created tasks must leave external_id NULL"
        );
        assert_eq!(
            task.assigned_agent, None,
            "UI-created tasks must leave assigned_agent NULL"
        );
        assert_eq!(task.created_at, task.updated_at);
    }

    #[test]
    fn insert_task_rejects_unknown_project() {
        let conn = test_conn();

        let err = insert_task(
            &conn,
            &CreateTaskInput {
                project_id: 9999,
                title: "Orphan".to_string(),
                description: None,
                status: None,
            },
        )
        .expect_err("dangling project_id must be rejected");

        assert!(
            err.contains("FOREIGN KEY constraint failed"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn insert_task_rejects_empty_title() {
        let conn = test_conn();
        let project_id = seed_project(&conn, "P");

        let err = insert_task(
            &conn,
            &CreateTaskInput {
                project_id,
                title: "   ".to_string(),
                description: None,
                status: None,
            },
        )
        .expect_err("whitespace-only title must be rejected");

        assert!(err.contains("cannot be empty"), "unexpected error: {err}");
        assert!(
            list_tasks(&conn, project_id).expect("list").is_empty(),
            "no row may be inserted when validation fails"
        );
    }

    #[test]
    fn insert_task_rejects_bad_status() {
        let conn = test_conn();
        let project_id = seed_project(&conn, "P");

        let err = insert_task(
            &conn,
            &CreateTaskInput {
                project_id,
                title: "Bad status".to_string(),
                description: None,
                status: Some("wontfix".to_string()),
            },
        )
        .expect_err("unknown status must be rejected");

        assert!(
            err.contains("Invalid task status: wontfix"),
            "unexpected error: {err}"
        );
        assert!(
            err.contains("in_progress"),
            "error must name the vocabulary: {err}"
        );
        assert!(list_tasks(&conn, project_id).expect("list").is_empty());
    }

    #[test]
    fn list_tasks_is_scoped_to_project() {
        let conn = test_conn();
        let a = seed_project(&conn, "A");
        let b = seed_project(&conn, "B");

        seed_task(&conn, a, "A-1");
        seed_task(&conn, a, "A-2");
        seed_task(&conn, b, "B-1");

        let a_tasks = list_tasks(&conn, a).expect("list A");
        let b_tasks = list_tasks(&conn, b).expect("list B");

        assert_eq!(a_tasks.len(), 2);
        assert_eq!(b_tasks.len(), 1);
        assert!(a_tasks.iter().all(|t| t.project_id == a));
        assert!(a_tasks.iter().all(|t| !t.title.starts_with("B-")));
        assert_eq!(b_tasks[0].title, "B-1");
    }

    #[test]
    fn update_task_changes_fields_and_updated_at() {
        let conn = test_conn();
        let project_id = seed_project(&conn, "P");
        let created = seed_task(&conn, project_id, "Before");

        std::thread::sleep(std::time::Duration::from_millis(5));

        let updated = update_task(
            &conn,
            &UpdateTaskInput {
                id: created.id,
                title: "After".to_string(),
                description: Some("changed".to_string()),
                status: "in_progress".to_string(),
            },
        )
        .expect("update task");

        assert_eq!(updated.title, "After");
        assert_eq!(updated.description.as_deref(), Some("changed"));
        assert_eq!(updated.status, "in_progress");
        assert_eq!(
            updated.project_id, project_id,
            "update must not move the task"
        );
        assert_eq!(
            updated.created_at, created.created_at,
            "created_at must not change on update"
        );
        assert_ne!(
            updated.updated_at, created.updated_at,
            "updated_at must advance on update"
        );

        let reread = list_tasks(&conn, project_id).expect("list");
        assert_eq!(reread[0].title, "After");
        assert_eq!(reread[0].updated_at, updated.updated_at);
    }

    /// The load-bearing test: the only automated guard against a struct-shaped
    /// UPDATE nulling out the integration columns (spec 4.3).
    #[test]
    fn update_task_preserves_external_id_and_agent() {
        let conn = test_conn();
        let project_id = seed_project(&conn, "P");

        // Seed the integration columns directly, the way a future milestone
        // or a sqlite3 session would.
        conn.execute(
            "INSERT INTO ai_agents (name, agent_type) VALUES ('Test Agent', 'test')",
            [],
        )
        .expect("insert agent");
        let agent_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO tasks (project_id, title, status, external_id, assigned_agent)
             VALUES (?1, 'Imported', 'open', 'JIRA-123', ?2)",
            rusqlite::params![project_id, agent_id],
        )
        .expect("insert imported task");
        let task_id = conn.last_insert_rowid();

        let updated = update_task(
            &conn,
            &UpdateTaskInput {
                id: task_id,
                title: "Renamed locally".to_string(),
                description: Some("edited in the UI".to_string()),
                status: "done".to_string(),
            },
        )
        .expect("update task");

        assert_eq!(updated.title, "Renamed locally");
        assert_eq!(
            updated.external_id.as_deref(),
            Some("JIRA-123"),
            "update_task must not clobber external_id"
        );
        assert_eq!(
            updated.assigned_agent,
            Some(agent_id),
            "update_task must not clobber assigned_agent"
        );

        // Same guarantee for the status-only path.
        let after_status = update_task_status(
            &conn,
            &UpdateTaskStatusInput {
                id: task_id,
                status: "blocked".to_string(),
            },
        )
        .expect("update status");

        assert_eq!(after_status.external_id.as_deref(), Some("JIRA-123"));
        assert_eq!(after_status.assigned_agent, Some(agent_id));
    }

    #[test]
    fn update_task_status_only() {
        let conn = test_conn();
        let project_id = seed_project(&conn, "P");
        let created = seed_task(&conn, project_id, "Titled");

        std::thread::sleep(std::time::Duration::from_millis(5));

        let updated = update_task_status(
            &conn,
            &UpdateTaskStatusInput {
                id: created.id,
                status: "done".to_string(),
            },
        )
        .expect("update status");

        assert_eq!(updated.status, "done");
        assert_eq!(updated.title, created.title, "title must not change");
        assert_eq!(
            updated.description, created.description,
            "description must not change"
        );
        assert_eq!(updated.created_at, created.created_at);
        assert_ne!(updated.updated_at, created.updated_at);

        let err = update_task_status(
            &conn,
            &UpdateTaskStatusInput {
                id: created.id,
                status: "archived".to_string(),
            },
        )
        .expect_err("unknown status must be rejected");
        assert!(
            err.contains("Invalid task status"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn delete_task_removes_only_that_task() {
        let conn = test_conn();
        let project_id = seed_project(&conn, "P");
        let keep = seed_task(&conn, project_id, "Keep");
        let doomed = seed_task(&conn, project_id, "Doomed");

        delete_task(&conn, doomed.id).expect("delete task");

        let remaining = list_tasks(&conn, project_id).expect("list");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, keep.id);

        let err = delete_task(&conn, doomed.id).expect_err("second delete must fail");
        assert!(err.contains("not found"), "unexpected error: {err}");
    }

    /// The NEXUS-003 invariant, re-asserted from the task module's perspective.
    #[test]
    fn delete_project_still_cascades_to_tasks() {
        let conn = test_conn();
        let keep = seed_project(&conn, "Keep");
        let doomed = seed_project(&conn, "Doomed");

        seed_task(&conn, keep, "keep-1");
        seed_task(&conn, doomed, "doomed-1");
        seed_task(&conn, doomed, "doomed-2");

        assert_eq!(list_tasks(&conn, doomed).expect("list").len(), 2);

        delete_project(&conn, doomed).expect("delete project");

        assert!(
            list_tasks(&conn, doomed).expect("list").is_empty(),
            "child tasks must be cascade-deleted with their project"
        );
        assert_eq!(
            list_tasks(&conn, keep).expect("list").len(),
            1,
            "another project's tasks must be untouched"
        );

        let orphans: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tasks
                  WHERE project_id NOT IN (SELECT id FROM projects)",
                [],
                |r| r.get(0),
            )
            .expect("count orphans");
        assert_eq!(orphans, 0, "no task may reference a removed project");
    }

    #[test]
    fn assign_task_agent_sets_and_clears() {
        let conn = test_conn();
        let project_id = seed_project(&conn, "P");

        conn.execute(
            "INSERT INTO ai_agents (name, agent_type) VALUES ('A1', 'assistant')",
            [],
        )
        .expect("insert agent");
        let agent_id = conn.last_insert_rowid();

        // Seed external_id directly so the assignment path can be shown not to
        // disturb it, the same guarantee update_task carries.
        conn.execute(
            "INSERT INTO tasks (project_id, title, description, status, external_id)
             VALUES (?1, 'Titled', 'described', 'blocked', 'JIRA-7')",
            rusqlite::params![project_id],
        )
        .expect("insert task");
        let task_id = conn.last_insert_rowid();
        let before = list_tasks(&conn, project_id).expect("list")[0]
            .updated_at
            .clone();

        std::thread::sleep(std::time::Duration::from_millis(5));

        let assigned = assign_task_agent(
            &conn,
            &AssignTaskAgentInput {
                id: task_id,
                agent_id: Some(agent_id),
            },
        )
        .expect("assign agent");

        assert_eq!(assigned.assigned_agent, Some(agent_id));
        assert_eq!(assigned.title, "Titled", "title must not change");
        assert_eq!(assigned.description.as_deref(), Some("described"));
        assert_eq!(assigned.status, "blocked", "status must not change");
        assert_eq!(
            assigned.external_id.as_deref(),
            Some("JIRA-7"),
            "assignment must not disturb external_id"
        );
        assert_ne!(assigned.updated_at, before, "updated_at must advance");

        let cleared = assign_task_agent(
            &conn,
            &AssignTaskAgentInput {
                id: task_id,
                agent_id: None,
            },
        )
        .expect("clear agent");

        assert_eq!(
            cleared.assigned_agent, None,
            "None must clear the assignment"
        );
        assert_eq!(cleared.external_id.as_deref(), Some("JIRA-7"));
    }

    #[test]
    fn assign_task_agent_rejects_unknown_agent() {
        let conn = test_conn();
        let project_id = seed_project(&conn, "P");
        let task = seed_task(&conn, project_id, "T");

        let err = assign_task_agent(
            &conn,
            &AssignTaskAgentInput {
                id: task.id,
                agent_id: Some(9999),
            },
        )
        .expect_err("dangling agent_id must be rejected");

        assert!(
            err.contains("FOREIGN KEY constraint failed"),
            "unexpected error: {err}"
        );
    }
}
