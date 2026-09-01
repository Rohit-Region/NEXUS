//! NEXUS-022: WhatsApp, within what is actually supported.
//!
//! **There is no personal-account API, and this milestone does not pretend
//! otherwise.** The position, stated plainly because the alternative is a
//! feature that gets someone's number banned:
//!
//! - The WhatsApp Business Platform needs a business account and a registered
//!   business number, and restricts business-initiated messages to approved
//!   templates outside a 24-hour window. It does not reach personal chats.
//! - Library-based automation of a personal account works by impersonating
//!   the web client. It violates the terms of service and risks the number
//!   being banned. NEXUS will not do it.
//! - There is no local surface either. WhatsApp is not installed on this Mac,
//!   and even where it is, it exposes no scripting dictionary.
//!
//! What *is* supported is the `whatsapp://send` URL scheme. NEXUS drafts the
//! message, the user approves the wording, WhatsApp opens with it already in
//! the box, and the user presses send. NEXUS never sends.
//!
//! **Reading incoming messages is NOT SUPPORTED and is not deferred.** No
//! action here claims to do it, because there is no supported mechanism to
//! implement later. Anything that offered it would be lying.

use rusqlite::Connection;
use serde::Deserialize;

use super::action::{ActionError, ActionSpec};
use super::connector::{
    Capabilities, Connector, ConnectorStatus, ExecCtx, FollowUp, ReferentDraft, Remedy,
    UnavailableAction,
};
use super::permission::{ConfirmPolicy, Permission, Reach};
use super::referent::ReferentKind;
use super::shell::{run, DEFAULT_TIMEOUT};

pub const CONNECTOR_ID: &str = "whatsapp";

const APP_PATH: &str = "/Applications/WhatsApp.app";
/// Beyond this a URL becomes unreliable, and it is not a chat message anyway.
const MAX_MESSAGE: usize = 1_000;

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        id: "whatsapp.status",
        connector_id: CONNECTOR_ID,
        summary: "Check what NEXUS can do with WhatsApp",
        permission: Permission::Read,
        confirm: ConfirmPolicy::Never,
        reach: Reach::LocalOnly,
        reversible: true,
    },
    ActionSpec {
        // Presses Return in the chat WhatsApp already has open, which is the
        // one the user is looking at. Separate from compose so the draft is
        // on screen, and the person it is addressed to is visible, before
        // anything is sent.
        id: "whatsapp.press_send",
        connector_id: CONNECTOR_ID,
        summary: "Send the message showing in WhatsApp",
        permission: Permission::Write,
        confirm: ConfirmPolicy::Always,
        reach: Reach::LocalOnly,
        // Nothing here can unsend it.
        reversible: false,
    },
    ActionSpec {
        id: "whatsapp.compose_message",
        connector_id: CONNECTOR_ID,
        // The summary says what actually happens. "Send a WhatsApp message"
        // would be a promise NEXUS cannot keep.
        summary: "Open WhatsApp with a message ready for you to send",
        permission: Permission::Write,
        confirm: ConfirmPolicy::Always,
        reach: Reach::LocalOnly,
        reversible: true,
    },
];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ComposeInput {
    /// International format. WhatsApp identifies people by number, not name.
    phone: String,
    message: String,
    /// Optional, only so the approval prompt and the referent can say who
    /// this is. Never sent anywhere.
    #[serde(default)]
    display_name: Option<String>,
}

/// Digits only, optionally introduced by `+`, in international form.
///
/// Validated because it goes into a URL, and because a malformed number
/// silently opens a chat with the wrong person, which is worse than an error.
pub fn valid_phone(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-' && *c != '(' && *c != ')')
        .collect();
    let digits = cleaned.strip_prefix('+').unwrap_or(&cleaned);
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    // E.164 allows up to 15 digits; fewer than 8 is not an international
    // number and is more likely a mistyped extension.
    if digits.len() < 8 || digits.len() > 15 {
        return None;
    }
    Some(digits.to_string())
}

fn encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() * 3);
    for byte in raw.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn check_message(raw: &str) -> Result<String, ActionError> {
    let text = raw.trim();
    if text.is_empty() {
        return Err(ActionError::InvalidInput {
            detail: "There was no message to write.".to_string(),
        });
    }
    if text.chars().count() > MAX_MESSAGE {
        return Err(ActionError::InvalidInput {
            detail: format!("That message is longer than {MAX_MESSAGE} characters."),
        });
    }
    Ok(text.to_string())
}

fn installed() -> bool {
    std::path::Path::new(APP_PATH).exists()
}

