//! Read-only aggregate queries across every table.
//!
//! This module owns no writes. It exists so counts are computed in SQL rather
//! than reduced over client arrays, keeping the database the single source of
//! truth and keeping the logic testable here (spec 006 2.3).
//!
//! The zero-row rule (spec 006 2.4): per-project and per-agent counts use
//! LEFT JOIN so a row with no matches appears with a count of 0 rather than
//! vanishing from the result. An INNER JOIN compiles, runs, and is silently
//! wrong.

use rusqlite::{Connection, Result as RusqliteResult};
use serde::{Deserialize, Serialize};

use super::tasks::{Task, TASK_STATUSES};

/// Workspace-wide totals.
///
/// `tasks` counts every task row. The four status buckets count only rows
/// whose status is in TASK_STATUSES, so on a database containing a status
/// written outside the application the buckets sum to less than `tasks`.
/// This is deliberate: see spec 006 2.6.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSummary {
    pub projects: i64,
    pub tasks: i64,
    pub tasks_open: i64,
    pub tasks_in_progress: i64,
    pub tasks_blocked: i64,
    pub tasks_done: i64,
    pub tasks_unassigned: i64,
    pub ides_total: i64,
    pub ides_enabled: i64,
    pub agents_total: i64,
    pub agents_enabled: i64,
}

/// Per-project task counts. One entry per project, including projects with
/// zero tasks (spec 006 2.4).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTaskCounts {
    pub project_id: i64,
    pub total: i64,
    pub open: i64,
    pub in_progress: i64,
    pub blocked: i64,
    pub done: i64,
}

/// Per-agent assigned-task counts. One entry per agent, including agents with
/// zero assigned tasks (spec 006 2.4).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTaskCounts {
    pub agent_id: i64,
    pub task_count: i64,
}

/// A task with the name of the project it belongs to.
/// Nests the NEXUS-004 Task rather than flattening it, so the existing serde
/// contract is reused verbatim and cannot drift (spec 006 2.5).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskWithProject {
    pub task: Task,
    pub project_name: String,
}

/// Recent-task limits are clamped rather than rejected: there is no input for
/// which the Overview must handle a failure it caused itself (spec 006 5.4).
fn clamp_limit(limit: i64) -> i64 {
    limit.clamp(1, 100)
}

/// The `tasks` columns in `Task` field order.
///
/// Declared here rather than imported because spec 006 N-07 keeps
/// `db/tasks.rs` unmodified, and its own column constant is private. The
/// `recent_tasks_carry_correct_project_name` test guards against drift.
const TASK_COLUMNS: &str = "t.id, t.external_id, t.title, t.description, t.status,
                            t.project_id, t.assigned_agent, t.created_at, t.updated_at";

fn map_task_with_project(row: &rusqlite::Row<'_>) -> RusqliteResult<TaskWithProject> {
    Ok(TaskWithProject {
        task: Task {
            id: row.get(0)?,
            external_id: row.get(1)?,
            title: row.get(2)?,
            description: row.get(3)?,
            status: row.get(4)?,
            project_id: row.get(5)?,
            assigned_agent: row.get(6)?,
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
        },
        project_name: row.get(9)?,
    })
}

// -- Queries -----------------------------------------------------------------

/// Every workspace total in one statement, built from scalar subqueries so it
/// cannot partially fail. Status values are bound from TASK_STATUSES rather
/// than typed as literals (spec 006 N-11).
pub fn workspace_summary(conn: &Connection) -> Result<WorkspaceSummary, String> {
    conn.query_row(
        "SELECT
           (SELECT COUNT(*) FROM projects),
           (SELECT COUNT(*) FROM tasks),
           (SELECT COUNT(*) FROM tasks WHERE status = ?1),
           (SELECT COUNT(*) FROM tasks WHERE status = ?2),
           (SELECT COUNT(*) FROM tasks WHERE status = ?3),
           (SELECT COUNT(*) FROM tasks WHERE status = ?4),
           (SELECT COUNT(*) FROM tasks WHERE assigned_agent IS NULL),
           (SELECT COUNT(*) FROM ides),
           (SELECT COUNT(*) FROM ides WHERE enabled = 1),
           (SELECT COUNT(*) FROM ai_agents),
           (SELECT COUNT(*) FROM ai_agents WHERE enabled = 1)",
        rusqlite::params![
            TASK_STATUSES[0],
            TASK_STATUSES[1],
            TASK_STATUSES[2],
            TASK_STATUSES[3],
        ],
        |row| {
            Ok(WorkspaceSummary {
                projects:            row.get(0)?,
                tasks:               row.get(1)?,
                tasks_open:          row.get(2)?,
                tasks_in_progress:   row.get(3)?,
                tasks_blocked:       row.get(4)?,
                tasks_done:          row.get(5)?,
                tasks_unassigned:    row.get(6)?,
                ides_total:          row.get(7)?,
                ides_enabled:        row.get(8)?,
                agents_total:        row.get(9)?,
                agents_enabled:      row.get(10)?,
            })
        },
    )
    .map_err(|e| format!("Failed to compute workspace summary: {e}"))
}

