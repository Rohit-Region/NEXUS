//! NEXUS-024: reading what macOS was already told.
//!
//! **This exists because WhatsApp cannot be read, and that has not changed.**
//! NEXUS-022 stated the position: no personal-account API, library automation
//! risks the number being banned, no scripting dictionary. Nothing here
//! revisits any of it.
//!
//! What changes is the *source*. macOS knows a message arrived, because
//! WhatsApp told it so in order to draw a banner, and that knowledge lives in
//! one SQLite file. Reading it gives sender and preview for every application
//! that posts notifications, from a single place, without automating any of
//! them.
//!
//! **It needs Full Disk Access, and that grant is not scoped to this file.**
//! It lets NEXUS read Messages, Mail, and every document on the machine. The
//! boundary that makes this defensible is not the grant, which cannot be
//! narrowed; it is [`opted_in_apps`], which starts **empty**. NEXUS reads
//! notifications from applications the user named and no others, and the
//! audit trail shows it.
//!
//! Three rules hold the rest together:
//!
//! - **Read only.** There is no action here that dismisses, clears, or acts
//!   on a notification. The database is opened read-only.
//! - **Nothing is kept.** A preview is read, spoken if the user asks for it,
//!   and forgotten. Only a cursor is persisted. Storing previews would create
//!   the transcript-on-disk that every voice milestone has spent effort not
//!   creating.
//! - **Denied and empty are different answers.** A missing grant reports
//!   `Unavailable` with the reason. It must never render as "no messages",
//!   because those look identical from the outside and the confusion costs a
//!   day.

use rusqlite::{Connection, OpenFlags};
use std::sync::Mutex;
use serde::{Deserialize, Serialize};

use super::action::{ActionError, ActionSpec};
use super::connector::{
    Capabilities, Connector, ConnectorStatus, ExecCtx, ReferentDraft, UnavailableAction,
};
use super::permission::{ConfirmPolicy, Permission, Reach};
use super::referent::ReferentKind;
use super::shell::{run_with_stdin_bytes, DEFAULT_TIMEOUT};

pub const CONNECTOR_ID: &str = "notifications";

/// Where macOS keeps every notification every application has posted.
const DB_RELATIVE: &str = "Library/Group Containers/group.com.apple.usernoted/db2/db";

/// Settings key holding the applications the user opted into, comma
/// separated. Absent or empty means NEXUS reads nothing at all.
pub const KEY_APPS: &str = "notification_apps";
/// Settings key holding the cursor: the highest `rec_id` already seen.
const KEY_CURSOR: &str = "notification_cursor";

/// Seconds between the Apple epoch (2001-01-01) and the Unix epoch.
const APPLE_EPOCH_OFFSET: i64 = 978_307_200;

/// Most rows read in one poll. A backlog is not an announcement queue: after
/// a long sleep the user wants to know what is waiting, not to be read forty
/// notifications in a row.
const POLL_CAP: usize = 20;

/// Most read back when the user asks for everything.
///
/// Higher than the announcing cap because they asked, and bounded anyway
/// because macOS prunes the store: it holds tens of notifications, not
/// thousands. A guard against a pathological store, not a policy.
const ASK_ALL_CAP: usize = 60;

/// How stale a notification may be and still be worth **announcing**.
///
/// Anything older was almost certainly already seen on screen. Announcing an
/// hour-old message as if it just arrived is how an assistant stops being
/// believed.
const FRESH_SECONDS: i64 = 300;

/// How far back NEXUS looks when the user **asks**.
///
/// Being interrupted and asking are different questions and deserve
/// different windows. "Tell me the moment something arrives" must be recent
/// or it is noise; "what have I missed" is asking precisely about things
/// that are no longer recent, and answering that with a five-minute window
/// gives "nothing new" to somebody who knows perfectly well there is
/// something.
const ASKED_SECONDS: i64 = 6 * 60 * 60;

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        id: "notifications.status",
        connector_id: CONNECTOR_ID,
        summary: "Check whether NEXUS can see notifications",
        permission: Permission::Read,
        confirm: ConfirmPolicy::Never,
        reach: Reach::LocalOnly,
        reversible: true,
    },
    ActionSpec {
        id: "notifications.recent",
        connector_id: CONNECTOR_ID,
        summary: "Check for new messages",
        permission: Permission::Read,
        confirm: ConfirmPolicy::Never,
        reach: Reach::LocalOnly,
        reversible: true,
    },
    ActionSpec {
        // NEXUS-025. The "yes" half of "shall I read it?".
        //
        // It carries the message rather than fetching it, because by the time
        // the user answers the row may be gone and NEXUS keeps nothing. The
        // text lives in the session's pending offer, which is memory only and
        // expires on the follow-up TTL.
        id: "notifications.read_aloud",
        connector_id: CONNECTOR_ID,
        summary: "Read a message out",
        // Speaking something the user was already shown on screen, at their
        // explicit request, changes nothing.
        permission: Permission::Read,
        confirm: ConfirmPolicy::Never,
        reach: Reach::LocalOnly,
        reversible: true,
    },
];

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Spoken {
    pub from: String,
    #[serde(default)]
    pub preview: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadAloud {
    /// Every message to read, in the order they arrived.
    ///
    /// A list rather than one message, because "read them" means all of them.
    /// The first version carried only the newest arrival, so NEXUS announced
    /// four messages and then read one, which is a worse answer than saying
    /// up front that it could only manage one.
    #[serde(default)]
    messages: Vec<Spoken>,
}

/// Most messages read out in one go.
///
/// Beyond this it stops being an answer and becomes a recital nobody can
/// follow. What was left is still reported.
const SPEAK_CAP: usize = 5;

/// One notification, reduced to what an announcement needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Arrival {
    pub id: i64,
    /// The application, as the user would name it: "WhatsApp", not
    /// "net.whatsapp.WhatsApp".
    pub app: String,
    /// Who it is from. The notification title, which for every messaging
    /// application is the sender.
    pub from: String,
    /// What it said. Never stored, never logged, never sent to a provider.
    pub preview: String,
    /// Seconds since the Unix epoch.
    pub at: i64,
}

/// Wall-clock seconds. Passed into [`since_cursor`] rather than read inside
/// it, so freshness is a testable property rather than a fact about when the
/// suite happened to run.
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn db_path() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(std::path::Path::new(&home).join(DB_RELATIVE))
}

/// Why NEXUS cannot see notifications, if it cannot.
///
/// Separated from "there are none" on purpose, and the two must never share a
/// rendering: a denied read and an empty table look identical from the
/// outside, and every hour spent debugging the wrong one is an hour wasted.
#[derive(Debug, Clone, PartialEq)]
pub enum Blocked {
    NoDatabase,
    NotPermitted,
}

