//! Typed application preferences over the key/value `settings` table.
//!
//! The table is (key TEXT, value TEXT). A single typed `Settings` struct
//! crosses the IPC boundary instead, so the frontend never sees a key, a raw
//! value, or a parse decision (spec 008 2.3).
//!
//! Two asymmetric rules govern this module:
//!
//! - **Read tolerates everything** (2.4). A missing, malformed, or
//!   unrecognised value yields that field's compile-time default. Nothing is
//!   an error except a genuine database failure. A read must never leave the
//!   user unable to launch the application.
//! - **Write validates strictly** (2.4). Invalid input is rejected with a
//!   typed error naming the accepted set, because a write is the
//!   application's own output and has no excuse for being invalid.
//!
//! Two invariants carry the milestone:
//!
//! - **Never destroy a key you do not own** (2.5). Saves and resets touch
//!   only the keys in KNOWN_KEYS, so an older build cannot silently delete a
//!   newer build's settings. Guarded by `unknown_key_in_table_is_preserved`.
//! - **A dangling id resolves on read and is never written back** (2.6).
//!   `settings` has no foreign key, so a stored registry id can outlive its
//!   row. Reads resolve it to `None`; reads never write.

use std::collections::HashMap;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::tasks::TASK_STATUSES;
// The shipped voice lives with the synthesizer that resolves it, so there is
// one definition rather than a constant here that can drift out of step.
use crate::voice::speech::DEFAULT_VOICE;

// -- Keys and accepted values -------------------------------------------------

const KEY_LAUNCH_SCREEN: &str = "launch_screen";
const KEY_PROJECT_SORT: &str = "project_sort";
const KEY_TASK_SORT: &str = "task_sort";
const KEY_REGISTRY_SORT: &str = "registry_sort";
const KEY_TASK_STATUS_FILTER: &str = "task_status_filter";
const KEY_NEW_PROJECT_DEFAULT_IDE_ID: &str = "new_project_default_ide_id";
const KEY_NEW_PROJECT_DEFAULT_AGENT_ID: &str = "new_project_default_agent_id";
const KEY_VOICE_ENABLED: &str = "voice_enabled";
const KEY_VOICE_NAME: &str = "voice_name";

/// Every key this version owns. Reset and save touch these and nothing else.
const KNOWN_KEYS: [&str; 9] = [
    KEY_LAUNCH_SCREEN,
    KEY_PROJECT_SORT,
    KEY_TASK_SORT,
    KEY_REGISTRY_SORT,
    KEY_TASK_STATUS_FILTER,
    KEY_NEW_PROJECT_DEFAULT_IDE_ID,
    KEY_NEW_PROJECT_DEFAULT_AGENT_ID,
    KEY_VOICE_ENABLED,
    KEY_VOICE_NAME,
];

/// Longest accepted voice name or identifier. System identifiers are well
/// under this; the cap exists so a hand-edited database cannot store an
/// unbounded string that later reaches the synthesizer.
const MAX_VOICE_NAME: usize = 64;

pub const LAUNCH_SCREENS: [&str; 2] = ["overview", "projects"];

pub const PROJECT_SORTS: [&str; 5] = [
    "created-desc",
    "created-asc",
    "updated-desc",
    "name-asc",
    "name-desc",
];
pub const TASK_SORTS: [&str; 5] = [
    "created-desc",
    "created-asc",
    "updated-desc",
    "title-asc",
    "status",
];
pub const REGISTRY_SORTS: [&str; 4] =
    ["created-desc", "created-asc", "name-asc", "type-asc"];

const DEFAULT_LAUNCH_SCREEN: &str = "overview";
const DEFAULT_SORT: &str = "created-desc";

// -- Model --------------------------------------------------------------------

/// All application preferences, fully populated.
///
/// Every field is guaranteed present: `get_settings` substitutes the
/// compile-time default for any key that is missing, unparseable, or
/// unrecognised (spec 008 2.4). The two registry ids are resolved against the
/// registry on read, so a deleted entry yields None (spec 008 2.6).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub launch_screen: String,
    pub project_sort: String,
    pub task_sort: String,
    pub registry_sort: String,
    pub task_status_filter: Vec<String>,
    pub new_project_default_ide_id: Option<i64>,
    pub new_project_default_agent_id: Option<i64>,
    /// NEXUS-010. Off by default; the microphone never starts while false.
    pub voice_enabled: bool,
    /// NEXUS-011. A voice name or system identifier for spoken responses.
    /// Empty means the system default voice. The stored value is a
    /// preference, not a guarantee: which voices exist differs per machine,
    /// so the synthesizer resolves it with a fallback chain at speak time.
    pub voice_name: String,
}