/// Task counts grouped by project.
///
/// LEFT JOIN plus COUNT(t.id) so a project with no tasks yields 0 rather than
/// being dropped. COUNT(*) would wrongly yield 1 for the null-filled row.
pub fn count_tasks_by_project(conn: &Connection) -> Result<Vec<ProjectTaskCounts>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT p.id,
                    COUNT(t.id),
                    SUM(CASE WHEN t.status = ?1 THEN 1 ELSE 0 END),
                    SUM(CASE WHEN t.status = ?2 THEN 1 ELSE 0 END),
                    SUM(CASE WHEN t.status = ?3 THEN 1 ELSE 0 END),
                    SUM(CASE WHEN t.status = ?4 THEN 1 ELSE 0 END)
               FROM projects p
               LEFT JOIN tasks t ON t.project_id = p.id
              GROUP BY p.id",
        )
        .map_err(|e| format!("Failed to prepare count_tasks_by_project: {e}"))?;

    let rows = stmt
        .query_map(
            rusqlite::params![
                TASK_STATUSES[0],
                TASK_STATUSES[1],
                TASK_STATUSES[2],
                TASK_STATUSES[3],
            ],
            |row| {
                Ok(ProjectTaskCounts {
                    project_id:  row.get(0)?,
                    total:       row.get(1)?,
                    // SUM over zero rows is NULL, not 0.
                    open:        row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    in_progress: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                    blocked:     row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                    done:        row.get::<_, Option<i64>>(5)?.unwrap_or(0),
                })
            },
        )
        .map_err(|e| format!("Failed to count tasks by project: {e}"))?;

    rows.collect::<RusqliteResult<Vec<_>>>()
        .map_err(|e| format!("Failed to collect project task counts: {e}"))
}

/// Assigned-task counts grouped by agent.
///
/// Tasks with a NULL assigned_agent join to no agent and are counted nowhere,
/// which is correct: they belong to no agent's workload.
pub fn count_tasks_by_agent(conn: &Connection) -> Result<Vec<AgentTaskCounts>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT a.id, COUNT(t.id)
               FROM ai_agents a
               LEFT JOIN tasks t ON t.assigned_agent = a.id
              GROUP BY a.id",
        )
        .map_err(|e| format!("Failed to prepare count_tasks_by_agent: {e}"))?;

    let rows = stmt
        .query_map([], |row| {
            Ok(AgentTaskCounts {
                agent_id:   row.get(0)?,
                task_count: row.get(1)?,
            })
        })
        .map_err(|e| format!("Failed to count tasks by agent: {e}"))?;

    rows.collect::<RusqliteResult<Vec<_>>>()
        .map_err(|e| format!("Failed to collect agent task counts: {e}"))
}

/// The most recently updated tasks across every project.
///
/// INNER JOIN is correct here and is not a zero-row-rule violation:
/// tasks.project_id is NOT NULL, so no task can be dropped by the join.
/// The id tiebreak keeps ordering stable for tasks sharing a millisecond.
pub fn list_recent_tasks(conn: &Connection, limit: i64) -> Result<Vec<TaskWithProject>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {TASK_COLUMNS}, p.name
               FROM tasks t
               INNER JOIN projects p ON p.id = t.project_id
              ORDER BY t.updated_at DESC, t.id DESC
              LIMIT ?1"
        ))
        .map_err(|e| format!("Failed to prepare list_recent_tasks: {e}"))?;

    let rows = stmt
        .query_map(rusqlite::params![clamp_limit(limit)], map_task_with_project)
        .map_err(|e| format!("Failed to list recent tasks: {e}"))?;

    rows.collect::<RusqliteResult<Vec<_>>>()
        .map_err(|e| format!("Failed to collect recent tasks: {e}"))
}

