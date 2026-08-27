//! NEXUS-021: deciding when to speak up.
//!
//! NEXUS-020 works out what is worth saying. This works out whether now is
//! the time, which is the harder half. An assistant that surfaces everything
//! it notices is one you learn to ignore, and an ignored assistant is worse
//! than a silent one: it has trained you not to look.
//!
//! Four rules, all of them about restraint:
//!
//! - **Cooldown.** A suggestion shown recently is not shown again yet.
//!   Persisted, because a cooldown that resets on restart is not a cooldown.
//! - **Fatigue.** Something raised repeatedly and never acted on gets a
//!   longer and longer cooldown. If you have ignored it five times, NEXUS
//!   was wrong about it.
//! - **A cap.** Never more than a few at once, however much is going on.
//! - **Silence is a valid answer.** Nothing to say means nothing shown, not
//!   a filler item.
//!
//! There are no operating-system notifications here. Surfacing happens inside
//! NEXUS, where the user is already looking; a banner that interrupts other
//! work is a much larger promise than this milestone should make on its own.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::suggestions::{generate, Suggestion};

/// Settings keys, read straight from the key/value table for the same reason
/// the reasoning policy is: subsystem policy, not a user-facing preference
/// the rest of the app reads.
const KEY_ENABLED: &str = "proactive_enabled";
const KEY_COOLDOWN: &str = "proactive_cooldown_minutes";

/// Long enough that a suggestion is not nagging, short enough to be useful
/// twice in a working day.
const DEFAULT_COOLDOWN_MINUTES: i64 = 240;
/// Most that will ever be surfaced at once, whatever the workspace looks like.
pub const MAX_SURFACED: usize = 3;
/// After this many unacted showings, NEXUS was wrong: back off hard.
const FATIGUE_THRESHOLD: i64 = 5;
/// Multiplier applied once fatigued.
const FATIGUE_FACTOR: i64 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProactivePolicy {
    pub enabled: bool,
    pub cooldown_minutes: i64,
}

impl Default for ProactivePolicy {
    fn default() -> Self {
        // On by default, because this surfaces inside NEXUS rather than
        // interrupting anything. The restraint lives in the cooldown and the
        // cap, not in making the feature opt-in and therefore unused.
        ProactivePolicy {
            enabled: true,
            cooldown_minutes: DEFAULT_COOLDOWN_MINUTES,
        }
    }
}

fn read_setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
        row.get::<_, String>(0)
    })
    .ok()
}

pub fn read_policy(conn: &Connection) -> ProactivePolicy {
    let default = ProactivePolicy::default();
    ProactivePolicy {
        enabled: read_setting(conn, KEY_ENABLED)
            .map(|v| v == "true")
            .unwrap_or(default.enabled),
        cooldown_minutes: read_setting(conn, KEY_COOLDOWN)
            .and_then(|v| v.parse::<i64>().ok())
            // A negative or absurd cooldown would defeat the whole mechanism.
            .filter(|minutes| (0..=10_080).contains(minutes))
            .unwrap_or(default.cooldown_minutes),
    }
}

pub fn set_policy(conn: &Connection, policy: ProactivePolicy) -> Result<(), String> {
    for (key, value) in [
        (KEY_ENABLED, if policy.enabled { "true".to_string() } else { "false".to_string() }),
        (KEY_COOLDOWN, policy.cooldown_minutes.clamp(0, 10_080).to_string()),
    ] {
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE
               SET value = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            rusqlite::params![key, value],
        )
        .map_err(|e| format!("Failed to store the proactive policy: {e}"))?;
    }
    Ok(())
}

/// How long this particular suggestion has to wait.
///
/// Grows with how often it has been shown and ignored. Something NEXUS keeps
/// raising that the user keeps passing over is, by that evidence, not worth
/// raising.
fn effective_cooldown(base_minutes: i64, shown_count: i64) -> i64 {
    if shown_count >= FATIGUE_THRESHOLD {
        base_minutes.saturating_mul(FATIGUE_FACTOR)
    } else {
        base_minutes
    }
}

