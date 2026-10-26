//! Approving NEXUS's actions from the phone.
//!
//! # It is the same approval, delivered elsewhere
//!
//! NEXUS already refuses to run a write action until the user confirms it: the
//! gate issues a single-use token bound to the exact action and input, which
//! expires in minutes and never touches disk (`approval`). A phone approval
//! does not add a second gate. It forwards that same request to the phone, and
//! an Approve tap redeems that same token, so every existing check (single use,
//! same action, same details, expiry) still decides.
//!
//! # How a tap gets back, and why it cannot be forged
//!
//! The request goes to an approval topic with Approve and Deny buttons. A tap
//! makes the ntfy app POST "approve <code>" to a second, private reply topic
//! that only NEXUS reads, so the Mac is never exposed to the internet. Each
//! request carries a fresh 128-bit one-time code from the OS random source; a
//! reply is acted on only if its code matches a request NEXUS is still waiting
//! on, and the code is spent on first use.
//!
//! On ntfy.sh anyone who knows a topic's name can read it, and the request
//! (with its code) is on the approval topic. So the approval topic is never
//! the user's stock-alert topic, a name a person chose and has likely shown
//! somewhere: NEXUS generates both topics with 80 random bits each, which
//! cannot be guessed. The security is exactly the secrecy of those two names,
//! plus single use and expiry; it is suited to one person's approvals, not to
//! anything that must survive the names leaking.
//!
//! # What is not here
//!
//! Starting work from the phone. A command channel secured only by a topic
//! name is not strong enough to let someone run Claude Code on a repository;
//! Claude Code's own Remote Control, which uses the Claude account's sign-in,
//! is the authenticated way to drive work from the phone.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::action::ActionRequest;

/// What an Approve tap does.
#[derive(Debug, Clone)]
pub enum Pending {
    /// Re-run a gated action with its approval token.
    Action { request: ActionRequest },
    /// Apply a finished clone run's diff to the working copy.
    CloneApply { run_id: i64 },
    /// The settings screen's round-trip check; runs nothing.
    Test,
}

#[derive(Debug, Clone)]
struct Entry {
    pending: Pending,
    summary: String,
    issued_at: Instant,
    ttl: Duration,
}

/// Requests waiting on the phone, by one-time code. In memory only, like the
/// approval store: nothing pre-approved survives a restart.
static WAITING: Mutex<Option<HashMap<String, Entry>>> = Mutex::new(None);

