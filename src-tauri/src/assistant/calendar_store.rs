//! The Mac's own calendar, through EventKit.
//!
//! # Why this exists rather than Microsoft Graph
//!
//! Graph needs an administrator to consent to the app registration, and in a
//! hardened tenant that consent may never come. The same mailbox is already
//! on this machine: adding the account under System Settings authenticates
//! with Apple's own client, which is pre-approved where NEXUS's is not, and
//! macOS then syncs the calendar locally. Reading it here needs no token, no
//! consent and no network.
//!
//! # Why EventKit rather than scripting Calendar.app
//!
//! Because AppleScript is wrong about recurring meetings, and a work calendar
//! is mostly recurring meetings.
//!
//! Asked for events in a date range, Calendar.app returns each series'
//! **master** event carrying its `RRULE`, not the occurrence that falls in
//! the range. Measured on this machine: a weekly sync with
//! `FREQ=WEEKLY;INTERVAL=1;BYDAY=TH` came back dated to the series start in
//! June when the occurrence being asked about was in September. Twenty-two of
//! the events in that one calendar were recurring.
//!
//! `eventsMatchingPredicate:` returns expanded occurrences, and gets `EXDATE`,
//! individually edited occurrences, all-day events, time zones and DST
//! transitions right. None of that is worth reimplementing.
//!
//! # What it does not do
//!
//! Reads. There is no write path in this module and no action that reaches
//! one, so nothing here can move a meeting or reply to an invitation.

use std::sync::atomic::{AtomicBool, Ordering};

use objc2::rc::Retained;
use objc2_event_kit::{EKAuthorizationStatus, EKEntityType, EKEvent, EKEventStore};
use objc2_foundation::NSDate;
use serde::Serialize;

/// One occurrence, flattened to what a person is asked about.
///
/// Times are unix seconds so the whole thing crosses into the frontend as
/// numbers. An all-day event still carries real start and end instants,
/// because EventKit gives it midnight-to-midnight in the calendar's own zone
/// and that is more useful than a bare date for "is this happening now".
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalEvent {
    pub title: String,
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
    pub location: Option<String>,
    pub organizer: Option<String>,
    /// Which calendar it came from, so a work meeting is distinguishable
    /// from a birthday when several accounts are synced.
    pub calendar: String,
}

/// Whether macOS will let NEXUS read the calendar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Access {
    /// Nobody has been asked yet. The remedy is to ask.
    NotDetermined,
    /// The user said no, or a policy says no. Only System Settings can undo
    /// this; asking again does nothing, macOS will not re-prompt.
    Denied,
    /// Managed device policy. Same practical effect as denied.
    Restricted,
    /// Granted, but for writing only, which is no use to a reader.
    WriteOnly,
    Granted,
}

impl Access {
    /// What to tell the user, in the terms they can act on.
    pub fn explain(self) -> &'static str {
        match self {
            Access::Granted => "NEXUS can read your calendar.",
            Access::NotDetermined => {
                "NEXUS has not asked for calendar access yet. Try again and macOS will ask."
            }
            Access::Denied => {
                "NEXUS is not allowed to read your calendar. Turn it on in System Settings, \
                 Privacy & Security, Calendars."
            }
            Access::Restricted => {
                "Calendar access is blocked by a policy on this Mac, so NEXUS cannot read it."
            }
            Access::WriteOnly => {
                "NEXUS has write-only calendar access, which cannot read events. Grant full \
                 access in System Settings, Privacy & Security, Calendars."
            }
        }
    }
}

fn classify(status: EKAuthorizationStatus) -> Access {
    // `Authorized` is an alias of `FullAccess` in these bindings, so matching
    // it separately would be an unreachable arm.
    match status {
        EKAuthorizationStatus::FullAccess => Access::Granted,
        EKAuthorizationStatus::WriteOnly => Access::WriteOnly,
        EKAuthorizationStatus::Denied => Access::Denied,
        EKAuthorizationStatus::Restricted => Access::Restricted,
        _ => Access::NotDetermined,
    }
}

/// Whether NEXUS may use the calendar at all, mirroring the user's
/// `calendar_enabled` setting (loaded at startup, changed from Settings).
/// Off means no reads and, the point of it, no macOS prompt: a build signed
/// with a new identity is a stranger to macOS, and every first read asks.
static ENABLED: AtomicBool = AtomicBool::new(true);

/// What every calendar read says while switched off. Worded without "allow"
/// or "permission": the Schedule card offers a grant button for those.
pub const OFF: &str = "Calendar is switched off in NEXUS. Turn it on in Settings, Mac access.";

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// The current permission, without asking for it.
pub fn access() -> Access {
    classify(unsafe { EKEventStore::authorizationStatusForEntityType(EKEntityType::Event) })
}

/// Raise the macOS permission prompt, if it has not been raised before.
///
/// Returns immediately. The prompt is answered on the user's own time and
/// macOS does not tell this process synchronously, so callers ask `access`
/// again afterwards rather than waiting on a result here.
///
/// Calling this when already denied is a no-op as far as the user can see:
/// macOS prompts once and never again, which is why `Access::Denied` sends
/// people to System Settings instead of telling them to retry.
pub fn request_access() {
    if !enabled() {
        return;
    }
    let store = unsafe { EKEventStore::new() };
    // The completion handler is required by the API and there is nothing
    // useful to do in it: the answer is read back through
    // `authorizationStatusForEntityType`, which is what the next call does.
    let completion = block2::RcBlock::new(|_granted: objc2::runtime::Bool, _err: *mut objc2_foundation::NSError| {});
    // The binding takes a raw block pointer. `completion` outlives the call,
    // and EventKit copies the block before returning.
    unsafe {
        store.requestFullAccessToEventsWithCompletion(&*completion as *const _ as *mut _)
    };
}