impl std::fmt::Display for Blocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Blocked::NoDatabase => write!(
                f,
                "This version of macOS does not keep notifications where NEXUS \
                 knows to look."
            ),
            Blocked::NotPermitted => write!(
                f,
                "NEXUS is not allowed to read notifications. Turn it on in \
                 System Settings > Privacy & Security > Full Disk Access."
            ),
        }
    }
}

/// Open the notification store read-only.
///
/// `SQLITE_OPEN_READ_ONLY` is not a formality. The file belongs to another
/// process which is writing to it continuously, and opening it for writing
/// would be NEXUS taking a lock on the operating system's notification
/// centre.
fn open_store() -> Result<Connection, Blocked> {
    let path = db_path().ok_or(Blocked::NoDatabase)?;
    if !path.exists() {
        return Err(Blocked::NoDatabase);
    }
    Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| Blocked::NotPermitted)
        .and_then(|conn| {
            // Opening can succeed and reading still be denied, so the probe
            // is a real query rather than the handle.
            match conn.query_row("SELECT COUNT(*) FROM record", [], |r| r.get::<_, i64>(0)) {
                Ok(_) => Ok(conn),
                Err(_) => Err(Blocked::NotPermitted),
            }
        })
}

pub fn available() -> Result<(), Blocked> {
    open_store().map(|_| ())
}

/// The newest record in the store, and the applications that have posted
/// recently.
///
/// Diagnostic only, and it earns its place: when NEXUS says "nothing new"
/// there are three quite different reasons, and from the outside they look
/// identical.
///
/// - Nothing was read at all (no permission).
/// - Records exist but none are newer than the cursor, meaning macOS never
///   recorded the message. Notifications are off for that app, or it was
///   frontmost when the message landed and no banner was posted.
/// - Records arrived but from applications the user did not opt into.
///
/// Guessing between those costs an afternoon. Reporting them costs a query.
pub fn store_state(conn: &Connection) -> Result<(i64, i64, Vec<String>), Blocked> {
    let store = open_store()?;
    // How many records exist at all, which is the number that distinguishes
    // "macOS recorded nothing" from "NEXUS filtered everything out".
    let newest: i64 = store
        .query_row("SELECT COUNT(*) FROM record", [], |r| r.get(0))
        .unwrap_or(0);

    // Ordered by the same coalesced timestamp the read uses, so what this
    // reports and what NEXUS acts on cannot disagree.
    let mut stmt = store
        .prepare(
            "SELECT app.identifier,
                    COUNT(*) AS n,
                    MAX(COALESCE(record.delivered_date,
                                 record.request_last_date,
                                 record.request_date)) AS newest
               FROM record JOIN app ON app.app_id = record.app_id
              GROUP BY app.identifier
              ORDER BY newest DESC
              LIMIT 20",
        )
        .map_err(|_| Blocked::NotPermitted)?;
    // Count *and* age. The count alone said "WhatsApp is posting" while the
    // read still found nothing, which narrows the problem to one question:
    // are those records inside the window, or older than it. Guessing at that
    // has already cost two rounds.
    let now = now_unix();
    let seen: Vec<String> = stmt
        .query_map([], |row| {
            let newest: Option<f64> = row.get(2)?;
            let age = newest
                .map(|t| {
                    let at = t as i64 + APPLE_EPOCH_OFFSET;
                    let minutes = (now - at) / 60;
                    if minutes < 90 {
                        format!("newest {minutes} minutes ago")
                    } else {
                        format!("newest {} hours ago", minutes / 60)
                    }
                })
                .unwrap_or_else(|| "no timestamp at all".to_string());
            Ok(format!(
                "{} ({}, {})",
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                age
            ))
        })
        .map_err(|_| Blocked::NotPermitted)?
        .flatten()
        .collect();

    Ok((cursor(conn), newest, seen))
}

/// The applications the user opted into, lowercased.
///
/// **Empty by default, and empty means nothing is read.** Full Disk Access
/// grants everything; this is what NEXUS chooses to use, and it is the whole
/// privacy argument for the milestone. A bug that made this default to "all"
/// would be the most serious defect in the codebase.
pub fn opted_in_apps(conn: &Connection) -> Vec<String> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        [KEY_APPS],
        |row| row.get::<_, String>(0),
    )
    .unwrap_or_default()
    .split(',')
    .map(|s| s.trim().to_lowercase())
    .filter(|s| !s.is_empty())
    .collect()
}

fn cursor(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        [KEY_CURSOR],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .and_then(|v| v.parse().ok())
    .unwrap_or(0)
}

fn set_cursor(conn: &Connection, value: i64) {
    let _ = conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![KEY_CURSOR, value.to_string()],
    );
}

/// A bundle identifier as a person would say it: `net.whatsapp.WhatsApp`
/// becomes `WhatsApp`.
fn app_label(identifier: &str) -> String {
    identifier
        .rsplit('.')
        .next()
        .unwrap_or(identifier)
        .to_string()
}