impl Settings {
    /// Must stay identical to DEFAULT_SETTINGS in src/types/index.ts, or the
    /// application behaves differently when settings fail to load than when
    /// they load empty.
    pub fn defaults() -> Self {
        Settings {
            launch_screen: DEFAULT_LAUNCH_SCREEN.to_string(),
            project_sort: DEFAULT_SORT.to_string(),
            task_sort: DEFAULT_SORT.to_string(),
            registry_sort: DEFAULT_SORT.to_string(),
            task_status_filter: Vec::new(),
            new_project_default_ide_id: None,
            new_project_default_agent_id: None,
            voice_enabled: false,
            voice_name: DEFAULT_VOICE.to_string(),
        }
    }
}

// -- Read-side parsing: none of these can fail --------------------------------

/// An unrecognised value yields the default rather than an error.
fn parse_enum(value: Option<&String>, accepted: &[&str], default: &str) -> String {
    match value {
        Some(v) if accepted.contains(&v.as_str()) => v.clone(),
        _ => default.to_string(),
    }
}

/// Comma-separated status tokens. Unknown tokens are dropped, duplicates
/// collapsed, and an empty result means no filtering rather than no results.
fn parse_status_filter(value: Option<&String>) -> Vec<String> {
    let raw = match value {
        Some(v) => v,
        None => return Vec::new(),
    };
    let mut out: Vec<String> = Vec::new();
    for token in raw.split(',') {
        let t = token.trim();
        if t.is_empty() || !TASK_STATUSES.contains(&t) {
            continue;
        }
        if !out.iter().any(|existing| existing == t) {
            out.push(t.to_string());
        }
    }
    out
}

/// Anything that is not exactly "true" reads as false, so a malformed value
/// can never silently enable the microphone.
fn parse_bool(value: Option<&String>) -> bool {
    matches!(value.map(|v| v.trim()), Some("true"))
}

/// An empty or non-integer value yields None.
fn parse_id(value: Option<&String>) -> Option<i64> {
    value.and_then(|v| v.trim().parse::<i64>().ok())
}

fn encode_id(value: Option<i64>) -> String {
    value.map(|v| v.to_string()).unwrap_or_default()
}

/// Whether a row with this id exists. Used to resolve a possibly dangling id.
fn row_exists(conn: &Connection, table: &str, id: i64) -> Result<bool, String> {
    conn.query_row(
        &format!("SELECT 1 FROM {table} WHERE id = ?1"),
        rusqlite::params![id],
        |_| Ok(()),
    )
    .map(|_| true)
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(false),
        other => Err(format!("Failed to check {table} {id}: {other}")),
    })
}

// -- Read ---------------------------------------------------------------------

/// Read every known key, applying defaults and resolving registry ids.
///
/// Contains no INSERT, UPDATE, or DELETE: a read has no write side effect
/// (spec 008 N-13). A stale id row is left in place; it is harmless because
/// it never resolves, and the next save writes what the UI actually showed.
pub fn get_settings(conn: &Connection) -> Result<Settings, String> {
    let mut stmt = conn
        .prepare("SELECT key, value FROM settings")
        .map_err(|e| format!("Failed to read settings: {e}"))?;

    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| format!("Failed to read settings: {e}"))?;

    let mut map: HashMap<String, String> = HashMap::new();
    for row in rows {
        let (k, v) = row.map_err(|e| format!("Failed to read settings: {e}"))?;
        map.insert(k, v);
    }

    // Keys the map holds that no field below claims are ignored here and
    // never removed (spec 008 2.5).
    let resolve = |key: &str, table: &str| -> Result<Option<i64>, String> {
        match parse_id(map.get(key)) {
            Some(id) if row_exists(conn, table, id)? => Ok(Some(id)),
            _ => Ok(None),
        }
    };

    // Built from defaults() so there is exactly one source of default values.
    let d = Settings::defaults();

    Ok(Settings {
        launch_screen: parse_enum(
            map.get(KEY_LAUNCH_SCREEN),
            &LAUNCH_SCREENS,
            &d.launch_screen,
        ),
        project_sort: parse_enum(map.get(KEY_PROJECT_SORT), &PROJECT_SORTS, &d.project_sort),
        task_sort: parse_enum(map.get(KEY_TASK_SORT), &TASK_SORTS, &d.task_sort),
        registry_sort: parse_enum(
            map.get(KEY_REGISTRY_SORT),
            &REGISTRY_SORTS,
            &d.registry_sort,
        ),
        task_status_filter: parse_status_filter(map.get(KEY_TASK_STATUS_FILTER)),
        new_project_default_ide_id: resolve(KEY_NEW_PROJECT_DEFAULT_IDE_ID, "ides")?,
        new_project_default_agent_id: resolve(
            KEY_NEW_PROJECT_DEFAULT_AGENT_ID,
            "ai_agents",
        )?,
        voice_enabled: parse_bool(map.get(KEY_VOICE_ENABLED)),
        voice_name: parse_voice_name(map.get(KEY_VOICE_NAME), &d.voice_name),
    })
}

