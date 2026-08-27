//! NEXUS-012: permission levels and standing grants.
//!
//! Two concepts live here that are easy to conflate and must not be:
//!
//! - A **grant** is standing, per connector, per level. It answers "may NEXUS
//!   ever do this kind of thing with this service?"
//! - A **confirmation** is per invocation and expires. It answers "may NEXUS
//!   do this specific thing, right now?" That lives in `approval.rs`.
//!
//! Conflating them is how assistants end up sending messages nobody approved.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// What a class of action can do, ordered by consequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    /// Observe without changing anything.
    Read,
    /// Move things around on screen: open, focus, navigate, search.
    Interact,
    /// Create or modify data somewhere.
    Write,
    /// Run a command, a build, a test, or injected code.
    Execute,
    /// Remove or irreversibly change something.
    Destructive,
}

impl Permission {
    pub fn as_str(self) -> &'static str {
        match self {
            Permission::Read => "read",
            Permission::Interact => "interact",
            Permission::Write => "write",
            Permission::Execute => "execute",
            Permission::Destructive => "destructive",
        }
    }

    pub fn parse(value: &str) -> Option<Permission> {
        match value {
            "read" => Some(Permission::Read),
            "interact" => Some(Permission::Interact),
            "write" => Some(Permission::Write),
            "execute" => Some(Permission::Execute),
            "destructive" => Some(Permission::Destructive),
            _ => None,
        }
    }

    /// Every level, in order of consequence. Used by the Settings UI.
    pub fn all() -> [Permission; 5] {
        [
            Permission::Read,
            Permission::Interact,
            Permission::Write,
            Permission::Execute,
            Permission::Destructive,
        ]
    }

    /// Whether this level always requires per-invocation confirmation.
    ///
    /// This is the rule, not a default an action may override downward: at
    /// Write and above, something outside NEXUS changes, and the user is owed
    /// the chance to stop it. `assert_confirm_policy_matches_permission`
    /// enforces that no registered action disagrees.
    pub fn always_confirms(self) -> bool {
        self >= Permission::Write
    }
}

/// Whether an action's effects stay on this machine.
///
/// Not used to make decisions in NEXUS-012, where everything is local. It is
/// declared now because retrofitting it once network connectors exist would
/// mean revisiting every action spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Reach {
    LocalOnly,
    LeavesMachine,
}

/// When the user must confirm an individual invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfirmPolicy {
    Never,
    Always,
}

// -- Grant storage ------------------------------------------------------------

/// Read every granted level for a connector.
///
/// A missing row is a denial. There is no third state, so a half-written
/// grants table cannot produce an accidental yes.
pub fn granted_levels(conn: &Connection, connector_id: &str) -> Result<Vec<Permission>, String> {
    let mut stmt = conn
        .prepare("SELECT level FROM permission_grants WHERE connector_id = ?1")
        .map_err(|e| format!("Failed to read grants: {e}"))?;

    let rows = stmt
        .query_map([connector_id], |row| row.get::<_, String>(0))
        .map_err(|e| format!("Failed to read grants: {e}"))?;

    let mut out = Vec::new();
    for row in rows {
        let raw = row.map_err(|e| format!("Failed to read grants: {e}"))?;
        // An unrecognised level in the table is ignored rather than fatal:
        // a downgrade must not brick the app, and an unknown level cannot
        // match any action's requirement anyway.
        if let Some(level) = Permission::parse(&raw) {
            out.push(level);
        }
    }
    out.sort();
    Ok(out)
}

pub fn is_granted(
    conn: &Connection,
    connector_id: &str,
    level: Permission,
) -> Result<bool, String> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM permission_grants
              WHERE connector_id = ?1 AND level = ?2",
            rusqlite::params![connector_id, level.as_str()],
            |row| row.get(0),
        )
        .map_err(|e| format!("Failed to read grant: {e}"))?;
    Ok(count > 0)
}

