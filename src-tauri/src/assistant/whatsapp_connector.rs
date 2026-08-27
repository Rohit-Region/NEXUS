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
    Capabilities, Connector, ConnectorStatus, ExecCtx, ReferentDraft, UnavailableAction,
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
                        let who = c
                            .display_name
                            .clone()
                            .unwrap_or_else(|| c.phone.clone());
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

            "whatsapp.compose_message" => {
                let target: ComposeInput =
                    serde_json::from_value(input).map_err(|e| ActionError::InvalidInput {
                        detail: e.to_string(),
                    })?;
                let phone = valid_phone(&target.phone).ok_or_else(|| {
                    ActionError::InvalidInput {
                        detail: format!(
                            "\"{}\" is not an international phone number.",
                            target.phone
                        ),
                    }
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
    fn international_numbers_are_accepted_in_the_shapes_people_write_them() {
        for raw in ["+919876543210", "919876543210", "+91 98765 43210", "+1 (415) 555-0100"] {
            assert!(valid_phone(raw).is_some(), "{raw}");
        }
        assert_eq!(valid_phone("+91 98765 43210").as_deref(), Some("919876543210"));
    }

    #[test]
    fn anything_that_is_not_a_number_is_refused() {
        // A malformed number opens a chat with the wrong person, which is
        // worse than an error.
        for hostile in [
            "",
            "12345",
            "alec@avetta.com",
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
        assert_eq!(ACTIONS.len(), 2, "status and compose, and nothing else");
    }

    #[test]
    fn the_summary_does_not_promise_to_send() {
        let compose = ACTIONS
            .iter()
            .find(|s| s.id == "whatsapp.compose_message")
            .expect("present");
        assert!(compose.summary.contains("for you to send"), "{}", compose.summary);
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
        assert_eq!(compose.reach, Reach::LocalOnly, "nothing is transmitted by NEXUS");
    }

    #[test]
    fn the_status_payload_states_the_limitation() {
        let production = include_str!("whatsapp_connector.rs");
        assert!(production.contains("\"canRead\": false"));
        assert!(production.contains("\"sent\": false"));
    }

    #[test]
    fn no_unofficial_automation_is_reached_for() {
        let production = include_str!("whatsapp_connector.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("marker");
        for forbidden in ["whatsapp-web", "Baileys", "web.whatsapp.com", "puppeteer", "osascript"] {
            assert!(!production.contains(forbidden), "found {forbidden}");
        }
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