// -- Tests -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrations::MIGRATIONS;
    use crate::db::projects::{insert_project, CreateProjectInput};
    use crate::db::tasks::{insert_task, update_task, CreateTaskInput, UpdateTaskInput};

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

    fn seed_task(conn: &Connection, project_id: i64, title: &str, status: &str) -> Task {
        insert_task(
            conn,
            &CreateTaskInput {
                project_id,
                title: title.to_string(),
                description: None,
                status: Some(status.to_string()),
            },
        )
        .expect("insert task")
    }

    fn seed_ide(conn: &Connection, name: &str, enabled: bool) -> i64 {
        conn.execute(
            "INSERT INTO ides (name, ide_type, enabled) VALUES (?1, 'editor', ?2)",
            rusqlite::params![name, enabled],
        )
        .expect("insert ide");
        conn.last_insert_rowid()
    }

    fn seed_agent(conn: &Connection, name: &str, enabled: bool) -> i64 {
        conn.execute(
            "INSERT INTO ai_agents (name, agent_type, enabled) VALUES (?1, 'assistant', ?2)",
            rusqlite::params![name, enabled],
        )
        .expect("insert agent");
        conn.last_insert_rowid()
    }

    // -- Summary -------------------------------------------------------------

    #[test]
    fn summary_on_empty_database() {
        let conn = test_conn();
        let s = workspace_summary(&conn).expect("summary");

        assert_eq!(s.projects, 0);
        assert_eq!(s.tasks, 0);
        assert_eq!(s.tasks_open, 0);
        assert_eq!(s.tasks_in_progress, 0);
        assert_eq!(s.tasks_blocked, 0);
        assert_eq!(s.tasks_done, 0);
        assert_eq!(s.tasks_unassigned, 0);
        assert_eq!(s.ides_total, 0);
        assert_eq!(s.ides_enabled, 0);
        assert_eq!(s.agents_total, 0);
        assert_eq!(s.agents_enabled, 0);
    }

    #[test]
    fn summary_counts_totals_and_status_buckets() {
        let conn = test_conn();
        let a = seed_project(&conn, "A");
        let b = seed_project(&conn, "B");
        seed_task(&conn, a, "t1", "open");
        seed_task(&conn, a, "t2", "open");
        seed_task(&conn, a, "t3", "in_progress");
        seed_task(&conn, b, "t4", "blocked");
        seed_task(&conn, b, "t5", "done");

        let s = workspace_summary(&conn).expect("summary");

        assert_eq!(s.projects, 2);
        assert_eq!(s.tasks, 5);
        assert_eq!(s.tasks_open, 2);
        assert_eq!(s.tasks_in_progress, 1);
        assert_eq!(s.tasks_blocked, 1);
        assert_eq!(s.tasks_done, 1);
        assert_eq!(
            s.tasks_open + s.tasks_in_progress + s.tasks_blocked + s.tasks_done,
            s.tasks,
            "with only known statuses the buckets must partition the total"
        );
    }

    #[test]
    fn summary_counts_enabled_registry_separately() {
        let conn = test_conn();
        seed_ide(&conn, "On", true);
        seed_ide(&conn, "Off", false);
        seed_agent(&conn, "On", true);
        seed_agent(&conn, "Off", false);

        let s = workspace_summary(&conn).expect("summary");

        assert_eq!(s.ides_total, 2);
        assert_eq!(s.ides_enabled, 1);
        assert_eq!(s.agents_total, 2);
        assert_eq!(s.agents_enabled, 1);
    }

    #[test]
    fn summary_counts_unassigned_tasks() {
        let conn = test_conn();
        let p = seed_project(&conn, "P");
        let agent = seed_agent(&conn, "A", true);
        seed_task(&conn, p, "unassigned-1", "open");
        seed_task(&conn, p, "unassigned-2", "open");
        let assigned = seed_task(&conn, p, "assigned", "open");
        conn.execute(
            "UPDATE tasks SET assigned_agent = ?1 WHERE id = ?2",
            rusqlite::params![agent, assigned.id],
        )
        .expect("assign");

        let s = workspace_summary(&conn).expect("summary");

        assert_eq!(s.tasks, 3);
        assert_eq!(s.tasks_unassigned, 2);
    }

    #[test]
    fn summary_tolerates_unknown_status() {
        let conn = test_conn();
        let p = seed_project(&conn, "P");
        seed_task(&conn, p, "known", "open");
        // Written outside the application, as a sqlite3 session could.
        conn.execute(
            "INSERT INTO tasks (project_id, title, status) VALUES (?1, 'archived one', 'archived')",
            rusqlite::params![p],
        )
        .expect("insert unknown status");

        let s = workspace_summary(&conn).expect("summary");

        assert_eq!(s.tasks, 2, "an unknown status still counts in the total");
        let bucketed = s.tasks_open + s.tasks_in_progress + s.tasks_blocked + s.tasks_done;
        assert_eq!(bucketed, 1, "it must land in no bucket");
        assert!(bucketed < s.tasks, "the buckets deliberately do not partition");
    }

    // -- Per-project counts --------------------------------------------------

    /// The zero-row rule. An INNER JOIN would silently drop the empty project.
    #[test]
    fn counts_by_project_includes_zero_task_projects() {
        let conn = test_conn();
        let busy = seed_project(&conn, "Busy");
        let empty = seed_project(&conn, "Empty");
        seed_task(&conn, busy, "t1", "open");

        let counts = count_tasks_by_project(&conn).expect("counts");

        assert_eq!(counts.len(), 2, "every project must appear");
        let e = counts
            .iter()
            .find(|c| c.project_id == empty)
            .expect("empty project must be present, not omitted");
        assert_eq!(e.total, 0);
        assert_eq!(e.open, 0);
        assert_eq!(e.in_progress, 0);
        assert_eq!(e.blocked, 0);
        assert_eq!(e.done, 0);
    }

    #[test]
    fn counts_by_project_are_scoped() {
        let conn = test_conn();
        let a = seed_project(&conn, "A");
        let b = seed_project(&conn, "B");
        seed_task(&conn, a, "a1", "open");
        seed_task(&conn, a, "a2", "blocked");
        seed_task(&conn, b, "b1", "done");

        let counts = count_tasks_by_project(&conn).expect("counts");
        let ca = counts.iter().find(|c| c.project_id == a).expect("A");
        let cb = counts.iter().find(|c| c.project_id == b).expect("B");

        assert_eq!(ca.total, 2);
        assert_eq!(ca.open, 1);
        assert_eq!(ca.blocked, 1);
        assert_eq!(ca.done, 0, "B's done task must not leak into A");
        assert_eq!(cb.total, 1);
        assert_eq!(cb.done, 1);
    }

    #[test]
    fn counts_by_project_on_empty_database() {
        let conn = test_conn();
        assert!(count_tasks_by_project(&conn).expect("counts").is_empty());
    }

    // -- Per-agent counts ----------------------------------------------------

    #[test]
    fn counts_by_agent_includes_zero_task_agents() {
        let conn = test_conn();
        let p = seed_project(&conn, "P");
        let busy = seed_agent(&conn, "Busy", true);
        let idle = seed_agent(&conn, "Idle", true);
        let t = seed_task(&conn, p, "t1", "open");
        conn.execute(
            "UPDATE tasks SET assigned_agent = ?1 WHERE id = ?2",
            rusqlite::params![busy, t.id],
        )
        .expect("assign");

        let counts = count_tasks_by_agent(&conn).expect("counts");

        assert_eq!(counts.len(), 2, "every agent must appear");
        let i = counts
            .iter()
            .find(|c| c.agent_id == idle)
            .expect("idle agent must be present, not omitted");
        assert_eq!(i.task_count, 0);
        let b = counts.iter().find(|c| c.agent_id == busy).expect("busy");
        assert_eq!(b.task_count, 1);
    }

    #[test]
    fn counts_by_agent_excludes_unassigned_tasks() {
        let conn = test_conn();
        let p = seed_project(&conn, "P");
        let agent = seed_agent(&conn, "A", true);
        let t = seed_task(&conn, p, "assigned", "open");
        seed_task(&conn, p, "unassigned", "open");
        conn.execute(
            "UPDATE tasks SET assigned_agent = ?1 WHERE id = ?2",
            rusqlite::params![agent, t.id],
        )
        .expect("assign");

        let counts = count_tasks_by_agent(&conn).expect("counts");
        let total: i64 = counts.iter().map(|c| c.task_count).sum();

        assert_eq!(total, 1, "unassigned tasks belong to no agent");
    }

    #[test]
    fn counts_by_agent_after_agent_delete() {
        let conn = test_conn();
        let p = seed_project(&conn, "P");
        let doomed = seed_agent(&conn, "Doomed", true);
        let keeper = seed_agent(&conn, "Keeper", true);
        let t = seed_task(&conn, p, "t1", "open");
        conn.execute(
            "UPDATE tasks SET assigned_agent = ?1 WHERE id = ?2",
            rusqlite::params![doomed, t.id],
        )
        .expect("assign");

        conn.execute("DELETE FROM ai_agents WHERE id = ?1", rusqlite::params![doomed])
            .expect("delete agent");

        let counts = count_tasks_by_agent(&conn).expect("counts");

        assert_eq!(counts.len(), 1, "only the surviving agent may appear");
        assert_eq!(counts[0].agent_id, keeper);
        assert_eq!(
            counts[0].task_count, 0,
            "ON DELETE SET NULL unassigned the task"
        );
        assert!(
            !counts.iter().any(|c| c.agent_id == doomed),
            "no orphan agent id may appear"
        );
    }

    // -- Recent tasks --------------------------------------------------------

    #[test]
    fn recent_tasks_are_ordered_by_updated_at_desc() {
        let conn = test_conn();
        let p = seed_project(&conn, "P");
        let first = seed_task(&conn, p, "first", "open");
        std::thread::sleep(std::time::Duration::from_millis(5));
        seed_task(&conn, p, "second", "open");

        std::thread::sleep(std::time::Duration::from_millis(5));
        update_task(
            &conn,
            &UpdateTaskInput {
                id: first.id,
                title: "first, touched".to_string(),
                description: None,
                status: "open".to_string(),
            },
        )
        .expect("update");

        let recent = list_recent_tasks(&conn, 10).expect("recent");

        assert_eq!(recent.len(), 2);
        assert_eq!(
            recent[0].task.title, "first, touched",
            "the most recently updated task must lead"
        );
    }

    #[test]
    fn recent_tasks_tiebreak_is_deterministic() {
        let conn = test_conn();
        let p = seed_project(&conn, "P");
        // Inserted back to back; they may share a millisecond.
        let a = seed_task(&conn, p, "a", "open");
        let b = seed_task(&conn, p, "b", "open");
        conn.execute(
            "UPDATE tasks SET updated_at = '2026-01-01T00:00:00.000Z' WHERE id IN (?1, ?2)",
            rusqlite::params![a.id, b.id],
        )
        .expect("force identical stamps");

        let first = list_recent_tasks(&conn, 10).expect("recent");
        let second = list_recent_tasks(&conn, 10).expect("recent again");

        assert_eq!(first[0].task.id, b.id, "id DESC breaks the tie");
        assert_eq!(first[1].task.id, a.id);
        let ids: Vec<i64> = first.iter().map(|r| r.task.id).collect();
        let ids_again: Vec<i64> = second.iter().map(|r| r.task.id).collect();
        assert_eq!(ids, ids_again, "repeated calls must return the same order");
    }

    #[test]
    fn recent_tasks_respect_limit() {
        let conn = test_conn();
        let p = seed_project(&conn, "P");
        for i in 0..5 {
            seed_task(&conn, p, &format!("t{i}"), "open");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        let recent = list_recent_tasks(&conn, 2).expect("recent");

        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].task.title, "t4", "the two most recent");
        assert_eq!(recent[1].task.title, "t3");
    }

    #[test]
    fn recent_tasks_clamp_limit() {
        let conn = test_conn();
        let p = seed_project(&conn, "P");
        seed_task(&conn, p, "t1", "open");
        seed_task(&conn, p, "t2", "open");

        assert_eq!(
            list_recent_tasks(&conn, 0).expect("zero").len(),
            1,
            "0 clamps up to 1"
        );
        assert_eq!(
            list_recent_tasks(&conn, -5).expect("negative").len(),
            1,
            "a negative limit clamps up to 1"
        );
        assert_eq!(
            list_recent_tasks(&conn, 9999).expect("huge").len(),
            2,
            "an oversized limit clamps down and still returns everything present"
        );
    }

    #[test]
    fn recent_tasks_carry_correct_project_name() {
        let conn = test_conn();
        let a = seed_project(&conn, "Alpha");
        let b = seed_project(&conn, "Beta");
        let ta = seed_task(&conn, a, "from alpha", "blocked");
        seed_task(&conn, b, "from beta", "done");

        let recent = list_recent_tasks(&conn, 10).expect("recent");

        let ra = recent
            .iter()
            .find(|r| r.task.id == ta.id)
            .expect("alpha task present");
        assert_eq!(ra.project_name, "Alpha");
        assert_eq!(ra.task.title, "from alpha");
        assert_eq!(ra.task.status, "blocked", "column order must map correctly");
        assert_eq!(ra.task.project_id, a);
        assert_eq!(ra.task.external_id, None);
        assert_eq!(ra.task.assigned_agent, None);

        let rb = recent
            .iter()
            .find(|r| r.project_name == "Beta")
            .expect("beta task present");
        assert_eq!(rb.task.title, "from beta");
    }

    #[test]
    fn recent_tasks_on_empty_database() {
        let conn = test_conn();
        assert!(list_recent_tasks(&conn, 8).expect("recent").is_empty());
    }
}