/// Whether a notification from `identifier` is one the user opted into.
///
/// **Not a plain equality check, and the reason is Microsoft Teams.** Its
/// bundle identifier is `com.microsoft.teams2`, so the last segment is
/// `teams2` and a user who typed "Teams" would never have matched it. The
/// failure is silent: Teams notifications are dropped and nothing anywhere
/// says so, which is the worst shape a bug can take in a feature whose whole
/// job is telling you about things.
///
/// Three ways to match, and the third is deliberately narrow:
///
/// - The whole identifier, so `com.microsoft.teams2` can be typed exactly.
/// - The last segment, which is what most people would say.
/// - The last segment with a trailing **version number** removed, which is
///   what `teams2` and `slack3` are.
///
/// A general prefix match would have been shorter and is wrong: "mail" would
/// then match `mailchimp`, and this list is the privacy boundary for a Full
/// Disk Access grant. Over-matching means reading notifications the user
/// never agreed to, so the rule only forgives a version digit.
fn matches_app(identifier: &str, wanted: &str) -> bool {
    let wanted = wanted.trim().to_lowercase();
    if wanted.is_empty() {
        return false;
    }
    let full = identifier.to_lowercase();
    if full == wanted {
        return true;
    }

    let label = app_label(identifier).to_lowercase();
    if label == wanted {
        return true;
    }

    // `teams2` for `teams`, but never `mailchimp` for `mail`.
    label
        .strip_prefix(&wanted)
        .map(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
        .unwrap_or(false)
}

/// Pull title, subtitle and body out of a notification's payload.
///
/// **One field at a time, and that is the third correction to this
/// function.** The obvious approach is to convert the whole payload to JSON
/// and read it as a document. It does not work: a real notification carries
/// `<data>` and `<date>` values, and JSON can represent neither, so `plutil`
/// refuses the entire conversion:
///
/// ```text
/// plutil -convert json  ->  "Invalid object in plist for JSON format"
/// plutil -extract req.titl raw  ->  "Priya"
/// ```
///
/// Every real notification failed that way, and the failure was silent: the
/// decode returned nothing and the caller dropped the record. A store with
/// WhatsApp messages one minute old reported "nothing in the last few
/// hours".
///
/// The fixtures did not catch it because they held only strings, which JSON
/// is perfectly happy with. Twice now this function has been broken by a
/// test written to be readable rather than representative.
///
/// `-extract ... raw` reads one value and ignores the rest of the document,
/// so an unrepresentable field somewhere else can no longer take the whole
/// notification down with it.
///
/// The blob reaches `plutil` on **stdin**, never argv: a notification body
/// is somebody's message, and a message on a command line ends up in process
/// listings.
fn field(blob: &[u8], keypath: &str) -> Option<String> {
    let out = run_with_stdin_bytes(
        "/usr/bin/plutil",
        &["-extract", keypath, "raw", "-o", "-", "-"],
        blob,
        DEFAULT_TIMEOUT,
    )
    .ok()?;
    if !out.success {
        // A missing key is an ordinary outcome, not a fault: a sticker has
        // no body and a direct message has no group name.
        return None;
    }
    let text = out.stdout.trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn decode(blob: &[u8]) -> Option<(String, String)> {
    // `titl` is the sender for every messaging application; `subt` carries
    // the group name when there is one.
    let from = match (field(blob, "req.titl"), field(blob, "req.subt")) {
        (Some(title), Some(sub)) => format!("{title} in {sub}"),
        (Some(title), None) => title,
        (None, Some(sub)) => sub,
        (None, None) => return None,
    };
    Some((from, field(blob, "req.body").unwrap_or_default()))
}

/// Notifications NEXUS has not announced yet, from applications the user
/// opted into. The announcing path: recent, and deduplicated.
pub fn since_cursor(conn: &Connection, now: i64) -> Result<Vec<Arrival>, Blocked> {
    arrivals(conn, now, Some(FRESH_SECONDS), true, POLL_CAP)
}

/// Everything within the asking window, whether or not it was announced.
///
/// The answer to "what have I missed", which is a different question from
/// "is there anything right now" and must not be deduplicated: a message
/// NEXUS already read out is still one the user is asking about.
pub fn recent(conn: &Connection, now: i64) -> Result<Vec<Arrival>, Blocked> {
    arrivals(conn, now, Some(ASKED_SECONDS), false, POLL_CAP)
}

/// Everything the store still holds from the watched applications.
///
/// **Not an unread list, and it cannot be one.** A notification is written
/// when a message arrives; nothing updates it when the message is later read
/// on a phone. So this is every message macOS recorded and has not yet
/// pruned, which is the most NEXUS can honestly offer for "show me
/// everything".
pub fn all_held(conn: &Connection, now: i64) -> Result<Vec<Arrival>, Blocked> {
    arrivals(conn, now, None, false, ASK_ALL_CAP)
}

fn arrivals(
    conn: &Connection,
    now: i64,
    window: Option<i64>,
    skip_announced: bool,
    cap: usize,
) -> Result<Vec<Arrival>, Blocked> {
    // Checked *before* the store is opened, and that ordering is the privacy
    // property, not an optimisation. With nothing opted in, NEXUS does not
    // read the notification store at all: it does not look and then discard,
    // it does not look. Full Disk Access is granted and unnarrowable, so what
    // NEXUS declines to open is the only boundary that means anything.
    if opted_in_apps(conn).is_empty() {
        return Ok(Vec::new());
    }

    let store = open_store()?;
    read_arrivals(&store, conn, now, window, skip_announced, cap)
}

/// The read itself, over a store handle rather than a path.
///
/// Split out so the whole path can be exercised against a store built in a
/// test, with the real schema and a real binary payload in it. Three separate
/// defects here survived a green suite because every test stopped at a
/// helper: the query, the filter and the decode were each checked alone, and
/// what was broken each time was how they fitted together.
fn read_arrivals(
    store: &Connection,
    conn: &Connection,
    now: i64,
    window: Option<i64>,
    skip_announced: bool,
    cap: usize,
) -> Result<Vec<Arrival>, Blocked> {
    let apps = opted_in_apps(conn);
    if apps.is_empty() {
        // Nothing opted in is not an error and not a failure to read. It is
        // the default state, and the honest answer is an empty list.
        return Ok(Vec::new());
    }

    // `None` means everything the store still holds. Expressed as a
    // threshold below any real timestamp rather than a second query, so
    // there is one statement to be right about.
    let oldest_wanted = window
        .map(|w| (now - w - APPLE_EPOCH_OFFSET) as f64)
        .unwrap_or(f64::MIN);

    // **Three date columns, not one.** `delivered_date` is only set for a
    // notification macOS actually presented, and it is NULL for plenty of
    // real records: anything delivered while a Focus was on, coalesced into
    // a summary, or updated in place. Filtering on it alone silently dropped
    // most of a busy WhatsApp conversation, and the user got "nothing" while
    // their phone had been buzzing all afternoon.
    //
    // `request_date` is set when the application asked for the notification,
    // which is the moment the message arrived, and is the honest fallback.
    let mut stmt = store
        .prepare(
            "SELECT record.rec_id, app.identifier, record.data,
                    COALESCE(record.delivered_date,
                             record.request_last_date,
                             record.request_date) AS at
               FROM record JOIN app ON app.app_id = record.app_id
              WHERE at >= ?1
              ORDER BY at DESC
              LIMIT ?2",
        )
        .map_err(|_| Blocked::NotPermitted)?;

    let rows = stmt
        .query_map(rusqlite::params![oldest_wanted, cap as i64], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Option<f64>>(3)?,
            ))
        })
        .map_err(|_| Blocked::NotPermitted)?;

    let mut out = Vec::new();
    let mut highest = 0i64;
    for row in rows.flatten() {
        let (id, identifier, blob, delivered) = row;
        highest = highest.max(id);

        if !apps.iter().any(|wanted| matches_app(&identifier, wanted)) {
            continue;
        }
        // Only the announcing path deduplicates. When the user asks, a
        // message NEXUS already read out is still one they are asking about.
        if skip_announced && already_announced(id) {
            continue;
        }

        let at = delivered
            .map(|d| d as i64 + APPLE_EPOCH_OFFSET)
            .unwrap_or(now);

        match decode(&blob) {
            Some((from, preview)) => out.push(Arrival {
                id,
                app: app_label(&identifier),
                from,
                preview,
                at,
            }),
            // **Counted, not swallowed.** Every failure in this connector has
            // been a record dropped without a word, and each time the symptom
            // was "nothing new" from a store that plainly had something in
            // it. A number NEXUS can report turns the next one into a
            // question with an answer instead of an afternoon.
            None => undecodable(),
        }
    }

    // Kept only so `notifications.status` can report how far the store has
    // got. Nothing selects on it any more, so it can no longer get stuck.
    if highest > 0 {
        set_cursor(conn, highest);
    }

    // Oldest first, so an announcement reads them in the order they arrived.
    out.reverse();
    Ok(out)
}

