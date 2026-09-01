//! Outlook mail and calendar, through Microsoft Graph.
//!
//! Built before the Teams connector can work, and deliberately so: the scopes
//! here (`Mail.Read`, `Calendars.Read`, `Mail.Send`) are user-consentable in a
//! default tenant, whereas `Chat.Read` is not. That means this can be signed
//! in and used today, while Teams waits on an administrator. When that
//! consent lands, Teams drops in behind the same sign-in.
//!
//! Everything here reads. The one action that writes, sending mail, is
//! `Write` and always confirmed, and the approval prompt shows the recipient
//! and the full body before anything leaves.
//!
//! Message bodies are **not** requested. The list actions ask Graph for
//! subjects, senders and times only, using `$select`, so the body of an email
//! is never fetched, never held, and never available to be sent anywhere.

use rusqlite::Connection;
use serde::Deserialize;

use super::action::{ActionError, ActionSpec};
use super::connector::{
    Capabilities, Connector, ConnectorStatus, ExecCtx, ReferentDraft, UnavailableAction,
};
use super::msgraph::{
    begin_sign_in, config, graph_get, graph_post, is_signed_in, poll_sign_in, sign_out,
    SignInProgress,
};
use super::permission::{ConfirmPolicy, Permission, Reach};
use super::referent::ReferentKind;
use super::shell::{run, DEFAULT_TIMEOUT};

pub const CONNECTOR_ID: &str = "outlook";

/// Most items fetched at once. A mailbox is not a report.
const PAGE: usize = 10;
/// Longest list read back aloud before it stops being an answer.
const SPOKEN_CAP: usize = 5;

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
        "outlook.status",
        "Check the Outlook connection",
        Permission::Read,
        ConfirmPolicy::Never,
        Reach::LocalOnly,
    ),
    spec(
        "outlook.sign_in",
        "Sign in to Microsoft",
        Permission::Interact,
        ConfirmPolicy::Never,
        Reach::LeavesMachine,
    ),
    spec(
        "outlook.finish_sign_in",
        "Finish signing in to Microsoft",
        Permission::Interact,
        ConfirmPolicy::Never,
        Reach::LeavesMachine,
    ),
    spec(
        "outlook.sign_out",
        "Sign out of Microsoft",
        Permission::Interact,
        ConfirmPolicy::Never,
        Reach::LocalOnly,
    ),
    spec(
        "outlook.unread_mail",
        "Check unread mail",
        Permission::Read,
        ConfirmPolicy::Never,
        Reach::LeavesMachine,
    ),
    spec(
        "outlook.today_schedule",
        // Says "calendar" as well as "meetings" because both are what people
        // call it, and keywords are derived from this text rather than a
        // hand-written synonym list.
        "Check today's calendar and meetings",
        Permission::Read,
        ConfirmPolicy::Never,
        Reach::LeavesMachine,
    ),
    ActionSpec {
        id: "outlook.send_mail",
        connector_id: CONNECTOR_ID,
        summary: "Send an email",
        permission: Permission::Write,
        confirm: ConfirmPolicy::Always,
        reach: Reach::LeavesMachine,
        // It cannot be unsent.
        reversible: false,
    },
];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SendMail {
    to: String,
    subject: String,
    body: String,
}

fn parse<T: serde::de::DeserializeOwned>(input: serde_json::Value) -> Result<T, ActionError> {
    serde_json::from_value(input).map_err(|e| ActionError::InvalidInput {
        detail: e.to_string(),
    })
}

fn failed(detail: String) -> ActionError {
    ActionError::Failed { detail }
}

/// A work email address, checked before it reaches a request body.
fn valid_address(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let at = trimmed.find('@')?;
    let (local, domain) = trimmed.split_at(at);
    let domain = &domain[1..];
    if local.is_empty()
        || !domain.contains('.')
        || trimmed.contains(char::is_whitespace)
        || trimmed.len() > 254
    {
        return None;
    }
    Some(trimmed.to_string())
}

/// Today, as the pair of instants Graph's calendar view wants.
///
/// From SQLite so the machine's timezone applies, and so no date crate is
/// needed for the one place NEXUS does arithmetic on a day.
fn today_bounds(conn: &Connection) -> (String, String) {
    conn.query_row(
        "SELECT strftime('%Y-%m-%dT00:00:00','now','localtime'),
                strftime('%Y-%m-%dT23:59:59','now','localtime')",
        [],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )
    .unwrap_or_else(|_| {
        (
            "1970-01-01T00:00:00".to_string(),
            "1970-01-01T23:59:59".to_string(),
        )
    })
}

