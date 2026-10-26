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
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::action::{ActionError, ActionSpec};
use super::connector::{
    Capabilities, Connector, ConnectorStatus, ExecCtx, ReferentDraft, UnavailableAction,
};
use super::permission::{ConfirmPolicy, Permission, Reach};
use super::referent::ReferentKind;
use super::shell::{run, RunError, DEFAULT_TIMEOUT};

pub const CONNECTOR_ID: &str = "github";

/// Where `gh` actually lives.
///
/// Absolute paths, because a bare program name is resolved against `PATH`
/// and a macOS application bundle does not have the one you have in a
/// terminal. Launched from the Dock, NEXUS inherits launchd's default
/// `/usr/bin:/bin:/usr/sbin:/sbin`, and Homebrew is not on it. So
/// `Command::new("gh")` failed with `NotFound` and NEXUS told the user to
/// install a tool that was sitting in `/opt/homebrew/bin` the whole time.
///
/// Ordered by likelihood on this platform: Homebrew on Apple silicon,
/// Homebrew on Intel, a system install, then a kegged one.
const GH_CANDIDATES: [&str; 4] = [
    "/opt/homebrew/bin/gh",
    "/usr/local/bin/gh",
    "/usr/bin/gh",
    "/opt/homebrew/opt/gh/bin/gh",
];

/// The `gh` binary to run.
///
/// Falls back to the bare name when none of the candidates exist. That is
/// deliberate: from a terminal launch `PATH` may well find an installation
/// in a place this list does not know about, and if it cannot, the failure
/// is the honest `NotFound` rather than a path nobody has.
fn gh_bin() -> &'static str {
    GH_CANDIDATES
        .into_iter()
        .find(|path| std::path::Path::new(path).exists())
        .unwrap_or("gh")
}
/// Keychain service for a token stored through NEXUS's own Settings. The
/// account is the GitHub login, so two identities can coexist.
pub const KEYCHAIN_SERVICE: &str = "nexus-github";
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
    spec(
        "github.build_status",
        "Check the latest build run for a repository",
        Permission::Read,
    ),
    spec(
        "github.my_prs",
        "List my pull requests across tracked projects since a date",
        Permission::Read,
    ),
];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProjectRef {
    project_id: i64,
    /// Narrow the list to pull requests matching this, using GitHub's own
    /// search. Absent means the whole recent list.
    ///
    /// Exists so "the PR for KAI-645" is one precise question rather than
    /// twenty results filtered afterwards, which is how a merged PR outside
    /// the most recent twenty goes missing.
    #[serde(default)]
    query: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PrRef {
    project_id: i64,
    number: u64,
}

/// A repository given directly, not via a tracked project.
///
/// The build-status card watches a couple of repos that are not NEXUS projects,
/// so it names them outright. Validated through `parse_repo` the same as any
/// other, so a bad value is refused rather than handed to `gh`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RepoRef {
    repo: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SinceRef {
    /// YYYY-MM-DD.
    since: String,
}

/// A date that may go into a `gh` search qualifier, or `None`. Exactly
/// YYYY-MM-DD: anything else could add qualifiers to the search.
fn valid_date(raw: &str) -> Option<&str> {
    let b = raw.as_bytes();
    let ok = b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit());
    ok.then_some(raw)
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

/// Which `gh` account owns a given GitHub organisation.
///
/// Read from this connector's `config_json`, shaped
/// `{"accounts": {"some-org": "the-gh-login"}}`.
///
/// It exists because one machine can have two GitHub identities: a personal
/// account that owns personal repositories, and a work account that is the
/// only one able to see the employer's. `gh` keeps both but has a single
/// *active* account, and that active account is also what git's credential
/// helper uses. Switching it globally to read a work pull request would
/// break pushing a personal repository from the same terminal.
///
/// So the account is chosen per repository, and nothing global changes.
/// Unmapped owners fall through to whichever account is active, which is the
/// behaviour every existing setup already has.
fn account_for(conn: &Connection, repo: &str) -> Option<String> {
    let owner = repo.split('/').next()?;
    let raw: Option<String> = conn
        .query_row(
            "SELECT config_json FROM connectors WHERE connector_id = ?1",
            [CONNECTOR_ID],
            |row| row.get(0),
        )
        .ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&raw?).ok()?;
    let login = parsed
        .get("accounts")?
        .get(owner)?
        .as_str()?
        .trim()
        .to_string();
    (!login.is_empty()).then_some(login)
}

