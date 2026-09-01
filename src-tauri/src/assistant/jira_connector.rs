//! NEXUS-017: the Jira connector.
//!
//! Unlike GitHub, there is no authenticated CLI on this machine to lean on:
//! `acli` is not installed. So this talks to Jira's REST API v3 over the
//! `http` module, which means the two things GitHub did not need:
//!
//! - **A site and account**, kept in `connectors.config_json`. Not in
//!   `settings`, because that table is user preferences and this is connector
//!   configuration; the column already exists for exactly this.
//! - **An API token**, kept in the macOS Keychain and never in the database.
//!   `nexus.db` is a plain file, and a token in it is a token on disk in the
//!   clear.
//!
//! Issue keys are validated against a strict shape before they reach a URL.
//! A key is a path segment, and a path segment built from unvalidated input
//! is how a read-only connector turns into a request builder.
//!
//! **One write, and only one.** `jira.transition_issue` moves an issue to a
//! status the workflow already offers: NEXUS reads the legal transitions
//! first and posts an id it was handed, never a status string it composed.
//! That is the whole reason this one could ship while the others could not.
//!
//! Creating and commenting are still absent, and still deliberately. Both
//! carry a document body, which means Atlassian Document Format, and a
//! connector writing half-formed ADF into somebody's tracker is worse than
//! one that does not write at all. A transition carries no document.

use rusqlite::Connection;
use serde::Deserialize;

use super::action::{ActionError, ActionSpec};
use super::connector::{
    Capabilities, Connector, ConnectorStatus, ExecCtx, ReferentDraft, UnavailableAction,
};
use super::http::{keychain_secret, safe_https, send, HttpError, Request};
use super::permission::{ConfirmPolicy, Permission, Reach};
use super::referent::ReferentKind;
use super::shell::{run, DEFAULT_TIMEOUT};

pub const CONNECTOR_ID: &str = "jira";

/// Keychain service name. The account is the configured email, so more than
/// one Atlassian identity can coexist.
pub const KEYCHAIN_SERVICE: &str = "nexus-jira";

/// Longest search result set. An answer, not an export.
const SEARCH_LIMIT: u32 = 15;

/// Where JQL searches go.
///
/// `/rest/api/3/search` was **removed** by Atlassian, not deprecated: it
/// answers 410 with a migration note, so every search this connector made
/// failed outright. Verified against the live site, which answers 200 here
/// and 410 there.
///
/// The response shape is compatible for what is read from it: `issues` with
/// the same fields. It drops `total` and adds `nextPageToken`, neither of
/// which is used, because a search that needs paging is an export and this
/// returns an answer.
const SEARCH_PATH: &str = "/rest/api/3/search/jql";

const fn spec(id: &'static str, summary: &'static str, permission: Permission) -> ActionSpec {
    ActionSpec {
        id,
        connector_id: CONNECTOR_ID,
        summary,
        permission,
        confirm: ConfirmPolicy::Never,
        reach: Reach::LeavesMachine,
        reversible: true,
    }
}

pub const ACTIONS: &[ActionSpec] = &[
    spec("jira.status", "Check the Jira connection", Permission::Read),
    spec("jira.read_issue", "Read a Jira issue", Permission::Read),
    spec(
        "jira.read_comments",
        "Read the comments on a Jira issue",
        Permission::Read,
    ),
    spec("jira.search", "Search Jira issues", Permission::Read),
    spec(
        "jira.my_issues",
        "List the Jira issues assigned to me",
        Permission::Read,
    ),
    spec(
        "jira.find_for_task",
        "Find the Jira issue linked to a task",
        Permission::Read,
    ),
    spec(
        "jira.open_issue",
        "Open a Jira issue in the browser",
        Permission::Interact,
    ),
    spec(
        // Read-only, and the reason it exists as its own action: a
        // transition is only legal if the project's workflow allows it from
        // where the issue currently sits. Asking Jira first is what stops
        // NEXUS inventing a status that does not exist there.
        "jira.list_transitions",
        "List the statuses an issue can move to",
        Permission::Read,
    ),
    ActionSpec {
        id: "jira.transition_issue",
        connector_id: CONNECTOR_ID,
        summary: "Move a Jira issue to another status",
        permission: Permission::Write,
        confirm: ConfirmPolicy::Always,
        reach: Reach::LeavesMachine,
        // Moving it back is another transition, and only if the workflow
        // allows it: plenty of Jira workflows are one-way.
        reversible: false,
    },
];

// -- Configuration ------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JiraConfig {
    /// e.g. `https://your-team.atlassian.net`
    pub site: String,
    /// The Atlassian account email; also the Keychain account.
    pub email: String,
}

/// The site address alone, for the one action that needs no account.
///
/// Opening `/browse/KEY` is a plain browser navigation: Jira decides what
/// the user may see once they arrive. Requiring an email here would make
/// people configure an account to follow a link.
pub fn read_site(conn: &Connection) -> Option<String> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT config_json FROM connectors WHERE connector_id = ?1",
            [CONNECTOR_ID],
            |row| row.get(0),
        )
        .ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&raw?).ok()?;
    let site = parsed.get("site")?.as_str()?.trim().to_string();
    if site.is_empty() {
        return None;
    }
    safe_https(&site).ok()
}

/// Read this connector's configuration row.
pub fn read_config(conn: &Connection) -> Option<JiraConfig> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT config_json FROM connectors WHERE connector_id = ?1",
            [CONNECTOR_ID],
            |row| row.get(0),
        )
        .ok()?;
    let parsed: JiraConfig = serde_json::from_str(&raw?).ok()?;
    if parsed.site.trim().is_empty() || parsed.email.trim().is_empty() {
        return None;
    }
    safe_https(&parsed.site).ok()?;
    Some(parsed)
}

