//! Loud phone alerts over ntfy, without a phone call.
//!
//! # Why this exists
//!
//! The user wanted their Android phone to ring when a watched item comes back
//! in stock. A real call needs a telephony service (Twilio and the like), which
//! costs money, runs out of trial credit, and hands the user's number to a
//! third party on every call. ntfy does the job the call was really for, which
//! is "get my attention now": the user's phone fires a high-priority
//! notification with an alarm sound, free and with no account.
//!
//! # How it works
//!
//! The user installs the ntfy Android app and subscribes to a topic, which is
//! any string they choose. NEXUS POSTs the alert to `https://ntfy.sh/<topic>`
//! and the phone goes off. The topic is the only secret: anyone who knows it
//! can publish to it, so an unguessable one keeps strangers from pinging the
//! phone. That is why the topic is validated to a tight character set here and
//! never interpolated into anything but the single path segment.
//!
//! # What it does not do
//!
//! Read the user's topics. The one read path is `poll_replies`, of the private
//! reply topic NEXUS generates for phone approvals. And it is not a phone call, so a phone with no data
//! connection will not be reached; a stock ping tolerates that, which is the
//! whole reason this was the right trade over a paid call.

use super::http::{send, HttpError, Request};

/// The server NEXUS publishes to. The public ntfy instance, which needs no
/// account. Kept as a constant so a self-hosted instance is a one-line change,
/// not a scattered edit.
const NTFY_BASE: &str = "https://ntfy.sh";

/// Longest topic ntfy accepts, and a sane cap regardless.
const MAX_TOPIC: usize = 64;

/// A topic cleaned to exactly what may sit in a URL path segment, or `None`.
///
/// ntfy topics are `[-_A-Za-z0-9]`. Anything else is refused rather than
/// escaped, because a topic is pasted straight after `ntfy.sh/` and a `/`, a
/// `?`, or a space in it would change which URL is hit. Refusing at the door
/// means the stored value is always a value the POST can trust.
pub fn valid_topic(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() || t.len() > MAX_TOPIC {
        return None;
    }
    if !t
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    Some(t.to_string())
}

/// Publish a loud alert to the user's topic.
///
/// `Priority: urgent` is ntfy's max level, the one that makes Android play a
/// sound and pop a heads-up notification rather than sit silently in the
/// shade. The emoji tag is cosmetic but makes the alert recognisable at a
/// glance. The message is the body; the title is a header, which ntfy requires
/// be a single line, so a newline in the title would be rejected by the
/// transport. The body may be multi-line in ntfy, but the curl-config
/// transport refuses line breaks in any value, so both are flattened to spaces
/// here rather than failing a send over formatting.
pub fn send_alert(topic: &str, title: &str, message: &str) -> Result<(), String> {
    let topic = valid_topic(topic).ok_or_else(|| {
        "That ntfy topic has characters a topic cannot. Use letters, numbers, - and _ only."
            .to_string()
    })?;

    let url = format!("{NTFY_BASE}/{topic}");
    let request = Request::post(&url, flatten(message))
        .with_header("Title", &flatten(title))
        .with_header("Priority", "urgent")
        .with_header("Tags", "rotating_light");

    match send(request) {
        Ok(resp) if resp.ok() => {
            log::info!("ntfy: published alert to topic");
            Ok(())
        }
        Ok(resp) => {
            log::warn!("ntfy: server returned {}", resp.status);
            Err(format!(
                "ntfy rejected the alert ({}). Check the topic name is right.",
                resp.status
            ))
        }
        Err(HttpError::Unreachable { detail }) => {
            log::warn!("ntfy: unreachable: {detail}");
            Err(format!("Could not reach ntfy: {detail}"))
        }
        Err(e) => {
            log::warn!("ntfy: {e}");
            Err(e.to_string())
        }
    }
}