/// Every occurrence between two instants, in unix seconds.
///
/// Occurrences, not series: a weekly meeting inside the window appears once
/// per week it actually falls on, each with its own real start time.
///
/// EventKit gives no ordering guarantee, so this sorts by start. A calendar
/// card that lists the day out of order is worse than no card.
pub fn events_between(from: i64, to: i64) -> Result<Vec<LocalEvent>, String> {
    if !enabled() {
        return Err(OFF.to_string());
    }
    let state = access();
    log::info!("calendar: events_between, access {state:?}");
    match state {
        Access::Granted => {}
        other => return Err(other.explain().to_string()),
    }

    // A store per call. EventKit wants one kept alive to receive change
    // notifications, but nothing here subscribes to those, and a calendar is
    // read when somebody asks rather than on a timer, so the cost is paid
    // rarely and a `static` that is neither Send nor Sync is avoided.
    let store = unsafe { EKEventStore::new() };

    let start = NSDate::dateWithTimeIntervalSince1970(from as f64);
    let end = NSDate::dateWithTimeIntervalSince1970(to as f64);
    let predicate =
        unsafe { store.predicateForEventsWithStartDate_endDate_calendars(&start, &end, None) };

    let found = unsafe { store.eventsMatchingPredicate(&predicate) };

    let mut out: Vec<LocalEvent> = found.iter().map(|event| flatten(&event)).collect();
    out.sort_by_key(|e| e.start);
    log::info!("calendar: {} events in window", out.len());
    Ok(out)
}

fn flatten(event: &Retained<EKEvent>) -> LocalEvent {
    unsafe {
        LocalEvent {
            title: event.title().to_string(),
            start: event.startDate().timeIntervalSince1970() as i64,
            end: event.endDate().timeIntervalSince1970() as i64,
            all_day: event.isAllDay(),
            location: event
                .location()
                .map(|l| l.to_string())
                .filter(|l| !l.trim().is_empty()),
            organizer: event
                .organizer()
                .and_then(|p| p.name())
                .map(|n| n.to_string())
                .filter(|n| !n.trim().is_empty()),
            calendar: event
                .calendar()
                .map(|c| c.title().to_string())
                .unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_off_message_does_not_offer_a_grant_button() {
        let m = OFF.to_lowercase();
        for word in ["allow", "asking", "permission"] {
            assert!(!m.contains(word), "{word}");
        }
    }

    #[test]
    fn every_authorization_status_maps_to_something_actionable() {
        // The point of the enum: each state has to tell the user what to do
        // about it, and "try again" is only true for one of them.
        for state in [
            Access::Granted,
            Access::NotDetermined,
            Access::Denied,
            Access::Restricted,
            Access::WriteOnly,
        ] {
            let said = state.explain();
            assert!(!said.is_empty());
            assert!(said.ends_with('.'), "{state:?}: {said}");
        }
        // Denied must not tell people to retry: macOS prompts once, so
        // retrying is advice that silently does nothing.
        assert!(Access::Denied.explain().contains("System Settings"));
        assert!(!Access::Denied.explain().contains("Try again"));
        assert!(Access::NotDetermined.explain().contains("Try again"));
    }

    #[test]
    fn write_only_is_not_treated_as_read_access() {
        // The trap this guards: `WriteOnly` is a granted state, so a naive
        // "is it not denied" check passes and then every read comes back
        // empty with no explanation.
        assert_ne!(classify(EKAuthorizationStatus::WriteOnly), Access::Granted);
        assert_eq!(classify(EKAuthorizationStatus::FullAccess), Access::Granted);
    }

    #[test]
    fn reading_without_permission_explains_rather_than_returning_nothing() {
        // Whatever this machine's state, the contract holds: either events,
        // or a sentence saying why not. Never an empty list standing in for
        // a permission problem, which is the defect that made the pull
        // request card claim "no PR" when it had failed to ask.
        match events_between(0, 1) {
            Ok(_) => assert_eq!(access(), Access::Granted),
            Err(why) => {
                assert!(!why.is_empty());
                assert_ne!(access(), Access::Granted);
            }
        }
    }

    /// Not part of the suite: it needs a real calendar, a granted
    /// permission and a machine with meetings on it. Run by hand with
    /// `cargo test -- --ignored --nocapture live_calendar` when checking
    /// that occurrences really are expanded.
    #[test]
    #[ignore]
    fn live_calendar_expands_recurring_occurrences() {
        if access() != Access::Granted {
            request_access();
            std::thread::sleep(std::time::Duration::from_secs(20));
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        match events_between(now - 86_400, now + 7 * 86_400) {
            Ok(events) => {
                println!("access: {:?}, {} occurrences", access(), events.len());
                for e in &events {
                    println!(
                        "  {} .. {}  [{}]  {}",
                        e.start, e.end, e.calendar, e.title
                    );
                }
            }
            Err(why) => println!("no events: {why}"),
        }
    }

    #[test]
    fn this_module_never_writes_to_the_calendar() {
        let production = include_str!("calendar_store.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("the file must keep its test module marker");
        for write in ["saveEvent", "removeEvent", "setTitle", "setStartDate", "commit"] {
            assert!(
                !production.contains(write),
                "the calendar is read-only here, found {write}"
            );
        }
    }
}