/// Tolerant read of the voice preference.
///
/// A missing key means the user has never chosen, so the shipped default
/// applies. An explicitly stored empty string is a real choice: the system
/// default voice. Anything malformed degrades to the shipped default rather
/// than erroring, matching every other reader here.
fn parse_voice_name(value: Option<&String>, default: &str) -> String {
    match value {
        None => default.to_string(),
        Some(v) => {
            let trimmed = v.trim();
            if trimmed.chars().count() > MAX_VOICE_NAME
                || trimmed.chars().any(|c| c.is_control())
            {
                default.to_string()
            } else {
                trimmed.to_string()
            }
        }
    }
}

// -- Write --------------------------------------------------------------------

fn validate_enum(value: &str, accepted: &[&str], label: &str) -> Result<(), String> {
    if accepted.contains(&value) {
        Ok(())
    } else {
        Err(format!(
            "Invalid {label}: {value}. Expected one of: {}",
            accepted.join(", ")
        ))
    }
}

/// Strict validation, run in full before any row is written.
fn validate(conn: &Connection, s: &Settings) -> Result<(), String> {
    validate_enum(&s.launch_screen, &LAUNCH_SCREENS, "launch screen")?;
    validate_enum(&s.project_sort, &PROJECT_SORTS, "project sort")?;
    validate_enum(&s.task_sort, &TASK_SORTS, "task sort")?;
    validate_enum(&s.registry_sort, &REGISTRY_SORTS, "registry sort")?;

    for token in &s.task_status_filter {
        if !TASK_STATUSES.contains(&token.as_str()) {
            return Err(format!(
                "Invalid task status: {token}. Expected one of: {}",
                TASK_STATUSES.join(", ")
            ));
        }
    }

    // Mirrors the foreign-key rejection update_project already produces for a
    // dangling default_ide_id, keeping behaviour consistent across the code.
    if let Some(id) = s.new_project_default_ide_id {
        if !row_exists(conn, "ides", id)? {
            return Err(format!("IDE {id} not found"));
        }
    }
    if let Some(id) = s.new_project_default_agent_id {
        if !row_exists(conn, "ai_agents", id)? {
            return Err(format!("Agent {id} not found"));
        }
    }

    // Deliberately not validated against the installed voice list. That list
    // is machine-specific and can change after the preference is saved, so
    // treating it as an enum would reject a legitimate choice on one Mac and
    // break an existing setting on another. Shape is checked; availability is
    // resolved at speak time (NEXUS-011).
    if s.voice_name.chars().count() > MAX_VOICE_NAME {
        return Err(format!(
            "Voice name too long: {} characters. Maximum is {MAX_VOICE_NAME}.",
            s.voice_name.chars().count()
        ));
    }
    if s.voice_name.chars().any(|c| c.is_control()) {
        return Err("Voice name must not contain control characters".to_string());
    }

    Ok(())
}