/// "14:30" out of Graph's timestamp, without parsing a full datetime.
fn clock_of(stamp: &str) -> String {
    stamp
        .split('T')
        .nth(1)
        .map(|time| time.chars().take(5).collect())
        .unwrap_or_else(|| stamp.to_string())
}

pub struct OutlookConnector;

impl Connector for OutlookConnector {
    fn id(&self) -> &'static str {
        CONNECTOR_ID
    }

    fn display_name(&self) -> &'static str {
        "Outlook"
    }

    fn actions(&self) -> &'static [ActionSpec] {
        ACTIONS
    }

    fn capabilities(&self, conn: &Connection) -> Capabilities {
        let configured = config(conn, CONNECTOR_ID);
        let signed_in = configured.as_ref().map(is_signed_in).unwrap_or(false);

        if signed_in {
            return Capabilities {
                available: ACTIONS.iter().map(|s| s.id.to_string()).collect(),
                unavailable: Vec::new(),
            };
        }

        let (reason, available) = match configured {
            Some(_) => (
                "Not signed in yet. Run the Outlook sign-in.",
                vec![
                    "outlook.status".to_string(),
                    "outlook.sign_in".to_string(),
                    "outlook.finish_sign_in".to_string(),
                ],
            ),
            None => (
                "No app registration configured. NEXUS needs an Application (client) id \
                 and Directory (tenant) id from Microsoft Entra.",
                vec!["outlook.status".to_string()],
            ),
        };

        Capabilities {
            unavailable: ACTIONS
                .iter()
                .filter(|s| !available.contains(&s.id.to_string()))
                .map(|s| UnavailableAction {
                    action_id: s.id.to_string(),
                    reason: reason.to_string(),
                })
                .collect(),
            available,
        }
    }

    fn status(&self, conn: &Connection) -> ConnectorStatus {
        match config(conn, CONNECTOR_ID) {
            None => ConnectorStatus::Unconfigured,
            Some(cfg) if is_signed_in(&cfg) => ConnectorStatus::Ready,
            Some(_) => ConnectorStatus::NeedsAuth,
        }
    }

    /// Signing in is two steps, and the second is easy to forget.
    ///
    /// `sign_in` opens a browser and reads out a code; nothing is signed in
    /// until `finish_sign_in` confirms it. Offering it means "yes" completes
    /// the exchange instead of the user having to remember a second phrase
    /// while reading a code off a screen.
    fn follow_up(
        &self,
        action_id: &str,
        _input: &serde_json::Value,
        _output: &serde_json::Value,
    ) -> Option<super::connector::FollowUp> {
        (action_id == "outlook.sign_in").then_some(super::connector::FollowUp {
            action_id: "outlook.finish_sign_in",
            input: serde_json::Value::Null,
        })
    }

    fn zero_input_actions(&self) -> &'static [&'static str] {
        &[
            "outlook.status",
            "outlook.sign_in",
            "outlook.finish_sign_in",
            "outlook.unread_mail",
            "outlook.today_schedule",
        ]
    }

    fn summarize(&self, action_id: &str, input: &serde_json::Value, _conn: &Connection) -> String {
        match action_id {
            "outlook.send_mail" => match serde_json::from_value::<SendMail>(input.clone()) {
                Ok(mail) => format!(
                    "Email {} with subject \"{}\": {}",
                    mail.to,
                    mail.subject,
                    mail.body.chars().take(200).collect::<String>()
                ),
                Err(_) => "Send an email".to_string(),
            },
            other => ACTIONS
                .iter()
                .find(|s| s.id == other)
                .map(|s| s.summary.to_string())
                .unwrap_or_else(|| other.to_string()),
        }
    }

    fn describe_result(&self, action_id: &str, output: &serde_json::Value) -> Option<String> {
        match action_id {
            "outlook.status" => Some(match output.get("state")?.as_str()? {
                "ready" => format!(
                    "Outlook is connected as {}.",
                    output.get("account").and_then(|v| v.as_str()).unwrap_or("you")
                ),
                "needsAuth" => "Outlook is configured but not signed in. Say \"sign in to Microsoft\".".to_string(),
                _ => "Outlook needs an app registration first: a client id and a tenant id from Microsoft Entra.".to_string(),
            }),

            "outlook.sign_in" => Some(format!(
                "Your browser is open. Enter the code {} to sign in, then say \"finish signing in\".",
                output.get("userCode")?.as_str()?
            )),

            "outlook.finish_sign_in" => Some(match output.get("state")?.as_str()? {
                "done" => format!(
                    "Signed in as {}.",
                    output.get("account").and_then(|v| v.as_str()).unwrap_or("you")
                ),
                "waiting" => "Still waiting for you to approve it in the browser.".to_string(),
                _ => output
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Sign-in did not finish.")
                    .to_string(),
            }),

            "outlook.sign_out" => Some("Signed out of Microsoft.".to_string()),

            "outlook.unread_mail" => {
                let mail = output.get("messages")?.as_array()?;
                if mail.is_empty() {
                    return Some("No unread mail.".to_string());
                }
                let listed: Vec<String> = mail
                    .iter()
                    .take(SPOKEN_CAP)
                    .filter_map(|m| {
                        Some(format!(
                            "{} from {}",
                            m.get("subject")?.as_str()?.chars().take(60).collect::<String>(),
                            m.get("from")?.as_str()?
                        ))
                    })
                    .collect();
                let more = mail.len().saturating_sub(listed.len());
                let mut text = format!(
                    "{} unread: {}",
                    mail.len(),
                    listed.join("; ")
                );
                if more > 0 {
                    text.push_str(&format!("; and {more} more"));
                }
                text.push('.');
                Some(text)
            }

            "outlook.today_schedule" => {
                let events = output.get("events")?.as_array()?;
                if events.is_empty() {
                    return Some("Nothing in your calendar today.".to_string());
                }
                let listed: Vec<String> = events
                    .iter()
                    .take(SPOKEN_CAP)
                    .filter_map(|e| {
                        Some(format!(
                            "{} at {}",
                            e.get("subject")?.as_str()?.chars().take(60).collect::<String>(),
                            e.get("start")?.as_str()?
                        ))
                    })
                    .collect();
                Some(format!(
                    "{} today: {}.",
                    events.len(),
                    listed.join("; ")
                ))
            }

            "outlook.send_mail" => Some(format!(
                "Sent to {}.",
                output.get("to")?.as_str()?
            )),

            _ => None,
        }
    }

    fn observe(
        &self,
        action_id: &str,
        _input: &serde_json::Value,
        output: &serde_json::Value,
        _conn: &Connection,
    ) -> Vec<ReferentDraft> {
        // Senders become referable, so "reply to her" has somewhere to point.
        if action_id != "outlook.unread_mail" {
            return Vec::new();
        }
        output
            .get("messages")
            .and_then(|m| m.as_array())
            .map(|messages| {
                messages
                    .iter()
                    .take(SPOKEN_CAP)
                    .filter_map(|m| {
                        let from = m.get("from")?.as_str()?;
                        Some(ReferentDraft {
                            kind: ReferentKind::Person,
                            display_name: from.to_string(),
                            metadata: serde_json::json!({ "email": from }),
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
        // Status is answerable without a registration, and is how the user
        // finds out they need one.
        if action_id == "outlook.status" {
            let configured = config(ctx.conn, CONNECTOR_ID);
            let state = match &configured {
                None => "unconfigured",
                Some(cfg) if is_signed_in(cfg) => "ready",
                Some(_) => "needsAuth",
            };
            return Ok(serde_json::json!({
                "state": state,
                "account": configured.as_ref().and_then(|c| c.account.clone()),
                "scopes": super::msgraph::SCOPES,
            }));
        }

        let cfg = config(ctx.conn, CONNECTOR_ID).ok_or_else(|| {
            failed(
                "Outlook has no app registration yet. NEXUS needs an Application (client) id \
                 and a Directory (tenant) id from Microsoft Entra."
                    .to_string(),
            )
        })?;

        match action_id {
            "outlook.sign_in" => {
                let prompt = begin_sign_in(&cfg).map_err(failed)?;
                // Opened for them: reading a code out and asking someone to
                // type a URL is a worse experience than it needs to be.
                let _ = run(
                    "/usr/bin/open",
                    &[&prompt.verification_uri],
                    DEFAULT_TIMEOUT,
                );
                Ok(serde_json::json!({
                    "userCode": prompt.user_code,
                    "verificationUri": prompt.verification_uri,
                    "expiresInSeconds": prompt.expires_in_seconds,
                }))
            }

            "outlook.finish_sign_in" => Ok(match poll_sign_in(ctx.conn, &cfg) {
                SignInProgress::Done { account } => {
                    serde_json::json!({ "state": "done", "account": account })
                }
                SignInProgress::Waiting => serde_json::json!({ "state": "waiting" }),
                SignInProgress::Failed { reason } => {
                    serde_json::json!({ "state": "failed", "reason": reason })
                }
            }),

            "outlook.sign_out" => {
                // NEXUS-027. A cached schedule outlives the session that
                // fetched it, and a stale meeting would keep NEXUS quiet
                // about a calendar it can no longer see.
                super::calendar::clear();
                sign_out(&cfg).map_err(failed)?;
                Ok(serde_json::json!({ "signedOut": true }))
            }

            "outlook.unread_mail" => {
                // $select is the privacy control: subjects, senders and times
                // only. The body of an email is never fetched.
                let value = graph_get(
                    &cfg,
                    &format!(
                        "/me/mailFolders/inbox/messages?$filter=isRead%20eq%20false\
                         &$top={PAGE}&$select=subject,from,receivedDateTime\
                         &$orderby=receivedDateTime%20desc"
                    ),
                )
                .map_err(failed)?;

                let messages: Vec<serde_json::Value> = value
                    .get("value")
                    .and_then(|v| v.as_array())
                    .map(|items| {
                        items
                            .iter()
                            .map(|m| {
                                serde_json::json!({
                                    "subject": m.get("subject")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("(no subject)"),
                                    "from": m.get("from")
                                        .and_then(|f| f.get("emailAddress"))
                                        .and_then(|e| e.get("name").or_else(|| e.get("address")))
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("someone"),
                                    "received": m.get("receivedDateTime")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or(""),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                Ok(serde_json::json!({ "messages": messages }))
            }

            "outlook.today_schedule" => {
                let (start, end) = today_bounds(ctx.conn);
                let value = graph_get(
                    &cfg,
                    &format!(
                        "/me/calendarView?startDateTime={start}&endDateTime={end}\
                         &$top={PAGE}&$select=subject,start,end,organizer\
                         &$orderby=start/dateTime"
                    ),
                )
                .map_err(failed)?;

                let events: Vec<serde_json::Value> = value
                    .get("value")
                    .and_then(|v| v.as_array())
                    .map(|items| {
                        items
                            .iter()
                            .map(|e| {
                                let start = e
                                    .get("start")
                                    .and_then(|s| s.get("dateTime"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                let finish = e
                                    .get("end")
                                    .and_then(|s| s.get("dateTime"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                serde_json::json!({
                                    "subject": e.get("subject")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("(no subject)"),
                                    "start": clock_of(start),
                                    // NEXUS-027. Additive: existing readers
                                    // keep using `start`. Both are local
                                    // wall-clock, in the same timezone the
                                    // window was requested in, so comparing
                                    // them to the local clock needs no
                                    // timezone arithmetic and no new crate.
                                    "endsAt": clock_of(finish),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                Ok(serde_json::json!({ "events": events }))
            }

            "outlook.send_mail" => {
                let mail: SendMail = parse(input)?;
                let to = valid_address(&mail.to).ok_or_else(|| ActionError::InvalidInput {
                    detail: format!("\"{}\" is not an email address.", mail.to),
                })?;
                if mail.subject.trim().is_empty() || mail.body.trim().is_empty() {
                    return Err(ActionError::InvalidInput {
                        detail: "An email needs a subject and a body.".to_string(),
                    });
                }

                graph_post(
                    &cfg,
                    "/me/sendMail",
                    serde_json::json!({
                        "message": {
                            "subject": mail.subject.trim(),
                            "body": { "contentType": "Text", "content": mail.body.trim() },
                            "toRecipients": [{ "emailAddress": { "address": to } }]
                        },
                        "saveToSentItems": true
                    }),
                )
                .map_err(failed)?;

                Ok(serde_json::json!({ "to": to, "subject": mail.subject.trim() }))
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
        for (_, sql) in MIGRATIONS {
            conn.execute_batch(sql).expect("migrate");
        }
        crate::assistant::register_connectors(&conn).expect("register");
        conn
    }

    #[test]
    fn message_bodies_are_never_requested() {
        // The privacy control, and it is a `$select`, not a promise.
        let production = include_str!("outlook_connector.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("marker");
        assert!(production.contains("$select=subject,from,receivedDateTime"));
        assert!(
            !production.contains("$select=body") && !production.contains(",body"),
            "an email body must never be fetched"
        );
        assert!(!production.contains("$expand"), "no expansion into content");
    }

    #[test]
    fn sending_writes_always_confirms_and_cannot_be_undone() {
        let send = ACTIONS
            .iter()
            .find(|s| s.id == "outlook.send_mail")
            .expect("present");
        assert_eq!(send.permission, Permission::Write);
        assert_eq!(send.confirm, ConfirmPolicy::Always);
        assert!(!send.reversible);
        assert_eq!(send.reach, Reach::LeavesMachine);
    }

    #[test]
    fn reads_are_marked_as_leaving_the_machine() {
        for id in ["outlook.unread_mail", "outlook.today_schedule"] {
            let spec = ACTIONS.iter().find(|s| s.id == id).expect(id);
            assert_eq!(spec.permission, Permission::Read, "{id}");
            assert_eq!(spec.reach, Reach::LeavesMachine, "{id}");
        }
        // Checking configuration and signing out touch nothing remote.
        for id in ["outlook.status", "outlook.sign_out"] {
            let spec = ACTIONS.iter().find(|s| s.id == id).expect(id);
            assert_eq!(spec.reach, Reach::LocalOnly, "{id}");
        }
    }

    #[test]
    fn an_address_is_checked_before_it_reaches_a_request() {
        assert!(valid_address("alec@acme.com").is_some());
        assert!(valid_address("  rohit.raja@acme.com  ").is_some());
        for bad in [
            "",
            "alec",
            "@acme.com",
            "alec@acme",
            "a b@c.com",
            "alec@ acme.com",
        ] {
            assert!(valid_address(bad).is_none(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn without_a_registration_only_status_is_offered() {
        // And status is how the user finds out they need one.
        let conn = test_conn();
        let caps = OutlookConnector.capabilities(&conn);
        assert_eq!(caps.available, vec!["outlook.status".to_string()]);
        assert!(caps.unavailable.iter().any(|u| u.reason.contains("Entra")));
        assert_eq!(
            OutlookConnector.status(&conn),
            ConnectorStatus::Unconfigured
        );
    }

    #[test]
    fn status_answers_even_with_nothing_configured() {
        let conn = test_conn();
        let out = OutlookConnector
            .dispatch(
                "outlook.status",
                serde_json::json!(null),
                &ExecCtx { conn: &conn },
            )
            .expect("status always answers");
        assert_eq!(out["state"], "unconfigured");
        let described = OutlookConnector
            .describe_result("outlook.status", &out)
            .expect("described");
        assert!(described.contains("Entra"), "{described}");
    }

    #[test]
    fn a_read_without_a_registration_says_what_is_missing() {
        let conn = test_conn();
        let err = OutlookConnector
            .dispatch(
                "outlook.unread_mail",
                serde_json::json!(null),
                &ExecCtx { conn: &conn },
            )
            .expect_err("must fail");
        assert!(format!("{err:?}").contains("client) id"), "{err:?}");
    }

    #[test]
    fn today_is_bounded_to_one_local_day() {
        let conn = test_conn();
        let (start, end) = today_bounds(&conn);
        assert!(start.ends_with("T00:00:00"), "{start}");
        assert!(end.ends_with("T23:59:59"), "{end}");
        assert_eq!(start[..10], end[..10], "both ends must be the same day");
    }

    #[test]
    fn a_timestamp_reads_back_as_a_clock() {
        assert_eq!(clock_of("2026-08-27T14:30:00.0000000"), "14:30");
        assert_eq!(clock_of("nonsense"), "nonsense");
    }

    #[test]
    fn senders_become_referable_but_subjects_do_not() {
        // "reply to her" needs a person. A subject line is not one.
        let conn = test_conn();
        let drafts = OutlookConnector.observe(
            "outlook.unread_mail",
            &serde_json::json!(null),
            &serde_json::json!({ "messages": [
                { "subject": "Budget review", "from": "Alec" },
                { "subject": "Standup", "from": "Priya" }
            ]}),
            &conn,
        );
        assert_eq!(drafts.len(), 2);
        assert!(drafts.iter().all(|d| d.kind == ReferentKind::Person));
        assert_eq!(drafts[0].display_name, "Alec");
    }

    #[test]
    fn action_ids_are_unique_and_namespaced() {
        let mut seen = std::collections::HashSet::new();
        for spec in ACTIONS {
            assert!(seen.insert(spec.id), "duplicate {}", spec.id);
            assert!(spec.id.starts_with("outlook."), "{}", spec.id);
            assert_eq!(spec.connector_id, CONNECTOR_ID);
        }
    }
}