/// Grant or revoke one level. Revoking is a delete, so absence stays denial.
pub fn set_grant(
    conn: &Connection,
    connector_id: &str,
    level: Permission,
    granted: bool,
) -> Result<(), String> {
    let known: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM connectors WHERE connector_id = ?1",
            [connector_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Failed to read connector: {e}"))?;
    if known == 0 {
        return Err(format!("Unknown connector: {connector_id}"));
    }

    if granted {
        conn.execute(
            "INSERT OR IGNORE INTO permission_grants (connector_id, level)
             VALUES (?1, ?2)",
            rusqlite::params![connector_id, level.as_str()],
        )
        .map_err(|e| format!("Failed to grant permission: {e}"))?;
    } else {
        conn.execute(
            "DELETE FROM permission_grants WHERE connector_id = ?1 AND level = ?2",
            rusqlite::params![connector_id, level.as_str()],
        )
        .map_err(|e| format!("Failed to revoke permission: {e}"))?;
    }
    Ok(())
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
    fn levels_are_ordered_by_consequence() {
        assert!(Permission::Read < Permission::Interact);
        assert!(Permission::Interact < Permission::Write);
        assert!(Permission::Write < Permission::Execute);
        assert!(Permission::Execute < Permission::Destructive);
    }

    #[test]
    fn write_and_above_always_confirm() {
        assert!(!Permission::Read.always_confirms());
        assert!(!Permission::Interact.always_confirms());
        assert!(Permission::Write.always_confirms());
        assert!(Permission::Execute.always_confirms());
        assert!(Permission::Destructive.always_confirms());
    }

    #[test]
    fn every_level_round_trips_through_its_string() {
        for level in Permission::all() {
            assert_eq!(Permission::parse(level.as_str()), Some(level));
        }
        assert_eq!(Permission::parse("superuser"), None);
        assert_eq!(Permission::parse(""), None);
    }

    #[test]
    fn the_local_connector_is_seeded_with_its_grants() {
        let conn = test_conn();
        let levels = granted_levels(&conn, "nexus").expect("read");
        assert_eq!(
            levels,
            vec![
                Permission::Read,
                Permission::Interact,
                Permission::Write,
                Permission::Destructive
            ],
            "nexus acts on the user's own workspace, but never runs commands"
        );
        assert!(
            !is_granted(&conn, "nexus", Permission::Execute).expect("read"),
            "a level with no actions must not be granted"
        );
    }

    #[test]
    fn an_unknown_connector_has_no_grants() {
        let conn = test_conn();
        assert!(granted_levels(&conn, "salesforce").expect("read").is_empty());
        assert!(!is_granted(&conn, "salesforce", Permission::Read).expect("read"));
    }

    #[test]
    fn revoking_removes_the_row_rather_than_storing_a_no() {
        let conn = test_conn();
        set_grant(&conn, "nexus", Permission::Destructive, false).expect("revoke");
        assert!(!is_granted(&conn, "nexus", Permission::Destructive).expect("read"));

        let rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM permission_grants
                  WHERE connector_id = 'nexus' AND level = 'destructive'",
                [],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(rows, 0, "a revoked grant must leave no row behind");
    }

    #[test]
    fn granting_is_idempotent() {
        let conn = test_conn();
        set_grant(&conn, "nexus", Permission::Execute, true).expect("grant");
        set_grant(&conn, "nexus", Permission::Execute, true).expect("grant again");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM permission_grants
                  WHERE connector_id = 'nexus' AND level = 'execute'",
                [],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(count, 1);
    }

    #[test]
    fn a_grant_for_an_unknown_connector_is_rejected() {
        let conn = test_conn();
        assert!(set_grant(&conn, "salesforce", Permission::Read, true).is_err());
    }

    #[test]
    fn an_unrecognised_stored_level_is_ignored_not_fatal() {
        let conn = test_conn();
        conn.execute(
            "INSERT INTO permission_grants (connector_id, level) VALUES ('nexus','sudo')",
            [],
        )
        .expect("seed");
        let levels = granted_levels(&conn, "nexus").expect("read");
        assert!(!levels.is_empty(), "a bad row must not blank the grants");
        assert_eq!(levels.len(), 4, "the unknown level is dropped");
    }
}
