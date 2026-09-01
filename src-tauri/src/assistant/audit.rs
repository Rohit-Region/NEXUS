//! NEXUS-012: the audit trail.
//!
//! An audit row answers "what did NEXUS do on my behalf", not "what did I
//! read". It stores the rendered summary the user already saw, never the
//! action's raw input and never anything NEXUS merely observed. Inbound
//! content has no route into this table.
//!
//! Rows are written before dispatch and updated after, so an action that
//! panics or hangs still leaves a trace. A refusal is written too: "NEXUS was
//! asked to do this and would not" is exactly what an audit trail is for.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::permission::Permission;

/// How an attempt ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    /// Written before dispatch. A row left in this state means NEXUS never
    /// came back, which is itself worth seeing.
    Attempted,
    Succeeded,
    Failed,
    /// Stopped at the gate: not permitted, unknown, or a bad approval.
    Refused,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Attempted => "attempted",
            Outcome::Succeeded => "succeeded",
            Outcome::Failed => "failed",
            Outcome::Refused => "refused",
        }
    }
}

/// One row of the trail, as the Activity view reads it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    pub id: i64,
    pub action_id: String,
    pub connector_id: String,
    pub permission: String,
    pub summary: String,
    pub outcome: String,
    pub error: Option<String>,
    pub duration_ms: Option<i64>,
    pub approved: bool,
    pub created_at: String,
}

/// Open a row before dispatch and return its id.
pub fn begin(
    conn: &Connection,
    action_id: &str,
    connector_id: &str,
    permission: Permission,
    summary: &str,
    approved: bool,
) -> Result<i64, String> {
    conn.execute(
        "INSERT INTO action_audit
             (action_id, connector_id, permission, summary, outcome, approved)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            action_id,
            connector_id,
            permission.as_str(),
            summary,
            Outcome::Attempted.as_str(),
            approved as i64,
        ],
    )
    .map_err(|e| format!("Failed to write audit row: {e}"))?;
    Ok(conn.last_insert_rowid())
}

/// Close a row opened by `begin`.
pub fn finish(
    conn: &Connection,
    id: i64,
    outcome: Outcome,
    error: Option<&str>,
    duration_ms: u128,
) -> Result<(), String> {
    conn.execute(
        "UPDATE action_audit
            SET outcome = ?2, error = ?3, duration_ms = ?4
          WHERE id = ?1",
        rusqlite::params![id, outcome.as_str(), error, duration_ms as i64],
    )
    .map_err(|e| format!("Failed to close audit row: {e}"))?;
    Ok(())
}

