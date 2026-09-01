//! NEXUS-017: the GitHub connector.
//!
//! Built on the `gh` CLI rather than the REST API, and that is a deliberate
//! choice worth stating. `gh` is already installed and already authenticated
//! on this machine, so this connector needs no TLS stack, no OAuth flow and
//! no credential storage: three things that would otherwise have to land
//! before a single pull request could be read. It also means NEXUS never
//! holds a GitHub token. `gh` does, and NEXUS asks it questions.
//!
//! Every invocation is a fixed argument vector with `--json`, parsed into
//! typed structs. No shell, no string interpolation, and no `gh api` with a
//! caller-supplied path.
//!
//! The repository comes from `projects.repository_url`, which is the
//! correlation the workspace already models: a project points at a repo, and
//! `tasks.external_id` points at a tracker issue. That is the join that makes
//! "the PR for this project" a query rather than a guess.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::action::{ActionError, ActionSpec};
use super::connector::{
    Capabilities, Connector, ConnectorStatus, ExecCtx, ReferentDraft, UnavailableAction,
};
use super::permission::{ConfirmPolicy, Permission, Reach};
use super::referent::ReferentKind;
use super::shell::{run, RunError, DEFAULT_TIMEOUT};

pub const CONNECTOR_ID: &str = "github";

const GH: &str = "gh";
/// Longest list NEXUS will fetch. Anything more is a report, not an answer.
const PR_LIMIT: &str = "20";

const fn spec(id: &'static str, summary: &'static str, permission: Permission) -> ActionSpec {
    ActionSpec {
        id,
        connector_id: CONNECTOR_ID,
        summary,
        permission,
        confirm: ConfirmPolicy::Never,
        // Reads leave the machine: they reach github.com. Marked honestly so
        // the offline contract and any future privacy rule can see it.
        reach: Reach::LeavesMachine,
        reversible: true,
    }
}

pub const ACTIONS: &[ActionSpec] = &[
    spec(
        "github.status",
        "Check the GitHub connection",
        Permission::Read,
    ),
    spec(
        "github.list_prs",
        "List pull requests for a project",
        Permission::Read,
    ),
    spec(
        "github.read_pr",
        "Read a pull request and its checks",
        Permission::Read,
    ),
    spec(
        "github.read_pr_comments",
        "Read the comments on a pull request",
        Permission::Read,
    ),
    spec(
        "github.open_pr",
        "Open a pull request in the browser",
        Permission::Interact,
    ),
];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProjectRef {
    project_id: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PrRef {
    project_id: i64,
    number: u64,
}

fn parse<T: serde::de::DeserializeOwned>(input: serde_json::Value) -> Result<T, ActionError> {
    serde_json::from_value(input).map_err(|e| ActionError::InvalidInput {
        detail: e.to_string(),
    })
}

fn json<T: Serialize>(value: T) -> Result<serde_json::Value, ActionError> {
    serde_json::to_value(value).map_err(|e| ActionError::Failed {
        detail: format!("Could not encode the result: {e}"),
    })
}

// -- Repository resolution ----------------------------------------------------

/// Extract `owner/repo` from whatever form the project stores.
///
/// Accepts the three shapes people actually paste: an HTTPS URL, an SSH
/// remote, and a bare `owner/repo`. Anything else is refused rather than
/// guessed at, because a wrong repository is a wrong answer that looks right.
pub fn parse_repo(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }

    let tail = if let Some(rest) = trimmed.strip_prefix("git@github.com:") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("https://github.com/") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("http://github.com/") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("ssh://git@github.com/") {
        rest
    } else if !trimmed.contains("://") && !trimmed.contains('@') {
        trimmed
    } else {
        // Some other host. NEXUS talks to github.com through `gh`, and
        // pretending otherwise would produce confident nonsense.
        return None;
    };

    let tail = tail.trim_end_matches(".git").trim_matches('/');
    let parts: Vec<&str> = tail.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() != 2 {
        return None;
    }
    // A path segment that looks like a flag would be argv poison downstream.
    if parts.iter().any(|p| p.starts_with('-')) {
        return None;
    }
    Some(format!("{}/{}", parts[0], parts[1]))
}