/// A fresh one-time code: 16 bytes from the OS random source, as hex.
pub fn new_code() -> Result<String, String> {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|e| format!("No random source: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Wait for a phone answer to `pending`, for at most `ttl`. Returns the code
/// to put on the buttons.
pub fn register(pending: Pending, summary: &str, ttl: Duration) -> Result<String, String> {
    let code = new_code()?;
    let mut guard = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    map.retain(|_, e| e.issued_at.elapsed() < e.ttl);
    // Bounded, like the approval store: a loop that keeps asking and never
    // gets answered cannot grow this without limit.
    if map.len() >= 32 {
        if let Some(oldest) = map.iter().min_by_key(|(_, e)| e.issued_at).map(|(k, _)| k.clone()) {
            map.remove(&oldest);
        }
    }
    map.insert(
        code.clone(),
        Entry {
            pending,
            summary: summary.to_string(),
            issued_at: Instant::now(),
            ttl,
        },
    );
    Ok(code)
}

/// Take a waiting request by its code. Single use: it is removed whatever the
/// verdict, so a replayed reply finds nothing. `None` if unknown or expired.
pub fn take(code: &str) -> Option<(Pending, String)> {
    let mut guard = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    let entry = guard.as_mut()?.remove(code)?;
    (entry.issued_at.elapsed() < entry.ttl).then_some((entry.pending, entry.summary))
}

/// Whether anything is waiting on the phone. The poller only reads the reply
/// topic when this is true, so an idle NEXUS makes no requests.
pub fn any_waiting() -> bool {
    let mut guard = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    match guard.as_mut() {
        Some(map) => {
            map.retain(|_, e| e.issued_at.elapsed() < e.ttl);
            !map.is_empty()
        }
        None => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Approve,
    Deny,
}

/// Read a reply: exactly "approve <code>" or "deny <code>", the code being 32
/// hex characters. Anything else is ignored rather than guessed at.
pub fn parse_reply(message: &str) -> Option<(Verdict, String)> {
    let mut parts = message.split_whitespace();
    let verdict = match parts.next()?.to_lowercase().as_str() {
        "approve" => Verdict::Approve,
        "deny" => Verdict::Deny,
        _ => return None,
    };
    let code = parts.next()?;
    if parts.next().is_some() || code.len() != 32 || !code.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some((verdict, code.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action() -> Pending {
        Pending::Action {
            request: ActionRequest {
                action_id: "whatsapp.send".into(),
                input: serde_json::json!({"to": "me"}),
                approval: Some(7),
            },
        }
    }

    #[test]
    fn codes_are_fresh_and_long() {
        let a = new_code().unwrap();
        let b = new_code().unwrap();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn a_code_is_spent_on_first_use() {
        let code = register(action(), "Send WhatsApp", Duration::from_secs(60)).unwrap();
        let (pending, summary) = take(&code).expect("first use");
        match pending {
            Pending::Action { request } => {
                assert_eq!(request.action_id, "whatsapp.send");
                assert_eq!(request.approval, Some(7));
            }
            other => panic!("wrong request: {other:?}"),
        }
        assert_eq!(summary, "Send WhatsApp");
        assert!(take(&code).is_none(), "a replayed approval must find nothing");
    }

    #[test]
    fn an_expired_request_cannot_be_approved() {
        let code = register(action(), "x", Duration::from_millis(0)).unwrap();
        std::thread::sleep(Duration::from_millis(5));
        assert!(take(&code).is_none());
    }

    #[test]
    fn an_unknown_code_approves_nothing() {
        assert!(take(&"0".repeat(32)).is_none());
    }

    /// Live, ignored by default: the round trip against real ntfy.sh through
    /// NEXUS's own publish, read and parse code, with the phone's tap
    /// simulated exactly as the ntfy app sends it.
    /// `cargo test --lib phone_approval::tests::live_round_trip -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_round_trip() {
        use crate::assistant::http::{send, Request};
        use crate::assistant::ntfy_connector::{poll_replies, publish_with_buttons, Button};
        let salt = &new_code().unwrap()[..12];
        let (alert, reply) = (format!("nexus-test-a-{salt}"), format!("nexus-test-r-{salt}"));
        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_string();

        let code = register(Pending::Test, "round trip", Duration::from_secs(120)).unwrap();
        let buttons = [
            Button { label: "Approve", body: format!("approve {code}") },
            Button { label: "Deny", body: format!("deny {code}") },
        ];
        publish_with_buttons(&alert, "NEXUS test", "round trip", &buttons, &reply).expect("publish");

        // What the ntfy app does when Approve is tapped.
        let tap = send(Request::post(&format!("https://ntfy.sh/{reply}"), format!("approve {code}")))
            .expect("tap");
        assert!(tap.ok());

        // ntfy stores a message a moment after accepting it (measured: none
        // on an immediate read, present 2s later), so read the way the app's
        // reader does: again a little later, without losing the cursor.
        let mut replies = Vec::new();
        for _ in 0..8 {
            replies = poll_replies(&reply, &started).expect("poll");
            if !replies.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        println!("replies: {replies:?}");
        let (verdict, got) = parse_reply(&replies.last().expect("a reply").message).expect("parse");
        assert_eq!(verdict, Verdict::Approve);
        assert!(matches!(take(&got), Some((Pending::Test, _))), "the code redeems once");
        assert!(take(&got).is_none(), "and never twice");
        println!("round trip OK");
    }

    #[test]
    fn replies_are_read_strictly() {
        let code = "00112233445566778899aabbccddeeff";
        assert_eq!(parse_reply(&format!("approve {code}")), Some((Verdict::Approve, code.into())));
        assert_eq!(parse_reply(&format!("  DENY   {code} ")), Some((Verdict::Deny, code.into())));
        for bad in [
            "approve",
            "approve 123",
            &format!("approve {code} extra"),
            &format!("yes {code}"),
            &format!("approve {}", "z".repeat(32)),
            "",
        ] {
            assert_eq!(parse_reply(bad), None, "{bad:?}");
        }
    }
}