/// Press Return in WhatsApp, and only in WhatsApp.
///
/// The activation, the focus check and the keystroke are one script on
/// purpose. Split across two, another window can take focus in between and
/// the message is typed into whatever arrived instead. That failure is
/// silent and lands the user's words somewhere they never chose, which is
/// the whole reason synthetic input is worth being careful with.
///
/// **It activates WhatsApp first, and that is a correction.** The check
/// alone refused whenever anything else was in front, which sounds careful
/// until you notice what is always in front: NEXUS. The user has just spoken
/// to the assistant panel, so the assistant window has focus by definition,
/// and the action could not succeed from the path it exists to serve. Two of
/// the three recorded attempts failed for exactly that reason. Refusing to
/// raise the window did not make the keystroke safer, it made it
/// unreachable.
///
/// The check is kept, and kept *after* the activation, so it still answers
/// the question that matters: is the thing about to receive a Return really
/// WhatsApp. Activation that silently fails now reports rather than types.
///
/// Return rather than a click on the send button: locating the button needs
/// the accessibility tree, and a wrong hit there presses something else.
const PRESS_SEND: [&str; 9] = [
    "tell application \"WhatsApp\" to activate",
    // Raising a window is not instant. Without the wait the frontmost check
    // reads the app that is on its way out and refuses a send that would
    // have been fine a moment later.
    "delay 0.4",
    "tell application \"System Events\"",
    "set n to name of first application process whose frontmost is true",
    "if n is not \"WhatsApp\" then return \"not-front:\" & n",
    "key code 36",
    "end tell",
    "delay 0.1",
    "return \"sent\"",
];

fn press_send() -> Result<(), ActionError> {
    let out = crate::assistant::shell::osascript(&PRESS_SEND, &[]).map_err(|e| {
        ActionError::Failed {
            detail: format!("Could not reach WhatsApp: {e}"),
        }
    })?;

    if !out.success {
        // The usual cause, and the one with a remedy the user can act on.
        let detail = if out.stderr.contains("not allowed") || out.stderr.contains("1002") {
            "NEXUS is not allowed to send keystrokes. Turn it on in System Settings \
             > Privacy & Security > Accessibility, then try again."
                .to_string()
        } else {
            format!("WhatsApp refused: {}", out.stderr.trim())
        };
        return Err(ActionError::Failed { detail });
    }

    match out.stdout.trim() {
        "sent" => Ok(()),
        other => Err(ActionError::Failed {
            detail: format!(
                "{} was in front instead of WhatsApp, so nothing was sent. \
                 Bring WhatsApp to the front and say it again.",
                other.strip_prefix("not-front:").unwrap_or(other)
            ),
        }),
    }
}

pub struct WhatsappConnector;

