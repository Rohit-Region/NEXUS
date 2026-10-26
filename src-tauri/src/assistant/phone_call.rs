//! Phone call alerts: the phone rings, alarm-style, and says why.
//!
//! # Not a phone call, on purpose
//!
//! A real call needs a paid telephony provider. This is free: an urgent ntfy
//! notification which the ntfy Android app can keep sounding until dismissed
//! ("Keep alerting for highest priority"), carrying the reason in words: a
//! meeting about to start, a message from someone the user picked, a reminder
//! that is due.
//!
//! # Where it goes
//!
//! A private topic NEXUS generates (80 random bits), never the stock-alert
//! topic: a message preview can be personal, and on ntfy.sh anyone who knows
//! a topic name can read it.
//!
//! # Ringing once
//!
//! Every alert has a key, recorded once sent, so a tick that sees the same
//! meeting or message again stays quiet. Reminders ring only within
//! `REMINDER_WINDOW` of their time, so switching this on does not set off
//! every reminder already overdue.
//!
//! This module decides what to ring and what it says. Reading the calendar,
//! notifications and reminders, and sending, happen in the command layer.

use serde::{Deserialize, Serialize};

/// How late a reminder may still ring: one missed tick, not a backlog.
pub const REMINDER_WINDOW: i64 = 120;
/// How recent a message must be to ring: older ones were seen at the Mac or
/// are no longer news.
pub const MESSAGE_WINDOW: i64 = 180;
/// No keyboard or mouse for this long counts as away from the Mac.
pub const AWAY_AFTER: i64 = 120;
/// Shortest name that can be matched, so "Al" does not match "Alerts".
const MIN_NAME: usize = 3;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallSettings {
    pub enabled: bool,
    pub meetings: bool,
    /// Minutes before a meeting starts.
    pub meeting_lead: u32,
    pub reminders: bool,
    /// Names to ring for, as they appear in WhatsApp, Teams or Mail.
    pub people: Vec<String>,
    /// Put the message text in the alert, not only who sent it.
    pub include_text: bool,
    /// Ring only when the Mac has been idle for `AWAY_AFTER`.
    pub only_when_away: bool,
}

impl Default for CallSettings {
    fn default() -> Self {
        CallSettings {
            enabled: false,
            meetings: true,
            meeting_lead: 5,
            reminders: true,
            people: Vec::new(),
            include_text: true,
            only_when_away: true,
        }
    }
}

impl CallSettings {
    /// Clamp what the window sends to sane values.
    pub fn sanitised(mut self) -> Self {
        self.meeting_lead = self.meeting_lead.clamp(1, 60);
        let mut seen = Vec::new();
        for p in self.people {
            let p = p.trim().to_string();
            if p.chars().count() >= MIN_NAME
                && p.chars().count() <= 60
                && !seen.iter().any(|s: &String| s.eq_ignore_ascii_case(&p))
            {
                seen.push(p);
            }
        }
        seen.truncate(30);
        self.people = seen;
        self
    }
}

/// One alert to send.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    /// Unique per thing rung for, so it rings once.
    pub key: String,
    pub title: String,
    pub message: String,
}

pub fn meeting_due(start: i64, now: i64, lead_minutes: u32) -> bool {
    start - lead_minutes as i64 * 60 <= now && now < start
}

pub fn reminder_due(due_at: i64, now: i64) -> bool {
    due_at <= now && now - due_at < REMINDER_WINDOW
}

pub fn message_fresh(at: i64, now: i64) -> bool {
    now - at < MESSAGE_WINDOW && at <= now + 60
}

/// The configured name a sender matches, if any. Case-insensitive and on the
/// whole word, so "Ravi" matches "Ravi Kumar" and "Ravi in Team Chat" but not
/// "Ravindra".
pub fn matching_person<'a>(from: &str, people: &'a [String]) -> Option<&'a String> {
    let from = from.to_lowercase();
    people.iter().find(|p| {
        let p = p.to_lowercase();
        from.match_indices(&p).any(|(i, _)| {
            let before = from[..i].chars().next_back();
            let after = from[i + p.len()..].chars().next();
            !before.is_some_and(|c| c.is_alphanumeric()) && !after.is_some_and(|c| c.is_alphanumeric())
        })
    })
}