/// Whether enough time has passed, done in SQL so the clock is the database's
/// and there is no timezone arithmetic to get wrong.
fn is_cool(conn: &Connection, key: &str, base_minutes: i64) -> bool {
    let row: Option<(String, i64)> = conn
        .query_row(
            "SELECT last_shown_at, shown_count FROM suggestion_activity
              WHERE suggestion_key = ?1",
            [key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();

    let (_, shown_count) = match row {
        // Never shown: nothing to wait for.
        None => return true,
        Some(found) => found,
    };

    let minutes = effective_cooldown(base_minutes, shown_count);
    conn.query_row(
        "SELECT last_shown_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?2)
           FROM suggestion_activity WHERE suggestion_key = ?1",
        rusqlite::params![key, format!("-{minutes} minutes")],
        |r| r.get::<_, i64>(0),
    )
    .map(|cool| cool != 0)
    .unwrap_or(true)
}

fn record_shown(conn: &Connection, key: &str) {
    let _ = conn.execute(
        "INSERT INTO suggestion_activity (suggestion_key) VALUES (?1)
         ON CONFLICT(suggestion_key) DO UPDATE
           SET last_shown_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
               shown_count   = shown_count + 1",
        [key],
    );
}

/// The user engaged with a suggestion, so its fatigue count resets.
///
/// Acting on something is evidence NEXUS was right to raise it, which is the
/// opposite of the evidence the fatigue counter accumulates.
pub fn record_accepted(conn: &Connection, key: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE suggestion_activity SET shown_count = 0 WHERE suggestion_key = ?1",
        [key],
    )
    .map_err(|e| format!("Failed to record acceptance: {e}"))?;
    Ok(())
}

/// What to show the user right now.
///
/// Calling this **records** that the suggestions were shown, so it is not a
/// read-only query. That is deliberate: the cooldown only means anything if
/// showing something is what starts it.
pub fn surface(conn: &Connection) -> Vec<Suggestion> {
    let policy = read_policy(conn);
    if !policy.enabled {
        return Vec::new();
    }

    let ready: Vec<Suggestion> = generate(conn)
        .into_iter()
        .filter(|s| is_cool(conn, &s.key, policy.cooldown_minutes))
        .take(MAX_SURFACED)
        .collect();

    for suggestion in &ready {
        record_shown(conn, &suggestion.key);
    }
    ready
}

/// What NEXUS would show, without starting anyone's cooldown.
///
/// Separated because a UI that polls must not be able to burn the cooldown by
/// looking, and because tests need to observe without changing.
pub fn preview(conn: &Connection) -> Vec<Suggestion> {
    let policy = read_policy(conn);
    if !policy.enabled {
        return Vec::new();
    }
    generate(conn)
        .into_iter()
        .filter(|s| is_cool(conn, &s.key, policy.cooldown_minutes))
        .take(MAX_SURFACED)
        .collect()
}

/// A short summary of where things stand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Briefing {
    pub headline: String,
    pub lines: Vec<String>,
    pub suggestions: Vec<Suggestion>,
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0)
}

