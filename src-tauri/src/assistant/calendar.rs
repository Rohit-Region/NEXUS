//! NEXUS-027: knowing when not to speak.
//!
//! The user chose immediate announcements over the restraint NEXUS-021 argued
//! for. This module is what keeps that decision liveable: a meeting is the
//! worst possible moment for an assistant that speaks first, and the calendar
//! is what makes it avoidable without asking anyone to configure a thing.
//!
//! **It fails open, and that is deliberate.** With Outlook unreachable, or
//! not signed in, NEXUS does not know whether a meeting is running and
//! announces anyway. The alternative, staying silent whenever the calendar
//! cannot be read, would mean one expired token turns the whole assistant
//! mute with no sign of why. An occasional interruption is a smaller failure
//! than silence that looks like a bug.
//!
//! Times are compared as **local wall-clock minutes**. The calendar window is
//! requested in the local timezone and the clock is read in the local
//! timezone, so no offset arithmetic happens anywhere and there is no third
//! representation to get wrong.

use rusqlite::Connection;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a fetched schedule is trusted.
///
/// The poll runs every few seconds and a calendar read is a network call, so
/// this is what stands between "knows about meetings" and hammering Graph.
/// A meeting that appears inside the window is missed for at most this long,
/// which is an acceptable trade for not making a request per tick.
const CACHE_FOR: Duration = Duration::from_secs(300);

/// How long before a meeting NEXUS mentions it.
pub const WARN_MINUTES: i64 = 5;

#[derive(Debug, Clone, PartialEq)]
pub struct Meeting {
    pub subject: String,
    /// Minutes past local midnight.
    pub starts: i64,
    pub ends: i64,
}

/// What the calendar says about right now.
#[derive(Debug, Clone, PartialEq)]
pub enum Now {
    /// A meeting is running. Do not speak.
    InMeeting,
    /// One is about to start, and has not been mentioned yet.
    StartingSoon(String),
    /// Nothing in the way.
    Clear,
    /// The calendar could not be read. Distinct from `Clear` on purpose: one
    /// is knowledge and the other is its absence, and a caller that treated
    /// them the same could not choose to fail open deliberately.
    Unknown,
}

