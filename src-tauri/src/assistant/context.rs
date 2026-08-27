//! NEXUS-013: context assembly.
//!
//! Context is **pulled per request, scoped, and never accumulated into a
//! blob**. There is no long-lived context object growing in the background;
//! there is a function that answers "what do I know that is relevant to
//! this", with a budget.
//!
//! That shape matters most for a milestone that has not been built yet. When
//! NEXUS-019 starts sending context to a reasoning provider, the thing it
//! sends has to be small and deliberate. Building the ambient-blob version
//! first and trimming it later never works, because by then everything reads
//! from the blob.
//!
//! Work context is **derived from the conversation**, not tracked separately.
//! The current project is the most recent project NEXUS mentioned, checked
//! against the database so a deleted row does not linger. A second tracker
//! would be a second thing to keep in step.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::audit::{list_recent, AuditEntry};
use super::referent::ReferentKind;
use super::session::{AssistantSession, SessionSnapshot};

/// How many past actions count as "recent". A budget, not a page size: this
/// is what a reasoning provider would eventually be shown.
const RECENT_ACTION_BUDGET: i64 = 10;

/// A project, as context rather than as a row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectContext {
    pub id: i64,
    pub name: String,
    pub open_tasks: i64,
    pub blocked_tasks: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskContext {
    pub id: i64,
    pub title: String,
    pub status: String,
    pub project_id: i64,
}

/// What NEXUS is working on.
///
/// Deliberately narrow. The current IDE, browser tab and frontmost
/// application belong here too, and the struct is shaped to take them, but
/// they arrive with the connectors that can actually observe them
/// (NEXUS-015, NEXUS-016). Adding always-empty fields now would be pretending
/// to know something.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkContext {
    pub current_project: Option<ProjectContext>,
    pub current_task: Option<TaskContext>,
}

/// Everything NEXUS knows right now, assembled on demand.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantContext {
    /// Conversation, referents and rendered lists.
    pub session: SessionSnapshot,
    pub work: WorkContext,
    /// What NEXUS has done lately, from the audit trail.
    pub recent_actions: Vec<AuditEntry>,
}

/// Read a project as context, or None if it no longer exists.
fn project_context(conn: &Connection, id: i64) -> Option<ProjectContext> {
    let name: String = conn
        .query_row("SELECT name FROM projects WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .ok()?;

    // One pass for both counts. `status` is a free-form column with a known
    // vocabulary, so an unexpected value simply counts as neither.
    let (open_tasks, blocked_tasks) = conn
        .query_row(
            "SELECT
                 SUM(CASE WHEN status = 'open'    THEN 1 ELSE 0 END),
                 SUM(CASE WHEN status = 'blocked' THEN 1 ELSE 0 END)
               FROM tasks WHERE project_id = ?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?.unwrap_or(0),
                    row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                ))
            },
        )
        .unwrap_or((0, 0));

    Some(ProjectContext {
        id,
        name,
        open_tasks,
        blocked_tasks,
    })
}

fn task_context(conn: &Connection, id: i64) -> Option<TaskContext> {
    conn.query_row(
        "SELECT id, title, status, project_id FROM tasks WHERE id = ?1",
        [id],
        |row| {
            Ok(TaskContext {
                id: row.get(0)?,
                title: row.get(1)?,
                status: row.get(2)?,
                project_id: row.get(3)?,
            })
        },
    )
    .ok()
}

/// The most recent referent of a kind, and the row id it points at.
///
/// Reads `metadata.id`, which is the convention every registration in
/// NEXUS-012 follows. A referent whose metadata does not carry one is skipped
/// rather than guessed at.
fn latest_row_id(snapshot: &SessionSnapshot, kind: ReferentKind) -> Option<i64> {
    snapshot
        .referents
        .iter()
        .filter(|r| r.kind == kind)
        .max_by_key(|r| (r.turn, r.id))
        .and_then(|r| r.metadata.get("id"))
        .and_then(|v| v.as_i64())
}

/// Derive what NEXUS is working on from what it has been talking about.
pub fn work_context(conn: &Connection, snapshot: &SessionSnapshot) -> WorkContext {
    let current_project = latest_row_id(snapshot, ReferentKind::Project)
        .and_then(|id| project_context(conn, id));
    let current_task =
        latest_row_id(snapshot, ReferentKind::Task).and_then(|id| task_context(conn, id));

    WorkContext {
        current_project,
        current_task,
    }
}