/// Upsert every known key in one transaction, then re-read.
///
/// Transactional so a failure leaves the previous settings wholly intact
/// rather than half-written. Returns the re-read result, so the caller
/// receives what was persisted rather than what was submitted.
pub fn update_settings(conn: &mut Connection, input: &Settings) -> Result<Settings, String> {
    validate(conn, input)?;

    // Duplicates were collapsed on read; collapse here too so a caller that
    // hand-builds the struct cannot store the same token twice.
    let mut statuses: Vec<&str> = Vec::new();
    for token in &input.task_status_filter {
        if !statuses.contains(&token.as_str()) {
            statuses.push(token.as_str());
        }
    }

    let pairs: [(&str, String); 9] = [
        (KEY_LAUNCH_SCREEN, input.launch_screen.clone()),
        (KEY_PROJECT_SORT, input.project_sort.clone()),
        (KEY_TASK_SORT, input.task_sort.clone()),
        (KEY_REGISTRY_SORT, input.registry_sort.clone()),
        (KEY_TASK_STATUS_FILTER, statuses.join(",")),
        (
            KEY_NEW_PROJECT_DEFAULT_IDE_ID,
            encode_id(input.new_project_default_ide_id),
        ),
        (
            KEY_NEW_PROJECT_DEFAULT_AGENT_ID,
            encode_id(input.new_project_default_agent_id),
        ),
        (
            KEY_VOICE_ENABLED,
            if input.voice_enabled { "true" } else { "false" }.to_string(),
        ),
        (KEY_VOICE_NAME, input.voice_name.trim().to_string()),
    ];

    let tx = conn
        .transaction()
        .map_err(|e| format!("Failed to write settings: {e}"))?;

    for (key, value) in &pairs {
        tx.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE
               SET value      = ?2,
                   updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            rusqlite::params![key, value],
        )
        .map_err(|e| format!("Failed to write settings: {e}"))?;
    }

    tx.commit()
        .map_err(|e| format!("Failed to write settings: {e}"))?;

    get_settings(conn)
}

/// Delete only the keys in KNOWN_KEYS, then re-read.
///
/// Deleting rather than writing defaults keeps the table's meaning clean:
/// absent means default. Unknown keys survive (spec 008 2.5). There is
/// deliberately no unscoped DELETE FROM settings anywhere in this codebase.
pub fn reset_settings(conn: &mut Connection) -> Result<Settings, String> {
    let tx = conn
        .transaction()
        .map_err(|e| format!("Failed to reset settings: {e}"))?;

    for key in KNOWN_KEYS {
        tx.execute(
            "DELETE FROM settings WHERE key = ?1",
            rusqlite::params![key],
        )
        .map_err(|e| format!("Failed to reset settings: {e}"))?;
    }

    tx.commit()
        .map_err(|e| format!("Failed to reset settings: {e}"))?;

    get_settings(conn)
}

