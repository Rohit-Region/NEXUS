//! NEXUS-018: the Microsoft Teams connector.
//!
//! Two halves, and they are in very different states.
//!
//! **The half that works today: deep-link handoff.** Teams registers the
//! `msteams:` URL scheme, so NEXUS can open a chat with a message already
//! typed into the compose box. The user presses send. That is a documented,
//! supported mechanism, needs no authorisation at all, and delivers most of
//! the value of "reply to Alec" without NEXUS ever holding a Teams
//! credential. NEXUS never sends: it drafts and hands over.
//!
//! **The half that is blocked: Microsoft Graph.** Reading messages has no
//! other supported route. Teams ships no AppleScript dictionary and no local
//! API, so there is nothing to automate on this machine; scraping the webview
//! would be both fragile and against the terms. Graph needs:
//!
//! 1. An Azure app registration (a client id and tenant id).
//! 2. **Tenant administrator consent** for `Chat.Read` / `Chat.ReadWrite`.
//!    In a managed corporate tenant these are not user-consentable, so this
//!    is blocked on an organisation, not on code.
//! 3. A token, kept in the Keychain like every other secret.
//!
//! The Graph code path is written and typed, and it has **never made a
//! request**. Every one of its actions reports itself unavailable until
//! configuration and a token exist. That is deliberate: a connector that
//! claimed to work because it compiled would be worse than one that says it
//! cannot.
//!
//! Change notifications are not used. NEXUS has no public endpoint to receive
//! a webhook, and Graph's chat-message subscriptions carry licensing
//! implications; polling is the shape that fits a desktop app.

use rusqlite::Connection;
use serde::Deserialize;

use super::action::{ActionError, ActionSpec};
use super::connector::{
    Capabilities, Connector, ConnectorStatus, ExecCtx, ReferentDraft, UnavailableAction,
};
use super::http::{keychain_secret, send, HttpError, Request};
use super::permission::{ConfirmPolicy, Permission, Reach};
use super::referent::ReferentKind;
use super::shell::{run, DEFAULT_TIMEOUT};

pub const CONNECTOR_ID: &str = "teams";
pub const KEYCHAIN_SERVICE: &str = "nexus-teams";

const GRAPH: &str = "https://graph.microsoft.com/v1.0";
const CHAT_LIMIT: u32 = 15;
const MESSAGE_LIMIT: u32 = 20;
/// Longest message NEXUS will put into a deep link. Beyond this the URL
/// itself becomes unreliable, and a wall of text is not a chat message.
const MAX_MESSAGE: usize = 1_000;

const fn spec(
    id: &'static str,
    summary: &'static str,
    permission: Permission,
    confirm: ConfirmPolicy,
    reach: Reach,
) -> ActionSpec {
    ActionSpec {
        id,
        connector_id: CONNECTOR_ID,
        summary,
        permission,
        confirm,
        reach,
        reversible: true,
    }
}

pub const ACTIONS: &[ActionSpec] = &[
    spec(
        "teams.status",
        "Check the Teams connection",
        Permission::Read,
        ConfirmPolicy::Never,
        Reach::LocalOnly,
    ),
    // Handoff: works today, no authorisation.
    spec(
        "teams.open_chat",
        "Open a Teams chat",
        Permission::Interact,
        ConfirmPolicy::Never,
        Reach::LocalOnly,
    ),
    spec(
        "teams.compose_message",
        "Draft a Teams message for you to send",
        Permission::Write,
        ConfirmPolicy::Always,
        Reach::LocalOnly,
    ),
    // Graph: blocked on tenant consent.
    spec(
        "teams.list_chats",
        "List recent Teams chats",
        Permission::Read,
        ConfirmPolicy::Never,
        Reach::LeavesMachine,
    ),
    spec(
        "teams.read_messages",
        "Read messages in a Teams chat",
        Permission::Read,
        ConfirmPolicy::Never,
        Reach::LeavesMachine,
    ),
    spec(
        "teams.send_message",
        "Send a Teams message",
        Permission::Write,
        ConfirmPolicy::Always,
        Reach::LeavesMachine,
    ),
];

/// Actions that reach Microsoft Graph, and therefore need consent.
const GRAPH_ACTIONS: &[&str] = &[
    "teams.list_chats",
    "teams.read_messages",
    "teams.send_message",
];

// -- Configuration ------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamsConfig {
    /// Azure app registration client id.
    pub client_id: String,
    /// Directory (tenant) id, or a domain such as `acme.com`.
    pub tenant_id: String,
    /// The signed-in account, used as the Keychain account name.
    pub account: String,
}