/// The briefing, assembled from local data only.
///
/// Deliberately factual. "You have three blocked tasks" is something NEXUS
/// knows; "you should focus on X today" is not, and inventing it would be the
/// kind of confident guess this whole architecture avoids.
pub fn briefing(conn: &Connection) -> Briefing {
    let projects = count(conn, "SELECT COUNT(*) FROM projects");
    let open = count(conn, "SELECT COUNT(*) FROM tasks WHERE status = 'open'");
    let blocked = count(conn, "SELECT COUNT(*) FROM tasks WHERE status = 'blocked'");
    let done_recently = count(
        conn,
        "SELECT COUNT(*) FROM tasks
          WHERE status = 'done'
            AND updated_at > strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-7 days')",
    );

    let headline = if projects == 0 {
        "Nothing set up yet.".to_string()
    } else if blocked > 0 {
        format!(
            "{blocked} blocked, {open} open across {projects} project{}.",
            if projects == 1 { "" } else { "s" }
        )
    } else if open > 0 {
        format!("{open} open, nothing blocked.")
    } else {
        "Nothing open.".to_string()
    };

    let mut lines = Vec::new();
    if done_recently > 0 {
        lines.push(format!("{done_recently} finished in the last week."));
    }
    if projects > 0 && open == 0 && blocked == 0 {
        lines.push("Every task is closed.".to_string());
    }

    Briefing {
        headline,
        lines,
        // Preview, not surface: reading a briefing must not silence the
        // suggestions inside it.
        suggestions: preview(conn),
    }
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

    fn blocked_workspace(conn: &Connection) {
        conn.execute("INSERT INTO projects (name) VALUES ('Atlas')", [])
            .expect("seed");
        let p = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO tasks (project_id, title, status) VALUES (?1,'Wire the gate','blocked')",
            [p],
        )
        .expect("seed");
    }

    /// Move a suggestion's last-shown stamp into the past.
    fn age(conn: &Connection, key: &str, minutes: i64) {
        conn.execute(
            "UPDATE suggestion_activity
                SET last_shown_at = strftime('%Y-%m-%dT%H:%M:%fZ','now',?2)
              WHERE suggestion_key = ?1",
            rusqlite::params![key, format!("-{minutes} minutes")],
        )
        .expect("age");
    }

    #[test]
    fn something_shown_is_not_shown_again_immediately() {
        let conn = test_conn();
        blocked_workspace(&conn);

        let first = surface(&conn);
        assert!(first.iter().any(|s| s.key == "blocked-tasks"));

        let second = surface(&conn);
        assert!(
            second.iter().all(|s| s.key != "blocked-tasks"),
            "the cooldown must actually hold"
        );
    }

    #[test]
    fn it_returns_once_the_cooldown_has_passed() {
        let conn = test_conn();
        blocked_workspace(&conn);
        surface(&conn);
        age(&conn, "blocked-tasks", DEFAULT_COOLDOWN_MINUTES + 1);
        assert!(surface(&conn).iter().any(|s| s.key == "blocked-tasks"));
    }

    #[test]
    fn something_ignored_repeatedly_backs_off_much_further() {
        // If you have passed over it five times, NEXUS was wrong about it.
        let conn = test_conn();
        blocked_workspace(&conn);
        for _ in 0..FATIGUE_THRESHOLD {
            surface(&conn);
            age(&conn, "blocked-tasks", DEFAULT_COOLDOWN_MINUTES + 1);
        }

        // A wait that used to be enough no longer is.
        age(&conn, "blocked-tasks", DEFAULT_COOLDOWN_MINUTES + 1);
        assert!(
            surface(&conn).iter().all(|s| s.key != "blocked-tasks"),
            "a fatigued suggestion must wait much longer"
        );

        age(
            &conn,
            "blocked-tasks",
            DEFAULT_COOLDOWN_MINUTES * FATIGUE_FACTOR + 1,
        );
        assert!(surface(&conn).iter().any(|s| s.key == "blocked-tasks"));
    }

    #[test]
    fn acting_on_something_resets_its_fatigue() {
        let conn = test_conn();
        blocked_workspace(&conn);
        for _ in 0..FATIGUE_THRESHOLD {
            surface(&conn);
            age(&conn, "blocked-tasks", DEFAULT_COOLDOWN_MINUTES + 1);
        }
        record_accepted(&conn, "blocked-tasks").expect("accepted");

        age(&conn, "blocked-tasks", DEFAULT_COOLDOWN_MINUTES + 1);
        assert!(
            surface(&conn).iter().any(|s| s.key == "blocked-tasks"),
            "acting on it is evidence NEXUS was right"
        );
    }

    #[test]
    fn previewing_does_not_start_a_cooldown() {
        // A polling UI must not be able to silence NEXUS by looking.
        let conn = test_conn();
        blocked_workspace(&conn);
        for _ in 0..5 {
            assert!(preview(&conn).iter().any(|s| s.key == "blocked-tasks"));
        }
        assert!(surface(&conn).iter().any(|s| s.key == "blocked-tasks"));
    }

    #[test]
    fn turning_it_off_means_silence() {
        let conn = test_conn();
        blocked_workspace(&conn);
        set_policy(
            &conn,
            ProactivePolicy {
                enabled: false,
                cooldown_minutes: DEFAULT_COOLDOWN_MINUTES,
            },
        )
        .expect("store");
        assert!(surface(&conn).is_empty());
        assert!(preview(&conn).is_empty());
        assert!(briefing(&conn).suggestions.is_empty());
    }

    #[test]
    fn the_policy_round_trips_and_absurd_values_are_clamped() {
        let conn = test_conn();
        set_policy(
            &conn,
            ProactivePolicy {
                enabled: true,
                cooldown_minutes: -50,
            },
        )
        .expect("store");
        assert_eq!(read_policy(&conn).cooldown_minutes, 0);

        set_policy(
            &conn,
            ProactivePolicy {
                enabled: true,
                cooldown_minutes: 999_999,
            },
        )
        .expect("store");
        assert_eq!(read_policy(&conn).cooldown_minutes, 10_080);
    }

    #[test]
    fn a_corrupt_stored_cooldown_falls_back_to_the_default() {
        let conn = test_conn();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('proactive_cooldown_minutes','soon')",
            [],
        )
        .expect("seed");
        assert_eq!(read_policy(&conn).cooldown_minutes, DEFAULT_COOLDOWN_MINUTES);
    }

    #[test]
    fn never_more_than_a_few_at_once() {
        let conn = test_conn();
        for i in 0..10 {
            conn.execute(
                "INSERT INTO projects (name, repository_url) VALUES (?1,'https://github.com/a/b')",
                [format!("P{i}")],
            )
            .expect("seed");
            let p = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO tasks (project_id, title, status) VALUES (?1,'x','blocked')",
                [p],
            )
            .expect("seed");
        }
        assert!(surface(&conn).len() <= MAX_SURFACED);
    }

    #[test]
    fn silence_is_a_valid_answer() {
        // Nothing to say means nothing shown, not a filler item.
        let conn = test_conn();
        assert!(surface(&conn).is_empty());
        let brief = briefing(&conn);
        assert!(brief.suggestions.is_empty());
        assert_eq!(brief.headline, "Nothing set up yet.");
    }

    #[test]
    fn the_briefing_states_facts_rather_than_advice() {
        let conn = test_conn();
        blocked_workspace(&conn);
        let brief = briefing(&conn);
        assert!(brief.headline.contains('1'), "{}", brief.headline);
        // "You should focus on X" is not something NEXUS knows.
        for invented in ["should", "recommend", "focus on", "probably"] {
            assert!(
                !brief.headline.to_lowercase().contains(invented),
                "the briefing must not advise: {}",
                brief.headline
            );
        }
    }

    #[test]
    fn reading_the_briefing_does_not_silence_its_own_suggestions() {
        let conn = test_conn();
        blocked_workspace(&conn);
        assert!(!briefing(&conn).suggestions.is_empty());
        assert!(
            !briefing(&conn).suggestions.is_empty(),
            "a briefing must be re-readable"
        );
    }

    #[test]
    fn no_operating_system_notification_is_raised() {
        // Interrupting other work is a much bigger promise than surfacing
        // inside NEXUS, and it is not one this milestone makes.
        let production = include_str!("proactive.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("marker");
        for forbidden in ["NSUserNotification", "UNUserNotification", "osascript", "notify"] {
            assert!(!production.contains(forbidden), "found {forbidden}");
        }
    }
}