impl Connector for WhatsappConnector {
    fn id(&self) -> &'static str {
        CONNECTOR_ID
    }

    fn display_name(&self) -> &'static str {
        "WhatsApp"
    }

    fn actions(&self) -> &'static [ActionSpec] {
        ACTIONS
    }

    fn capabilities(&self, _conn: &Connection) -> Capabilities {
        if installed() {
            Capabilities {
                available: ACTIONS.iter().map(|s| s.id.to_string()).collect(),
                unavailable: Vec::new(),
            }
        } else {
            Capabilities {
                available: vec!["whatsapp.status".to_string()],
                unavailable: vec![UnavailableAction {
                    action_id: "whatsapp.compose_message".to_string(),
                    reason: "WhatsApp is not installed on this Mac.".to_string(),
                }],
            }
        }
    }

    fn status(&self, _conn: &Connection) -> ConnectorStatus {
        if installed() {
            // Degraded, permanently and honestly: half of what a messaging
            // connector should do cannot be built at all.
            ConnectorStatus::Degraded
        } else {
            ConnectorStatus::Unavailable
        }
    }

    fn summarize(&self, action_id: &str, input: &serde_json::Value, _conn: &Connection) -> String {
        match action_id {
            "whatsapp.compose_message" => {
                match serde_json::from_value::<ComposeInput>(input.clone()) {
                    Ok(c) => {
                        let who = c.display_name.clone().unwrap_or_else(|| c.phone.clone());
                        format!(
                            "Open WhatsApp to {who} with this ready to send: \"{}\"",
                            c.message.chars().take(140).collect::<String>()
                        )
                    }
                    Err(_) => "Open WhatsApp with a message ready".to_string(),
                }
            }
            other => ACTIONS
                .iter()
                .find(|s| s.id == other)
                .map(|s| s.summary.to_string())
                .unwrap_or_else(|| other.to_string()),
        }
    }

    fn validate_input(
        &self,
        action_id: &str,
        input: &serde_json::Value,
    ) -> Result<(), ActionError> {
        if action_id != "whatsapp.compose_message" {
            return Ok(());
        }
        let target: ComposeInput =
            serde_json::from_value(input.clone()).map_err(|e| ActionError::InvalidInput {
                detail: e.to_string(),
            })?;
        valid_phone(&target.phone).ok_or_else(|| ActionError::InvalidInput {
            detail: format!("\"{}\" is not an international phone number.", target.phone),
        })?;
        check_message(&target.message).map(|_| ())
    }

    fn observe(
        &self,
        action_id: &str,
        input: &serde_json::Value,
        _output: &serde_json::Value,
        _conn: &Connection,
    ) -> Vec<ReferentDraft> {
        if action_id != "whatsapp.compose_message" {
            return Vec::new();
        }
        serde_json::from_value::<ComposeInput>(input.clone())
            .ok()
            .and_then(|c| {
                valid_phone(&c.phone).map(|phone| ReferentDraft {
                    kind: ReferentKind::Person,
                    display_name: c.display_name.unwrap_or_else(|| format!("+{phone}")),
                    metadata: serde_json::json!({ "phone": phone }),
                })
            })
            .into_iter()
            .collect()
    }

    fn describe_result(&self, action_id: &str, output: &serde_json::Value) -> Option<String> {
        match action_id {
            "whatsapp.status" => Some(if output.get("installed")?.as_bool().unwrap_or(false) {
                "WhatsApp is installed. NEXUS can draft a message for you to send, \
                 but it cannot read your conversations."
                    .to_string()
            } else {
                // A dead end here is a false negative: the desktop app is
                // not the only WhatsApp on this machine, and saying only
                // that it is missing hides the route that does work.
                // A bare "not installed" is a false dead end: the desktop
                // app is not the only WhatsApp on this machine. Chrome is
                // reached through the browser connector, not from here, so
                // this points at it rather than automating it.
                "The WhatsApp desktop app is not installed on this Mac. \
                 WhatsApp Web works in Chrome: say \"go to WhatsApp\" and I \
                 will switch to that tab."
                    .to_string()
            }),
            "whatsapp.compose_message" => Some(
                "WhatsApp is open with your message ready. Check it is the right chat."
                    .to_string(),
            ),
            "whatsapp.press_send" => Some("Sent.".to_string()),
            _ => None,
        }
    }

    fn zero_input_actions(&self) -> &'static [&'static str] {
        // press_send needs no arguments, so "send it" reaches it directly.
        // It is still gated: ConfirmPolicy::Always means the user confirms
        // before anything leaves.
        &["whatsapp.status", "whatsapp.press_send"]
    }

    /// A draft on screen is a question, and "yes" is an answer to it.
    ///
    /// Only from `compose_message`: `press_send` itself offers nothing,
    /// because the message is already gone and there is nothing left to
    /// agree to.
    /// The Accessibility grant, which is the only failure here with a fix
    /// NEXUS can point at. A network problem or a chat that moved has no
    /// remedy worth offering, and offering one anyway spends the user's
    /// attention on something that will not work.
    fn remedy(&self, _action_id: &str, error: &ActionError) -> Option<Remedy> {
        let detail = match error {
            ActionError::Failed { detail } => detail,
            _ => return None,
        };
        detail.contains("not allowed to send keystrokes").then(|| Remedy {
            prompt: "NEXUS is not allowed to send keystrokes. Shall I open \
                     Accessibility settings?"
                .to_string(),
            action_id: "system.open_settings_pane",
            input: serde_json::json!({ "pane": "accessibility" }),
        })
    }

    fn follow_up(
        &self,
        action_id: &str,
        _input: &serde_json::Value,
        _output: &serde_json::Value,
    ) -> Option<FollowUp> {
        (action_id == "whatsapp.compose_message").then_some(FollowUp {
            action_id: "whatsapp.press_send",
            // Presses Return in the chat already on screen, so there is
            // nothing to carry.
            input: serde_json::Value::Null,
        })
    }

    fn dispatch(
        &self,
        action_id: &str,
        input: serde_json::Value,
        _ctx: &ExecCtx<'_>,
    ) -> Result<serde_json::Value, ActionError> {
        match action_id {
            "whatsapp.status" => Ok(serde_json::json!({
                "installed": installed(),
                "canDraft": installed(),
                // Stated in the payload, not just the docs, so any surface
                // that reads it has to confront the limitation.
                "canRead": false,
                "canSendAutomatically": false,
                "limitation":
                    "WhatsApp has no personal-account API. NEXUS can draft a message and \
                     hand it to WhatsApp for you to send, but it cannot read your \
                     conversations, and no supported mechanism exists to add that."
            })),

            "whatsapp.press_send" => {
                if !installed() {
                    return Err(ActionError::Failed {
                        detail: "WhatsApp is not installed on this Mac.".to_string(),
                    });
                }
                press_send()?;
                Ok(serde_json::json!({ "sent": true }))
            }
            "whatsapp.compose_message" => {
                let target: ComposeInput =
                    serde_json::from_value(input).map_err(|e| ActionError::InvalidInput {
                        detail: e.to_string(),
                    })?;
                let phone =
                    valid_phone(&target.phone).ok_or_else(|| ActionError::InvalidInput {
                        detail: format!(
                            "\"{}\" is not an international phone number.",
                            target.phone
                        ),
                    })?;
                let message = check_message(&target.message)?;

                if !installed() {
                    return Err(ActionError::Failed {
                        detail: "WhatsApp is not installed on this Mac.".to_string(),
                    });
                }

                let url = format!(
                    "whatsapp://send?phone={}&text={}",
                    encode(&phone),
                    encode(&message)
                );
                let out = run("/usr/bin/open", &[&url], DEFAULT_TIMEOUT).map_err(|e| {
                    ActionError::Failed {
                        detail: e.to_string(),
                    }
                })?;
                if !out.success {
                    return Err(ActionError::Failed {
                        detail: "WhatsApp would not open.".to_string(),
                    });
                }

                Ok(serde_json::json!({
                    "handedOff": true,
                    "to": phone,
                    "message": message,
                    // NEXUS did not send this. Saying so in the payload keeps
                    // every caller honest about what happened.
                    "sent": false
                }))
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

    #[test]
    fn a_draft_offers_the_send_that_follows_it() {
        let connector = WhatsappConnector;
        let nothing = serde_json::Value::Null;
        assert_eq!(
            connector
                .follow_up("whatsapp.compose_message", &nothing, &nothing)
                .map(|f| f.action_id),
            Some("whatsapp.press_send"),
            "a draft on screen is a question, and \"yes\" must have somewhere to go"
        );
        // Nothing left to agree to once it has gone.
        assert!(connector
            .follow_up("whatsapp.press_send", &nothing, &nothing)
            .is_none());
        assert!(connector
            .follow_up("whatsapp.status", &nothing, &nothing)
            .is_none());
    }

    #[test]
    fn the_send_script_raises_whatsapp_before_it_checks_what_is_in_front() {
        // The defect: the check alone refused whenever anything else had
        // focus, and after the user speaks to the assistant the thing with
        // focus is always NEXUS. Two of three recorded attempts failed on
        // exactly that. Activating first is what makes the action reachable
        // from the path it exists to serve.
        let activate = PRESS_SEND
            .iter()
            .position(|line| line.contains("activate"))
            .expect("the script must raise WhatsApp");
        let check = PRESS_SEND
            .iter()
            .position(|line| line.contains("frontmost"))
            .expect("the script must still check what is in front");
        let keystroke = PRESS_SEND
            .iter()
            .position(|line| line.contains("key code 36"))
            .expect("the script must press Return");

        assert!(activate < check, "raising it before the check is the fix");
        assert!(
            check < keystroke,
            "the check must still gate the keystroke, or a Return lands in \
             whatever happens to be in front"
        );
        assert!(
            PRESS_SEND.iter().any(|line| line.contains("delay")),
            "raising a window is not instant; without the wait the check \
             reads the app on its way out"
        );
    }

    #[test]
    fn international_numbers_are_accepted_in_the_shapes_people_write_them() {
        for raw in [
            "+919876543210",
            "919876543210",
            "+91 98765 43210",
            "+1 (415) 555-0100",
        ] {
            assert!(valid_phone(raw).is_some(), "{raw}");
        }
        assert_eq!(
            valid_phone("+91 98765 43210").as_deref(),
            Some("919876543210")
        );
    }

    #[test]
    fn anything_that_is_not_a_number_is_refused() {
        // A malformed number opens a chat with the wrong person, which is
        // worse than an error.
        for hostile in [
            "",
            "12345",
            "alec@acme.com",
            "+91987654321012345678",
            "+91-abc-defg",
            "919876543210&text=hijacked",
        ] {
            assert_eq!(valid_phone(hostile), None, "{hostile:?} must be refused");
        }
    }

    #[test]
    fn message_text_cannot_add_url_parameters() {
        let encoded = encode("hi&phone=919999999999");
        assert!(!encoded.contains('&') && !encoded.contains('='));
    }

    #[test]
    fn a_message_is_bounded_and_never_empty() {
        assert!(check_message("   ").is_err());
        assert!(check_message(&"x".repeat(MAX_MESSAGE + 1)).is_err());
        assert_eq!(check_message(" hello ").expect("ok"), "hello");
    }

    #[test]
    fn no_action_claims_to_read_messages() {
        // NOT SUPPORTED, and not deferred: there is no mechanism to add
        // later, so offering it would be a lie with a roadmap attached.
        for spec in ACTIONS {
            let id = spec.id;
            assert!(
                !id.contains("read") && !id.contains("list") && !id.contains("messages"),
                "{id} implies inbound access that cannot be built"
            );
        }
        // Status, compose, and press-send. Reading conversations is still
        // absent and still unbuildable; sending one the user is looking at
        // is a different thing from reading the ones they are not.
        assert_eq!(ACTIONS.len(), 3, "no action beyond these three");
    }

    #[test]
    fn the_summary_does_not_promise_to_send() {
        let compose = ACTIONS
            .iter()
            .find(|s| s.id == "whatsapp.compose_message")
            .expect("present");
        assert!(
            compose.summary.contains("for you to send"),
            "{}",
            compose.summary
        );
        assert!(!compose.summary.starts_with("Send"));
    }

    #[test]
    fn composing_writes_but_always_asks_first() {
        let compose = ACTIONS
            .iter()
            .find(|s| s.id == "whatsapp.compose_message")
            .expect("present");
        assert_eq!(compose.permission, Permission::Write);
        assert_eq!(compose.confirm, ConfirmPolicy::Always);
        assert_eq!(
            compose.reach,
            Reach::LocalOnly,
            "nothing is transmitted by NEXUS"
        );
    }

    #[test]
    fn the_status_payload_states_the_limitation() {
        let production = include_str!("whatsapp_connector.rs");
        assert!(production.contains("\"canRead\": false"));
        assert!(production.contains("\"sent\": false"));
    }

    #[test]
    fn no_unofficial_automation_is_reached_for() {
        // "osascript" was on this list until the user asked, twice and
        // explicitly, for NEXUS to press send on their own machine and their
        // own account. What the list still forbids is the part that was
        // never about their choice: reverse-engineered clients and scraping
        // WhatsApp's web app, which impersonate the client rather than drive
        // the one they installed.
        let production = include_str!("whatsapp_connector.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("marker");
        for forbidden in ["whatsapp-web", "Baileys", "web.whatsapp.com", "puppeteer"] {
            assert!(!production.contains(forbidden), "found {forbidden}");
        }
    }

    #[test]
    fn the_keystroke_can_never_land_in_another_application() {
        // The one guarantee that matters once synthetic input is allowed:
        // the check that WhatsApp is frontmost and the keystroke itself are
        // in the SAME script, so nothing can steal focus between them. Two
        // scripts would be a race, and losing it means typing into whatever
        // window arrived instead.
        let production = include_str!("whatsapp_connector.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("marker");
        let script = production
            .split_once("const PRESS_SEND: [&str; ")
            .map(|(_, rest)| rest)
            .expect("the send script must be one literal");
        let body = &script[..script.find("];").expect("end of script")];
        let front = body.find("frontmost").expect("must check what is frontmost");
        let key = body.find("key code 36").expect("must press return");
        assert!(front < key, "the focus check has to come first");
        assert!(
            body.contains("if n is not \\\"WhatsApp\\\" then"),
            "the keystroke must be guarded by the check, not merely preceded by it"
        );
    }

    #[test]
    fn a_malformed_number_is_rejected_before_anything_opens() {
        let err = WhatsappConnector
            .validate_input(
                "whatsapp.compose_message",
                &serde_json::json!({ "phone": "nope", "message": "hi" }),
            )
            .expect_err("must reject");
        assert!(matches!(err, ActionError::InvalidInput { .. }), "{err:?}");
    }

    #[test]
    fn the_recipient_becomes_referable() {
        let conn = Connection::open_in_memory().expect("open");
        let drafts = WhatsappConnector.observe(
            "whatsapp.compose_message",
            &serde_json::json!({
                "phone": "+919876543210",
                "message": "hi",
                "displayName": "Priya"
            }),
            &serde_json::json!({}),
            &conn,
        );
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].display_name, "Priya");
        assert_eq!(drafts[0].kind, ReferentKind::Person);
    }
}