// -- Tests --------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrations::MIGRATIONS;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .expect("enable foreign keys");
        for &(_, sql) in MIGRATIONS {
            conn.execute_batch(sql).expect("apply migration");
        }
        conn
    }

    fn seed_ide(conn: &Connection) -> i64 {
        conn.execute(
            "INSERT INTO ides (name, ide_type) VALUES ('Editor', 'editor')",
            [],
        )
        .expect("insert ide");
        conn.last_insert_rowid()
    }

    fn seed_agent(conn: &Connection) -> i64 {
        conn.execute(
            "INSERT INTO ai_agents (name, agent_type) VALUES ('Helper', 'assistant')",
            [],
        )
        .expect("insert agent");
        conn.last_insert_rowid()
    }

    fn put(conn: &Connection, key: &str, value: &str) {
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = ?2",
            rusqlite::params![key, value],
        )
        .expect("put setting");
    }

    fn row_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM settings", [], |r| r.get(0))
            .expect("count settings")
    }

    // -- Defaults and round-trip --------------------------------------------

    #[test]
    fn get_returns_defaults_on_empty_table() {
        let conn = test_conn();
        let s = get_settings(&conn).expect("get");

        assert_eq!(s.launch_screen, "overview");
        assert_eq!(s.project_sort, "created-desc");
        assert_eq!(s.task_sort, "created-desc");
        assert_eq!(s.registry_sort, "created-desc");
        assert!(s.task_status_filter.is_empty());
        assert_eq!(s.new_project_default_ide_id, None);
        assert_eq!(s.new_project_default_agent_id, None);
        assert_eq!(row_count(&conn), 0, "a read must not write (N-13)");
    }

    #[test]
    fn update_then_get_round_trips() {
        let mut conn = test_conn();
        let ide = seed_ide(&conn);
        let agent = seed_agent(&conn);

        let input = Settings {
            launch_screen: "projects".to_string(),
            project_sort: "name-asc".to_string(),
            task_sort: "status".to_string(),
            registry_sort: "type-asc".to_string(),
            task_status_filter: vec!["open".to_string(), "blocked".to_string()],
            new_project_default_ide_id: Some(ide),
            new_project_default_agent_id: Some(agent),
            voice_enabled: true,
            voice_name: "Rishi".to_string(),
        };

        let saved = update_settings(&mut conn, &input).expect("update");
        assert_eq!(saved.launch_screen, "projects");
        assert_eq!(saved.project_sort, "name-asc");
        assert_eq!(saved.task_sort, "status");
        assert_eq!(saved.registry_sort, "type-asc");
        assert_eq!(saved.task_status_filter, vec!["open", "blocked"]);
        assert_eq!(saved.new_project_default_ide_id, Some(ide));
        assert_eq!(saved.new_project_default_agent_id, Some(agent));
        assert!(saved.voice_enabled, "voice_enabled must round-trip");
        assert_eq!(saved.voice_name, "Rishi", "voice_name must round-trip");

        let reread = get_settings(&conn).expect("get");
        assert_eq!(reread.task_status_filter, vec!["open", "blocked"]);
        assert_eq!(reread.launch_screen, "projects");
    }

    #[test]
    fn update_sets_updated_at_and_preserves_created_at() {
        let mut conn = test_conn();
        let input = Settings::defaults();
        update_settings(&mut conn, &input).expect("first save");

        let (created, updated): (String, String) = conn
            .query_row(
                "SELECT created_at, updated_at FROM settings WHERE key = ?1",
                rusqlite::params![KEY_LAUNCH_SCREEN],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("read stamps");
        assert_eq!(created, updated, "first insert leaves them equal");

        std::thread::sleep(std::time::Duration::from_millis(5));
        let mut changed = Settings::defaults();
        changed.launch_screen = "projects".to_string();
        update_settings(&mut conn, &changed).expect("second save");

        let (created2, updated2): (String, String) = conn
            .query_row(
                "SELECT created_at, updated_at FROM settings WHERE key = ?1",
                rusqlite::params![KEY_LAUNCH_SCREEN],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("read stamps again");
        assert_eq!(created, created2, "created_at must not change");
        assert_ne!(updated, updated2, "updated_at must advance");
    }

    #[test]
    fn update_upserts_without_duplicating_rows() {
        let mut conn = test_conn();
        update_settings(&mut conn, &Settings::defaults()).expect("first");
        update_settings(&mut conn, &Settings::defaults()).expect("second");

        assert_eq!(
            row_count(&conn),
            KNOWN_KEYS.len() as i64,
            "exactly one row per known key"
        );
    }

    // -- Tolerance on read ---------------------------------------------------

    #[test]
    fn invalid_launch_screen_falls_back_to_default() {
        let conn = test_conn();
        put(&conn, KEY_LAUNCH_SCREEN, "nonsense");

        let s = get_settings(&conn).expect("get");
        assert_eq!(s.launch_screen, "overview");
    }

    #[test]
    fn invalid_sort_value_falls_back_to_default() {
        let conn = test_conn();
        put(&conn, KEY_PROJECT_SORT, "");
        put(&conn, KEY_TASK_SORT, "by-vibes");
        put(&conn, KEY_REGISTRY_SORT, "name-asc-desc");

        let s = get_settings(&conn).expect("get");
        assert_eq!(s.project_sort, "created-desc");
        assert_eq!(s.task_sort, "created-desc");
        assert_eq!(s.registry_sort, "created-desc");
    }

    #[test]
    fn unknown_status_tokens_are_dropped_from_filter() {
        let conn = test_conn();
        put(&conn, KEY_TASK_STATUS_FILTER, "open,archived,blocked");
        assert_eq!(
            get_settings(&conn).expect("get").task_status_filter,
            vec!["open", "blocked"]
        );

        put(&conn, KEY_TASK_STATUS_FILTER, "archived");
        assert!(
            get_settings(&conn).expect("get").task_status_filter.is_empty(),
            "all-unknown yields no filtering, not no results"
        );

        put(&conn, KEY_TASK_STATUS_FILTER, "open, open ,blocked");
        assert_eq!(
            get_settings(&conn).expect("get").task_status_filter,
            vec!["open", "blocked"],
            "whitespace trimmed and duplicates collapsed"
        );
    }

    #[test]
    fn non_integer_id_value_falls_back_to_none() {
        let conn = test_conn();
        put(&conn, KEY_NEW_PROJECT_DEFAULT_IDE_ID, "abc");
        assert_eq!(
            get_settings(&conn).expect("get").new_project_default_ide_id,
            None
        );
    }

    #[test]
    fn empty_string_id_value_is_none() {
        let conn = test_conn();
        put(&conn, KEY_NEW_PROJECT_DEFAULT_AGENT_ID, "");
        assert_eq!(
            get_settings(&conn).expect("get").new_project_default_agent_id,
            None
        );
    }

    // -- The load-bearing guards --------------------------------------------

    /// Spec 008 2.5: an older build must not delete a newer build's keys.
    #[test]
    fn unknown_key_in_table_is_preserved() {
        let mut conn = test_conn();
        put(&conn, "nexus_future_key", "value-from-tomorrow");

        update_settings(&mut conn, &Settings::defaults()).expect("save");
        let after_save: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'nexus_future_key'",
                [],
                |r| r.get(0),
            )
            .expect("unknown key must survive a save");
        assert_eq!(after_save, "value-from-tomorrow");

        reset_settings(&mut conn).expect("reset");
        let after_reset: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'nexus_future_key'",
                [],
                |r| r.get(0),
            )
            .expect("unknown key must survive a reset");
        assert_eq!(after_reset, "value-from-tomorrow");
    }

    /// Spec 008 2.6: no foreign key can protect a TEXT column.
    #[test]
    fn deleted_default_ide_resolves_to_none() {
        let mut conn = test_conn();
        let ide = seed_ide(&conn);
        let mut input = Settings::defaults();
        input.launch_screen = "projects".to_string();
        input.new_project_default_ide_id = Some(ide);
        update_settings(&mut conn, &input).expect("save");

        conn.execute("DELETE FROM ides WHERE id = ?1", rusqlite::params![ide])
            .expect("delete ide");

        let s = get_settings(&conn).expect("get");
        assert_eq!(s.new_project_default_ide_id, None, "dangling resolves to none");
        assert_eq!(
            s.launch_screen, "projects",
            "every other setting must be unaffected"
        );

        let stale: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                rusqlite::params![KEY_NEW_PROJECT_DEFAULT_IDE_ID],
                |r| r.get(0),
            )
            .expect("row must still exist");
        assert_eq!(stale, ide.to_string(), "the read must not have rewritten it");
    }

    #[test]
    fn deleted_default_agent_resolves_to_none() {
        let mut conn = test_conn();
        let agent = seed_agent(&conn);
        let mut input = Settings::defaults();
        input.new_project_default_agent_id = Some(agent);
        update_settings(&mut conn, &input).expect("save");

        conn.execute(
            "DELETE FROM ai_agents WHERE id = ?1",
            rusqlite::params![agent],
        )
        .expect("delete agent");

        assert_eq!(
            get_settings(&conn).expect("get").new_project_default_agent_id,
            None
        );
    }

    /// Disabled is not dangling: the row exists, so it must still resolve.
    #[test]
    fn disabled_default_still_resolves() {
        let mut conn = test_conn();
        let ide = seed_ide(&conn);
        let mut input = Settings::defaults();
        input.new_project_default_ide_id = Some(ide);
        update_settings(&mut conn, &input).expect("save");

        conn.execute(
            "UPDATE ides SET enabled = 0 WHERE id = ?1",
            rusqlite::params![ide],
        )
        .expect("disable ide");

        assert_eq!(
            get_settings(&conn).expect("get").new_project_default_ide_id,
            Some(ide),
            "a disabled entry still exists and must still resolve"
        );
    }

    // -- Validation on write -------------------------------------------------

    #[test]
    fn update_rejects_invalid_launch_screen() {
        let mut conn = test_conn();
        let mut input = Settings::defaults();
        input.launch_screen = "dashboard".to_string();

        let err = update_settings(&mut conn, &input).expect_err("must reject");
        assert!(err.contains("Invalid launch screen: dashboard"), "{err}");
        assert!(err.contains("overview"), "must name the accepted set: {err}");
        assert_eq!(row_count(&conn), 0, "no row may be written");
    }

    #[test]
    fn update_rejects_invalid_sort_mode() {
        let mut conn = test_conn();

        for (field, label) in [
            ("project", "project sort"),
            ("task", "task sort"),
            ("registry", "registry sort"),
        ] {
            let mut input = Settings::defaults();
            match field {
                "project" => input.project_sort = "bogus".to_string(),
                "task" => input.task_sort = "bogus".to_string(),
                _ => input.registry_sort = "bogus".to_string(),
            }
            let err = update_settings(&mut conn, &input).expect_err("must reject");
            assert!(err.contains(label), "expected {label} in: {err}");
            assert!(err.contains("Expected one of"), "{err}");
        }
        assert_eq!(row_count(&conn), 0);
    }

    #[test]
    fn update_rejects_invalid_status_token() {
        let mut conn = test_conn();
        let mut input = Settings::defaults();
        input.task_status_filter = vec!["open".to_string(), "archived".to_string()];

        let err = update_settings(&mut conn, &input).expect_err("must reject");
        assert!(err.contains("Invalid task status: archived"), "{err}");
        assert!(err.contains("in_progress"), "must name the vocabulary: {err}");
        assert_eq!(row_count(&conn), 0);
    }

    #[test]
    fn update_rejects_unknown_ide() {
        let mut conn = test_conn();
        let mut input = Settings::defaults();
        input.new_project_default_ide_id = Some(9999);

        let err = update_settings(&mut conn, &input).expect_err("must reject");
        assert!(err.contains("IDE 9999 not found"), "{err}");
        assert_eq!(row_count(&conn), 0);
    }

    #[test]
    fn update_rejects_unknown_agent() {
        let mut conn = test_conn();
        let mut input = Settings::defaults();
        input.new_project_default_agent_id = Some(4242);

        let err = update_settings(&mut conn, &input).expect_err("must reject");
        assert!(err.contains("Agent 4242 not found"), "{err}");
        assert_eq!(row_count(&conn), 0);
    }

    #[test]
    fn failed_update_leaves_previous_settings_intact() {
        let mut conn = test_conn();
        let mut good = Settings::defaults();
        good.launch_screen = "projects".to_string();
        good.task_sort = "title-asc".to_string();
        update_settings(&mut conn, &good).expect("first save");

        let mut bad = Settings::defaults();
        bad.launch_screen = "nope".to_string();
        update_settings(&mut conn, &bad).expect_err("must reject");

        let s = get_settings(&conn).expect("get");
        assert_eq!(s.launch_screen, "projects", "previous value must survive");
        assert_eq!(s.task_sort, "title-asc");
    }

    // -- Reset ---------------------------------------------------------------

    #[test]
    fn voice_is_disabled_by_default() {
        let conn = test_conn();
        assert!(
            !get_settings(&conn).expect("get").voice_enabled,
            "voice must default to off (NEXUS-010 C-05)"
        );
    }

    #[test]
    fn malformed_voice_value_reads_as_disabled() {
        let conn = test_conn();
        for bogus in ["1", "yes", "TRUE", "on", "", "maybe"] {
            put(&conn, KEY_VOICE_ENABLED, bogus);
            assert!(
                !get_settings(&conn).expect("get").voice_enabled,
                "value {bogus:?} must not enable the microphone"
            );
        }
        put(&conn, KEY_VOICE_ENABLED, "true");
        assert!(get_settings(&conn).expect("get").voice_enabled);
    }

    #[test]
    fn reset_disables_voice() {
        let mut conn = test_conn();
        let mut input = Settings::defaults();
        input.voice_enabled = true;
        update_settings(&mut conn, &input).expect("save");
        assert!(get_settings(&conn).expect("get").voice_enabled);

        let after = reset_settings(&mut conn).expect("reset");
        assert!(!after.voice_enabled, "reset must return voice to off");
    }

    #[test]
    fn reset_restores_defaults() {
        let mut conn = test_conn();
        let ide = seed_ide(&conn);
        let mut input = Settings::defaults();
        input.launch_screen = "projects".to_string();
        input.project_sort = "name-desc".to_string();
        input.task_status_filter = vec!["done".to_string()];
        input.new_project_default_ide_id = Some(ide);
        update_settings(&mut conn, &input).expect("save");

        let s = reset_settings(&mut conn).expect("reset");

        assert_eq!(s.launch_screen, "overview");
        assert_eq!(s.project_sort, "created-desc");
        assert_eq!(s.task_sort, "created-desc");
        assert_eq!(s.registry_sort, "created-desc");
        assert!(s.task_status_filter.is_empty());
        assert_eq!(s.new_project_default_ide_id, None);
        assert_eq!(s.new_project_default_agent_id, None);
        assert!(!s.voice_enabled);
    }

    #[test]
    fn reset_deletes_only_known_keys() {
        let mut conn = test_conn();
        update_settings(&mut conn, &Settings::defaults()).expect("save");
        put(&conn, "some_other_tool_key", "keep me");

        reset_settings(&mut conn).expect("reset");

        assert_eq!(
            row_count(&conn),
            1,
            "only the unknown key may remain after a reset"
        );
        let kept: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'some_other_tool_key'",
                [],
                |r| r.get(0),
            )
            .expect("unknown key must survive");
        assert_eq!(kept, "keep me");
    }

    // -- NEXUS-011: voice preference ------------------------------------------

    #[test]
    fn voice_name_defaults_to_the_shipped_voice() {
        let conn = test_conn();
        assert_eq!(get_settings(&conn).expect("get").voice_name, DEFAULT_VOICE);
    }

    #[test]
    fn the_default_voice_is_rishi_end_to_end() {
        // Pins the user-visible default, not just the constant: a fresh
        // database must hand the synthesizer Rishi. Tara is deliberately not
        // the default because AVSpeechSynthesizer does not expose her.
        let conn = test_conn();
        assert_eq!(get_settings(&conn).expect("get").voice_name, "Rishi");
    }

    #[test]
    fn the_typescript_default_matches_the_rust_default() {
        // Settings load can fail, and when it does the frontend falls back to
        // DEFAULT_SETTINGS. If the two disagree, NEXUS speaks in a different
        // voice depending on whether the database was readable.
        let ts = include_str!("../../../src/types/index.ts");
        assert!(
            ts.contains(&format!("voiceName: '{DEFAULT_VOICE}'")),
            "DEFAULT_SETTINGS.voiceName in src/types/index.ts must be {DEFAULT_VOICE}"
        );
    }

    #[test]
    fn an_explicitly_empty_voice_name_means_the_system_default() {
        // Distinct from a missing key: the user chose "System default" rather
        // than never having chosen at all.
        let conn = test_conn();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('voice_name', '')",
            [],
        )
        .expect("seed");
        assert_eq!(get_settings(&conn).expect("get").voice_name, "");
    }

    #[test]
    fn a_malformed_stored_voice_name_degrades_to_the_default() {
        let conn = test_conn();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('voice_name', ?1)",
            rusqlite::params![format!("{}x", "A".repeat(MAX_VOICE_NAME))],
        )
        .expect("seed");
        assert_eq!(
            get_settings(&conn).expect("get").voice_name,
            DEFAULT_VOICE,
            "an over-long name must not reach the synthesizer"
        );
    }

    #[test]
    fn an_unknown_voice_name_is_accepted_and_stored() {
        // Availability is machine-specific and resolved at speak time, so the
        // database must not reject a voice this Mac happens not to have.
        let mut conn = test_conn();
        let mut input = get_settings(&conn).expect("get");
        input.voice_name = "Some Voice Not Installed Here".to_string();
        let saved = update_settings(&mut conn, &input).expect("update");
        assert_eq!(saved.voice_name, "Some Voice Not Installed Here");
    }

    #[test]
    fn a_control_character_in_a_voice_name_is_rejected_on_write() {
        let mut conn = test_conn();
        let mut input = get_settings(&conn).expect("get");
        input.voice_name = "Ta\u{0007}ra".to_string();
        assert!(update_settings(&mut conn, &input).is_err());
    }

    #[test]
    fn an_over_long_voice_name_is_rejected_on_write() {
        let mut conn = test_conn();
        let mut input = get_settings(&conn).expect("get");
        input.voice_name = "A".repeat(MAX_VOICE_NAME + 1);
        assert!(update_settings(&mut conn, &input).is_err());
    }

    #[test]
    fn reset_returns_the_voice_to_the_shipped_default() {
        let mut conn = test_conn();
        let mut input = get_settings(&conn).expect("get");
        input.voice_name = "Daniel".to_string();
        update_settings(&mut conn, &input).expect("update");
        assert_eq!(get_settings(&conn).expect("get").voice_name, "Daniel");

        let after = reset_settings(&mut conn).expect("reset");
        assert_eq!(after.voice_name, DEFAULT_VOICE);
    }

}
