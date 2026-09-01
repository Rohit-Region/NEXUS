//! NEXUS-028: the things you said you would do.
//!
//! **The whole milestone is one rule: only what the user said explicitly.**
//! A system that infers commitments from conversation will be wrong often,
//! and being reminded of something you never agreed to is worse than not
//! being reminded at all. The trigger is a phrase a person deliberately uses,
//! and everything else is ignored, including things NEXUS could reasonably
//! guess were commitments.
//!
//! Deliberately not a task, not scheduling, and not synced anywhere. Tasks
//! already exist in the workspace: they belong to a project, outlive a day,
//! and are something you go and look at. This is the shorter-lived thing you
//! say out loud and forget by lunchtime, and NEXUS brings it back **once**.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// How far past its time a commitment is still worth raising.
///
/// Past this the moment has gone, and "you said you would call the dentist
/// four hours ago" is a reproach rather than a reminder.
pub const STALE_AFTER: i64 = 4 * 60 * 60;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Commitment {
    pub id: i64,
    /// The user's own words. Kept verbatim so NEXUS repeats what they said
    /// rather than its own summary of it, which would be a second chance to
    /// be wrong about something they never has to correct.
    pub what: String,
    /// Unix seconds. `None` means someday: recorded, never raised, visible in
    /// the UI where the user can act on it themselves.
    pub due_at: Option<i64>,
    pub state: String,
    pub raised_at: Option<i64>,
    pub created_at: String,
}

fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Commitment> {
    Ok(Commitment {
        id: row.get(0)?,
        what: row.get(1)?,
        due_at: row.get(2)?,
        state: row.get(3)?,
        raised_at: row.get(4)?,
        created_at: row.get(5)?,
    })
}

const COLUMNS: &str = "id, what, due_at, state, raised_at, created_at";

pub fn create(conn: &Connection, what: &str, due_at: Option<i64>) -> Result<Commitment, String> {
    let text = what.trim();
    if text.is_empty() {
        return Err("There was nothing to remember.".to_string());
    }
    conn.execute(
        "INSERT INTO commitments (what, due_at) VALUES (?1, ?2)",
        rusqlite::params![text, due_at],
    )
    .map_err(|e| format!("Failed to record that: {e}"))?;
    get(conn, conn.last_insert_rowid())
}

pub fn get(conn: &Connection, id: i64) -> Result<Commitment, String> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM commitments WHERE id = ?1"),
        [id],
        row,
    )
    .map_err(|e| format!("Failed to read that back: {e}"))
}

pub fn list(conn: &Connection, open_only: bool) -> Result<Vec<Commitment>, String> {
    let filter = if open_only { "WHERE state = 'open'" } else { "" };
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {COLUMNS} FROM commitments {filter}
              ORDER BY due_at IS NULL, due_at ASC, id DESC"
        ))
        .map_err(|e| format!("Failed to list: {e}"))?;
    let rows = stmt
        .query_map([], row)
        .map_err(|e| format!("Failed to list: {e}"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("Failed to list: {e}"))
}

pub fn set_state(conn: &Connection, id: i64, state: &str) -> Result<(), String> {
    if !matches!(state, "open" | "done" | "dropped") {
        return Err(format!("\"{state}\" is not a state a commitment has."));
    }
    conn.execute(
        "UPDATE commitments SET state = ?2 WHERE id = ?1",
        rusqlite::params![id, state],
    )
    .map_err(|e| format!("Failed to update: {e}"))?;
    Ok(())
}

/// Push a commitment out to a new time and let it be raised again.
///
/// Clearing `raised_at` is the point: deferring is the user saying "not now,
/// but yes", and a commitment that could only ever be raised once would
/// silently swallow that.
pub fn defer(conn: &Connection, id: i64, due_at: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE commitments SET due_at = ?2, raised_at = NULL, state = 'open' WHERE id = ?1",
        rusqlite::params![id, due_at],
    )
    .map_err(|e| format!("Failed to defer: {e}"))?;
    Ok(())
}

pub fn delete(conn: &Connection, id: i64) -> Result<(), String> {
    conn.execute("DELETE FROM commitments WHERE id = ?1", [id])
        .map_err(|e| format!("Failed to delete: {e}"))?;
    Ok(())
}