/// A confirmation, at normal priority: the phone shows it without the alarm
/// an alert or an approval request makes.
pub fn send_note(topic: &str, title: &str, message: &str) -> Result<(), String> {
    let topic = valid_topic(topic).ok_or("The ntfy topic is not valid.")?;
    let request = Request::post(&format!("{NTFY_BASE}/{topic}"), flatten(message))
        .with_header("Title", &flatten(title))
        .with_header("Priority", "default")
        .with_header("Tags", "white_check_mark");
    match send(request) {
        Ok(resp) if resp.ok() => Ok(()),
        Ok(resp) => Err(format!("ntfy rejected the note ({}).", resp.status)),
        Err(e) => Err(e.to_string()),
    }
}

/// A call-style alert: urgent, with the phone icon, sent as JSON so the title
/// can be any text (a header could not safely carry every name).
pub fn send_call(topic: &str, title: &str, message: &str) -> Result<(), String> {
    let topic = valid_topic(topic).ok_or("The call topic is not valid.")?;
    let body = serde_json::json!({
        "topic": topic,
        "title": flatten(title),
        "message": flatten(message),
        "priority": 5,
        "tags": ["telephone_receiver"],
    })
    .to_string();
    let resp = send(Request::post(NTFY_BASE, body).with_header("Content-Type", "application/json"))
        .map_err(|e| e.to_string())?;
    if resp.ok() {
        Ok(())
    } else {
        Err(format!("ntfy rejected the alert ({}).", resp.status))
    }
}

/// A button on a notification. Tapping it makes the ntfy app POST `body` to
/// the reply topic, which is how an approval travels back from the phone.
pub struct Button<'a> {
    pub label: &'a str,
    pub body: String,
}

/// The JSON for a notification with buttons. Separate from the send so the
/// shape the phone receives is testable without the network.
pub fn actions_payload(
    topic: &str,
    title: &str,
    message: &str,
    buttons: &[Button<'_>],
    reply_topic: &str,
) -> serde_json::Value {
    let actions: Vec<serde_json::Value> = buttons
        .iter()
        .map(|b| {
            serde_json::json!({
                "action": "http",
                "label": b.label,
                "url": format!("{NTFY_BASE}/{reply_topic}"),
                "method": "POST",
                "body": b.body,
                // The notification disappears once answered, so a second tap
                // cannot send a second reply.
                "clear": true,
            })
        })
        .collect();
    serde_json::json!({
        "topic": topic,
        "title": flatten(title),
        "message": flatten(message),
        "priority": 5,
        "tags": ["question"],
        "actions": actions,
    })
}

/// Publish a notification with buttons to the user's topic.
pub fn publish_with_buttons(
    topic: &str,
    title: &str,
    message: &str,
    buttons: &[Button<'_>],
    reply_topic: &str,
) -> Result<(), String> {
    let topic = valid_topic(topic).ok_or("The ntfy topic is not valid.")?;
    let reply = valid_topic(reply_topic).ok_or("The reply topic is not valid.")?;
    let body = actions_payload(&topic, title, message, buttons, &reply).to_string();
    let resp = send(Request::post(NTFY_BASE, body).with_header("Content-Type", "application/json"))
        .map_err(|e| e.to_string())?;
    if resp.ok() {
        Ok(())
    } else {
        Err(format!("ntfy rejected the approval request ({}).", resp.status))
    }
}

/// One message read back from the reply topic.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub id: String,
    pub message: String,
}

/// Read the private reply topic, where the phone's button taps land.
///
/// The one read this module does, and only ever of the reply topic NEXUS
/// generated for itself: never the user's alert topic, so nothing the user
/// publishes there is read back. `since` is an ntfy message id or a duration
/// such as "10m".
pub fn poll_replies(reply_topic: &str, since: &str) -> Result<Vec<Reply>, String> {
    let reply = valid_topic(reply_topic).ok_or("The reply topic is not valid.")?;
    let since: String = since
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(32)
        .collect();
    let url = format!("{NTFY_BASE}/{reply}/json?poll=1&since={since}");
    let resp = send(Request::get(&url)).map_err(|e| e.to_string())?;
    if !resp.ok() {
        return Err(format!("ntfy returned {} reading replies.", resp.status));
    }
    Ok(parse_replies(&resp.body))
}

/// ntfy answers a poll with one JSON object per line; only `message` events
/// carry a reply (others are keepalives and opens).
fn parse_replies(body: &str) -> Vec<Reply> {
    body.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v.get("event").and_then(|e| e.as_str()) == Some("message"))
        .filter_map(|v| {
            Some(Reply {
                id: v.get("id")?.as_str()?.to_string(),
                message: v.get("message")?.as_str()?.trim().to_string(),
            })
        })
        .collect()
}