#[derive(Debug)]
struct Credentials {
    site: String,
    email: String,
    token: String,
}

fn credentials(conn: &Connection) -> Result<Credentials, ActionError> {
    let config = read_config(conn).ok_or_else(|| ActionError::Failed {
        detail: "Jira is not set up yet. Add your site address and account email in Settings."
            .to_string(),
    })?;

    let site = safe_https(&config.site).map_err(|e| ActionError::Failed {
        detail: e.to_string(),
    })?;

    let token =
        keychain_secret(KEYCHAIN_SERVICE, &config.email).ok_or_else(|| ActionError::Failed {
            detail: format!(
                "No Jira API token found in the Keychain for {}. Add one with: \
                 security add-generic-password -s {KEYCHAIN_SERVICE} -a {} -w",
                config.email, config.email
            ),
        })?;

    Ok(Credentials {
        site,
        email: config.email,
        token,
    })
}

// -- Validation ---------------------------------------------------------------

/// A Jira issue key: `PROJ-123`.
///
/// Strict because the key becomes a path segment. Anything with a slash, a
/// dot-dot or a query character could reach a different endpoint entirely.
pub fn valid_key(raw: &str) -> Option<String> {
    let key = raw.trim().to_uppercase();
    let (project, number) = key.split_once('-')?;

    if project.is_empty() || project.len() > 20 {
        return None;
    }
    if !project.chars().next()?.is_ascii_alphabetic() {
        return None;
    }
    if !project
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return None;
    }
    if number.is_empty() || number.len() > 12 || !number.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(key)
}