/// Notifications already handed to the user this run.
///
/// Deliberately in memory. A persisted "already seen" marker is what the
/// cursor was, and its failure mode was silent and permanent. This one is
/// wrong for at most one notification after a restart, which the user can
/// see and shrug at.
static ANNOUNCED: Mutex<Vec<i64>> = Mutex::new(Vec::new());

/// How many ids are remembered. Comfortably more than a freshness window can
/// hold, small enough to stay a rounding error.
const ANNOUNCED_CAP: usize = 200;

fn already_announced(id: i64) -> bool {
    let mut seen = ANNOUNCED.lock().unwrap_or_else(|e| e.into_inner());
    if seen.contains(&id) {
        return true;
    }
    seen.push(id);
    if seen.len() > ANNOUNCED_CAP {
        let overflow = seen.len() - ANNOUNCED_CAP;
        seen.drain(..overflow);
    }
    false
}

/// Notifications that matched an application the user watches but whose
/// payload could not be read.
///
/// Zero is the expected value. Anything else means the payload format has
/// moved and NEXUS is quietly discarding real messages, which is precisely
/// the failure that has already happened three times here.
static UNDECODABLE: Mutex<i64> = Mutex::new(0);

fn undecodable() {
    *UNDECODABLE.lock().unwrap_or_else(|e| e.into_inner()) += 1;
}