/// Collapse line breaks to spaces so a value never breaks the transport.
fn flatten(raw: &str) -> String {
    raw.replace(['\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topics_are_restricted_to_url_safe_characters() {
        for good in ["rohit-stock", "nexus_alerts_9f2a", "ABC123"] {
            assert_eq!(valid_topic(good).as_deref(), Some(good), "{good}");
        }
        // A trailing path, a query, or a space would change the URL that gets
        // hit, so each is refused rather than quietly sent to the wrong place.
        for bad in [
            "a/b",
            "topic?x=1",
            "has space",
            "emoji😀",
            "two\nlines",
            "",
            "   ",
        ] {
            assert!(valid_topic(bad).is_none(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn a_topic_longer_than_the_limit_is_refused() {
        let long: String = std::iter::repeat('a').take(MAX_TOPIC + 1).collect();
        assert!(valid_topic(&long).is_none());
        let at_limit: String = std::iter::repeat('a').take(MAX_TOPIC).collect();
        assert!(valid_topic(&at_limit).is_some());
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        assert_eq!(valid_topic("  my-topic  ").as_deref(), Some("my-topic"));
    }

    #[test]
    fn line_breaks_are_flattened_so_a_send_never_fails_on_formatting() {
        assert_eq!(flatten("line one\nline two"), "line one line two");
        assert_eq!(flatten("a\r\nb"), "a  b");
    }

    #[test]
    fn the_only_read_is_of_the_private_reply_topic() {
        // Originally publish-only. Phone approvals need exactly one read: the
        // reply topic NEXUS generates for itself, where button taps land. A
        // second GET (say, reading the user's alert topic back) must trip this.
        let production = include_str!("ntfy_connector.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("the file must keep its test module marker");
        assert_eq!(production.matches("Request::get").count(), 1, "one read path only");
        let reader = production
            .split_once("pub fn poll_replies")
            .map(|(_, rest)| rest)
            .expect("the reader");
        let body = &reader[..reader.find("\n}").expect("end")];
        assert!(body.contains("Request::get"), "the one GET lives in poll_replies");
        assert!(production.contains("Request::post"));
    }

    #[test]
    fn approval_buttons_post_back_to_the_reply_topic_and_clear() {
        let v = actions_payload(
            "alerts-abc",
            "NEXUS needs approval",
            "Send WhatsApp\nto Rohit",
            &[
                Button { label: "Approve", body: "approve 00ff".into() },
                Button { label: "Deny", body: "deny 00ff".into() },
            ],
            "nexus-reply-123",
        );
        assert_eq!(v["topic"], "alerts-abc");
        assert_eq!(v["message"], "Send WhatsApp to Rohit", "line breaks flattened");
        let a = &v["actions"][0];
        assert_eq!(a["action"], "http");
        assert_eq!(a["method"], "POST");
        assert_eq!(a["url"], "https://ntfy.sh/nexus-reply-123");
        assert_eq!(a["body"], "approve 00ff");
        assert_eq!(a["clear"], true);
        assert_eq!(v["actions"][1]["label"], "Deny");
    }

    #[test]
    fn only_message_events_are_read_as_replies() {
        let body = r#"{"id":"a1","event":"open","topic":"t"}
{"id":"b2","time":1,"event":"message","topic":"t","message":" approve 00ff "}
{"id":"c3","event":"keepalive"}
not json"#;
        assert_eq!(
            parse_replies(body),
            vec![Reply { id: "b2".into(), message: "approve 00ff".into() }]
        );
    }
}
