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
//! Everything here is `Read` or `Interact`. Creating and commenting are
//! `Write`, and deliberately not in this milestone.

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
    spec("jira.read_comments", "Read the comments on a Jira issue", Permission::Read),
    spec("jira.search", "Search Jira issues", Permission::Read),
    spec("jira.find_for_task", "Find the Jira issue linked to a task", Permission::Read),
    spec("jira.open_issue", "Open a Jira issue in the browser", Permission::Interact),
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

    let token = keychain_secret(KEYCHAIN_SERVICE, &config.email).ok_or_else(|| {
        ActionError::Failed {
            detail: format!(
                "No Jira API token found in the Keychain for {}. Add one with: \
                 security add-generic-password -s {KEYCHAIN_SERVICE} -a {} -w",
                config.email, config.email
            ),
        }
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
    if !project.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
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

fn get_json(creds: &Credentials, path: &str) -> Result<serde_json::Value, ActionError> {
    let url = format!("{}{path}", creds.site);
    let response = send(Request::get(&url).with_basic_auth(&creds.email, &creds.token)).map_err(
        |e| ActionError::Failed {
            detail: match e {
                HttpError::Unreachable { detail } => {
                    format!("Jira could not be reached. {detail}")
                }
                other => other.to_string(),
            },
        },
    )?;

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
                valid_key(&target.key).map(|_| ()).ok_or_else(|| bad_key(&target.key))
            }
            "jira.search" => parse::<SearchInput>(input.clone()).map(|_| ()),
            "jira.find_for_task" => parse::<TaskRef>(input.clone()).map(|_| ()),
            _ => Ok(()),
        }
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
            let config = read_config(ctx.conn).ok_or_else(|| ActionError::Failed {
                detail: "Jira is not set up yet. Add your site address in Settings.".to_string(),
            })?;
            let site = safe_https(&config.site).map_err(|e| ActionError::Failed {
                detail: e.to_string(),
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
                let value = get_json(
                    &creds,
                    &format!("/rest/api/3/issue/{key}?fields=summary,status,assignee,reporter,updated"),
                )?;
                Ok(shape_issue(&value))
            }

            "jira.read_comments" => {
                let target: IssueRef = parse(input)?;
                let key = valid_key(&target.key).ok_or_else(|| bad_key(&target.key))?;
                let value =
                    get_json(&creds, &format!("/rest/api/3/issue/{key}/comment?maxResults=20"))?;
                Ok(serde_json::json!({
                    "key": key,
                    "comments": value.get("comments").cloned().unwrap_or(serde_json::json!([]))
                }))
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
                // useful answer for "KAI-515" is that issue.
                let jql = match valid_key(phrase) {
                    Some(key) => format!("key = {key}"),
                    None => format!("text ~ \"{}\" ORDER BY updated DESC", jql_literal(phrase)),
                };
                let value = get_json(
                    &creds,
                    &format!(
                        "/rest/api/3/search?jql={}&maxResults={SEARCH_LIMIT}&fields=summary,status,assignee,updated",
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

                let value = get_json(
                    &creds,
                    &format!("/rest/api/3/issue/{key}?fields=summary,status,assignee,reporter,updated"),
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
        assert_eq!(valid_key("KAI-515").as_deref(), Some("KAI-515"));
        assert_eq!(valid_key("kai-515").as_deref(), Some("KAI-515"));
        assert_eq!(valid_key("  KAI-515  ").as_deref(), Some("KAI-515"));
        assert_eq!(valid_key("AB1-9").as_deref(), Some("AB1-9"));
    }

    #[test]
    fn anything_that_could_change_the_endpoint_is_refused() {
        // A key is a path segment. These are the shapes that would escape it.
        for hostile in [
            "KAI-515/../../admin",
            "KAI-515?expand=all",
            "KAI-515/comment",
            "../../secrets",
            "KAI 515",
            "KAI-515#x",
            "KAI-",
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
        assert_eq!(valid_key(&format!("KAI-{}", "9".repeat(40))), None);
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
        // "KAI-515" means that issue, not a text match somewhere in it.
        assert!(valid_key("KAI-515").is_some());
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
        assert!(caps.unavailable.iter().all(|u| u.reason.contains("Settings")));
    }

    #[test]
    fn a_non_https_site_is_refused_even_if_configured() {
        let conn = test_conn();
        configure(&conn, r#"{"site":"http://jira.internal","email":"a@b.com"}"#);
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
        configure(&conn, r#"{"site":"https://x.atlassian.net/","email":"a@b.com"}"#);
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
            "key": "KAI-515",
            "fields": {
                "summary": "Wire the gate",
                "status": { "name": "In Progress" },
                "assignee": { "displayName": "Rohit" },
                "updated": "2026-08-27T10:00:00.000+0000",
                "description": { "content": "a very long ADF document" }
            }
        });
        let shaped = shape_issue(&raw);
        assert_eq!(shaped["key"], "KAI-515");
        assert_eq!(shaped["status"], "In Progress");
        assert_eq!(shaped["assignee"], "Rohit");
        assert!(shaped.get("description").is_none(), "ADF bodies are not carried");
    }

    #[test]
    fn a_sparse_issue_does_not_panic() {
        let shaped = shape_issue(&serde_json::json!({ "key": "KAI-1" }));
        assert_eq!(shaped["key"], "KAI-1");
        assert_eq!(shaped["summary"], "");
        assert!(shaped["assignee"].is_null());
    }

    #[test]
    fn reading_an_issue_makes_it_referable() {
        let conn = test_conn();
        let drafts = JiraConnector.observe(
            "jira.read_issue",
            &serde_json::json!({}),
            &serde_json::json!({ "key": "KAI-515", "summary": "Wire the gate" }),
            &conn,
        );
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].kind, ReferentKind::JiraIssue);
        assert!(drafts[0].display_name.contains("KAI-515"));
    }

    // -- Registry consistency -------------------------------------------------

    #[test]
    fn everything_here_is_read_or_interact() {
        for spec in ACTIONS {
            assert!(
                spec.permission <= Permission::Interact,
                "{} must not write in this milestone",
                spec.id
            );
            assert_eq!(spec.reach, Reach::LeavesMachine, "{}", spec.id);
        }
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