/// That account's token, straight from `gh`'s own store.
///
/// Fetched at the moment of use and never written anywhere. This is a real
/// weakening of what NEXUS used to guarantee, which was that it never held a
/// GitHub token at all, and it is recorded here rather than left to be
/// discovered: the token now lives in this process's memory for the length of
/// one subprocess.
///
/// It reaches the child through the environment, never `argv`, because every
/// process on this machine can read another's command line.
fn token_for(login: &str) -> Option<String> {
    // A token stored through NEXUS's own Settings comes first, so the whole
    // setup can be done in the application without asking the user to
    // operate `security` or `gh` in a terminal.
    if let Some(stored) = super::http::keychain_secret(KEYCHAIN_SERVICE, login) {
        let trimmed = stored.trim().to_string();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }

    // Otherwise ask `gh`, which may already hold that account.
    let out = run(gh_bin(), &["auth", "token", "--user", login], DEFAULT_TIMEOUT).ok()?;
    if !out.success {
        return None;
    }
    let token = out.stdout.trim().to_string();
    (!token.is_empty()).then_some(token)
}

/// Run `gh` as whichever account can see this repository.
fn gh_json_as(
    conn: &Connection,
    repo: &str,
    args: &[&str],
) -> Result<serde_json::Value, ActionError> {
    match account_for(conn, repo).and_then(|login| token_for(&login)) {
        Some(token) => gh_json_with_env(args, &[("GH_TOKEN", token.as_str())]),
        // No mapping, or `gh` does not hold that account: fall back to the
        // active one and let GitHub's own error explain itself. Guessing a
        // different account would be worse than a clear "could not find".
        None => gh_json(args),
    }
}

fn gh_json(args: &[&str]) -> Result<serde_json::Value, ActionError> {
    gh_json_with_env(args, &[])
}