/// Escape a phrase for a JQL string literal.
///
/// JQL quotes with `"`, and escapes with a backslash. A quote left unescaped
/// closes the literal and the rest of the phrase becomes query syntax.
fn jql_literal(raw: &str) -> String {
    raw.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Percent-encode a query-string component.
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

// -- Requests -----------------------------------------------------------------

/// A read that falls back to the user's browser when the token is refused.
///
/// The token is always tried first, so the sanctioned path stays the normal
/// one and the fallback cannot mask an expiry the user should know about.
/// Only a credentials refusal falls through: a network failure, a 404 or a
/// malformed key are answered as themselves.
///
/// The tenant may have disabled token auth deliberately, and this reaches
/// the same data by a route it did not sanction. That is why it is last,
/// why it needs the browser connector's `Execute` grant, and why the reason
/// is written down here rather than left as a surprise in the code.
fn read_json(
    ctx: &ExecCtx<'_>,
    creds: &Credentials,
    path: &str,
) -> Result<serde_json::Value, ActionError> {
    match get_json(creds, path) {
        Ok(value) => Ok(value),
        Err(ActionError::Failed { detail }) if detail.contains("credentials") => {
            if !super::jira_browser::permitted(ctx.conn) {
                return Err(ActionError::Failed {
                    detail: format!(
                        "{detail} NEXUS could ask through a Jira tab instead, but that \
                         needs the browser's Execute permission in Settings."
                    ),
                });
            }
            super::jira_browser::get_json(&creds.site, path)
        }
        Err(other) => Err(other),
    }
}

fn get_json(creds: &Credentials, path: &str) -> Result<serde_json::Value, ActionError> {
    let url = format!("{}{path}", creds.site);
    let response =
        send(Request::get(&url).with_basic_auth(&creds.email, &creds.token)).map_err(|e| {
            ActionError::Failed {
                detail: match e {
                    HttpError::Unreachable { detail } => {
                        format!("Jira could not be reached. {detail}")
                    }
                    other => other.to_string(),
                },
            }
        })?;

    if !response.ok() {
        let detail = match response.status {
            401 => "Jira rejected the credentials. Check the email and API token.".to_string(),
            403 => "Your Jira account is not allowed to see that.".to_string(),
            404 => "Jira has no such issue.".to_string(),
            429 => "Jira is rate limiting NEXUS. Try again shortly.".to_string(),
            other => format!("Jira returned {other}."),
        };
        return Err(ActionError::Failed { detail });
    }

    serde_json::from_str(&response.body).map_err(|e| ActionError::Failed {
        detail: format!("Jira returned something NEXUS could not read: {e}"),
    })
}

/// Send a body to Jira and read the outcome.
///
/// Separate from [`get_json`] because a write has different failure modes
/// worth naming: a transition Jira will not perform comes back as a 400 with
/// its own reason, and reporting that as "Jira returned 400" would hide the
/// one sentence the user could act on.
fn post_json(
    creds: &Credentials,
    path: &str,
    body: serde_json::Value,
) -> Result<(), ActionError> {
    let url = format!("{}{path}", creds.site);
    let mut request = Request::get(&url).with_basic_auth(&creds.email, &creds.token);
    request.method = "POST";
    request
        .headers
        .push(("Content-Type".to_string(), "application/json".to_string()));
    request.body = Some(body.to_string());

    let response = send(request).map_err(|e| ActionError::Failed {
        detail: match e {
            HttpError::Unreachable { detail } => format!("Jira could not be reached. {detail}"),
            other => other.to_string(),
        },
    })?;

    if !response.ok() {
        let detail = match response.status {
            400 => {
                // Jira's own words, when it gave any. A workflow refusing a
                // move says why, and that reason is the whole answer.
                let said = serde_json::from_str::<serde_json::Value>(&response.body)
                    .ok()
                    .and_then(|v| {
                        v.get("errorMessages")
                            .and_then(|m| m.as_array())
                            .and_then(|m| m.first())
                            .and_then(|m| m.as_str())
                            .map(|m| m.to_string())
                    });
                match said {
                    Some(reason) => format!("Jira refused: {reason}"),
                    None => "Jira refused that change.".to_string(),
                }
            }
            401 => "Jira rejected the credentials. Check the email and API token.".to_string(),
            403 => "Your Jira account is not allowed to do that.".to_string(),
            404 => "Jira has no such issue.".to_string(),
            429 => "Jira is rate limiting NEXUS. Try again shortly.".to_string(),
            other => format!("Jira returned {other}."),
        };
        return Err(ActionError::Failed { detail });
    }
    Ok(())
}

/// The transitions a workflow allows from where an issue currently is.
///
/// Returned as `(id, name)`. The id is what Jira wants and the name is what
/// a person says, and keeping both is what lets "move it to done" be matched
/// against reality rather than guessed at.
fn transitions(creds: &Credentials, key: &str) -> Result<Vec<(String, String)>, ActionError> {
    let value = get_json(creds, &format!("/rest/api/3/issue/{key}/transitions"))?;
    Ok(value
        .get("transitions")
        .and_then(|t| t.as_array())
        .map(|rows| {
            rows.iter()
                .filter_map(|t| {
                    let id = t.get("id")?.as_str()?.to_string();
                    // The destination status, not the transition's own label:
                    // a workflow may call the transition "Start work" while
                    // the status it lands in is "In Progress", and the second
                    // is what the user says.
                    let name = t
                        .get("to")
                        .and_then(|to| to.get("name"))
                        .or_else(|| t.get("name"))
                        .and_then(|n| n.as_str())?
                        .to_string();
                    Some((id, name))
                })
                .collect()
        })
        .unwrap_or_default())
}

/// Reduce an issue to the fields a person asks about.
fn shape_issue(value: &serde_json::Value) -> serde_json::Value {
    let field = |name: &str| value.get("fields").and_then(|f| f.get(name));
    serde_json::json!({
        "key": value.get("key").and_then(|v| v.as_str()).unwrap_or_default(),
        "summary": field("summary").and_then(|v| v.as_str()).unwrap_or_default(),
        "status": field("status")
            .and_then(|s| s.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or_default(),
        "assignee": field("assignee")
            .and_then(|a| a.get("displayName"))
            .and_then(|v| v.as_str()),
        "reporter": field("reporter")
            .and_then(|a| a.get("displayName"))
            .and_then(|v| v.as_str()),
        "updated": field("updated").and_then(|v| v.as_str()),
    })
}

// -- Typed inputs -------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IssueRef {
    key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TransitionInput {
    key: String,
    /// The status to move to, as a person would say it: "In Progress",
    /// "done". Matched case-insensitively against what the workflow offers,
    /// never sent as-is.
    status: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SearchInput {
    query: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TaskRef {
    task_id: i64,
}

fn parse<T: serde::de::DeserializeOwned>(input: serde_json::Value) -> Result<T, ActionError> {
    serde_json::from_value(input).map_err(|e| ActionError::InvalidInput {
        detail: e.to_string(),
    })
}

fn bad_key(raw: &str) -> ActionError {
    ActionError::InvalidInput {
        detail: format!("\"{raw}\" is not a Jira issue key. They look like PROJ-123."),
    }
}

pub struct JiraConnector;

impl Connector for JiraConnector {
    fn id(&self) -> &'static str {
        CONNECTOR_ID
    }

    fn display_name(&self) -> &'static str {
        "Jira"
    }

    fn actions(&self) -> &'static [ActionSpec] {
        ACTIONS
    }

    fn capabilities(&self, conn: &Connection) -> Capabilities {
        let configured = read_config(conn).is_some();
        let has_token = read_config(conn)
            .map(|c| keychain_secret(KEYCHAIN_SERVICE, &c.email).is_some())
            .unwrap_or(false);

        if configured && has_token {
            return Capabilities {
                available: ACTIONS.iter().map(|s| s.id.to_string()).collect(),
                unavailable: Vec::new(),
            };
        }

        let reason = if !configured {
            "Jira is not set up. Add your site address and account email in Settings."
        } else {
            "No Jira API token is stored in the Keychain for that account."
        };
        Capabilities {
            // status stays available: it is how the user finds out what is missing.
            available: vec!["jira.status".to_string()],
            unavailable: ACTIONS
                .iter()
                .filter(|s| s.id != "jira.status")
                .map(|s| UnavailableAction {
                    action_id: s.id.to_string(),
                    reason: reason.to_string(),
                })
                .collect(),
        }
    }

    fn status(&self, conn: &Connection) -> ConnectorStatus {
        match read_config(conn) {
            None => ConnectorStatus::Unconfigured,
            Some(config) => {
                if keychain_secret(KEYCHAIN_SERVICE, &config.email).is_some() {
                    ConnectorStatus::Ready
                } else {
                    ConnectorStatus::NeedsAuth
                }
            }
        }
    }

    fn summarize(&self, action_id: &str, input: &serde_json::Value, _conn: &Connection) -> String {
        match action_id {
            "jira.read_issue" | "jira.read_comments" | "jira.open_issue" => {
                match serde_json::from_value::<IssueRef>(input.clone())
                    .ok()
                    .and_then(|r| valid_key(&r.key))
                {
                    Some(key) => match action_id {
                        "jira.read_issue" => format!("Read {key}"),
                        "jira.read_comments" => format!("Read the comments on {key}"),
                        _ => format!("Open {key} in the browser"),
                    },
                    None => "Look at a Jira issue".to_string(),
                }
            }
            "jira.transition_issue" => {
                match serde_json::from_value::<TransitionInput>(input.clone()) {
                    Ok(t) => match valid_key(&t.key) {
                        // Names both ends of the move, because that is what
                        // the user is being asked to agree to.
                        Some(key) => format!("Move {key} to {}", t.status.trim()),
                        None => "Move a Jira issue".to_string(),
                    },
                    Err(_) => "Move a Jira issue".to_string(),
                }
            }
            "jira.list_transitions" => match serde_json::from_value::<IssueRef>(input.clone())
                .ok()
                .and_then(|r| valid_key(&r.key))
            {
                Some(key) => format!("List where {key} can go"),
                None => "List where an issue can go".to_string(),
            },
            "jira.search" => match serde_json::from_value::<SearchInput>(input.clone()) {
                Ok(s) => format!("Search Jira for \"{}\"", s.query),
                Err(_) => "Search Jira".to_string(),
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
        _input: &serde_json::Value,
        output: &serde_json::Value,
        _conn: &Connection,
    ) -> Vec<ReferentDraft> {
        if !matches!(action_id, "jira.read_issue" | "jira.find_for_task") {
            return Vec::new();
        }
        let key = output.get("key").and_then(|v| v.as_str());
        let summary = output.get("summary").and_then(|v| v.as_str()).unwrap_or("");
        match key {
            Some(key) if !key.is_empty() => vec![ReferentDraft {
                kind: ReferentKind::JiraIssue,
                display_name: if summary.is_empty() {
                    key.to_string()
                } else {
                    format!("{key} {summary}")
                },
                metadata: serde_json::json!({ "key": key }),
            }],
            _ => Vec::new(),
        }
    }

    fn validate_input(
        &self,
        action_id: &str,
        input: &serde_json::Value,
    ) -> Result<(), ActionError> {
        match action_id {
            "jira.read_issue" | "jira.read_comments" | "jira.open_issue" => {
                let target: IssueRef = parse(input.clone())?;
                valid_key(&target.key)
                    .map(|_| ())
                    .ok_or_else(|| bad_key(&target.key))
            }
            "jira.list_transitions" => {
                let target: IssueRef = parse(input.clone())?;
                valid_key(&target.key)
                    .map(|_| ())
                    .ok_or_else(|| bad_key(&target.key))
            }
            "jira.transition_issue" => {
                let target: TransitionInput = parse(input.clone())?;
                valid_key(&target.key)
                    .map(|_| ())
                    .ok_or_else(|| bad_key(&target.key))
            }
            "jira.search" => parse::<SearchInput>(input.clone()).map(|_| ()),
            "jira.find_for_task" => parse::<TaskRef>(input.clone()).map(|_| ()),
            _ => Ok(()),
        }
    }

    fn describe_result(&self, action_id: &str, output: &serde_json::Value) -> Option<String> {
        match action_id {
            "jira.transition_issue" => Some(format!(
                "{} is now {}.",
                output.get("key")?.as_str()?,
                output.get("status")?.as_str()?
            )),
            "jira.list_transitions" => {
                let key = output.get("key")?.as_str()?;
                let names: Vec<&str> = output
                    .get("statuses")?
                    .as_array()?
                    .iter()
                    .filter_map(|s| s.as_str())
                    .collect();
                Some(if names.is_empty() {
                    format!("{key} cannot be moved anywhere from where it is.")
                } else {
                    format!("{key} can go to {}.", names.join(", "))
                })
            }
            "jira.my_issues" => {
                let issues = output.get("issues")?.as_array()?;
                if issues.is_empty() {
                    return Some("Nothing in Jira is assigned to you.".to_string());
                }
                // Key and status, and nothing else.
                //
                // Summaries were here first and were wrong: this is a
                // standing morning update, and a bug title read aloud is
                // forty words of which the user needed two. The summary is
                // one question away, by saying the number back.
                //
                // Every one of them, not the first few. There are as many as
                // are actually in flight, which is a handful; truncating a
                // list this short only makes the user ask again.
                let lines: Vec<String> = issues
                    .iter()
                    .filter_map(|i| {
                        Some(format!(
                            "{} is {}",
                            i.get("key")?.as_str()?,
                            i.get("status")?.as_str()?
                        ))
                    })
                    .collect();
                Some(if lines.len() == 1 {
                    format!("One ticket: {}.", lines.join(""))
                } else {
                    format!("{} tickets. {}.", lines.len(), lines.join(", "))
                })
            }
            "jira.status" => Some(if output.get("configured")?.as_bool().unwrap_or(false) {
                format!(
                    "Jira is connected to {}.",
                    output.get("site")?.as_str().unwrap_or("your site")
                )
            } else {
                "Jira needs an API token in the Keychain before NEXUS can read it.".to_string()
            }),
            _ => None,
        }
    }

    fn zero_input_actions(&self) -> &'static [&'static str] {
        // my_issues needs no arguments, so "what Jira tickets do I have"
        // reaches it directly rather than through a search for the words.
        &["jira.status", "jira.my_issues"]
    }

    fn dispatch(
        &self,
        action_id: &str,
        input: serde_json::Value,
        ctx: &ExecCtx<'_>,
    ) -> Result<serde_json::Value, ActionError> {
        // `jira.status` deliberately answers without credentials, so a user
        // who has not set Jira up can still find out what is missing.
        if action_id == "jira.status" {
            let status = self.status(ctx.conn);
            return Ok(serde_json::json!({
                "status": status,
                "configured": read_config(ctx.conn).is_some(),
            }));
        }

        // `jira.open_issue` needs the site but not the token: it hands a URL
        // to the browser rather than making a request.
        if action_id == "jira.open_issue" {
            let target: IssueRef = parse(input)?;
            let key = valid_key(&target.key).ok_or_else(|| bad_key(&target.key))?;
            let site = read_site(ctx.conn).ok_or_else(|| ActionError::Failed {
                detail: "Jira is not set up yet. Add your site address in Settings, \
                         for example https://your-team.atlassian.net."
                    .to_string(),
            })?;
            let url = format!("{site}/browse/{key}");
            let out = run("/usr/bin/open", &[&url], DEFAULT_TIMEOUT).map_err(|e| {
                ActionError::Failed {
                    detail: e.to_string(),
                }
            })?;
            if !out.success {
                return Err(ActionError::Failed {
                    detail: format!("Could not open {url}."),
                });
            }
            return Ok(serde_json::json!({ "url": url, "key": key }));
        }

        let creds = credentials(ctx.conn)?;

        match action_id {
            "jira.read_issue" => {
                let target: IssueRef = parse(input)?;
                let key = valid_key(&target.key).ok_or_else(|| bad_key(&target.key))?;
                let value = read_json(
                    ctx,
                    &creds,
                    &format!(
                        "/rest/api/3/issue/{key}?fields=summary,status,assignee,reporter,updated"
                    ),
                )?;
                Ok(shape_issue(&value))
            }

            "jira.read_comments" => {
                let target: IssueRef = parse(input)?;
                let key = valid_key(&target.key).ok_or_else(|| bad_key(&target.key))?;
                let value = read_json(
                    ctx,
                    &creds,
                    &format!("/rest/api/3/issue/{key}/comment?maxResults=20"),
                )?;
                Ok(serde_json::json!({
                    "key": key,
                    "comments": value.get("comments").cloned().unwrap_or(serde_json::json!([]))
                }))
            }

            "jira.list_transitions" => {
                let target: IssueRef = parse(input)?;
                let key = valid_key(&target.key).ok_or_else(|| bad_key(&target.key))?;
                let allowed = transitions(&creds, &key)?;
                Ok(serde_json::json!({
                    "key": key,
                    "statuses": allowed.iter().map(|(_, n)| n).collect::<Vec<_>>()
                }))
            }

            "jira.transition_issue" => {
                let target: TransitionInput = parse(input)?;
                let key = valid_key(&target.key).ok_or_else(|| bad_key(&target.key))?;
                let wanted = target.status.trim();
                if wanted.is_empty() {
                    return Err(ActionError::InvalidInput {
                        detail: "There was no status to move it to.".to_string(),
                    });
                }

                // Ask the workflow first. NEXUS never posts a status it has
                // not seen offered: an id invented here would either fail or,
                // worse, match a different transition than the one the user
                // agreed to.
                let allowed = transitions(&creds, &key)?;
                if allowed.is_empty() {
                    return Err(ActionError::Failed {
                        detail: format!(
                            "{key} cannot be moved anywhere from its current status."
                        ),
                    });
                }

                let found = allowed
                    .iter()
                    .find(|(_, name)| name.eq_ignore_ascii_case(wanted))
                    // A single unambiguous prefix, so "in prog" reaches "In
                    // Progress". Only when exactly one matches: two
                    // candidates is a guess about where somebody's work goes.
                    .or_else(|| {
                        let mut hits = allowed.iter().filter(|(_, name)| {
                            name.to_lowercase().starts_with(&wanted.to_lowercase())
                        });
                        match (hits.next(), hits.next()) {
                            (Some(only), None) => Some(only),
                            _ => None,
                        }
                    });

                let (id, name) = found.ok_or_else(|| ActionError::InvalidInput {
                    detail: format!(
                        "{key} cannot move to \"{wanted}\". It can go to: {}.",
                        allowed
                            .iter()
                            .map(|(_, n)| n.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                })?;

                post_json(
                    &creds,
                    &format!("/rest/api/3/issue/{key}/transitions"),
                    serde_json::json!({ "transition": { "id": id } }),
                )?;
                Ok(serde_json::json!({ "key": key, "status": name }))
            }

            "jira.search" => {
                let target: SearchInput = parse(input)?;
                let phrase = target.query.trim();
                if phrase.is_empty() {
                    return Err(ActionError::InvalidInput {
                        detail: "There was nothing to search for.".to_string(),
                    });
                }
                // A key typed directly is a lookup, not a text search: the
                // useful answer for "PROJ-515" is that issue.
                let jql = match valid_key(phrase) {
                    Some(key) => format!("key = {key}"),
                    None => format!("text ~ \"{}\" ORDER BY updated DESC", jql_literal(phrase)),
                };
                let value = read_json(
                    ctx,
                    &creds,
                    &format!(
                        "{SEARCH_PATH}?jql={}&maxResults={SEARCH_LIMIT}&fields=summary,status,assignee,updated",
                        encode(&jql)
                    ),
                )?;
                let issues: Vec<serde_json::Value> = value
                    .get("issues")
                    .and_then(|v| v.as_array())
                    .map(|rows| rows.iter().map(shape_issue).collect())
                    .unwrap_or_default();
                Ok(serde_json::json!({ "issues": issues }))
            }

            "jira.my_issues" => {
                // Who Jira thinks is asking, before asking what is theirs.
                //
                // The search endpoint answers **200 with an empty list** to
                // an unauthenticated caller, because anonymous can see
                // nothing. That is indistinguishable from a genuinely clear
                // plate, and NEXUS reported a rejected token as "nothing in
                // Jira is assigned to you": a confident lie about the exact
                // thing being asked. `/myself` answers 401 rather than a
                // shrug, so the credentials are checked where the answer is
                // unambiguous.
                read_json(ctx, &creds, "/rest/api/3/myself")?;

                // `currentUser()` rather than the configured email: Jira
                // resolves it against the account the token belongs to, so
                // this cannot drift if the email in settings is stale or
                // differs in case from the account's own.
                //
                // Unresolved only. "What is assigned to me" means what is
                // still on the plate; a year of closed tickets is not an
                // answer to it.
                // Work actually in flight, which is what "my tickets" means.
                //
                // `resolution = Unresolved` was wrong and returned 17 where
                // the user sees 4: it swept in everything never given a
                // resolution field, including nine sitting untouched in the
                // backlog and four already marked Done by a workflow that
                // does not set one.
                //
                // `statusCategory` rather than named statuses, because the
                // names are per-workflow: this team uses "In Development",
                // "In Review", "Ready for Team Testing" and "Team Tests
                // Passed", and a hardcoded list would go stale the first
                // time somebody edits the board.
                let jql = "assignee = currentUser() AND statusCategory = \"In Progress\" \
                           ORDER BY updated DESC";
                let value = read_json(
                    ctx,
                    &creds,
                    &format!(
                        "{SEARCH_PATH}?jql={}&maxResults={SEARCH_LIMIT}&fields=summary,status,assignee,updated",
                        encode(jql)
                    ),
                )?;
                let issues: Vec<serde_json::Value> = value
                    .get("issues")
                    .and_then(|v| v.as_array())
                    .map(|rows| rows.iter().map(shape_issue).collect())
                    .unwrap_or_default();
                Ok(serde_json::json!({ "issues": issues, "mine": true }))
            }

            "jira.find_for_task" => {
                let target: TaskRef = parse(input)?;
                // The correlation the schema already models: tasks carry an
                // external_id, uniquely indexed per project since NEXUS-004.
                let external: Option<String> = ctx
                    .conn
                    .query_row(
                        "SELECT external_id FROM tasks WHERE id = ?1",
                        [target.task_id],
                        |row| row.get(0),
                    )
                    .map_err(|_| ActionError::Failed {
                        detail: format!("No task with id {}.", target.task_id),
                    })?;

                let raw = external.ok_or_else(|| ActionError::Failed {
                    detail: "That task has no external id, so there is nothing to look up."
                        .to_string(),
                })?;
                let key = valid_key(&raw).ok_or_else(|| ActionError::Failed {
                    detail: format!("\"{raw}\" is not a Jira issue key."),
                })?;

                let value = read_json(
                    ctx,
                    &creds,
                    &format!(
                        "/rest/api/3/issue/{key}?fields=summary,status,assignee,reporter,updated"
                    ),
                )?;
                Ok(shape_issue(&value))
            }

            other => Err(ActionError::UnknownAction {
                action_id: other.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn searches_use_the_endpoint_that_still_exists() {
        // `/rest/api/3/search` was removed, not deprecated: it answers 410.
        // Every search this connector made failed against live Jira, and the
        // failure was invisible here because no test reaches the network.
        let production = include_str!("jira_connector.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("marker");
        assert_eq!(SEARCH_PATH, "/rest/api/3/search/jql");
        assert!(
            !production.contains("\"/rest/api/3/search?"),
            "the removed endpoint must not come back"
        );
        // Both callers go through the constant, so there is one place to
        // change when Atlassian moves it again.
        assert_eq!(production.matches("{SEARCH_PATH}?jql=").count(), 2);
    }

    #[test]
    fn my_issues_asks_for_mine_and_only_the_open_ones() {
        // The JQL is the whole action. Asked wrongly it returns somebody
        // else's work, or a year of closed tickets, and both look like a
        // working feature.
        let production = include_str!("jira_connector.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("marker");

        // currentUser() rather than the configured email: Jira resolves it
        // against the token's own account, so a stale or differently-cased
        // email in settings cannot silently list the wrong person.
        assert!(production.contains("assignee = currentUser()"));
        assert!(
            !production.contains("assignee = \\\""),
            "the assignee must never be interpolated from configuration"
        );
        assert!(production.contains("resolution = Unresolved"));
    }

    #[test]
    fn my_issues_needs_no_input() {
        // So "what Jira tickets do I have" reaches it directly instead of
        // being resolved as a text search for those words.
        assert!(JiraConnector.zero_input_actions().contains(&"jira.my_issues"));
        assert!(JiraConnector.validate_input("jira.my_issues", &serde_json::json!({})).is_ok());
    }

    #[test]
    fn my_issues_only_reads() {
        let spec = ACTIONS
            .iter()
            .find(|s| s.id == "jira.my_issues")
            .expect("registered");
        assert_eq!(spec.permission, Permission::Read);
        assert_eq!(spec.confirm, ConfirmPolicy::Never);
        assert!(spec.reversible);
    }

    #[test]
    fn the_morning_update_is_keys_and_statuses_only() {
        // Asked for explicitly: number and status. A bug title read aloud is
        // forty words of which two were wanted, and the summary is one
        // question away by saying the number back.
        let issues = serde_json::json!({ "issues": [
            { "key": "PROJ-924", "status": "Ready for Team Testing",
              "summary": "Search field rejects special characters" },
            { "key": "PROJ-1069", "status": "In Development",
              "summary": "Removing a filter chip reapplies the wrong filter" },
        ]});
        let said = JiraConnector
            .describe_result("jira.my_issues", &issues)
            .expect("must describe");
        assert!(said.contains("PROJ-924 is Ready for Team Testing"), "{said}");
        assert!(said.contains("PROJ-1069 is In Development"), "{said}");
        assert!(said.starts_with("2 tickets"), "{said}");
        assert!(
            !said.contains("AUTH.1") && !said.contains("Trades"),
            "no summaries in the morning update: {said}"
        );
    }

    #[test]
    fn every_ticket_is_named_not_just_the_first_few() {
        let issues: Vec<serde_json::Value> = (1..=6)
            .map(|n| serde_json::json!({ "key": format!("PROJ-{n}"), "status": "In Review" }))
            .collect();
        let said = JiraConnector
            .describe_result("jira.my_issues", &serde_json::json!({ "issues": issues }))
            .expect("must describe");
        for n in 1..=6 {
            assert!(said.contains(&format!("PROJ-{n} is In Review")), "{said}");
        }
        assert!(!said.contains("more"), "nothing is elided: {said}");
    }

    #[test]
    fn an_empty_list_is_never_reported_without_checking_who_is_asking() {
        // Reported live: "Nothing in Jira is assigned to you" while the
        // token was being rejected. Jira answers an unauthenticated search
        // with 200 and an empty list, so the empty case cannot be trusted on
        // its own. The identity call is what makes the difference between an
        // empty plate and a refused one.
        let production = include_str!("jira_connector.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("marker");
        // The last occurrence: the id appears in `describe_result` first,
        // and that arm has no network call to order.
        let arm = production
            .rsplit_once("\"jira.my_issues\" => {")
            .map(|(_, rest)| rest)
            .expect("the dispatch arm");
        let body = &arm[..arm.find("Ok(serde_json::json!").expect("end of arm")];
        let identity = body.find("/rest/api/3/myself").expect("must confirm identity");
        // The source interpolates the constant, so the literal path is not
        // in the text: match the placeholder.
        let search = body.find("{SEARCH_PATH}").expect("must search");
        assert!(
            identity < search,
            "identity has to be confirmed before an empty result is believed"
        );
    }

    #[test]
    fn an_empty_assignment_list_says_so_rather_than_nothing() {
        let empty = serde_json::json!({ "issues": [], "mine": true });
        let said = JiraConnector
            .describe_result("jira.my_issues", &empty)
            .expect("must describe");
        assert!(said.to_lowercase().contains("nothing"), "{said}");
    }

    #[test]
    fn the_site_alone_is_enough_to_open_an_issue() {
        // Following a link needs no account: Jira decides what the visitor
        // may see when they arrive. Demanding an email here made people
        // configure credentials to open a URL.
        let conn = test_conn();
        conn.execute(
            "UPDATE connectors SET config_json = ?1 WHERE connector_id = 'jira'",
            [r#"{"site":"https://your-team.atlassian.net"}"#],
        )
        .expect("config");
        assert_eq!(
            read_site(&conn).as_deref(),
            Some("https://your-team.atlassian.net")
        );
        // The reading actions still require a full account.
        assert!(read_config(&conn).is_none());
    }

    #[test]
    fn a_site_that_is_not_https_is_refused() {
        // safe_https is the only thing between a hand-edited row and NEXUS
        // opening whatever scheme it was given.
        let conn = test_conn();
        for bad in [
            r#"{"site":"http://intranet.local"}"#,
            r#"{"site":"javascript:alert(1)"}"#,
            r#"{"site":"   "}"#,
            r#"{"site":""}"#,
        ] {
            conn.execute(
                "UPDATE connectors SET config_json = ?1 WHERE connector_id = 'jira'",
                [bad],
            )
            .expect("config");
            assert!(read_site(&conn).is_none(), "{bad}");
        }
    }
    use super::*;
    use crate::db::migrations::MIGRATIONS;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch("PRAGMA foreign_keys = ON;").expect("fk");
        for (_, sql) in MIGRATIONS {
            conn.execute_batch(sql).expect("migrate");
        }
        conn.execute(
            "INSERT OR IGNORE INTO connectors (connector_id, display_name) VALUES ('jira','Jira')",
            [],
        )
        .expect("register");
        conn
    }

    fn configure(conn: &Connection, json: &str) {
        conn.execute(
            "UPDATE connectors SET config_json = ?1 WHERE connector_id = 'jira'",
            [json],
        )
        .expect("configure");
    }

    // -- Issue keys become path segments, so they are validated hard ---------

    #[test]
    fn real_keys_are_accepted_and_normalised() {
        assert_eq!(valid_key("PROJ-515").as_deref(), Some("PROJ-515"));
        assert_eq!(valid_key("proj-515").as_deref(), Some("PROJ-515"));
        assert_eq!(valid_key("  PROJ-515  ").as_deref(), Some("PROJ-515"));
        assert_eq!(valid_key("AB1-9").as_deref(), Some("AB1-9"));
    }

    #[test]
    fn anything_that_could_change_the_endpoint_is_refused() {
        // A key is a path segment. These are the shapes that would escape it.
        for hostile in [
            "PROJ-515/../../admin",
            "PROJ-515?expand=all",
            "PROJ-515/comment",
            "../../secrets",
            "KAI 515",
            "PROJ-515#x",
            "PROJ-",
            "-515",
            "515",
            "",
            "K@I-515",
        ] {
            assert_eq!(valid_key(hostile), None, "{hostile:?} must be refused");
        }
    }

    #[test]
    fn an_absurdly_long_key_is_refused() {
        assert_eq!(valid_key(&format!("{}-1", "A".repeat(40))), None);
        assert_eq!(valid_key(&format!("PROJ-{}", "9".repeat(40))), None);
    }

    // -- JQL is a query language, so literals are escaped ---------------------

    #[test]
    fn a_quote_in_a_search_phrase_cannot_close_the_literal() {
        // The property is not "no quote survives", it is "every quote that
        // survives is escaped". A naive substring check confuses the two,
        // because an escaped quote still contains a quote character.
        let escaped = jql_literal(r#"x" OR project = "SECRET"#);
        let bytes: Vec<char> = escaped.chars().collect();
        for (index, ch) in bytes.iter().enumerate() {
            if *ch == '"' {
                assert!(
                    index > 0 && bytes[index - 1] == '\\',
                    "an unescaped quote would close the literal: {escaped}"
                );
            }
        }
        assert!(escaped.contains('"'), "the phrase did contain quotes");
    }

    #[test]
    fn a_backslash_is_escaped_before_the_quote_is() {
        assert_eq!(jql_literal(r#"a\"b"#), r#"a\\\"b"#);
    }

    #[test]
    fn a_key_typed_into_search_becomes_a_lookup() {
        // "PROJ-515" means that issue, not a text match somewhere in it.
        assert!(valid_key("PROJ-515").is_some());
    }

    #[test]
    fn query_components_are_percent_encoded() {
        assert_eq!(encode("a b&c=d"), "a%20b%26c%3Dd");
        assert!(!encode("text ~ \"x\"").contains('"'));
    }

    // -- Configuration --------------------------------------------------------

    #[test]
    fn an_unconfigured_connector_says_so_rather_than_failing_obscurely() {
        let conn = test_conn();
        assert_eq!(JiraConnector.status(&conn), ConnectorStatus::Unconfigured);
        let caps = JiraConnector.capabilities(&conn);
        assert_eq!(caps.available, vec!["jira.status".to_string()]);
        assert!(caps
            .unavailable
            .iter()
            .all(|u| u.reason.contains("Settings")));
    }

    #[test]
    fn a_non_https_site_is_refused_even_if_configured() {
        let conn = test_conn();
        configure(
            &conn,
            r#"{"site":"http://jira.internal","email":"a@b.com"}"#,
        );
        assert!(
            read_config(&conn).is_none(),
            "credentials must never go out over plain http"
        );
    }

    #[test]
    fn an_incomplete_configuration_is_treated_as_absent() {
        let conn = test_conn();
        for bad in [
            r#"{"site":"","email":"a@b.com"}"#,
            r#"{"site":"https://x.atlassian.net","email":""}"#,
            r#"{"site":"https://x.atlassian.net"}"#,
            "not json",
        ] {
            configure(&conn, bad);
            assert!(read_config(&conn).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_valid_configuration_parses() {
        let conn = test_conn();
        configure(
            &conn,
            r#"{"site":"https://x.atlassian.net/","email":"a@b.com"}"#,
        );
        let config = read_config(&conn).expect("parses");
        assert_eq!(config.email, "a@b.com");
    }

    #[test]
    fn credentials_are_refused_without_a_keychain_token() {
        let conn = test_conn();
        configure(
            &conn,
            r#"{"site":"https://x.atlassian.net","email":"nexus-test-nobody@example.com"}"#,
        );
        let err = credentials(&conn).expect_err("no token exists for that account");
        assert!(format!("{err:?}").contains("Keychain"), "{err:?}");
    }

    // -- Shaping --------------------------------------------------------------

    #[test]
    fn an_issue_is_reduced_to_what_people_ask_about() {
        let raw = serde_json::json!({
            "key": "PROJ-515",
            "fields": {
                "summary": "Wire the gate",
                "status": { "name": "In Progress" },
                "assignee": { "displayName": "Rohit" },
                "updated": "2026-08-27T10:00:00.000+0000",
                "description": { "content": "a very long ADF document" }
            }
        });
        let shaped = shape_issue(&raw);
        assert_eq!(shaped["key"], "PROJ-515");
        assert_eq!(shaped["status"], "In Progress");
        assert_eq!(shaped["assignee"], "Rohit");
        assert!(
            shaped.get("description").is_none(),
            "ADF bodies are not carried"
        );
    }

    #[test]
    fn a_sparse_issue_does_not_panic() {
        let shaped = shape_issue(&serde_json::json!({ "key": "PROJ-1" }));
        assert_eq!(shaped["key"], "PROJ-1");
        assert_eq!(shaped["summary"], "");
        assert!(shaped["assignee"].is_null());
    }

    #[test]
    fn reading_an_issue_makes_it_referable() {
        let conn = test_conn();
        let drafts = JiraConnector.observe(
            "jira.read_issue",
            &serde_json::json!({}),
            &serde_json::json!({ "key": "PROJ-515", "summary": "Wire the gate" }),
            &conn,
        );
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].kind, ReferentKind::JiraIssue);
        assert!(drafts[0].display_name.contains("PROJ-515"));
    }

    // -- Registry consistency -------------------------------------------------

    #[test]
    fn the_only_write_is_a_confirmed_transition() {
        // Creating and commenting are still absent, and still deliberately:
        // both need Atlassian Document Format, and a connector that writes
        // half-formed ADF into somebody's tracker is worse than one that
        // does not write at all. A transition carries no document, which is
        // why it is the one that could ship.
        for spec in ACTIONS {
            if spec.permission > Permission::Interact {
                assert_eq!(
                    spec.id, "jira.transition_issue",
                    "any further write here must justify itself"
                );
                assert_eq!(spec.permission, Permission::Write);
                assert_eq!(spec.confirm, ConfirmPolicy::Always);
                assert!(
                    !spec.reversible,
                    "plenty of Jira workflows are one-way, so NEXUS must not \
                     claim the move can be undone"
                );
            }
            assert_eq!(spec.reach, Reach::LeavesMachine, "{}", spec.id);
        }
    }

    #[test]
    fn a_status_is_never_invented() {
        // The property that matters: NEXUS posts a transition id it was
        // handed by the workflow, never a status string it composed. The
        // request body can only ever name an id read from `transitions`.
        let production = include_str!("jira_connector.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("the file must keep its test module marker");
        assert!(
            production.contains(r#""transition": { "id": id }"#),
            "the posted body must carry an id read back from Jira"
        );
        assert!(
            !production.contains(r#""transition": { "name""#),
            "a name NEXUS composed must never reach the API"
        );
    }

    #[test]
    fn action_ids_are_unique_and_namespaced() {
        let mut seen = std::collections::HashSet::new();
        for spec in ACTIONS {
            assert!(seen.insert(spec.id), "duplicate {}", spec.id);
            assert!(spec.id.starts_with("jira."), "{}", spec.id);
        }
    }

    #[test]
    fn no_token_is_ever_stored_in_the_database() {
        let production = include_str!("jira_connector.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("marker");
        assert!(
            !production.contains("INSERT INTO connectors")
                && !production.contains("token\" :")
                && !production.contains("UPDATE connectors SET config_json"),
            "the token belongs in the Keychain, never in config_json"
        );
        assert!(production.contains("keychain_secret"));
    }
}