/// Assemble the full context.
pub fn assemble(
    conn: &Connection,
    session: &AssistantSession,
    pending_approvals: usize,
) -> Result<AssistantContext, String> {
    let snapshot = session.snapshot(pending_approvals);
    let work = work_context(conn, &snapshot);
    let recent_actions = list_recent(conn, RECENT_ACTION_BUDGET)?;

    Ok(AssistantContext {
        session: snapshot,
        work,
        recent_actions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assistant::session::{AssistantState, TurnInput};
    use crate::db::migrations::MIGRATIONS;
    use serde_json::json;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch("PRAGMA foreign_keys = ON;").expect("fk");
        for (_, sql) in MIGRATIONS {
            conn.execute_batch(sql).expect("migrate");
        }
        conn
    }

    fn seed_project(conn: &Connection, name: &str) -> i64 {
        conn.execute("INSERT INTO projects (name) VALUES (?1)", [name])
            .expect("seed");
        conn.last_insert_rowid()
    }

    fn seed_task(conn: &Connection, project: i64, title: &str, status: &str) -> i64 {
        conn.execute(
            "INSERT INTO tasks (project_id, title, status) VALUES (?1, ?2, ?3)",
            rusqlite::params![project, title, status],
        )
        .expect("seed");
        conn.last_insert_rowid()
    }

    fn session_with_project(project: i64) -> AssistantSession {
        let session = AssistantSession::default();
        session.begin_turn(TurnInput::Text {
            text: "open atlas".to_string(),
        });
        session.remember(
            ReferentKind::Project,
            "Atlas",
            "nexus",
            json!({ "id": project }),
        );
        session.advance(AssistantState::Completed, None, None);
        session
    }

    #[test]
    fn the_current_project_is_derived_from_the_conversation() {
        let conn = test_conn();
        let project = seed_project(&conn, "Atlas");
        seed_task(&conn, project, "One", "open");
        seed_task(&conn, project, "Two", "blocked");
        seed_task(&conn, project, "Three", "done");

        let session = session_with_project(project);
        let context = assemble(&conn, &session, 0).expect("assemble");

        let current = context.work.current_project.expect("a current project");
        assert_eq!(current.name, "Atlas");
        assert_eq!(current.open_tasks, 1);
        assert_eq!(current.blocked_tasks, 1);
    }

    #[test]
    fn a_project_deleted_after_being_mentioned_stops_being_current() {
        // The reason work context is derived rather than tracked: a separate
        // tracker would still be pointing at the deleted row.
        let conn = test_conn();
        let project = seed_project(&conn, "Atlas");
        let session = session_with_project(project);

        conn.execute("DELETE FROM projects WHERE id = ?1", [project])
            .expect("delete");

        let context = assemble(&conn, &session, 0).expect("assemble");
        assert!(context.work.current_project.is_none());
    }

    #[test]
    fn the_most_recently_mentioned_project_wins() {
        let conn = test_conn();
        let first = seed_project(&conn, "Atlas");
        let second = seed_project(&conn, "Beta");

        let session = AssistantSession::default();
        session.begin_turn(TurnInput::Text { text: "a".to_string() });
        session.remember(ReferentKind::Project, "Atlas", "nexus", json!({ "id": first }));
        session.advance(AssistantState::Completed, None, None);
        session.begin_turn(TurnInput::Text { text: "b".to_string() });
        session.remember(ReferentKind::Project, "Beta", "nexus", json!({ "id": second }));
        session.advance(AssistantState::Completed, None, None);

        let context = assemble(&conn, &session, 0).expect("assemble");
        assert_eq!(
            context.work.current_project.expect("current").name,
            "Beta"
        );
    }

    #[test]
    fn a_referent_without_a_row_id_is_skipped_not_guessed_at() {
        let conn = test_conn();
        seed_project(&conn, "Atlas");

        let session = AssistantSession::default();
        session.begin_turn(TurnInput::Text { text: "x".to_string() });
        // No `id` in the metadata: nothing to look up.
        session.remember(ReferentKind::Project, "Atlas", "nexus", json!({}));
        session.advance(AssistantState::Completed, None, None);

        let context = assemble(&conn, &session, 0).expect("assemble");
        assert!(context.work.current_project.is_none());
    }

    #[test]
    fn the_current_task_carries_its_status() {
        let conn = test_conn();
        let project = seed_project(&conn, "Atlas");
        let task = seed_task(&conn, project, "Ship it", "blocked");

        let session = AssistantSession::default();
        session.begin_turn(TurnInput::Text { text: "x".to_string() });
        session.remember(ReferentKind::Task, "Ship it", "nexus", json!({ "id": task }));
        session.advance(AssistantState::Completed, None, None);

        let current = assemble(&conn, &session, 0)
            .expect("assemble")
            .work
            .current_task
            .expect("a current task");
        assert_eq!(current.title, "Ship it");
        assert_eq!(current.status, "blocked");
        assert_eq!(current.project_id, project);
    }

    #[test]
    fn an_empty_conversation_yields_empty_work_context() {
        let conn = test_conn();
        let session = AssistantSession::default();
        let context = assemble(&conn, &session, 0).expect("assemble");
        assert!(context.work.current_project.is_none());
        assert!(context.work.current_task.is_none());
        assert!(context.recent_actions.is_empty());
    }

    #[test]
    fn a_project_with_no_tasks_counts_zero_rather_than_failing() {
        let conn = test_conn();
        let project = seed_project(&conn, "Empty");
        let session = session_with_project(project);
        let current = assemble(&conn, &session, 0)
            .expect("assemble")
            .work
            .current_project
            .expect("current");
        assert_eq!((current.open_tasks, current.blocked_tasks), (0, 0));
    }

    #[test]
    fn recent_actions_are_budgeted() {
        // Context is assembled to a budget because it is eventually a prompt.
        let conn = test_conn();
        for i in 0..(RECENT_ACTION_BUDGET + 15) {
            super::super::audit::refusal(
                &conn,
                "nexus.open_settings",
                "nexus",
                crate::assistant::permission::Permission::Interact,
                &format!("Attempt {i}"),
                "not-permitted",
            )
            .expect("write");
        }
        let session = AssistantSession::default();
        let context = assemble(&conn, &session, 0).expect("assemble");
        assert_eq!(context.recent_actions.len() as i64, RECENT_ACTION_BUDGET);
    }

    #[test]
    fn the_snapshot_travels_with_the_context() {
        let conn = test_conn();
        let project = seed_project(&conn, "Atlas");
        let session = session_with_project(project);
        let context = assemble(&conn, &session, 3).expect("assemble");
        assert_eq!(context.session.pending_approvals, 3);
        assert_eq!(context.session.referents.len(), 1);
    }

    #[test]
    fn context_serialises_as_camel_case() {
        let conn = test_conn();
        let session = AssistantSession::default();
        let json = serde_json::to_string(&assemble(&conn, &session, 0).expect("assemble"))
            .expect("serialize");
        assert!(json.contains("\"recentActions\""), "{json}");
        assert!(json.contains("\"currentProject\""), "{json}");
    }
}