fn gh_json_with_env(
    args: &[&str],
    env: &[(&str, &str)],
) -> Result<serde_json::Value, ActionError> {
    let out = super::shell::run_with_env(gh_bin(), args, env, DEFAULT_TIMEOUT).map_err(|e| match e {
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
/// How long a `gh auth status` answer is trusted.
///
/// Measured, not guessed: that command costs about 450ms on this machine,
/// because it talks to github.com rather than reading a file. Two things ask
/// every connector for its status every eight seconds (the rail's dots and
/// the core's nodes), so it was running twice a cycle: nearly a second of
/// work per eight, for an answer that changes when somebody runs
/// `gh auth login` and at no other time.
///
/// A minute is the cost of being wrong: after signing in, the dot goes green
/// within a minute rather than within eight seconds. That is the whole
/// downside, and it buys back the frame budget.
const READY_TTL: Duration = Duration::from_secs(60);

/// Last answer and when it was given. `None` until the first call.
static READY_CACHE: Mutex<Option<(Instant, ConnectorStatus)>> = Mutex::new(None);

fn gh_ready() -> ConnectorStatus {
    // A poisoned mutex here means a previous caller panicked mid-check. The
    // cached value is still a valid `ConnectorStatus` either way, so the
    // recovery is to use it rather than to bring the status bar down.
    let mut cache = READY_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((asked, status)) = *cache {
        if asked.elapsed() < READY_TTL {
            return status;
        }
    }

    let status = match run(gh_bin(), &["auth", "status"], DEFAULT_TIMEOUT) {
        Ok(out) if out.success => ConnectorStatus::Ready,
        Ok(_) => ConnectorStatus::NeedsAuth,
        Err(RunError::NotFound { .. }) => ConnectorStatus::Unavailable,
        Err(_) => ConnectorStatus::Degraded,
    };
    *cache = Some((Instant::now(), status));
    status
}

/// Forget the cached answer.
///
/// Called when the account configuration changes, so a fresh sign-in shows
/// up immediately instead of after the TTL. Without this the user fixes
/// their token, sees the dot stay red, and reasonably concludes it did not
/// work.
pub fn forget_ready() {
    *READY_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
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
                        "No pull requests found in {}.",
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
                // Every state, not just open.
                //
                // `gh pr list` defaults to open, so a merged pull request was
                // invisible: asking "is there a PR for this ticket" answered
                // "no" for work that had already shipped, which is the most
                // misleading answer available.
                let mut args: Vec<&str> = vec![
                    "pr",
                    "list",
                    "--repo",
                    &repo,
                    "--state",
                    "all",
                    "--limit",
                    PR_LIMIT,
                    "--json",
                    "number,title,state,isDraft,url,author",
                ];
                let search = target.query.unwrap_or_default();
                if !search.trim().is_empty() {
                    args.push("--search");
                    args.push(search.trim());
                }
                let value = gh_json_as(ctx.conn, &repo, &args)?;
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
                let value = gh_json_as(
                    ctx.conn,
                    &repo,
                    &[
                    "pr",
                    "view",
                    &number,
                    "--repo",
                    &repo,
                    "--json",
                    "number,title,state,isDraft,url,author,mergeable,statusCheckRollup",
                    ],
                )?;

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
                // Both kinds: a PR's conversation has `comments`, but a reviewer
                // who leaves feedback through the review flow shows up under
                // `reviews`, not `comments`. Pulling only one dropped half the
                // people from the tally, which is how a PR with two reviewers
                // showed one.
                let value = gh_json_as(
                    ctx.conn,
                    &repo,
                    &[
                        "pr", "view", &number, "--repo", &repo, "--json",
                        "comments,reviews",
                    ],
                )?;
                json(serde_json::json!({
                    "repo": repo,
                    "number": target.number,
                    "comments": value.get("comments").cloned().unwrap_or(serde_json::json!([])),
                    "reviews": value.get("reviews").cloned().unwrap_or(serde_json::json!([]))
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

            "github.build_status" => {
                let target: RepoRef = parse(input)?;
                let repo = parse_repo(&target.repo).ok_or_else(|| ActionError::InvalidInput {
                    detail: format!("\"{}\" is not a GitHub repository.", target.repo),
                })?;
                // The one most recent run, across every workflow, which is what
                // the top of the repo's Actions page shows. `conclusion` is only
                // set once `status` is "completed"; an in-flight run has a null
                // conclusion, and the card reads both so a running build is not
                // mistaken for a failed one.
                let value = gh_json_as(
                    ctx.conn,
                    &repo,
                    &[
                        "run",
                        "list",
                        "--repo",
                        &repo,
                        "--limit",
                        "1",
                        "--json",
                        "number,displayTitle,headBranch,status,conclusion,workflowName,createdAt,url",
                    ],
                )?;
                // `gh run list` returns an array; the card wants the single
                // latest run, or null when the repo has never run a workflow.
                let latest = value
                    .as_array()
                    .and_then(|runs| runs.first())
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                json(serde_json::json!({
                    "repo": repo,
                    "run": latest,
                }))
            }

            "github.my_prs" => {
                // For the standup: the user's own PRs, in every tracked repo,
                // touched since the given date. Per repo rather than one
                // `gh search`, because each repo is asked as the account
                // mapped to its owner: `@me` must mean the work account for
                // work repositories, and a global search would ask as
                // whichever account happens to be active.
                let target: SinceRef = parse(input)?;
                let since = valid_date(&target.since).ok_or_else(|| ActionError::InvalidInput {
                    detail: format!("\"{}\" is not a YYYY-MM-DD date.", target.since),
                })?;
                let mut repos: Vec<String> = Vec::new();
                if let Ok(mut stmt) = ctx.conn.prepare(
                    "SELECT repository_url FROM projects WHERE repository_url IS NOT NULL",
                ) {
                    if let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) {
                        for url in rows.filter_map(Result::ok) {
                            if let Some(repo) = parse_repo(&url) {
                                if !repos.contains(&repo) {
                                    repos.push(repo);
                                }
                            }
                        }
                    }
                }

                let search = format!("updated:>={since}");
                let mut prs: Vec<serde_json::Value> = Vec::new();
                let mut errors: Vec<String> = Vec::new();
                for repo in &repos {
                    match gh_json_as(
                        ctx.conn,
                        repo,
                        &[
                            "pr",
                            "list",
                            "--repo",
                            repo,
                            "--author",
                            "@me",
                            "--state",
                            "all",
                            "--search",
                            &search,
                            "--limit",
                            "20",
                            "--json",
                            "number,title,state,isDraft,url,updatedAt,mergedAt,reviewDecision,statusCheckRollup",
                        ],
                    ) {
                        Ok(list) => {
                            for pr in list.as_array().cloned().unwrap_or_default() {
                                let checks = summarise_checks(pr.get("statusCheckRollup"));
                                let mut shaped = pr.clone();
                                if let Some(obj) = shaped.as_object_mut() {
                                    obj.remove("statusCheckRollup");
                                    obj.insert("checks".into(), json(checks)?);
                                    obj.insert("repo".into(), serde_json::json!(repo));
                                }
                                prs.push(shaped);
                            }
                        }
                        // One unreachable repo must not hide the rest.
                        Err(e) => errors.push(format!("{repo}: {e}")),
                    }
                }
                json(serde_json::json!({ "prs": prs, "errors": errors, "repos": repos.len() }))
            }

            other => Err(ActionError::UnknownAction {
                action_id: other.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {

    // -- Two identities on one machine ---------------------------------------

    /// A database with the connector rows the resolver reads.
    ///
    /// Registration is what creates them; migrations alone leave the table
    /// empty, so an UPDATE would match nothing and the resolver would look
    /// broken when it was only unconfigured.
    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        for (_, sql) in crate::db::migrations::MIGRATIONS {
            conn.execute_batch(sql).expect("migrate");
        }
        crate::assistant::register_connectors(&conn).expect("register");
        conn
    }

    fn production() -> &'static str {
        include_str!("github_connector.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("marker")
    }

    #[test]
    fn only_a_plain_date_reaches_a_search_qualifier() {
        assert_eq!(valid_date("2026-10-22"), Some("2026-10-22"));
        for bad in ["2026-10-2", "2026/10/22", "2026-10-22 is:open", "x", "2026-1a-22"] {
            assert!(valid_date(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn a_token_never_reaches_the_command_line() {
        // Every process on this machine can read another's `argv` through
        // `ps`. The environment of a child is not in that output, which is
        // the whole reason the token goes that way.
        assert!(production().contains("(\"GH_TOKEN\", token.as_str())"));
        assert!(
            !production().contains("\"--token\""),
            "a token must never be an argument"
        );
    }

    #[test]
    fn a_token_is_never_written_anywhere() {
        // Fetched at the moment of use and dropped with the call. Nothing
        // persists it, which is what keeps the narrowed claim true.
        let getter = production()
            .split_once("fn token_for")
            .map(|(_, rest)| rest)
            .expect("the token reader");
        let body = &getter[..getter.find("\n}").expect("end")];
        for forbidden in ["INSERT", "UPDATE", "write", "store_keychain", "log::"] {
            assert!(!body.contains(forbidden), "found {forbidden}");
        }
    }

    #[test]
    fn nothing_switches_the_active_account() {
        // Switching globally is what breaks pushing a personal repository
        // from a terminal that shares the same `gh`.
        assert!(
            !production().contains("\"switch\""),
            "the active account must not be changed"
        );
    }

    #[test]
    fn an_unmapped_owner_uses_the_active_account() {
        // The behaviour every existing setup already has: no config, no
        // change. Guessing a different account would be worse than letting
        // GitHub's own "could not find" explain itself.
        let conn = migrated();
        assert_eq!(account_for(&conn, "someone/thing"), None);
    }

    #[test]
    fn a_mapped_owner_resolves_to_its_account() {
        let conn = migrated();
        conn.execute(
            "UPDATE connectors SET config_json = ?1 WHERE connector_id = 'github'",
            [r#"{"accounts":{"acme":"work-login"}}"#],
        )
        .expect("config");
        assert_eq!(account_for(&conn, "acme/ui").as_deref(), Some("work-login"));
        // Only the owner decides, so every repository under it is covered
        // without being listed.
        assert_eq!(
            account_for(&conn, "acme/anything-else").as_deref(),
            Some("work-login")
        );
        assert_eq!(account_for(&conn, "personal/thing"), None);
    }
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

    #[test]
    fn gh_is_never_run_by_bare_name() {
        // The defect this prevents: a macOS application bundle inherits
        // launchd's PATH, which has no Homebrew on it. `Command::new("gh")`
        // then fails with NotFound, and NEXUS tells the user to install a
        // tool they already have. Every call site must name a path.
        let production = include_str!("github_connector.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("the file must keep its test module marker");
        for bare in ["run(GH,", "run_with_env(GH,", "run(\"gh\"", "= \"gh\";"] {
            assert!(
                !production.contains(bare),
                "gh must be resolved to an absolute path, found {bare}"
            );
        }
        assert!(
            production.contains("GH_CANDIDATES"),
            "the candidate path list must remain the way gh is found"
        );
    }

    #[test]
    fn every_candidate_gh_path_is_absolute() {
        for path in GH_CANDIDATES {
            assert!(
                path.starts_with('/'),
                "{path} would be resolved against PATH, which is the bug"
            );
        }
    }

    #[test]
    fn a_present_gh_resolves_to_a_path_rather_than_a_name() {
        // Only asserts the property when `gh` is genuinely installed here,
        // so the suite still passes on a machine without it.
        if GH_CANDIDATES
            .into_iter()
            .any(|p| std::path::Path::new(p).exists())
        {
            assert!(
                gh_bin().starts_with('/'),
                "an installed gh must be run by path, got {}",
                gh_bin()
            );
        }
    }
}