/// Write a row for something that never reached a connector.
pub fn refusal(
    conn: &Connection,
    action_id: &str,
    connector_id: &str,
    permission: Permission,
    summary: &str,
    reason: &str,
) -> Result<i64, String> {
    conn.execute(
        "INSERT INTO action_audit
             (action_id, connector_id, permission, summary, outcome, error, duration_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
        rusqlite::params![
            action_id,
            connector_id,
            permission.as_str(),
            summary,
            Outcome::Refused.as_str(),
            reason,
        ],
    )
    .map_err(|e| format!("Failed to write audit row: {e}"))?;
    Ok(conn.last_insert_rowid())
}

/// Cap on one page of history. Matches SEARCH_RESULT_CAP's reasoning: a UI
/// list is read, not scrolled forever.
pub const AUDIT_PAGE_CAP: i64 = 100;

/// Most recent first.
pub fn list_recent(conn: &Connection, limit: i64) -> Result<Vec<AuditEntry>, String> {
    let capped = limit.clamp(1, AUDIT_PAGE_CAP);
    let mut stmt = conn
        .prepare(
            "SELECT id, action_id, connector_id, permission, summary,
                    outcome, error, duration_ms, approved, created_at
               FROM action_audit
              ORDER BY created_at DESC, id DESC
              LIMIT ?1",
        )
        .map_err(|e| format!("Failed to read audit trail: {e}"))?;

    let rows = stmt
        .query_map([capped], |row| {
            Ok(AuditEntry {
                id: row.get(0)?,
                action_id: row.get(1)?,
                connector_id: row.get(2)?,
                permission: row.get(3)?,
                summary: row.get(4)?,
                outcome: row.get(5)?,
                error: row.get(6)?,
                duration_ms: row.get(7)?,
                approved: row.get::<_, i64>(8)? != 0,
                created_at: row.get(9)?,
            })
        })
        .map_err(|e| format!("Failed to read audit trail: {e}"))?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("Failed to read audit trail: {e}"))?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrations::MIGRATIONS;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch("PRAGMA foreign_keys = ON;").expect("fk");
        for (_, sql) in MIGRATIONS {
            conn.execute_batch(sql).expect("migrate");
        }
        conn
    }

    #[test]
    fn a_row_is_open_before_dispatch_and_closed_after() {
        let conn = test_conn();
        let id = begin(
            &conn,
            "nexus.delete_task",
            "nexus",
            Permission::Destructive,
            "Delete task Ship it",
            true,
        )
        .expect("begin");

        let open = list_recent(&conn, 10).expect("list");
        assert_eq!(open[0].outcome, "attempted", "must be open before dispatch");

        finish(&conn, id, Outcome::Succeeded, None, 12).expect("finish");
        let closed = list_recent(&conn, 10).expect("list");
        assert_eq!(closed[0].outcome, "succeeded");
        assert_eq!(closed[0].duration_ms, Some(12));
        assert!(closed[0].approved);
    }

    #[test]
    fn a_refusal_is_recorded() {
        // "NEXUS was asked to do this and would not" is exactly what an audit
        // trail is for.
        let conn = test_conn();
        refusal(
            &conn,
            "nexus.delete_project",
            "nexus",
            Permission::Destructive,
            "Delete project Atlas",
            "not-permitted",
        )
        .expect("refusal");

        let rows = list_recent(&conn, 10).expect("list");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].outcome, "refused");
        assert_eq!(rows[0].error.as_deref(), Some("not-permitted"));
        assert!(!rows[0].approved);
    }

    #[test]
    fn history_is_newest_first() {
        let conn = test_conn();
        for i in 0..5 {
            refusal(
                &conn,
                "nexus.open_settings",
                "nexus",
                Permission::Interact,
                &format!("Attempt {i}"),
                "not-permitted",
            )
            .expect("write");
        }
        let rows = list_recent(&conn, 10).expect("list");
        assert_eq!(rows[0].summary, "Attempt 4");
        assert_eq!(rows[4].summary, "Attempt 0");
    }

    #[test]
    fn the_page_size_is_capped_and_never_zero() {
        let conn = test_conn();
        for i in 0..(AUDIT_PAGE_CAP + 20) {
            refusal(
                &conn,
                "nexus.open_settings",
                "nexus",
                Permission::Interact,
                &format!("Row {i}"),
                "x",
            )
            .expect("write");
        }
        assert_eq!(
            list_recent(&conn, 10_000).expect("list").len() as i64,
            AUDIT_PAGE_CAP
        );
        assert_eq!(list_recent(&conn, 0).expect("list").len(), 1);
        assert_eq!(list_recent(&conn, -5).expect("list").len(), 1);
    }

    #[test]
    fn an_unfinished_row_stays_visible_as_attempted() {
        // If NEXUS dies mid-action, the trail must show that it started.
        let conn = test_conn();
        begin(
            &conn,
            "nexus.create_task",
            "nexus",
            Permission::Write,
            "Create task Ship it",
            true,
        )
        .expect("begin");
        let rows = list_recent(&conn, 1).expect("list");
        assert_eq!(rows[0].outcome, "attempted");
        assert_eq!(rows[0].duration_ms, None);
    }

    #[test]
    fn the_table_has_no_column_for_raw_input() {
        // Guards the privacy rule structurally: there is nowhere to put
        // observed content even if a future caller tried.
        let conn = test_conn();
        let mut stmt = conn
            .prepare("PRAGMA table_info(action_audit)")
            .expect("pragma");
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .expect("query")
            .map(|r| r.expect("row"))
            .collect();
        for forbidden in ["input", "payload", "body", "content", "transcript"] {
            assert!(
                !columns.iter().any(|c| c == forbidden),
                "audit must not store {forbidden}"
            );
        }
        assert!(columns.iter().any(|c| c == "summary"));
    }
}