/// "15:30" as "3:30 pm".
pub fn twelve_hour(hhmm: &str) -> String {
    let (h, m) = match hhmm.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().unwrap_or(0), m),
        None => return hhmm.to_string(),
    };
    let suffix = if h >= 12 { "pm" } else { "am" };
    let h12 = match h % 12 {
        0 => 12,
        x => x,
    };
    format!("{h12}:{m} {suffix}")
}

pub fn meeting_call(title: &str, start: i64, start_local: &str, location: Option<&str>, now: i64) -> Call {
    let minutes = ((start - now) as f64 / 60.0).ceil().max(1.0) as i64;
    let mut message = format!("{} at {}. Please join.", one_line(title, 120), twelve_hour(start_local));
    if let Some(place) = location.filter(|l| !l.trim().is_empty()) {
        message.push_str(&format!(" Where: {}", one_line(place, 80)));
    }
    Call {
        key: format!("meeting:{start}:{}", one_line(title, 60)),
        title: format!("Meeting in {minutes} min"),
        message,
    }
}

pub fn reminder_call(id: i64, due_at: i64, what: &str) -> Call {
    Call {
        key: format!("reminder:{id}:{due_at}"),
        title: "Reminder".to_string(),
        message: one_line(what, 200),
    }
}

pub fn message_call(id: i64, app: &str, from: &str, preview: &str, include_text: bool) -> Call {
    let message = if include_text && !preview.trim().is_empty() {
        one_line(preview, 200)
    } else {
        format!("Open {app} to read it.")
    };
    Call {
        key: format!("message:{id}"),
        title: format!("Message from {} ({app})", one_line(from, 60)),
        message,
    }
}

fn one_line(raw: &str, max: usize) -> String {
    let flat = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        let cut: String = flat.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_meeting_rings_inside_its_lead_time_and_never_after_it_starts() {
        let start = 10_000;
        assert!(!meeting_due(start, start - 301, 5));
        assert!(meeting_due(start, start - 300, 5));
        assert!(meeting_due(start, start - 1, 5));
        assert!(!meeting_due(start, start, 5), "too late once it has begun");
    }

    #[test]
    fn only_a_reminder_due_just_now_rings_never_a_backlog() {
        assert!(!reminder_due(1_000, 999));
        assert!(reminder_due(1_000, 1_000));
        assert!(reminder_due(1_000, 1_000 + REMINDER_WINDOW - 1));
        assert!(!reminder_due(1_000, 1_000 + REMINDER_WINDOW), "overdue reminders do not all go off at once");
    }

    #[test]
    fn people_match_on_whole_names_only() {
        let people = vec!["Ghazal".to_string(), "Amma".to_string()];
        assert_eq!(matching_person("Ghazal Aghvami", &people), Some(&people[0]));
        assert_eq!(matching_person("amma in Family", &people), Some(&people[1]));
        assert_eq!(matching_person("Rammakant", &people), None, "a name inside another word");
        assert_eq!(matching_person("Someone else", &people), None);
    }

    #[test]
    fn settings_drop_names_too_short_to_match_safely_and_duplicates() {
        let s = CallSettings {
            people: vec!["Al".into(), " Ravi ".into(), "ravi".into(), "".into()],
            meeting_lead: 0,
            ..CallSettings::default()
        }
        .sanitised();
        assert_eq!(s.people, vec!["Ravi".to_string()]);
        assert_eq!(s.meeting_lead, 1);
    }

    #[test]
    fn alerts_say_what_and_when() {
        let m = meeting_call("Sprint planning", 10_000, "15:30", Some("Teams"), 10_000 - 290);
        assert_eq!(m.title, "Meeting in 5 min");
        assert_eq!(m.message, "Sprint planning at 3:30 pm. Please join. Where: Teams");
        let msg = message_call(7, "Teams", "Ghazal Aghvami", "can you check\nthe PR", true);
        assert_eq!(msg.title, "Message from Ghazal Aghvami (Teams)");
        assert_eq!(msg.message, "can you check the PR");
        assert_eq!(message_call(7, "WhatsApp", "Amma", "secret", false).message, "Open WhatsApp to read it.");
        assert_eq!(reminder_call(3, 99, "Call Amma").key, "reminder:3:99");
        assert_eq!(twelve_hour("00:05"), "12:05 am");
        assert_eq!(twelve_hour("12:00"), "12:00 pm");
    }
}