pub fn undecodable_count() -> i64 {
    *UNDECODABLE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Forget what has been announced. Tests only; the process is the lifetime.
#[cfg(test)]
fn forget_announced() {
    ANNOUNCED.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

pub struct NotificationConnector;

impl Connector for NotificationConnector {
    fn id(&self) -> &'static str {
        CONNECTOR_ID
    }

    fn display_name(&self) -> &'static str {
        "Notifications"
    }

    fn actions(&self) -> &'static [ActionSpec] {
        ACTIONS
    }

    fn capabilities(&self, _conn: &Connection) -> Capabilities {
        match available() {
            Ok(()) => Capabilities {
                available: ACTIONS.iter().map(|s| s.id.to_string()).collect(),
                unavailable: Vec::new(),
            },
            Err(why) => Capabilities {
                // Status still works, and must: it is how the user finds out
                // what is wrong.
                available: vec!["notifications.status".to_string()],
                unavailable: vec![UnavailableAction {
                    action_id: "notifications.recent".to_string(),
                    reason: why.to_string(),
                }],
            },
        }
    }

    fn status(&self, conn: &Connection) -> ConnectorStatus {
        match available() {
            Err(_) => ConnectorStatus::Unavailable,
            // Readable, but pointed at nothing. Distinct from Ready, because
            // "working and silent" and "working and watching" are different
            // states and the user should be able to tell which they are in.
            Ok(()) if opted_in_apps(conn).is_empty() => ConnectorStatus::Unconfigured,
            Ok(()) => ConnectorStatus::Ready,
        }
    }

    fn zero_input_actions(&self) -> &'static [&'static str] {
        // `read_aloud` is absent: it needs the message, which only the
        // announcement that offered it has.
        &["notifications.status", "notifications.recent"]
    }

    fn summarize(&self, action_id: &str, _input: &serde_json::Value, _conn: &Connection) -> String {
        ACTIONS
            .iter()
            .find(|s| s.id == action_id)
            .map(|s| s.summary.to_string())
            .unwrap_or_else(|| action_id.to_string())
    }

    fn describe_result(&self, action_id: &str, output: &serde_json::Value) -> Option<String> {
        match action_id {
            "notifications.status" => {
                if let Some(reason) = output.get("blocked").and_then(|v| v.as_str()) {
                    return Some(reason.to_string());
                }
                let apps = output.get("apps")?.as_array()?;
                if apps.is_empty() {
                    return Some(
                        "I can see notifications, but I am not watching any \
                         applications yet. Add them under Messages in Settings."
                            .to_string(),
                    );
                }
                let watching = apps
                    .iter()
                    .filter_map(|a| a.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                let seen: Vec<&str> = output
                    .get("seenApps")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();

                // Which of the watched apps has actually posted anything
                // lately. This is the question "why did my WhatsApp message
                // not come through" reduces to, and answering it is the
                // difference between a fix and an afternoon.
                // The identifiers come back as "com.example.app (12)", so
                // the count is trimmed off before matching.
                let posting: Vec<&str> = seen
                    .iter()
                    .copied()
                    .filter(|entry| {
                        let id = entry.split(' ').next().unwrap_or(entry);
                        apps.iter()
                            .filter_map(|a| a.as_str())
                            .any(|wanted| matches_app(id, wanted))
                    })
                    .collect();

                // Both numbers, plainly. Every wrong turn in this feature has
                // come from a sentence that drew a conclusion the data did not
                // support, so this one reports and lets the reader conclude.
                let total = output.get("newest").and_then(|v| v.as_i64()).unwrap_or(0);

                if !posting.is_empty() {
                    let failed = output
                        .get("undecodable")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    let trouble = if failed > 0 {
                        format!(
                            " I could not read {failed} of them, which means \
                             the payload format has moved and I am discarding \
                             real messages."
                        )
                    } else {
                        String::new()
                    };
                    return Some(format!(
                        "I am watching {watching}. The store holds {total} \
                         notifications and these are posting: {}.{trouble}",
                        posting.join(", ")
                    ));
                }

                if !seen.is_empty() && posting.is_empty() {
                    // Records are arriving, just not from anything watched.
                    // Naming what *is* arriving lets the user see whether
                    // their opt-in list matches reality.
                    return Some(format!(
                        "I am watching {watching}. The store holds {total} \
                         notifications, but none are from those apps. What is \
                         posting: {}.",
                        seen.iter().take(6).copied().collect::<Vec<_>>().join(", ")
                    ));
                }

                // `newest` and `cursor` are reported, not interpreted.
                //
                // The previous version concluded from `newest <= cursor` that
                // macOS had recorded nothing and told the user to check their
                // notification settings. That was wrong, and confidently so:
                // the store is pruned, so the highest id genuinely does fall
                // below a cursor set earlier, and the message sent somebody
                // to fix settings that were already correct. Selection no
                // longer uses the cursor at all, and this no longer draws a
                // conclusion the numbers do not support.
                Some(if seen.is_empty() {
                    format!(
                        "I am watching {watching}, and I can read the notification \
                         store, but it is empty. If messages are arriving, check \
                         notifications are switched on for those apps in System \
                         Settings."
                    )
                } else {
                    format!(
                        "I am watching {watching} and I can read notifications. \
                         The most recent are from {}. I will speak up when one \
                         arrives from an app you are watching.",
                        seen.iter().take(4).copied().collect::<Vec<_>>().join(", ")
                    )
                })
            }
            "notifications.read_aloud" => {
                let items = output.get("messages")?.as_array()?;
                let total = output.get("total")?.as_i64().unwrap_or(0) as usize;

                let spoken: Vec<String> = items
                    .iter()
                    .filter_map(|m| {
                        let from = m.get("from")?.as_str()?;
                        let preview = m.get("preview")?.as_str().unwrap_or_default();
                        Some(if preview.is_empty() {
                            // A photo or a sticker. That it arrived is the
                            // useful half, and inventing content for it
                            // would not be.
                            format!("{from} sent something with no text in it")
                        } else {
                            format!("{from} says: {preview}")
                        })
                    })
                    .collect();

                let mut said = spoken.join(". ");
                if total > items.len() {
                    // Says what was left, rather than stopping part way and
                    // letting the user think that was all of them.
                    said.push_str(&format!(
                        ". There are {} more I have not read out.",
                        total - items.len()
                    ));
                } else {
                    said.push('.');
                }
                Some(said)
            }
            "notifications.recent" => {
                let items = output.get("arrivals")?.as_array()?;
                if items.is_empty() {
                    // Reached only when the store genuinely holds nothing
                    // from the watched apps, which after "check all" is a
                    // real finding rather than a window being too narrow.
                    // Names the window, so "nothing" is a finding rather than
                    // a shrug. A bare "nothing new" is the same sentence a
                    // stuck cursor produced for hours.
                    return Some(
                        "I have no record of any message from the apps you are \
                         watching. macOS only keeps a notification while it is \
                         recent, so anything you already cleared is gone."
                            .to_string(),
                    );
                }
                let who: Vec<String> = items
                    .iter()
                    .filter_map(|a| {
                        Some(format!(
                            "{} on {}",
                            a.get("from")?.as_str()?,
                            a.get("app")?.as_str()?
                        ))
                    })
                    .collect();
                Some(format!("{} messages: {}.", items.len(), who.join(", ")))
            }
            _ => None,
        }
    }

    /// A list of senders is a question: shall I read them?
    ///
    /// Built from the output, so what NEXUS offers to read is exactly what it
    /// just listed. Deriving it any other way is how the announcement came to
    /// name four messages and read one.
    fn follow_up(
        &self,
        action_id: &str,
        _input: &serde_json::Value,
        output: &serde_json::Value,
    ) -> Option<super::connector::FollowUp> {
        if action_id != "notifications.recent" {
            return None;
        }
        let messages: Vec<Spoken> = output
            .get("arrivals")?
            .as_array()?
            .iter()
            .filter_map(|a| {
                Some(Spoken {
                    from: a.get("from")?.as_str()?.to_string(),
                    preview: a
                        .get("preview")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                })
            })
            .collect();

        (!messages.is_empty()).then(|| super::connector::FollowUp {
            action_id: "notifications.read_aloud",
            input: serde_json::json!({ "messages": messages }),
        })
    }

    fn observe(
        &self,
        action_id: &str,
        _input: &serde_json::Value,
        output: &serde_json::Value,
        _conn: &Connection,
    ) -> Vec<ReferentDraft> {
        if action_id != "notifications.recent" {
            return Vec::new();
        }
        // Who wrote, so "reply to her" has somebody to resolve against. The
        // message itself is deliberately not remembered.
        output
            .get("arrivals")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|a| {
                        Some(ReferentDraft {
                            kind: ReferentKind::Person,
                            display_name: a.get("from")?.as_str()?.to_string(),
                            metadata: serde_json::json!({ "app": a.get("app")?.as_str()? }),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn dispatch(
        &self,
        action_id: &str,
        input: serde_json::Value,
        ctx: &ExecCtx<'_>,
    ) -> Result<serde_json::Value, ActionError> {
        match action_id {
            "notifications.status" => {
                let state = store_state(ctx.conn);
                let blocked = state.as_ref().err().map(|why| why.to_string());
                let (cursor_at, newest, seen) = state.unwrap_or((0, 0, Vec::new()));
                Ok(serde_json::json!({
                    "readable": blocked.is_none(),
                    "blocked": blocked,
                    "apps": opted_in_apps(ctx.conn),
                    "undecodable": undecodable_count(),
                    "cursor": cursor_at,
                    "newest": newest,
                    // What has actually posted lately, so an opt-in list that
                    // does not match reality is visible rather than inferred.
                    "seenApps": seen,
                }))
            }

            "notifications.read_aloud" => {
                let target: ReadAloud =
                    serde_json::from_value(input).map_err(|e| ActionError::InvalidInput {
                        detail: e.to_string(),
                    })?;
                if target.messages.is_empty() {
                    return Err(ActionError::InvalidInput {
                        detail: "There was nothing to read.".to_string(),
                    });
                }
                Ok(serde_json::json!({
                    "total": target.messages.len(),
                    "messages": target
                        .messages
                        .iter()
                        .take(SPEAK_CAP)
                        .cloned()
                        .collect::<Vec<_>>(),
                }))
            }

            "notifications.recent" => {
                let now = now_unix();
                // "check my messages" looks at the last few hours; "check all
                // my messages" looks at everything the store still holds.
                let all = input
                    .get("all")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let found = if all {
                    all_held(ctx.conn, now)
                } else {
                    recent(ctx.conn, now)
                };
                match found {
                    Ok(arrivals) => Ok(serde_json::json!({ "arrivals": arrivals })),
                    // A refusal, phrased as the thing the user can act on,
                    // and never as "nothing new".
                    Err(why) => Err(ActionError::Failed {
                        detail: why.to_string(),
                    }),
                }
            }

            other => Err(ActionError::UnknownAction {
                action_id: other.to_string(),
            }),
        }
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

    fn opt_in(conn: &Connection, apps: &str) {
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![KEY_APPS, apps],
        )
        .expect("opt in");
    }

    #[test]
    fn nothing_is_watched_by_default() {
        // The most important assertion in this file. Full Disk Access grants
        // everything; this list is what NEXUS chooses to use, and a bug that
        // made it default to "all" would be the worst defect in the codebase.
        let conn = test_conn();
        assert!(
            opted_in_apps(&conn).is_empty(),
            "notifications must be opt-in, never opt-out"
        );
        assert_eq!(
            since_cursor(&conn, 0).expect("must not error"),
            Vec::new(),
            "with nothing opted in, nothing is read at all"
        );
    }

    #[test]
    fn an_empty_setting_is_not_a_wildcard() {
        let conn = test_conn();
        for value in ["", "   ", ",", " , , "] {
            opt_in(&conn, value);
            assert!(
                opted_in_apps(&conn).is_empty(),
                "{value:?} must not be read as \"everything\""
            );
        }
    }

    #[test]
    fn opted_in_apps_are_matched_case_insensitively() {
        let conn = test_conn();
        opt_in(&conn, "WhatsApp, Microsoft Teams");
        assert_eq!(opted_in_apps(&conn), vec!["whatsapp", "microsoft teams"]);
    }

    #[test]
    fn a_bundle_identifier_becomes_a_name_a_person_would_say() {
        assert_eq!(app_label("net.whatsapp.WhatsApp"), "WhatsApp");
        assert_eq!(app_label("com.microsoft.teams2"), "teams2");
        // Nothing to strip is not an error.
        assert_eq!(app_label("Finder"), "Finder");
    }

    #[test]
    fn a_versioned_bundle_identifier_still_matches_the_name_people_say() {
        // Real identifiers from this machine. Teams is the one that broke:
        // `com.microsoft.teams2` has a last segment of `teams2`, so a user
        // who typed "Teams" matched nothing and their Teams notifications
        // were dropped in silence.
        assert!(matches_app("com.microsoft.teams2", "Teams"));
        assert!(matches_app("com.microsoft.teams2", "teams"));
        assert!(matches_app("net.whatsapp.WhatsApp", "whatsapp"));
        assert!(matches_app("com.apple.mail", "Mail"));
        // The whole identifier, for anyone who would rather be exact.
        assert!(matches_app("com.microsoft.teams2", "com.microsoft.teams2"));
    }

    #[test]
    fn only_a_version_digit_is_forgiven() {
        // This list is the privacy boundary for a Full Disk Access grant, so
        // the match is allowed to forgive a version number and nothing else.
        // A general prefix rule would be shorter and would read notifications
        // the user never agreed to.
        assert!(!matches_app("com.mailchimp.desktop", "mail"));
        assert!(!matches_app("com.slack.Slackbot", "slack"));
        assert!(!matches_app("com.apple.mailboxes", "mail"));
        assert!(!matches_app("net.whatsapp.WhatsApp", ""));
        assert!(!matches_app("net.whatsapp.WhatsApp", "   "));
        assert!(!matches_app("com.apple.mail", "teams"));
    }

    #[test]
    fn the_same_notification_is_not_announced_twice() {
        // The poll runs every few seconds over a five-minute window, so
        // without this every message would be read out around forty times.
        forget_announced();
        assert!(!already_announced(9_001), "first sight is new");
        assert!(already_announced(9_001), "second is not");
        assert!(!already_announced(9_002));
    }

    #[test]
    fn nothing_selects_on_a_persisted_marker_any_more() {
        // The defect this exists to prevent coming back. Selecting on a
        // stored high-water mark assumes the table only grows; macOS prunes
        // delivered notifications, so the highest id falls below a cursor set
        // earlier and the query then matches nothing, permanently. From the
        // outside that is indistinguishable from a quiet afternoon, and it
        // told the user their notification settings were wrong.
        let production = include_str!("notification_connector.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("the file must keep its test module marker");
        assert!(
            production.contains("WHERE at >= ?1"),
            "arrivals must be selected by time"
        );
        assert!(
            production.contains("COALESCE(record.delivered_date"),
            "delivered_date alone is NULL for plenty of real notifications"
        );
        assert!(
            !production.contains("record.rec_id > ?1"),
            "a persisted cursor must never gate selection again"
        );
    }

    #[test]
    fn denial_and_emptiness_never_read_the_same() {
        // The confusion this exists to prevent: a missing grant and a quiet
        // afternoon are indistinguishable unless they are separate types.
        // One is an error the user can act on; the other is good news.
        assert_ne!(
            Blocked::NotPermitted.to_string(),
            Blocked::NoDatabase.to_string()
        );
        assert!(
            Blocked::NotPermitted
                .to_string()
                .contains("Full Disk Access"),
            "the refusal must name the remedy"
        );
        assert!(
            !Blocked::NotPermitted.to_string().contains("no messages"),
            "a refusal must never be phrased as an absence"
        );
    }

    #[test]
    fn the_cursor_survives_a_restart() {
        let conn = test_conn();
        assert_eq!(cursor(&conn), 0, "a fresh install has seen nothing");
        set_cursor(&conn, 4_312);
        assert_eq!(cursor(&conn), 4_312);
        set_cursor(&conn, 4_400);
        assert_eq!(cursor(&conn), 4_400, "the cursor moves forward");
    }

    #[test]
    fn a_payload_yields_a_sender_and_a_body() {
        // An XML plist rather than binary: `plutil` reads both, and the
        // fixture stays legible to whoever reads this test next.
        let plist = br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>req</key><dict>
<key>titl</key><string>Priya</string>
<key>body</key><string>are we still on for 4?</string>
</dict></dict></plist>"#;
        let (from, preview) = decode(plist).expect("must decode");
        assert_eq!(from, "Priya");
        assert_eq!(preview, "are we still on for 4?");
    }

    /// A stand-in for the macOS store: the real schema, a real binary
    /// payload, and the awkward types a real notification carries.
    fn fake_store(identifier: &str, apple_seconds: f64) -> Connection {
        let store = Connection::open_in_memory().expect("open");
        store
            .execute_batch(
                "CREATE TABLE app (app_id INTEGER PRIMARY KEY, identifier TEXT);
                 CREATE TABLE record (
                    rec_id            INTEGER PRIMARY KEY,
                    app_id            INTEGER,
                    data              BLOB,
                    request_date      REAL,
                    request_last_date REAL,
                    delivered_date    REAL
                 );",
            )
            .expect("schema");

        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>req</key><dict>
  <key>titl</key><string>Priya</string>
  <key>body</key><string>are we still on for 4?</string>
</dict>
<key>attachment</key><data>SGVsbG8=</data>
<key>arrived</key><date>2026-08-30T09:00:00Z</date>
</dict></plist>"#;
        let payload = crate::assistant::shell::run_with_stdin_bytes(
            "/usr/bin/plutil",
            &["-convert", "binary1", "-o", "-", "-"],
            xml,
            DEFAULT_TIMEOUT,
        )
        .expect("plutil")
        .raw_stdout;

        store
            .execute(
                "INSERT INTO app (app_id, identifier) VALUES (1, ?1)",
                [identifier],
            )
            .expect("app");
        store
            .execute(
                "INSERT INTO record (rec_id, app_id, data, delivered_date)
                 VALUES (1, 1, ?1, ?2)",
                rusqlite::params![payload, apple_seconds],
            )
            .expect("record");
        store
    }

    const NOW: i64 = 1_800_000_000;

    fn apple(now_unix: i64, seconds_ago: i64) -> f64 {
        (now_unix - seconds_ago - APPLE_EPOCH_OFFSET) as f64
    }

    #[test]
    fn with_nothing_watched_the_store_is_not_even_opened() {
        // The ordering that makes the Full Disk Access grant defensible.
        // NEXUS does not read and discard; with an empty list it does not
        // read. Guarded here because moving the check below `open_store`
        // looks like a harmless tidy-up and quietly removes the boundary.
        let production = include_str!("notification_connector.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("the file must keep its test module marker");
        let arrivals = production
            .split_once("fn arrivals(")
            .map(|(_, rest)| rest)
            .and_then(|rest| rest.split_once("fn read_arrivals("))
            .map(|(body, _)| body)
            .expect("arrivals must exist");

        let check = arrivals
            .find("opted_in_apps(conn).is_empty()")
            .expect("the opt-in check must be here");
        let open = arrivals.find("open_store()").expect("the store is opened here");
        assert!(check < open, "check the opt-in list before opening the store");
    }

    #[test]
    fn every_message_is_read_out_not_just_the_newest() {
        // The defect: NEXUS announced four messages and then read one,
        // because the offer carried only the last arrival. Announcing more
        // than you deliver is worse than saying up front that you can only
        // manage one.
        let three = serde_json::json!({ "messages": [
            { "from": "Priya",   "preview": "still on for 4?" },
            { "from": "Amma", "preview": "call me" },
            { "from": "Divi",    "preview": "sent the file" },
        ]});
        let conn = test_conn();
        let out = NotificationConnector
            .dispatch("notifications.read_aloud", three, &ExecCtx { conn: &conn })
            .expect("must read");

        let said = NotificationConnector
            .describe_result("notifications.read_aloud", &out)
            .expect("must speak");
        for who in ["Priya", "Amma", "Divi"] {
            assert!(said.contains(who), "{who} was not read out: {said}");
        }
        assert!(said.contains("still on for 4?"), "{said}");
    }

    #[test]
    fn a_long_backlog_says_how_much_it_left() {
        // Stopping part way without saying so lets the user believe that was
        // all of them, which is the same failure in a quieter form.
        let many: Vec<serde_json::Value> = (0..9)
            .map(|i| serde_json::json!({ "from": format!("Person {i}"), "preview": "hello" }))
            .collect();
        let conn = test_conn();
        let out = NotificationConnector
            .dispatch(
                "notifications.read_aloud",
                serde_json::json!({ "messages": many }),
                &ExecCtx { conn: &conn },
            )
            .expect("must read");
        let said = NotificationConnector
            .describe_result("notifications.read_aloud", &out)
            .expect("must speak");
        assert!(said.contains("4 more"), "must name what was left: {said}");
    }

    #[test]
    fn listing_messages_offers_to_read_exactly_those_messages() {
        // What is offered must be what was just listed. Deriving it any other
        // way is how the announcement and the reading drifted apart.
        let listed = serde_json::json!({ "arrivals": [
            { "from": "Priya", "preview": "one", "app": "whatsapp", "id": 1, "at": 0 },
            { "from": "Divi",  "preview": "two", "app": "whatsapp", "id": 2, "at": 0 },
        ]});
        let offer = NotificationConnector
            .follow_up("notifications.recent", &serde_json::Value::Null, &listed)
            .expect("a list is a question");
        assert_eq!(offer.action_id, "notifications.read_aloud");
        assert_eq!(
            offer.input["messages"].as_array().map(|m| m.len()),
            Some(2),
            "both listed messages must be offered"
        );
    }

    #[test]
    fn asking_for_everything_ignores_the_window() {
        // A message older than the asking window is still one the user asked
        // about when they said "all". The store prunes itself, so "all" is
        // bounded by macOS rather than by NEXUS guessing at a horizon.
        forget_announced();
        let conn = test_conn();
        opt_in(&conn, "whatsapp");
        let old = fake_store("net.whatsapp.whatsapp", apple(NOW, ASKED_SECONDS * 4));

        assert!(
            read_arrivals(&old, &conn, NOW, Some(ASKED_SECONDS), false, POLL_CAP)
                .expect("read")
                .is_empty(),
            "outside the asking window"
        );
        assert_eq!(
            read_arrivals(&old, &conn, NOW, None, false, ASK_ALL_CAP)
                .expect("read")
                .len(),
            1,
            "but \"all\" must still find it"
        );
    }

    #[test]
    fn a_watched_message_is_read_all_the_way_out_of_the_store() {
        // The end-to-end test that should have existed from the start. Query,
        // app match, freshness and decode, together, against the real schema.
        // Every previous defect here lived in the seam between two of those.
        forget_announced();
        let conn = test_conn();
        opt_in(&conn, "whatsapp");
        let store = fake_store("net.whatsapp.whatsapp", apple(NOW, 60));

        let found = read_arrivals(&store, &conn, NOW, Some(ASKED_SECONDS), false, POLL_CAP)
            .expect("the read must succeed");

        assert_eq!(found.len(), 1, "a minute-old WhatsApp message must be read");
        assert_eq!(found[0].from, "Priya");
        assert_eq!(found[0].preview, "are we still on for 4?");
        assert_eq!(found[0].app, "whatsapp");
        assert_eq!(undecodable_count(), 0, "nothing may be dropped silently");
    }

    #[test]
    fn a_versioned_identifier_is_read_end_to_end_too() {
        // `com.microsoft.teams2` for a user who typed "Teams".
        forget_announced();
        let conn = test_conn();
        opt_in(&conn, "Teams");
        let store = fake_store("com.microsoft.teams2", apple(NOW, 30));
        let found = read_arrivals(&store, &conn, NOW, Some(ASKED_SECONDS), false, POLL_CAP).expect("read");
        assert_eq!(found.len(), 1, "the version digit must not lose the message");
    }

    #[test]
    fn an_unwatched_app_and_a_stale_message_are_both_left_out() {
        forget_announced();
        let conn = test_conn();
        opt_in(&conn, "whatsapp");

        let other = fake_store("com.apple.Safari", apple(NOW, 60));
        assert!(read_arrivals(&other, &conn, NOW, Some(ASKED_SECONDS), false, POLL_CAP)
            .expect("read")
            .is_empty());

        let old = fake_store("net.whatsapp.whatsapp", apple(NOW, ASKED_SECONDS + 600));
        assert!(
            read_arrivals(&old, &conn, NOW, Some(ASKED_SECONDS), false, POLL_CAP)
                .expect("read")
                .is_empty(),
            "older than the window is not new"
        );
    }

    #[test]
    fn the_announcing_path_says_a_thing_once() {
        forget_announced();
        let conn = test_conn();
        opt_in(&conn, "whatsapp");
        let store = fake_store("net.whatsapp.whatsapp", apple(NOW, 30));

        assert_eq!(
            read_arrivals(&store, &conn, NOW, Some(FRESH_SECONDS), true, POLL_CAP)
                .expect("read")
                .len(),
            1
        );
        assert!(
            read_arrivals(&store, &conn, NOW, Some(FRESH_SECONDS), true, POLL_CAP)
                .expect("read")
                .is_empty(),
            "the poll runs every few seconds; it must not repeat itself"
        );
        // But asking still returns it: a message already announced is still
        // one the user is asking about.
        assert_eq!(
            read_arrivals(&store, &conn, NOW, Some(ASKED_SECONDS), false, POLL_CAP)
                .expect("read")
                .len(),
            1
        );
    }

    #[test]
    fn a_payload_carrying_data_and_dates_still_decodes() {
        // The third and worst version of the same defect. A real notification
        // holds a `<data>` attachment and a `<date>`, neither of which JSON
        // can represent, so converting the whole document failed and the
        // decode returned nothing. Every real notification hit this, and the
        // drop was silent.
        //
        // Twice this function was broken by fixtures written to be readable
        // rather than representative. This one carries the awkward types on
        // purpose, so a whole-document conversion cannot pass it.
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>req</key><dict>
  <key>titl</key><string>Priya</string>
  <key>body</key><string>are we still on for 4?</string>
</dict>
<key>attachment</key><data>SGVsbG8gdGhlcmU=</data>
<key>arrived</key><date>2026-08-30T09:00:00Z</date>
</dict></plist>"#;
        let binary = crate::assistant::shell::run_with_stdin_bytes(
            "/usr/bin/plutil",
            &["-convert", "binary1", "-o", "-", "-"],
            xml,
            DEFAULT_TIMEOUT,
        )
        .expect("plutil must convert");
        let bytes = binary.raw_stdout.as_slice();

        // The document genuinely cannot become JSON. If this ever starts
        // succeeding, the fixture has stopped standing for the real thing.
        let as_json = crate::assistant::shell::run_with_stdin_bytes(
            "/usr/bin/plutil",
            &["-convert", "json", "-o", "-", "-"],
            bytes,
            DEFAULT_TIMEOUT,
        )
        .expect("plutil runs");
        assert!(
            !as_json.success,
            "the fixture must contain something JSON cannot hold, or it \
             proves nothing about real notifications"
        );

        let (from, preview) = decode(bytes).expect("it must decode anyway");
        assert_eq!(from, "Priya");
        assert_eq!(preview, "are we still on for 4?");
    }

    #[test]
    fn a_binary_payload_decodes_as_well_as_an_xml_one() {
        // The defect this exists to prevent coming back, and the reason it
        // survived a suite that was already testing `decode`: every fixture
        // was an XML plist, which is valid UTF-8 and survived being passed
        // through a `&str`. Real payloads are binary and did not, so the
        // parse failed and every notification was dropped in silence.
        //
        // Built by converting an XML fixture with the same `plutil` the
        // production path uses, so the test cannot drift from the format.
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>req</key><dict>
<key>titl</key><string>Priya</string>
<key>body</key><string>are we still on for 4?</string>
</dict></dict></plist>"#;
        let binary = crate::assistant::shell::run_with_stdin_bytes(
            "/usr/bin/plutil",
            &["-convert", "binary1", "-o", "-", "-"],
            xml,
            DEFAULT_TIMEOUT,
        )
        .expect("plutil must convert");
        assert!(binary.success, "fixture conversion failed");

        let bytes = binary.raw_stdout.as_slice();
        assert!(
            bytes.starts_with(b"bplist"),
            "the fixture must actually be binary, or this proves nothing"
        );
        assert!(
            String::from_utf8(bytes.to_vec()).is_err(),
            "and must not survive being treated as text, which is the bug"
        );

        let (from, preview) = decode(bytes).expect("a binary payload must decode");
        assert_eq!(from, "Priya");
        assert_eq!(preview, "are we still on for 4?");
    }

    #[test]
    fn a_group_message_names_the_group_as_well_as_the_sender() {
        let plist = br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>req</key><dict>
<key>titl</key><string>Priya</string>
<key>subt</key><string>Family</string>
<key>body</key><string>dinner at 8</string>
</dict></dict></plist>"#;
        let (from, _) = decode(plist).expect("must decode");
        assert_eq!(from, "Priya in Family", "who, and where they said it");
    }

    #[test]
    fn a_payload_with_no_sender_is_dropped_rather_than_announced_anonymously() {
        let plist = br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>req</key><dict>
<key>body</key><string>something happened</string>
</dict></dict></plist>"#;
        assert!(
            decode(plist).is_none(),
            "\"you have a message from nobody\" is not worth saying"
        );
        assert!(decode(b"not a plist at all").is_none());
    }

    #[test]
    fn a_notification_with_no_body_still_names_who_sent_it() {
        // A photo or a sticker has no text. That the message exists is still
        // the useful half.
        let plist = br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>req</key><dict>
<key>titl</key><string>Amma</string>
</dict></dict></plist>"#;
        let (from, preview) = decode(plist).expect("must decode");
        assert_eq!(from, "Amma");
        assert!(preview.is_empty());
    }

    #[test]
    fn every_action_here_is_read_only() {
        // The rule the milestone rests on. Nothing dismisses, clears, or
        // acts on a notification, and nothing leaves the machine.
        for spec in ACTIONS {
            assert_eq!(spec.permission, Permission::Read, "{}", spec.id);
            assert_eq!(spec.reach, Reach::LocalOnly, "{}", spec.id);
            assert_eq!(spec.confirm, ConfirmPolicy::Never, "{}", spec.id);
            assert!(spec.reversible, "{}", spec.id);
        }
    }

    #[test]
    fn the_store_is_never_opened_for_writing() {
        let production = include_str!("notification_connector.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("the file must keep its test module marker");
        assert!(
            production.contains("SQLITE_OPEN_READ_ONLY"),
            "the notification store belongs to another process"
        );
        assert!(
            !production.contains("OpenFlags::SQLITE_OPEN_READ_WRITE"),
            "NEXUS must never take a write lock on the notification centre"
        );
    }

    #[test]
    fn a_preview_never_reaches_a_command_line() {
        // A notification body is somebody's message, and argv is visible to
        // every process on the machine.
        let production = include_str!("notification_connector.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("the file must keep its test module marker");
        assert!(
            production.contains("run_with_stdin"),
            "the payload must reach plutil on stdin"
        );
    }
}
