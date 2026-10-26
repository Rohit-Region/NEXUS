//! Phone call alerts: settings, a test, and the tick that rings the phone.
//!
//! The tick runs on its own thread every `TICK` seconds. It reads what is due
//! under the database lock, releases it, sends, then records what was sent.

use tauri::State;

use super::{put_setting, random_topic, setting};
use crate::assistant::calendar_store;
use crate::assistant::notification_connector;
use crate::assistant::ntfy_connector::send_call;
use crate::assistant::phone_call::{
    self, matching_person, meeting_call, meeting_due, message_call, message_fresh, reminder_call,
    reminder_due, Call, CallSettings,
};
use crate::assistant::wellbeing;
use crate::db::DbState;

const SETTINGS_KEY: &str = "phone_call_settings";
const TOPIC_KEY: &str = "ntfy_call_topic";
/// How often the tick looks for something to ring about.
pub const TICK: std::time::Duration = std::time::Duration::from_secs(30);

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn load(conn: &rusqlite::Connection) -> CallSettings {
    setting(conn, SETTINGS_KEY)
        .and_then(|raw| serde_json::from_str::<CallSettings>(&raw).ok())
        .unwrap_or_default()
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallView {
    pub settings: CallSettings,
    /// The topic to subscribe to in the ntfy app, once generated.
    pub topic: Option<String>,
}

#[tauri::command(async)]
pub fn nexus_call_settings(state: State<'_, DbState>) -> Result<CallView, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    Ok(CallView { settings: load(&conn), topic: setting(&conn, TOPIC_KEY) })
}

/// Save the settings. Turning on makes the private topic the first time.
#[tauri::command(async)]
pub fn nexus_set_call_settings(state: State<'_, DbState>, settings: CallSettings) -> Result<CallView, String> {
    let settings = settings.sanitised();
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    if settings.enabled && setting(&conn, TOPIC_KEY).is_none() {
        put_setting(&conn, TOPIC_KEY, &random_topic("nexus-call")?)?;
    }
    put_setting(&conn, SETTINGS_KEY, &serde_json::to_string(&settings).map_err(|e| e.to_string())?)?;
    Ok(CallView { settings, topic: setting(&conn, TOPIC_KEY) })
}

#[tauri::command(async)]
pub fn nexus_call_test(state: State<'_, DbState>) -> Result<(), String> {
    let topic = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        setting(&conn, TOPIC_KEY).ok_or("Turn on phone call alerts first.")?
    };
    send_call(&topic, "Test call from NEXUS", "If your phone kept ringing, phone call alerts are set up.")
}

/// Local "HH:MM" for a unix time, from SQLite's view of the local zone.
fn local_hhmm(conn: &rusqlite::Connection, unix: i64) -> String {
    conn.query_row("SELECT strftime('%H:%M', ?1, 'unixepoch', 'localtime')", [unix], |r| r.get(0))
        .unwrap_or_default()
}

fn already_sent(conn: &rusqlite::Connection, key: &str) -> bool {
    conn.query_row("SELECT 1 FROM phone_calls_sent WHERE key = ?1", [key], |_| Ok(()))
        .is_ok()
}

/// What should ring right now and has not yet.
fn due_calls(conn: &rusqlite::Connection, s: &CallSettings, now: i64) -> Vec<Call> {
    let mut calls = Vec::new();

    if s.reminders {
        let mut stmt = match conn.prepare(
            "SELECT id, what, due_at FROM commitments
              WHERE state = 'open' AND due_at IS NOT NULL AND due_at BETWEEN ?1 AND ?2",
        ) {
            Ok(stmt) => stmt,
            Err(_) => return calls,
        };
        let rows = stmt
            .query_map([now - phone_call::REMINDER_WINDOW, now], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
            })
            .map(|rows| rows.flatten().collect::<Vec<_>>())
            .unwrap_or_default();
        for (id, what, due_at) in rows {
            if reminder_due(due_at, now) {
                calls.push(reminder_call(id, due_at, &what));
            }
        }
    }

    // The calendar switch in Settings is respected: off means not read.
    if s.meetings && calendar_store::enabled() {
        let lead = s.meeting_lead as i64 * 60;
        if let Ok(events) = calendar_store::events_between(now, now + lead + 60) {
            for e in events.iter().filter(|e| !e.all_day && meeting_due(e.start, now, s.meeting_lead)) {
                let local = local_hhmm(conn, e.start);
                calls.push(meeting_call(&e.title, e.start, &local, e.location.as_deref(), now));
            }
        }
    }

    if !s.people.is_empty() {
        if let Ok(arrivals) = notification_connector::recent(conn, now) {
            for a in arrivals.iter().filter(|a| message_fresh(a.at, now)) {
                if matching_person(&a.from, &s.people).is_some() {
                    calls.push(message_call(a.id, &a.app, &a.from, &a.preview, s.include_text));
                }
            }
        }
    }

    calls.retain(|c| !already_sent(conn, &c.key));
    calls
}

/// One tick: find what is due, ring for each, record what was sent.
pub fn tick(state: &DbState) {
    let now = now_unix();
    let (topic, calls) = {
        let Ok(conn) = state.0.lock() else { return };
        let settings = load(&conn);
        if !settings.enabled {
            return;
        }
        let Some(topic) = setting(&conn, TOPIC_KEY) else { return };
        if settings.only_when_away
            && wellbeing::idle_seconds().is_some_and(|idle| idle < phone_call::AWAY_AFTER)
        {
            return;
        }
        let calls = due_calls(&conn, &settings, now);
        (topic, calls)
    };
    for call in calls {
        // Recorded only once sent, so a failed send is retried next tick
        // while it is still due.
        if send_call(&topic, &call.title, &call.message).is_ok() {
            if let Ok(conn) = state.0.lock() {
                let _ = conn.execute(
                    "INSERT OR IGNORE INTO phone_calls_sent (key, sent_at) VALUES (?1, ?2)",
                    rusqlite::params![call.key, now],
                );
                let _ = conn.execute(
                    "DELETE FROM phone_calls_sent WHERE sent_at < ?1",
                    [now - 7 * 86_400],
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        for (_, sql) in crate::db::migrations::MIGRATIONS {
            conn.execute_batch(sql).unwrap();
        }
        conn
    }

    #[test]
    fn a_reminder_due_now_rings_once_and_an_old_one_never() {
        let conn = db();
        let now = 1_800_000_000;
        conn.execute(
            "INSERT INTO commitments (what, due_at) VALUES ('Call Amma', ?1), ('Old thing', ?2)",
            [now - 30, now - 86_400],
        )
        .unwrap();
        let s = CallSettings { enabled: true, meetings: false, ..CallSettings::default() };
        let calls = due_calls(&conn, &s, now);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].message, "Call Amma");

        conn.execute("INSERT INTO phone_calls_sent (key, sent_at) VALUES (?1, ?2)", rusqlite::params![calls[0].key, now])
            .unwrap();
        assert!(due_calls(&conn, &s, now + 30).is_empty(), "rings once");
    }

    #[test]
    fn nothing_is_read_for_a_switched_off_kind() {
        let conn = db();
        let now = 1_800_000_000;
        conn.execute("INSERT INTO commitments (what, due_at) VALUES ('x', ?1)", [now]).unwrap();
        let s = CallSettings { enabled: true, reminders: false, meetings: false, ..CallSettings::default() };
        assert!(due_calls(&conn, &s, now).is_empty());
    }
}