fn repo_for_project(conn: &Connection, project_id: i64) -> Result<(String, String), ActionError> {
    let (name, url): (String, Option<String>) = conn
        .query_row(
            "SELECT name, repository_url FROM projects WHERE id = ?1",
            [project_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| ActionError::Failed {
            detail: format!("No project with id {project_id}."),
        })?;

    let url = url.ok_or_else(|| ActionError::Failed {
        detail: format!(
            "{name} has no repository URL. Add one so NEXUS knows which repo to ask about."
        ),
    })?;

    let repo = parse_repo(&url).ok_or_else(|| ActionError::Failed {
        detail: format!("NEXUS could not read a GitHub repository out of \"{url}\"."),
    })?;

    Ok((name, repo))
}

// -- gh invocation ------------------------------------------------------------

fn gh_json(args: &[&str]) -> Result<serde_json::Value, ActionError> {
    let out = run(GH, args, DEFAULT_TIMEOUT).map_err(|e| match e {
        RunError::NotFound { .. } => ActionError::Failed {
            detail: "The GitHub CLI is not installed. Install it with `brew install gh`."
                .to_string(),
        },
        other => ActionError::Failed {
            detail: other.to_string(),
        },
    })?;

    if !out.success {
        let stderr = out.stderr.to_lowercase();
        let detail = if stderr.contains("auth") || stderr.contains("logged in") {
            "The GitHub CLI is not signed in. Run `gh auth login` in a terminal.".to_string()
        } else if stderr.contains("could not resolve") || stderr.contains("not found") {
            "GitHub could not find that. Check the repository and number.".to_string()
        } else if stderr.contains("network") || stderr.contains("dial tcp") {
            "GitHub could not be reached. Check your connection.".to_string()
        } else if out.stderr.is_empty() {
            "The GitHub CLI failed without saying why.".to_string()
        } else {
            out.stderr
        };
        return Err(ActionError::Failed { detail });
    }

    serde_json::from_str(&out.stdout).map_err(|e| ActionError::Failed {
        detail: format!("GitHub returned something NEXUS could not read: {e}"),
    })
}

/// Whether `gh` exists and is signed in. Cheap enough for a capability read.
fn gh_ready() -> ConnectorStatus {
    match run(GH, &["auth", "status"], DEFAULT_TIMEOUT) {
        Ok(out) if out.success => ConnectorStatus::Ready,
        Ok(_) => ConnectorStatus::NeedsAuth,
        Err(RunError::NotFound { .. }) => ConnectorStatus::Unavailable,
        Err(_) => ConnectorStatus::Degraded,
    }
}

// -- Shaping the answers ------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CheckSummary {
    total: usize,
    failing: usize,
    pending: usize,
    failing_names: Vec<String>,
}

/// Reduce `statusCheckRollup` to the thing a person asks about.
///
/// "Does this PR have failing checks" is the question; a hundred rollup
/// entries is not an answer. Unknown states count as pending rather than
/// passing, because a check NEXUS does not understand must never read green.
fn summarise_checks(rollup: Option<&serde_json::Value>) -> CheckSummary {
    let entries = rollup
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut failing_names = Vec::new();
    let mut pending = 0usize;

    for entry in &entries {
        let name = entry
            .get("name")
            .or_else(|| entry.get("context"))
            .and_then(|v| v.as_str())
            .unwrap_or("a check")
            .to_string();

        // Checks report `conclusion`; legacy statuses report `state`.
        let verdict = entry
            .get("conclusion")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| entry.get("state").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_uppercase();

        match verdict.as_str() {
            "SUCCESS" | "NEUTRAL" | "SKIPPED" => {}
            "FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "ERROR" => {
                failing_names.push(name)
            }
            _ => pending += 1,
        }
    }

    CheckSummary {
        total: entries.len(),
        failing: failing_names.len(),
        pending,
        failing_names,
    }
}