pub fn read_config(conn: &Connection) -> Option<TeamsConfig> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT config_json FROM connectors WHERE connector_id = ?1",
            [CONNECTOR_ID],
            |row| row.get(0),
        )
        .ok()?;
    let parsed: TeamsConfig = serde_json::from_str(&raw?).ok()?;
    if parsed.client_id.trim().is_empty()
        || parsed.tenant_id.trim().is_empty()
        || parsed.account.trim().is_empty()
    {
        return None;
    }
    Some(parsed)
}

fn graph_token(conn: &Connection) -> Option<String> {
    let config = read_config(conn)?;
    keychain_secret(KEYCHAIN_SERVICE, &config.account)
}

/// The exact reason Graph is unavailable, in the user's terms.
fn graph_blocker(conn: &Connection) -> Option<String> {
    if read_config(conn).is_none() {
        return Some(
            "Teams needs an Azure app registration. Add the client id, tenant id and your \
             account in Settings. Your IT administrator has to consent to the Chat.Read \
             permission before this will work."
                .to_string(),
        );
    }
    if graph_token(conn).is_none() {
        return Some(format!(
            "No Microsoft access token is stored in the Keychain. Add one under the service \
             {KEYCHAIN_SERVICE}."
        ));
    }
    None
}

// -- Validation ---------------------------------------------------------------

/// A user principal name, e.g. `alec@acme.com`.
///
/// Validated because it is interpolated into a URL. Deliberately narrow: an
/// address with a slash or a query character could rewrite the deep link into
/// something other than a chat.
pub fn valid_upn(raw: &str) -> Option<String> {
    let upn = raw.trim();
    if upn.len() < 3 || upn.len() > 254 {
        return None;
    }
    let (local, domain) = upn.split_once('@')?;
    if local.is_empty() || domain.is_empty() || !domain.contains('.') {
        return None;
    }
    let allowed = |c: char| c.is_ascii_alphanumeric() || "._%+-".contains(c);
    if !local.chars().all(allowed)
        || !domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        return None;
    }
    Some(upn.to_string())
}

/// A Graph chat id. Opaque, so only the characters Microsoft actually uses.
pub fn valid_chat_id(raw: &str) -> Option<String> {
    let id = raw.trim();
    if id.is_empty() || id.len() > 200 {
        return None;
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || ":_-@.".contains(c))
    {
        return None;
    }
    Some(id.to_string())
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
            detail: "There was no message to send.".to_string(),
        });
    }
    if text.chars().count() > MAX_MESSAGE {
        return Err(ActionError::InvalidInput {
            detail: format!("That message is longer than {MAX_MESSAGE} characters."),
        });
    }
    Ok(text.to_string())
}

// -- Graph --------------------------------------------------------------------

fn graph_get(conn: &Connection, path: &str) -> Result<serde_json::Value, ActionError> {
    let token = graph_token(conn).ok_or_else(|| ActionError::Failed {
        detail: graph_blocker(conn).unwrap_or_else(|| "Teams is not connected.".to_string()),
    })?;

    let response = send(Request {
        method: "GET",
        url: format!("{GRAPH}{path}"),
        headers: vec![
            ("Accept".to_string(), "application/json".to_string()),
            ("Authorization".to_string(), format!("Bearer {token}")),
        ],
        basic_auth: None,
        body: None,
    })
    .map_err(|e| ActionError::Failed {
        detail: match e {
            HttpError::Unreachable { detail } => {
                format!("Microsoft Graph could not be reached. {detail}")
            }
            other => other.to_string(),
        },
    })?;

    if !response.ok() {
        return Err(ActionError::Failed {
            detail: match response.status {
                401 => "Microsoft rejected the token. It has probably expired.".to_string(),
                403 => "Your tenant has not granted NEXUS permission to read Teams chats. \
                        This needs an administrator."
                    .to_string(),
                404 => "Microsoft Graph has no such chat.".to_string(),
                429 => "Microsoft Graph is rate limiting NEXUS. Try again shortly.".to_string(),
                other => format!("Microsoft Graph returned {other}."),
            },
        });
    }

    serde_json::from_str(&response.body).map_err(|e| ActionError::Failed {
        detail: format!("Microsoft Graph returned something NEXUS could not read: {e}"),
    })
}