struct Cache {
    meetings: Vec<Meeting>,
    /// Whether `meetings` is an answer or an absence.
    ///
    /// A failed read still stamps the cache, or `stale()` never stops being
    /// true and a calendar NEXUS cannot reach is retried on every poll: a
    /// network call every few seconds, for as long as the token stays
    /// expired. Recording the attempt without pretending it succeeded is what
    /// makes the back-off possible while keeping `Unknown` honest.
    known: bool,
    fetched: Instant,
    /// Subjects already announced, so a warning is given once rather than
    /// every few seconds for the five minutes before a meeting.
    warned: Vec<String>,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

/// "14:30" as minutes past midnight.
pub fn clock_minutes(hhmm: &str) -> Option<i64> {
    let (h, m) = hhmm.trim().split_once(':')?;
    let hours: i64 = h.parse().ok()?;
    let mins: i64 = m.chars().take(2).collect::<String>().parse().ok()?;
    (0..24).contains(&hours).then_some(hours * 60 + mins)
}

/// Local wall-clock minutes past midnight, from SQLite rather than a crate.
///
/// The same source `local_clock` uses, so NEXUS never has two ideas of what
/// time it is.
pub fn local_minutes(conn: &Connection) -> Option<i64> {
    conn.query_row(
        "SELECT strftime('%H:%M','now','localtime')",
        [],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .and_then(|now| clock_minutes(&now))
}

/// Today's day of the week, Sunday as 0.
///
/// Taken from SQLite rather than computed, for the same reason
/// `local_minutes` is: it already knows the machine's timezone, and a
/// hand-rolled conversion is a bug waiting for a daylight-saving boundary.
pub fn weekday_today(conn: &Connection) -> Option<i64> {
    conn.query_row("SELECT strftime('%w','now','localtime')", [], |r| {
        r.get::<_, String>(0)
    })
    .ok()
    .and_then(|d| d.parse::<i64>().ok())
    .filter(|d| (0..=6).contains(d))
}

/// Turn `outlook.today_schedule` output into meetings.
pub fn parse_schedule(output: &serde_json::Value) -> Vec<Meeting> {
    output
        .get("events")
        .and_then(|v| v.as_array())
        .map(|events| {
            events
                .iter()
                .filter_map(|e| {
                    let starts = clock_minutes(e.get("start")?.as_str()?)?;
                    // An event with no end is treated as an hour: a calendar
                    // entry with a start and no finish is far more likely to
                    // be a meeting than an instant.
                    let ends = e
                        .get("endsAt")
                        .and_then(|v| v.as_str())
                        .and_then(clock_minutes)
                        .filter(|end| *end > starts)
                        .unwrap_or(starts + 60);
                    Some(Meeting {
                        subject: e.get("subject")?.as_str()?.to_string(),
                        starts,
                        ends,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Replace the cached schedule. Called after a successful calendar read.
pub fn remember(meetings: Vec<Meeting>) {
    store(meetings, true);
}

/// Record that a read was attempted and failed.
///
/// Keeps the state `Unknown`, so NEXUS still fails open, while stopping the
/// retry-every-poll loop a bare failure would otherwise cause.
pub fn remember_failure() {
    store(Vec::new(), false);
}

fn store(meetings: Vec<Meeting>, known: bool) {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let warned = cache.as_ref().map(|c| c.warned.clone()).unwrap_or_default();
    *cache = Some(Cache {
        meetings,
        known,
        fetched: Instant::now(),
        warned,
    });
}

/// Whether the cache is old enough that a caller should refresh it.
pub fn stale() -> bool {
    CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        // `map_or` rather than `is_none_or`: the latter is stable since
        // 1.82 and this crate declares 1.77.2. Never read counts as stale,
        // which is what makes the first poll fetch.
        .map_or(true, |c| c.fetched.elapsed() >= CACHE_FOR)
}

/// Forget everything. Sign-out, and the tests.
pub fn clear() {
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// What the calendar says, given the time.
///
/// `now` is passed rather than read so this is a pure function of the cache
/// and the clock, and therefore testable without waiting for a meeting.
pub fn state_at(now: i64) -> Now {
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = match guard.as_mut() {
        // A read that failed knows nothing, however recently it ran.
        Some(cache) if !cache.known => return Now::Unknown,
        Some(cache) => cache,
        // Never read, or cleared. Fail open: see the module note.
        None => return Now::Unknown,
    };

    if cache
        .meetings
        .iter()
        .any(|m| now >= m.starts && now < m.ends)
    {
        return Now::InMeeting;
    }

    let soon = cache
        .meetings
        .iter()
        .find(|m| m.starts > now && m.starts - now <= WARN_MINUTES);

    match soon {
        Some(meeting) if !cache.warned.contains(&meeting.subject) => {
            // Recorded as warned before it is handed over, so a caller that
            // polls every few seconds gets it once rather than sixty times.
            cache.warned.push(meeting.subject.clone());
            Now::StartingSoon(meeting.subject.clone())
        }
        _ => Now::Clear,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cache is one per process, which is right in production and a race
    /// in a test runner that runs these in parallel. Every test here takes
    /// this first, so they queue rather than clobber each other's fixture.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn guard() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn at(h: i64, m: i64) -> i64 {
        h * 60 + m
    }

    fn seed(meetings: Vec<Meeting>) {
        clear();
        remember(meetings);
    }

    fn standup() -> Meeting {
        Meeting {
            subject: "Standup".to_string(),
            starts: at(9, 30),
            ends: at(9, 45),
        }
    }

    #[test]
    fn a_running_meeting_silences_everything() {
        let _serial = guard();
        // The highest-value line in the milestone: immediate announcements
        // plus a meeting is the worst case for an assistant that speaks
        // first.
        seed(vec![standup()]);
        assert_eq!(state_at(at(9, 30)), Now::InMeeting, "the moment it starts");
        assert_eq!(state_at(at(9, 44)), Now::InMeeting);
    }

    #[test]
    fn the_end_of_a_meeting_is_not_part_of_it() {
        let _serial = guard();
        seed(vec![standup()]);
        assert_eq!(
            state_at(at(9, 45)),
            Now::Clear,
            "a meeting ending at 9:45 is over at 9:45"
        );
    }

    #[test]
    fn a_meeting_is_mentioned_once_and_not_again() {
        let _serial = guard();
        // Polling runs every few seconds. Without this the five minutes
        // before a meeting would be sixty announcements of the same one.
        seed(vec![standup()]);
        assert_eq!(
            state_at(at(9, 26)),
            Now::StartingSoon("Standup".to_string())
        );
        assert_eq!(state_at(at(9, 26)), Now::Clear, "said once");
        assert_eq!(state_at(at(9, 29)), Now::Clear);
    }

    #[test]
    fn a_meeting_further_off_than_the_warning_is_not_mentioned() {
        let _serial = guard();
        seed(vec![standup()]);
        assert_eq!(state_at(at(9, 20)), Now::Clear, "ten minutes is not soon");
        assert_eq!(state_at(at(8, 0)), Now::Clear);
    }

    #[test]
    fn an_unread_calendar_is_not_a_clear_one() {
        let _serial = guard();
        // The distinction the caller needs to fail open deliberately rather
        // than by accident. Silence whenever a token expires would be an
        // assistant that looks broken.
        clear();
        assert_eq!(state_at(at(9, 30)), Now::Unknown);
        assert_ne!(state_at(at(9, 30)), Now::Clear);
    }

    #[test]
    fn a_failed_read_stops_the_retry_without_claiming_to_know_anything() {
        let _serial = guard();
        // Both halves matter. Without the stamp, an unreachable calendar is
        // retried on every poll: a network call every few seconds for as long
        // as a token stays expired. Without `known`, the failure would look
        // like an empty day and NEXUS would talk through a meeting.
        clear();
        remember_failure();
        assert!(!stale(), "a failed attempt still counts as an attempt");
        assert_eq!(state_at(at(9, 30)), Now::Unknown, "and knows nothing");
    }

    #[test]
    fn a_schedule_with_nothing_in_it_is_clear_rather_than_unknown() {
        let _serial = guard();
        seed(Vec::new());
        assert_eq!(state_at(at(9, 30)), Now::Clear);
    }

    #[test]
    fn back_to_back_meetings_leave_no_gap_to_speak_in() {
        let _serial = guard();
        seed(vec![
            standup(),
            Meeting {
                subject: "Design review".to_string(),
                starts: at(9, 45),
                ends: at(10, 30),
            },
        ]);
        assert_eq!(state_at(at(9, 44)), Now::InMeeting);
        assert_eq!(state_at(at(9, 45)), Now::InMeeting, "straight into the next");
        assert_eq!(state_at(at(10, 30)), Now::Clear);
    }

    #[test]
    fn a_clock_string_becomes_minutes() {
        assert_eq!(clock_minutes("09:30"), Some(570));
        assert_eq!(clock_minutes("00:00"), Some(0));
        assert_eq!(clock_minutes("23:59"), Some(1_439));
        assert_eq!(clock_minutes("14:05:30"), Some(845), "seconds are ignored");
        // Nonsense is refused rather than becoming midnight, which would put
        // an imaginary meeting at the start of every day.
        assert_eq!(clock_minutes("25:00"), None);
        assert_eq!(clock_minutes("not a time"), None);
        assert_eq!(clock_minutes(""), None);
    }

    #[test]
    fn a_schedule_payload_becomes_meetings() {
        let output = serde_json::json!({
            "events": [
                { "subject": "Standup", "start": "09:30", "endsAt": "09:45" },
                { "subject": "One to one", "start": "11:00" },
            ]
        });
        let parsed = parse_schedule(&output);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0], standup());
        // No end time is treated as an hour: a calendar entry with a start
        // and no finish is a meeting, not an instant.
        assert_eq!(parsed[1].starts, at(11, 0));
        assert_eq!(parsed[1].ends, at(12, 0));
    }

    #[test]
    fn an_end_before_its_start_is_ignored_rather_than_trusted() {
        let output = serde_json::json!({
            "events": [{ "subject": "Broken", "start": "16:00", "endsAt": "09:00" }]
        });
        let parsed = parse_schedule(&output);
        // A meeting that ends before it begins would never match, so NEXUS
        // would talk straight through it.
        assert_eq!(parsed[0].ends, at(17, 0));
    }
}