/// The one commitment worth raising now, if any.
///
/// **Once, and only once.** `raised_at` is set the moment one is handed out,
/// so a commitment the user did not answer is not brought up again. Nagging
/// is exactly how somebody learns to ignore an assistant, and NEXUS-021 spent
/// a milestone arguing that an ignored assistant is worse than a silent one.
///
/// One at a time, oldest first, so a backlog is worked through rather than
/// read out in a heap.
pub fn due_now(conn: &Connection, now: i64) -> Option<Commitment> {
    let found = conn
        .query_row(
            &format!(
                "SELECT {COLUMNS} FROM commitments
                  WHERE state = 'open'
                    AND raised_at IS NULL
                    AND due_at IS NOT NULL
                    AND due_at <= ?1
                    AND due_at > ?2
                  ORDER BY due_at ASC
                  LIMIT 1"
            ),
            rusqlite::params![now, now - STALE_AFTER],
            row,
        )
        .ok()?;

    let _ = conn.execute(
        "UPDATE commitments SET raised_at = ?2 WHERE id = ?1",
        rusqlite::params![found.id, now],
    );
    Some(found)
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

    const NOW: i64 = 1_800_000_000;

    #[test]
    fn a_commitment_is_raised_once_and_then_left_alone() {
        // The rule the milestone rests on. A reminder that keeps coming back
        // is how somebody learns to stop listening.
        let conn = test_conn();
        create(&conn, "call the dentist", Some(NOW - 60)).expect("create");

        let first = due_now(&conn, NOW).expect("must be raised when due");
        assert_eq!(first.what, "call the dentist");
        assert!(
            due_now(&conn, NOW).is_none(),
            "a commitment must not be raised twice"
        );
        assert!(
            due_now(&conn, NOW + 10_000).is_none(),
            "not later either; it was already said"
        );
    }

    #[test]
    fn nothing_is_raised_before_its_time() {
        let conn = test_conn();
        create(&conn, "reply to Priya", Some(NOW + 3_600)).expect("create");
        assert!(due_now(&conn, NOW).is_none());
        assert!(due_now(&conn, NOW + 3_601).is_some(), "and then it is");
    }

    #[test]
    fn a_moment_that_has_passed_is_not_raised_at_all() {
        // "You said you would do that four hours ago" is a reproach, not a
        // reminder, and an assistant that delivers those gets turned off.
        let conn = test_conn();
        create(&conn, "join the standup", Some(NOW - STALE_AFTER - 1)).expect("create");
        assert!(due_now(&conn, NOW).is_none());
    }

    #[test]
    fn someday_means_never_raised_but_never_lost() {
        let conn = test_conn();
        create(&conn, "learn the guitar", None).expect("create");
        assert!(
            due_now(&conn, NOW + 10_000_000).is_none(),
            "no time means NEXUS never brings it up"
        );
        assert_eq!(
            list(&conn, true).expect("list").len(),
            1,
            "but it is still there to be looked at"
        );
    }

    #[test]
    fn deferring_lets_it_come_back() {
        // "Not now" is the user saying yes, later. A commitment that could
        // only be raised once would swallow that answer.
        let conn = test_conn();
        let made = create(&conn, "send the invoice", Some(NOW - 60)).expect("create");
        due_now(&conn, NOW).expect("raised");
        assert!(due_now(&conn, NOW).is_none());

        defer(&conn, made.id, NOW + 3_600).expect("defer");
        assert!(
            due_now(&conn, NOW + 3_601).is_some(),
            "a deferred commitment must be raisable again"
        );
    }

    #[test]
    fn a_finished_commitment_stops_existing_as_far_as_raising_goes() {
        let conn = test_conn();
        let made = create(&conn, "book the flight", Some(NOW + 60)).expect("create");
        set_state(&conn, made.id, "done").expect("done");
        assert!(due_now(&conn, NOW + 61).is_none());

        let dropped = create(&conn, "tidy the garage", Some(NOW + 60)).expect("create");
        set_state(&conn, dropped.id, "dropped").expect("drop");
        assert!(due_now(&conn, NOW + 61).is_none());
    }

    #[test]
    fn the_users_own_words_are_kept() {
        // NEXUS repeats what was said rather than its summary of it. A
        // paraphrase is a second chance to be wrong about something the user
        // never gets to correct.
        let conn = test_conn();
        let made = create(&conn, "  ping Divi about the KAI ticket  ", Some(NOW)).expect("create");
        assert_eq!(made.what, "ping Divi about the KAI ticket");
    }

    #[test]
    fn nothing_is_a_refusal_rather_than_an_empty_reminder() {
        let conn = test_conn();
        for empty in ["", "   ", "\n"] {
            assert!(create(&conn, empty, Some(NOW)).is_err(), "{empty:?}");
        }
    }

    #[test]
    fn an_invented_state_is_refused() {
        let conn = test_conn();
        let made = create(&conn, "x", None).expect("create");
        assert!(set_state(&conn, made.id, "maybe").is_err());
        assert_eq!(get(&conn, made.id).expect("get").state, "open");
    }

    #[test]
    fn the_soonest_is_raised_first() {
        let conn = test_conn();
        create(&conn, "later one", Some(NOW - 10)).expect("create");
        create(&conn, "earlier one", Some(NOW - 600)).expect("create");
        assert_eq!(
            due_now(&conn, NOW).expect("raised").what,
            "earlier one",
            "a backlog is worked through, not read out in a heap"
        );
    }
}