pub struct GithubConnector;

impl Connector for GithubConnector {
    fn id(&self) -> &'static str {
        CONNECTOR_ID
    }

    fn display_name(&self) -> &'static str {
        "GitHub"
    }

    fn actions(&self) -> &'static [ActionSpec] {
        ACTIONS
    }

    fn capabilities(&self, _conn: &Connection) -> Capabilities {
        match gh_ready() {
            ConnectorStatus::Ready => Capabilities {
                available: ACTIONS.iter().map(|s| s.id.to_string()).collect(),
                unavailable: Vec::new(),
            },
            other => {
                let reason = match other {
                    ConnectorStatus::Unavailable => {
                        "The GitHub CLI is not installed. Install it with `brew install gh`."
                    }
                    ConnectorStatus::NeedsAuth => {
                        "The GitHub CLI is not signed in. Run `gh auth login`."
                    }
                    _ => "The GitHub CLI is not responding.",
                };
                Capabilities {
                    // status stays available: it is how the user finds out why.
                    available: vec!["github.status".to_string()],
                    unavailable: ACTIONS
                        .iter()
                        .filter(|s| s.id != "github.status")
                        .map(|s| UnavailableAction {
                            action_id: s.id.to_string(),
                            reason: reason.to_string(),
                        })
                        .collect(),
                }
            }
        }
    }

    fn status(&self, _conn: &Connection) -> ConnectorStatus {
        gh_ready()
    }

    fn summarize(&self, action_id: &str, input: &serde_json::Value, conn: &Connection) -> String {
        let project_name = |id: i64| {
            conn.query_row("SELECT name FROM projects WHERE id = ?1", [id], |r| {
                r.get::<_, String>(0)
            })
            .ok()
        };

        match action_id {
            "github.list_prs" => match serde_json::from_value::<ProjectRef>(input.clone())
                .ok()
                .and_then(|p| project_name(p.project_id))
            {
                Some(name) => format!("List pull requests for {name}"),
                None => "List pull requests".to_string(),
            },
            "github.read_pr" | "github.read_pr_comments" | "github.open_pr" => {
                match serde_json::from_value::<PrRef>(input.clone()) {
                    Ok(target) => {
                        let verb = match action_id {
                            "github.read_pr" => "Read",
                            "github.read_pr_comments" => "Read the comments on",
                            _ => "Open",
                        };
                        format!("{verb} PR #{}", target.number)
                    }
                    Err(_) => "Look at a pull request".to_string(),
                }
            }
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
        output: &serde_json::Value,
        _conn: &Connection,
    ) -> Vec<ReferentDraft> {
        let mut drafts = Vec::new();

        // A pull request NEXUS just read becomes "the PR" for follow-ups.
        if matches!(action_id, "github.read_pr" | "github.open_pr") {
            if let Ok(target) = serde_json::from_value::<PrRef>(input.clone()) {
                let title = output
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("pull request");
                drafts.push(ReferentDraft {
                    kind: ReferentKind::PullRequest,
                    display_name: format!("PR #{} {title}", target.number),
                    metadata: serde_json::json!({
                        "number": target.number,
                        "projectId": target.project_id
                    }),
                });
            }
            // And its author, so "reply to them" has somewhere to point once
            // a messaging connector exists.
            if let Some(login) = output
                .get("author")
                .and_then(|a| a.get("login"))
                .and_then(|v| v.as_str())
            {
                drafts.push(ReferentDraft {
                    kind: ReferentKind::Person,
                    display_name: login.to_string(),
                    metadata: serde_json::json!({ "login": login }),
                });
            }
        }

        drafts
    }

    fn validate_input(
        &self,
        action_id: &str,
        input: &serde_json::Value,
    ) -> Result<(), ActionError> {
        match action_id {
            "github.list_prs" => parse::<ProjectRef>(input.clone()).map(|_| ()),
            "github.read_pr" | "github.read_pr_comments" | "github.open_pr" => {
                parse::<PrRef>(input.clone()).map(|_| ())
            }
            _ => Ok(()),
        }
    }

    fn describe_result(&self, action_id: &str, output: &serde_json::Value) -> Option<String> {
        match action_id {
            "github.status" => Some(if output.get("signedIn")?.as_bool().unwrap_or(false) {
                "GitHub is connected.".to_string()
            } else {
                "GitHub is not signed in. Run `gh auth login`.".to_string()
            }),
            "github.list_prs" => {
                let prs = output.get("pullRequests")?.as_array()?;
                if prs.is_empty() {
                    return Some(format!(
                        "No open pull requests in {}.",
                        output.get("repo")?.as_str()?
                    ));
                }
                let listed: Vec<String> = prs
                    .iter()
                    .take(5)
                    .filter_map(|p| {
                        Some(format!(
                            "#{} {}",
                            p.get("number")?.as_i64()?,
                            p.get("title")?
                                .as_str()?
                                .chars()
                                .take(50)
                                .collect::<String>()
                        ))
                    })
                    .collect();
                Some(format!("{} open: {}.", prs.len(), listed.join(", ")))
            }
            "github.read_pr" => {
                let number = output.get("number")?.as_i64()?;
                let title = output.get("title")?.as_str()?;
                let checks = output.get("checks")?;
                let failing = checks.get("failing")?.as_i64().unwrap_or(0);
                let pending = checks.get("pending")?.as_i64().unwrap_or(0);
                let verdict = if failing > 0 {
                    let names: Vec<&str> = checks
                        .get("failingNames")
                        .and_then(|v| v.as_array())
                        .map(|a| a.iter().filter_map(|n| n.as_str()).collect())
                        .unwrap_or_default();
                    format!("{failing} failing ({})", names.join(", "))
                } else if pending > 0 {
                    format!("{pending} still running")
                } else {
                    "all checks passing".to_string()
                };
                Some(format!("PR #{number} {title}: {verdict}."))
            }
            _ => None,
        }
    }

    fn zero_input_actions(&self) -> &'static [&'static str] {
        &["github.status"]
    }

    fn dispatch(
        &self,
        action_id: &str,
        input: serde_json::Value,
        ctx: &ExecCtx<'_>,
    ) -> Result<serde_json::Value, ActionError> {
        match action_id {
            "github.status" => {
                let status = gh_ready();
                json(serde_json::json!({
                    "status": status,
                    "signedIn": status == ConnectorStatus::Ready
                }))
            }

            "github.list_prs" => {
                let target: ProjectRef = parse(input)?;
                let (project, repo) = repo_for_project(ctx.conn, target.project_id)?;
                let value = gh_json(&[
                    "pr",
                    "list",
                    "--repo",
                    &repo,
                    "--limit",
                    PR_LIMIT,
                    "--json",
                    "number,title,state,isDraft,url,author",
                ])?;
                json(serde_json::json!({
                    "project": project,
                    "repo": repo,
                    "pullRequests": value
                }))
            }

            "github.read_pr" => {
                let target: PrRef = parse(input)?;
                let (_, repo) = repo_for_project(ctx.conn, target.project_id)?;
                let number = target.number.to_string();
                let value = gh_json(&[
                    "pr",
                    "view",
                    &number,
                    "--repo",
                    &repo,
                    "--json",
                    "number,title,state,isDraft,url,author,mergeable,statusCheckRollup",
                ])?;

                let checks = summarise_checks(value.get("statusCheckRollup"));
                // The rollup itself is dropped: it is long, and every question
                // people ask about it is answered by the summary.
                let mut trimmed = value.clone();
                if let Some(object) = trimmed.as_object_mut() {
                    object.remove("statusCheckRollup");
                    object.insert("checks".to_string(), json(checks)?);
                    object.insert("repo".to_string(), serde_json::json!(repo));
                }
                Ok(trimmed)
            }

            "github.read_pr_comments" => {
                let target: PrRef = parse(input)?;
                let (_, repo) = repo_for_project(ctx.conn, target.project_id)?;
                let number = target.number.to_string();
                let value =
                    gh_json(&["pr", "view", &number, "--repo", &repo, "--json", "comments"])?;
                json(serde_json::json!({
                    "repo": repo,
                    "number": target.number,
                    "comments": value.get("comments").cloned().unwrap_or(serde_json::json!([]))
                }))
            }

            "github.open_pr" => {
                let target: PrRef = parse(input)?;
                let (_, repo) = repo_for_project(ctx.conn, target.project_id)?;
                let url = format!("https://github.com/{repo}/pull/{}", target.number);
                // Opened through the same mechanism the browser connector's
                // tier 1 uses: no automation permission, no scripting.
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
                json(serde_json::json!({
                    "url": url,
                    "title": format!("PR #{}", target.number)
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
    use serde_json::json as j;

    // -- Repository parsing, the join the workspace already models -----------

    #[test]
    fn every_shape_people_actually_paste_is_accepted() {
        for raw in [
            "https://github.com/acme/AdminService",
            "https://github.com/acme/AdminService.git",
            "https://github.com/acme/AdminService/",
            "git@github.com:acme/AdminService.git",
            "ssh://git@github.com/acme/AdminService",
            "acme/AdminService",
        ] {
            assert_eq!(
                parse_repo(raw).as_deref(),
                Some("acme/AdminService"),
                "{raw}"
            );
        }
    }

    #[test]
    fn another_host_is_refused_rather_than_assumed() {
        // A wrong repository is a wrong answer that looks right.
        for raw in [
            "https://gitlab.com/acme/AdminService",
            "https://bitbucket.org/acme/AdminService",
            "git@gitlab.com:acme/AdminService.git",
        ] {
            assert_eq!(parse_repo(raw), None, "{raw}");
        }
    }

    #[test]
    fn a_malformed_repository_is_refused() {
        for raw in [
            "",
            "   ",
            "https://github.com/",
            "https://github.com/only-one",
            "https://github.com/a/b/c",
        ] {
            assert_eq!(parse_repo(raw), None, "{raw:?}");
        }
    }

    #[test]
    fn a_segment_that_looks_like_a_flag_is_refused() {
        // It would reach `gh` as argv. Fixed vectors stop it being parsed as
        // an option, but refusing it earlier keeps the failure legible.
        assert_eq!(parse_repo("https://github.com/--repo/evil"), None);
        assert_eq!(parse_repo("-x/y"), None);
    }

    // -- Check summarising ----------------------------------------------------

    #[test]
    fn failing_checks_are_counted_and_named() {
        let rollup = j!([
            { "name": "build", "conclusion": "SUCCESS" },
            { "name": "test",  "conclusion": "FAILURE" },
            { "name": "lint",  "conclusion": "TIMED_OUT" }
        ]);
        let summary = summarise_checks(Some(&rollup));
        assert_eq!(summary.total, 3);
        assert_eq!(summary.failing, 2);
        assert!(summary.failing_names.contains(&"test".to_string()));
        assert!(summary.failing_names.contains(&"lint".to_string()));
    }

    #[test]
    fn an_unknown_state_counts_as_pending_never_as_passing() {
        // A check NEXUS does not understand must not read as green.
        let rollup = j!([
            { "name": "future", "conclusion": "SOMETHING_NEW" },
            { "name": "queued", "conclusion": "" },
            { "name": "legacy", "state": "PENDING" }
        ]);
        let summary = summarise_checks(Some(&rollup));
        assert_eq!(summary.failing, 0);
        assert_eq!(summary.pending, 3);
    }

    #[test]
    fn legacy_statuses_are_read_from_state() {
        let rollup = j!([
            { "context": "ci/legacy", "state": "FAILURE" },
            { "context": "ci/other",  "state": "SUCCESS" }
        ]);
        let summary = summarise_checks(Some(&rollup));
        assert_eq!(summary.failing, 1);
        assert_eq!(summary.failing_names, vec!["ci/legacy".to_string()]);
    }

    #[test]
    fn neutral_and_skipped_are_not_failures() {
        let rollup = j!([
            { "name": "a", "conclusion": "NEUTRAL" },
            { "name": "b", "conclusion": "SKIPPED" }
        ]);
        let summary = summarise_checks(Some(&rollup));
        assert_eq!((summary.failing, summary.pending), (0, 0));
    }

    #[test]
    fn no_checks_at_all_is_not_a_failure() {
        assert_eq!(summarise_checks(None).total, 0);
        assert_eq!(summarise_checks(Some(&j!([]))).total, 0);
        assert_eq!(summarise_checks(Some(&j!("not an array"))).total, 0);
    }

    // -- Referents ------------------------------------------------------------

    #[test]
    fn reading_a_pr_makes_it_and_its_author_referable() {
        let conn = Connection::open_in_memory().expect("open");
        let drafts = GithubConnector.observe(
            "github.read_pr",
            &j!({ "projectId": 1, "number": 8792 }),
            &j!({ "title": "Fix the gate", "author": { "login": "alec" } }),
            &conn,
        );
        assert_eq!(drafts.len(), 2);
        assert_eq!(drafts[0].kind, ReferentKind::PullRequest);
        assert!(drafts[0].display_name.contains("8792"));
        assert_eq!(drafts[1].kind, ReferentKind::Person);
        assert_eq!(drafts[1].display_name, "alec");
    }

    #[test]
    fn listing_pull_requests_creates_no_referents() {
        // A list of twenty is not twenty things the user just referred to.
        let conn = Connection::open_in_memory().expect("open");
        assert!(GithubConnector
            .observe("github.list_prs", &j!({ "projectId": 1 }), &j!({}), &conn)
            .is_empty());
    }

    // -- Registry consistency -------------------------------------------------

    #[test]
    fn everything_here_is_read_or_interact() {
        // NEXUS-017 reads GitHub. Creating and commenting are Write-level and
        // deliberately not in this milestone.
        for spec in ACTIONS {
            assert!(
                spec.permission <= Permission::Interact,
                "{} must not write in this milestone",
                spec.id
            );
        }
    }

    #[test]
    fn reads_are_marked_as_leaving_the_machine() {
        // They reach github.com. Saying so keeps the offline contract and any
        // future privacy rule honest.
        for spec in ACTIONS {
            if spec.permission == Permission::Read {
                assert_eq!(spec.reach, Reach::LeavesMachine, "{}", spec.id);
            }
        }
    }

    #[test]
    fn action_ids_are_unique_and_namespaced() {
        let mut seen = std::collections::HashSet::new();
        for spec in ACTIONS {
            assert!(seen.insert(spec.id), "duplicate {}", spec.id);
            assert!(spec.id.starts_with("github."), "{}", spec.id);
            assert_eq!(spec.connector_id, CONNECTOR_ID);
        }
    }

    #[test]
    fn no_caller_supplied_api_path_is_ever_passed_to_gh() {
        // `gh api <path>` with a path from a caller would be a general-purpose
        // request builder wearing a connector's clothes.
        let production = include_str!("github_connector.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("marker");
        assert!(!production.contains("\"api\""), "gh api must not be used");
        assert!(!production.contains("/bin/sh"), "no shell");
    }

    #[test]
    fn errors_name_the_remedy() {
        let production = include_str!("github_connector.rs");
        assert!(production.contains("gh auth login"));
        assert!(production.contains("brew install gh"));
    }
}