/// Reduce a Graph chatMessage to what a person asks about.
fn shape_message(value: &serde_json::Value) -> serde_json::Value {
    let from = value
        .get("from")
        .and_then(|f| f.get("user"))
        .and_then(|u| u.get("displayName"))
        .and_then(|v| v.as_str());
    serde_json::json!({
        "id": value.get("id").and_then(|v| v.as_str()),
        "from": from,
        "preview": value
            .get("body")
            .and_then(|b| b.get("content"))
            .and_then(|v| v.as_str())
            .map(|text| {
                let stripped = strip_html(text);
                stripped.chars().take(280).collect::<String>()
            }),
        "createdAt": value.get("createdDateTime").and_then(|v| v.as_str()),
    })
}

/// Graph returns message bodies as HTML. This is a preview, not a renderer:
/// tags are removed so the text is readable, and nothing is interpreted.
fn strip_html(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut inside = false;
    for ch in raw.chars() {
        match ch {
            '<' => inside = true,
            '>' => inside = false,
            c if !inside => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

// -- Typed inputs -------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersonRef {
    upn: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ComposeInput {
    upn: String,
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ChatRef {
    chat_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SendInput {
    chat_id: String,
    message: String,
}

fn parse<T: serde::de::DeserializeOwned>(input: serde_json::Value) -> Result<T, ActionError> {
    serde_json::from_value(input).map_err(|e| ActionError::InvalidInput {
        detail: e.to_string(),
    })
}

pub struct TeamsConnector;

impl Connector for TeamsConnector {
    fn id(&self) -> &'static str {
        CONNECTOR_ID
    }

    fn display_name(&self) -> &'static str {
        "Microsoft Teams"
    }

    fn actions(&self) -> &'static [ActionSpec] {
        ACTIONS
    }

    fn capabilities(&self, conn: &Connection) -> Capabilities {
        let teams_installed = std::path::Path::new("/Applications/Microsoft Teams.app").exists();

        let mut available = vec!["teams.status".to_string()];
        let mut unavailable = Vec::new();

        // Handoff works whenever Teams is installed. No token, no consent.
        for id in ["teams.open_chat", "teams.compose_message"] {
            if teams_installed {
                available.push(id.to_string());
            } else {
                unavailable.push(UnavailableAction {
                    action_id: id.to_string(),
                    reason: "Microsoft Teams is not installed on this Mac.".to_string(),
                });
            }
        }

        match graph_blocker(conn) {
            None => available.extend(GRAPH_ACTIONS.iter().map(|id| id.to_string())),
            Some(reason) => unavailable.extend(GRAPH_ACTIONS.iter().map(|id| UnavailableAction {
                action_id: (*id).to_string(),
                reason: reason.clone(),
            })),
        }

        Capabilities {
            available,
            unavailable,
        }
    }

    fn status(&self, conn: &Connection) -> ConnectorStatus {
        match (read_config(conn), graph_token(conn)) {
            (Some(_), Some(_)) => ConnectorStatus::Ready,
            (Some(_), None) => ConnectorStatus::NeedsAuth,
            // Not Unavailable: the handoff half works with no setup at all.
            (None, _) => ConnectorStatus::Degraded,
        }
    }

    fn summarize(&self, action_id: &str, input: &serde_json::Value, _conn: &Connection) -> String {
        match action_id {
            "teams.compose_message" => {
                match serde_json::from_value::<ComposeInput>(input.clone()) {
                    Ok(c) => format!(
                        "Open a Teams chat with {} and put this in the box: \"{}\"",
                        c.upn,
                        c.message.chars().take(120).collect::<String>()
                    ),
                    Err(_) => "Draft a Teams message".to_string(),
                }
            }
            "teams.send_message" => match serde_json::from_value::<SendInput>(input.clone()) {
                Ok(s) => format!(
                    "Send to Teams: \"{}\"",
                    s.message.chars().take(160).collect::<String>()
                ),
                Err(_) => "Send a Teams message".to_string(),
            },
            "teams.open_chat" => match serde_json::from_value::<PersonRef>(input.clone()) {
                Ok(p) => format!("Open a Teams chat with {}", p.upn),
                Err(_) => "Open a Teams chat".to_string(),
            },
            other => ACTIONS
                .iter()
                .find(|s| s.id == other)
                .map(|s| s.summary.to_string())
                .unwrap_or_else(|| other.to_string()),
        }
    }

    fn observe(
        &self,
        action_id: &str,
        input: &serde_json::Value,
        _output: &serde_json::Value,
        _conn: &Connection,
    ) -> Vec<ReferentDraft> {
        // Whoever the conversation is now with becomes "him"/"her"/"them".
        if !matches!(action_id, "teams.open_chat" | "teams.compose_message") {
            return Vec::new();
        }
        serde_json::from_value::<PersonRef>(input.clone())
            .ok()
            .map(|p| p.upn)
            .or_else(|| {
                serde_json::from_value::<ComposeInput>(input.clone())
                    .ok()
                    .map(|c| c.upn)
            })
            .and_then(|upn| valid_upn(&upn))
            .map(|upn| ReferentDraft {
                kind: ReferentKind::Person,
                display_name: upn.split('@').next().unwrap_or(&upn).to_string(),
                metadata: serde_json::json!({ "upn": upn }),
            })
            .into_iter()
            .collect()
    }

    fn validate_input(
        &self,
        action_id: &str,
        input: &serde_json::Value,
    ) -> Result<(), ActionError> {
        match action_id {
            "teams.open_chat" => parse::<PersonRef>(input.clone()).map(|_| ()),
            "teams.compose_message" => {
                let target: ComposeInput = parse(input.clone())?;
                check_message(&target.message).map(|_| ())
            }
            "teams.read_messages" => parse::<ChatRef>(input.clone()).map(|_| ()),
            "teams.send_message" => {
                let target: SendInput = parse(input.clone())?;
                check_message(&target.message).map(|_| ())
            }
            _ => Ok(()),
        }
    }

    fn describe_result(&self, action_id: &str, output: &serde_json::Value) -> Option<String> {
        match action_id {
            "teams.status" => {
                let installed = output.get("installed")?.as_bool().unwrap_or(false);
                let can_read = output.get("canRead")?.as_bool().unwrap_or(false);
                Some(if !installed {
                    "Teams is not installed on this Mac.".to_string()
                } else if can_read {
                    "Teams is connected.".to_string()
                } else {
                    "Teams is installed. NEXUS can open chats and draft messages, \
                     but reading them needs Microsoft Graph access your tenant \
                     administrator has to approve."
                        .to_string()
                })
            }
            "teams.compose_message" => Some(format!(
                "Teams is open with your message ready. Press send when you are happy with it: \"{}\"",
                output.get("message")?.as_str()?
            )),
            _ => None,
        }
    }

    fn zero_input_actions(&self) -> &'static [&'static str] {
        &["teams.status", "teams.list_chats"]
    }

    fn dispatch(
        &self,
        action_id: &str,
        input: serde_json::Value,
        ctx: &ExecCtx<'_>,
    ) -> Result<serde_json::Value, ActionError> {
        match action_id {
            "teams.status" => Ok(serde_json::json!({
                "status": self.status(ctx.conn),
                "handoffAvailable":
                    std::path::Path::new("/Applications/Microsoft Teams.app").exists(),
                "graphBlocker": graph_blocker(ctx.conn),
            })),

            // -- Handoff: works today ------------------------------------------
            "teams.open_chat" => {
                let target: PersonRef = parse(input)?;
                let upn = valid_upn(&target.upn).ok_or_else(|| ActionError::InvalidInput {
                    detail: format!("\"{}\" is not a work email address.", target.upn),
                })?;
                let url = format!("msteams:/l/chat/0/0?users={}", encode(&upn));
                open_url(&url)?;
                Ok(serde_json::json!({ "opened": upn }))
            }

            "teams.compose_message" => {
                let target: ComposeInput = parse(input)?;
                let upn = valid_upn(&target.upn).ok_or_else(|| ActionError::InvalidInput {
                    detail: format!("\"{}\" is not a work email address.", target.upn),
                })?;
                let message = check_message(&target.message)?;
                // NEXUS does not send. It opens the chat with the text ready
                // and the user presses send, which is why this needs no Teams
                // credential at all.
                let url = format!(
                    "msteams:/l/chat/0/0?users={}&message={}",
                    encode(&upn),
                    encode(&message)
                );
                open_url(&url)?;
                Ok(serde_json::json!({
                    "handedOff": true,
                    "to": upn,
                    "message": message,
                    "sent": false
                }))
            }

            // -- Graph: blocked until an administrator consents ----------------
            "teams.list_chats" => {
                let value = graph_get(
                    ctx.conn,
                    &format!("/me/chats?$top={CHAT_LIMIT}&$orderby=lastMessagePreview/createdDateTime desc"),
                )?;
                Ok(serde_json::json!({
                    "chats": value.get("value").cloned().unwrap_or(serde_json::json!([]))
                }))
            }

            "teams.read_messages" => {
                let target: ChatRef = parse(input)?;
                let chat =
                    valid_chat_id(&target.chat_id).ok_or_else(|| ActionError::InvalidInput {
                        detail: "That is not a Teams chat id.".to_string(),
                    })?;
                let value = graph_get(
                    ctx.conn,
                    &format!("/me/chats/{}/messages?$top={MESSAGE_LIMIT}", encode(&chat)),
                )?;
                let messages: Vec<serde_json::Value> = value
                    .get("value")
                    .and_then(|v| v.as_array())
                    .map(|rows| rows.iter().map(shape_message).collect())
                    .unwrap_or_default();
                Ok(serde_json::json!({ "chatId": chat, "messages": messages }))
            }

            "teams.send_message" => {
                let target: SendInput = parse(input)?;
                let chat =
                    valid_chat_id(&target.chat_id).ok_or_else(|| ActionError::InvalidInput {
                        detail: "That is not a Teams chat id.".to_string(),
                    })?;
                let message = check_message(&target.message)?;
                let token = graph_token(ctx.conn).ok_or_else(|| ActionError::Failed {
                    detail: graph_blocker(ctx.conn)
                        .unwrap_or_else(|| "Teams is not connected.".to_string()),
                })?;

                let body = serde_json::json!({ "body": { "content": message } }).to_string();
                let response = send(Request {
                    method: "POST",
                    url: format!("{GRAPH}/me/chats/{}/messages", encode(&chat)),
                    headers: vec![
                        ("Content-Type".to_string(), "application/json".to_string()),
                        ("Authorization".to_string(), format!("Bearer {token}")),
                    ],
                    basic_auth: None,
                    body: Some(body),
                })
                .map_err(|e| ActionError::Failed {
                    detail: e.to_string(),
                })?;

                if !response.ok() {
                    return Err(ActionError::Failed {
                        detail: match response.status {
                            401 => "Microsoft rejected the token.".to_string(),
                            403 => "Your tenant has not granted NEXUS permission to send Teams \
                                    messages. This needs an administrator."
                                .to_string(),
                            other => format!("Microsoft Graph returned {other}."),
                        },
                    });
                }
                Ok(serde_json::json!({ "sent": true, "chatId": chat }))
            }

            other => Err(ActionError::UnknownAction {
                action_id: other.to_string(),
            }),
        }
    }
}

fn open_url(url: &str) -> Result<(), ActionError> {
    let out = run("/usr/bin/open", &[url], DEFAULT_TIMEOUT).map_err(|e| ActionError::Failed {
        detail: e.to_string(),
    })?;
    if !out.success {
        return Err(ActionError::Failed {
            detail: "Microsoft Teams would not open.".to_string(),
        });
    }
    Ok(())
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
        conn.execute(
            "INSERT OR IGNORE INTO connectors (connector_id, display_name) VALUES ('teams','Teams')",
            [],
        )
        .expect("register");
        conn
    }

    #[test]
    fn a_work_address_is_accepted_and_anything_url_shaped_is_not() {
        assert_eq!(
            valid_upn("alec@acme.com").as_deref(),
            Some("alec@acme.com")
        );
        assert_eq!(
            valid_upn(" a.b-c@sub.acme.com ").as_deref(),
            Some("a.b-c@sub.acme.com")
        );
        for hostile in [
            "alec@acme.com&message=hijacked",
            "alec@acme.com/../x",
            "alec@acme.com?x=1",
            "alec",
            "@acme.com",
            "alec@",
            "alec@nodot",
            "",
        ] {
            assert_eq!(valid_upn(hostile), None, "{hostile:?} must be refused");
        }
    }

    #[test]
    fn a_chat_id_cannot_carry_url_syntax() {
        assert!(valid_chat_id("19:abc_def-1@thread.v2").is_some());
        for hostile in ["19:abc/../me", "19:abc?$top=999", "19:abc&x=1", ""] {
            assert_eq!(valid_chat_id(hostile), None, "{hostile:?}");
        }
    }

    #[test]
    fn a_message_is_bounded_and_never_empty() {
        assert!(check_message("  ").is_err());
        assert!(check_message(&"x".repeat(MAX_MESSAGE + 1)).is_err());
        assert_eq!(check_message("  hello  ").expect("ok"), "hello");
    }

    #[test]
    fn message_text_is_encoded_so_it_cannot_add_url_parameters() {
        let encoded = encode("hi&users=someone-else@evil.com");
        assert!(!encoded.contains('&'));
        assert!(!encoded.contains('='));
    }

    #[test]
    fn html_bodies_are_flattened_not_rendered() {
        assert_eq!(
            strip_html("<p>Can you review <b>PR #8792</b>?</p>"),
            "Can you review PR #8792?"
        );
        assert_eq!(strip_html("<script>alert(1)</script>hi"), "alert(1)hi");
    }

    // -- The blocker is stated, never hidden ---------------------------------

    #[test]
    fn graph_actions_are_unavailable_and_say_an_administrator_is_needed() {
        let conn = test_conn();
        let caps = TeamsConnector.capabilities(&conn);
        for id in GRAPH_ACTIONS {
            let entry = caps
                .unavailable
                .iter()
                .find(|u| &u.action_id == id)
                .unwrap_or_else(|| panic!("{id} must be reported unavailable"));
            assert!(
                entry.reason.contains("administrator"),
                "the real blocker must be named: {}",
                entry.reason
            );
            assert!(!caps.available.contains(&(*id).to_string()));
        }
    }

    #[test]
    fn handoff_needs_no_configuration_at_all() {
        // The half that works today, with no token and no consent.
        let conn = test_conn();
        let caps = TeamsConnector.capabilities(&conn);
        if std::path::Path::new("/Applications/Microsoft Teams.app").exists() {
            assert!(caps
                .available
                .contains(&"teams.compose_message".to_string()));
            assert!(caps.available.contains(&"teams.open_chat".to_string()));
        }
    }

    #[test]
    fn the_connector_is_degraded_not_unavailable_without_graph() {
        let conn = test_conn();
        assert_eq!(TeamsConnector.status(&conn), ConnectorStatus::Degraded);
    }

    #[test]
    fn an_incomplete_registration_is_treated_as_absent() {
        let conn = test_conn();
        for bad in [
            r#"{"clientId":"","tenantId":"t","account":"a@b.com"}"#,
            r#"{"clientId":"c","tenantId":"","account":"a@b.com"}"#,
            r#"{"clientId":"c","tenantId":"t"}"#,
            "nonsense",
        ] {
            conn.execute(
                "UPDATE connectors SET config_json = ?1 WHERE connector_id = 'teams'",
                [bad],
            )
            .expect("configure");
            assert!(read_config(&conn).is_none(), "{bad}");
        }
    }

    // -- Sending is never something NEXUS does by itself ---------------------

    #[test]
    fn composing_hands_off_and_reports_that_nothing_was_sent() {
        // The output says `sent: false` because NEXUS genuinely did not send.
        // A connector that reported success here would be lying.
        let production = include_str!("teams_connector.rs");
        assert!(production.contains("\"sent\": false"));
    }

    #[test]
    fn anything_that_writes_always_confirms() {
        for spec in ACTIONS {
            if spec.permission >= Permission::Write {
                assert_eq!(spec.confirm, ConfirmPolicy::Always, "{}", spec.id);
            }
        }
    }

    #[test]
    fn handoff_actions_stay_on_the_machine_and_graph_actions_do_not() {
        for spec in ACTIONS {
            let expected = if GRAPH_ACTIONS.contains(&spec.id) {
                Reach::LeavesMachine
            } else {
                Reach::LocalOnly
            };
            assert_eq!(spec.reach, expected, "{}", spec.id);
        }
    }

    #[test]
    fn opening_a_chat_makes_the_person_referable() {
        let conn = test_conn();
        let drafts = TeamsConnector.observe(
            "teams.compose_message",
            &serde_json::json!({ "upn": "alec@acme.com", "message": "hi" }),
            &serde_json::json!({}),
            &conn,
        );
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].kind, ReferentKind::Person);
        assert_eq!(drafts[0].display_name, "alec");
    }

    #[test]
    fn no_scraping_of_the_teams_client() {
        let production = include_str!("teams_connector.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("marker");
        for forbidden in ["osascript", "Cookies", "leveldb", "sqlite3 ", "/bin/sh"] {
            assert!(!production.contains(forbidden), "found {forbidden}");
        }
    }

    #[test]
    fn action_ids_are_unique_and_namespaced() {
        let mut seen = std::collections::HashSet::new();
        for spec in ACTIONS {
            assert!(seen.insert(spec.id), "duplicate {}", spec.id);
            assert!(spec.id.starts_with("teams."), "{}", spec.id);
        }
    }
}
