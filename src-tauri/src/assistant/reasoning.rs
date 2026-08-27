//! NEXUS-019: the reasoning layer.
//!
//! A reasoning provider is a resource NEXUS consults. It is not the
//! assistant, and it is emphatically not the security boundary. Three rules
//! give that teeth rather than leaving it as a slogan:
//!
//! 1. **A provider returns an answer or a plan, never an effect.** The two
//!    are separate variants of separate types, so a paragraph of prose cannot
//!    become executable by containing action-shaped words.
//! 2. **A plan is validated against the action registry before the user sees
//!    it.** An unknown action id is rejected outright, and every step's input
//!    must deserialise into that action's own typed struct. NEXUS does not
//!    improvise a near match.
//! 3. **Every validated step still goes through the gate.** Permission,
//!    confirmation and audit are unchanged. A plan is a *proposal*, and the
//!    most a provider can achieve is to suggest something the user is then
//!    asked to approve.
//!
//! The context a provider sees is built here too, and it is built by
//! subtraction: fields are added deliberately, never copied wholesale. What
//! travels is names and counts; message bodies, file contents and page text
//! require an explicit per-connector grant, and credentials never travel at
//! all.
//!
//! This milestone ships **no provider**. The trait, the validator, the
//! context builder and the audit are here; a local provider arrives in
//! NEXUS-022a and cloud providers in NEXUS-022b. With no provider registered,
//! escalation reports that reasoning is unavailable and every deterministic
//! tier keeps working.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::action::ActionError;
use super::context::AssistantContext;
use super::permission::Reach;

/// What a provider was asked for. Recorded in the audit; never the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Purpose {
    /// A question with no action attached.
    Answer,
    /// "Do this thing", where the thing is not a known command.
    Plan,
    /// Condense something the user already has access to.
    Summarise,
    /// Turn an intent into wording, for the user to approve.
    Draft,
}

impl Purpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Purpose::Answer => "answer",
            Purpose::Plan => "plan",
            Purpose::Summarise => "summarise",
            Purpose::Draft => "draft",
        }
    }
}

/// A category of information, for the audit and for the privacy rule.
///
/// The audit records these words, never the values behind them, so the trail
/// can say "3 task titles and one PR number" without keeping either.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContextCategory {
    /// The user's own words. Always sent: it is the question.
    Request,
    /// Action ids and one-line summaries. Contains no user data.
    ActionCatalogue,
    /// Project and task names, statuses and counts.
    WorkspaceNames,
    /// What NEXUS said earlier this session.
    ConversationSummary,
    /// Identifiers such as a PR number or an issue key.
    ExternalIdentifiers,
    /// Message bodies, file contents, page text. Requires a content grant.
    ExternalContent,
}

impl ContextCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            ContextCategory::Request => "request",
            ContextCategory::ActionCatalogue => "action catalogue",
            ContextCategory::WorkspaceNames => "workspace names",
            ContextCategory::ConversationSummary => "conversation summary",
            ContextCategory::ExternalIdentifiers => "external identifiers",
            ContextCategory::ExternalContent => "external content",
        }
    }

    /// Whether sending this off the machine needs an explicit grant.
    pub fn needs_content_grant(self) -> bool {
        matches!(self, ContextCategory::ExternalContent)
    }
}

/// One action a provider proposes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanStep {
    pub action_id: String,
    #[serde(default)]
    pub input: serde_json::Value,
}

/// What a provider returned.
///
/// Two variants of one enum, deliberately not one type with optional fields:
/// an answer has no `steps` to accidentally read, and a plan has no prose to
/// accidentally show as if it were an answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Reasoning {
    #[serde(rename_all = "camelCase")]
    Answer { text: String },
    #[serde(rename_all = "camelCase")]
    Plan {
        steps: Vec<PlanStep>,
        rationale: String,
    },
}

/// What NEXUS sends to a provider.
///
/// Built by [`build_context`], which adds fields deliberately. Nothing here
/// is a handle: a provider receives text and cannot follow it anywhere.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiContext {
    pub request: String,
    /// `(action id, one-line summary)` for actions the user may actually run.
    pub actions: Vec<(String, String)>,
    /// Short factual lines: "Project Atlas, 3 open, 1 blocked".
    pub workspace: Vec<String>,
    /// What NEXUS said earlier, one line per turn.
    pub conversation: Vec<String>,
    /// Which categories are present. Travels with the context so the audit
    /// records exactly what was included.
    pub categories: Vec<ContextCategory>,
}

/// Ceiling on what any single request may carry.
///
/// A budget rather than a guess: context is the thing that grows silently,
/// and an unbounded prompt is both a cost and a privacy problem.
pub const MAX_ACTIONS: usize = 40;
pub const MAX_WORKSPACE_LINES: usize = 12;
pub const MAX_CONVERSATION_LINES: usize = 8;
pub const MAX_REQUEST_CHARS: usize = 2_000;

/// Whether external content may leave the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrivacyPolicy {
    /// The master switch: "Allow NEXUS to use external AI for reasoning".
    pub external_reasoning_allowed: bool,
    /// Separately granted, and off by default: message bodies, file and page
    /// contents.
    pub content_sharing_allowed: bool,
}

impl Default for PrivacyPolicy {
    fn default() -> Self {
        // Both off. A user who has not chosen has not consented.
        PrivacyPolicy {
            external_reasoning_allowed: false,
            content_sharing_allowed: false,
        }
    }
}

/// Assemble the context for one request.
///
/// Deliberately takes the whole [`AssistantContext`] and returns much less.
/// Reading the body of this function should be enough to know everything that
/// can ever reach a provider.
pub fn build_context(
    request: &str,
    context: &AssistantContext,
    catalogue: &[(String, String)],
) -> AiContext {
    let mut categories = vec![
        ContextCategory::Request,
        ContextCategory::ActionCatalogue,
    ];

    let request_text: String = request.chars().take(MAX_REQUEST_CHARS).collect();

    let actions: Vec<(String, String)> = catalogue.iter().take(MAX_ACTIONS).cloned().collect();

    // Names, statuses and counts. No descriptions, no repository paths, no
    // external ids: a project's name is what a question is phrased in terms
    // of, and the rest is detail a provider does not need to reason.
    let mut workspace = Vec::new();
    if let Some(project) = &context.work.current_project {
        workspace.push(format!(
            "Current project {}: {} open, {} blocked",
            project.name, project.open_tasks, project.blocked_tasks
        ));
    }
    if let Some(task) = &context.work.current_task {
        workspace.push(format!("Current task {} ({})", task.title, task.status));
    }
    for referent in context.session.referents.iter().rev().take(6) {
        workspace.push(format!("{:?}: {}", referent.kind, referent.display_name));
    }
    workspace.truncate(MAX_WORKSPACE_LINES);
    if !workspace.is_empty() {
        categories.push(ContextCategory::WorkspaceNames);
    }

    // What NEXUS said, not what it read. Turn summaries are NEXUS's own
    // sentences, which is why they are safe to send.
    let conversation: Vec<String> = context
        .session
        .turns
        .iter()
        .rev()
        .filter_map(|turn| turn.summary.clone())
        .take(MAX_CONVERSATION_LINES)
        .collect();
    if !conversation.is_empty() {
        categories.push(ContextCategory::ConversationSummary);
    }

    categories.sort();
    categories.dedup();

    // Belt and braces. Nothing above adds a content category, and this makes
    // that a property of the function rather than of the reader's attention:
    // if a future edit starts including message bodies, it has to remove this
    // line to do it.
    categories.retain(|category| !category.needs_content_grant());

    AiContext {
        request: request_text,
        actions,
        workspace,
        conversation,
        categories,
    }
}

/// Why a provider was not consulted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ReasoningUnavailable {
    /// No provider is configured at all.
    NoProvider,
    /// The user has not allowed external reasoning.
    NotAllowed,
    /// Configured, but not reachable right now.
    Unreachable { detail: String },
}

impl std::fmt::Display for ReasoningUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReasoningUnavailable::NoProvider => write!(
                f,
                "NEXUS has no reasoning provider set up, so it can only do what it knows \
                 how to do directly."
            ),
            ReasoningUnavailable::NotAllowed => write!(
                f,
                "External AI reasoning is turned off, so NEXUS stayed local. You can turn \
                 it on in Settings."
            ),
            ReasoningUnavailable::Unreachable { detail } => {
                write!(f, "The reasoning provider could not be reached. {detail}")
            }
        }
    }
}

/// A reasoning provider.
///
/// Note what is absent: no handle on the connectors, no handle on the gate,
/// no database. A provider receives text and returns text or a proposal.
pub trait ReasoningProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn model(&self) -> String;
    /// `LocalOnly` for a model on this machine. Anything else means the
    /// context leaves, which changes what may be included in it.
    fn reach(&self) -> Reach;
    fn available(&self) -> bool;
    fn reason(
        &self,
        purpose: Purpose,
        context: &AiContext,
    ) -> Result<Reasoning, ReasoningUnavailable>;
}

/// Settings keys for the privacy switches.
///
/// Read straight from the key/value table rather than added to the typed
/// `Settings` struct: they are a policy for a subsystem, not a user
/// preference the rest of the app reads, and the settings module already
/// preserves keys it does not own.
const KEY_EXTERNAL_ALLOWED: &str = "ai_external_allowed";
const KEY_CONTENT_ALLOWED: &str = "ai_content_allowed";

fn read_flag(conn: &Connection, key: &str) -> bool {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        [key],
        |row| row.get::<_, String>(0),
    )
    .map(|v| v == "true")
    .unwrap_or(false)
}

/// What the user has allowed. Absent means denied.
pub fn read_policy(conn: &Connection) -> PrivacyPolicy {
    PrivacyPolicy {
        external_reasoning_allowed: read_flag(conn, KEY_EXTERNAL_ALLOWED),
        content_sharing_allowed: read_flag(conn, KEY_CONTENT_ALLOWED),
    }
}

pub fn set_policy(conn: &Connection, policy: PrivacyPolicy) -> Result<(), String> {
    for (key, value) in [
        (KEY_EXTERNAL_ALLOWED, policy.external_reasoning_allowed),
        (KEY_CONTENT_ALLOWED, policy.content_sharing_allowed),
    ] {
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE
               SET value = ?2,
                   updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            rusqlite::params![key, if value { "true" } else { "false" }],
        )
        .map_err(|e| format!("Failed to store the reasoning policy: {e}"))?;
    }
    Ok(())
}

/// Whether a provider may be consulted at all, and why not.
///
/// A local provider is exempt from the external switch: nothing leaves the
/// machine, so there is nothing for the user to have consented to.
pub fn may_consult(
    conn: &Connection,
    provider: &dyn ReasoningProvider,
) -> Result<(), ReasoningUnavailable> {
    if !provider.available() {
        return Err(ReasoningUnavailable::Unreachable {
            detail: format!("{} is not responding.", provider.id()),
        });
    }
    if provider.reach() == Reach::LeavesMachine && !read_policy(conn).external_reasoning_allowed {
        return Err(ReasoningUnavailable::NotAllowed);
    }
    Ok(())
}

/// Every provider NEXUS knows about, in preference order.
///
/// **Local first, always.** A model on this machine answers without anything
/// leaving it, so it is tried before any cloud provider regardless of what is
/// configured. Owned rather than `'static` because a provider carries
/// configuration read from the database.
pub fn providers(conn: &Connection) -> Vec<Box<dyn ReasoningProvider>> {
    let mut out: Vec<Box<dyn ReasoningProvider>> = vec![Box::new(
        super::ollama_provider::OllamaProvider::from_settings(conn),
    )];
    out.extend(super::cloud_provider::configured(conn));
    out
}

/// The first provider that is usable, and why none is if none is.
///
/// Local before remote, and the privacy switch applied per provider rather
/// than globally: a local model is not an external service, so turning
/// external reasoning off must not silence it.
pub fn best_provider(
    conn: &Connection,
) -> Result<Box<dyn ReasoningProvider>, ReasoningUnavailable> {
    let candidates = providers(conn);
    if candidates.is_empty() {
        return Err(ReasoningUnavailable::NoProvider);
    }

    let mut last = ReasoningUnavailable::NoProvider;
    for provider in candidates {
        match may_consult(conn, provider.as_ref()) {
            Ok(()) => return Ok(provider),
            Err(reason) => last = reason,
        }
    }
    Err(last)
}

// -- Plan validation ----------------------------------------------------------

/// A plan step that survived validation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidatedStep {
    pub action_id: String,
    pub input: serde_json::Value,
    /// The gate's own wording for this step, so the user is shown the same
    /// sentence they will be asked to approve.
    pub summary: String,
    pub requires_confirmation: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PlanRejection {
    #[serde(rename_all = "camelCase")]
    UnknownAction { action_id: String },
    #[serde(rename_all = "camelCase")]
    InvalidInput { action_id: String, detail: String },
    #[serde(rename_all = "camelCase")]
    TooManySteps { limit: usize },
    Empty,
}

impl std::fmt::Display for PlanRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanRejection::UnknownAction { action_id } => write!(
                f,
                "The reasoning provider proposed {action_id}, which NEXUS cannot do."
            ),
            PlanRejection::InvalidInput { action_id, detail } => write!(
                f,
                "The reasoning provider's input for {action_id} was malformed: {detail}"
            ),
            PlanRejection::TooManySteps { limit } => {
                write!(f, "That plan had more than {limit} steps.")
            }
            PlanRejection::Empty => write!(f, "That plan had no steps in it."),
        }
    }
}

/// A plan longer than this is not a plan; it is a program.
pub const MAX_PLAN_STEPS: usize = 6;

/// Check every step against the action registry.
///
/// The whole point of the reasoning layer: what comes back is treated as a
/// *proposal in NEXUS's own vocabulary*, and anything outside that vocabulary
/// is rejected rather than interpreted. An action id NEXUS does not have is
/// not resolved to a near match, because a plausible wrong action is worse
/// than no action.
pub fn validate_plan(
    steps: &[PlanStep],
    conn: &Connection,
) -> Result<Vec<ValidatedStep>, PlanRejection> {
    if steps.is_empty() {
        return Err(PlanRejection::Empty);
    }
    if steps.len() > MAX_PLAN_STEPS {
        return Err(PlanRejection::TooManySteps {
            limit: MAX_PLAN_STEPS,
        });
    }

    let mut validated = Vec::with_capacity(steps.len());
    for step in steps {
        let (connector, spec) = super::connectors()
            .into_iter()
            .find_map(|c| c.spec(&step.action_id).map(|spec| (c, spec)))
            .ok_or_else(|| PlanRejection::UnknownAction {
                action_id: step.action_id.clone(),
            })?;

        // Deserialisation is the validation. `dry_run` never dispatches: it
        // asks the connector to parse the input and nothing else.
        if let Err(ActionError::InvalidInput { detail }) =
            connector.validate_input(spec.id, &step.input)
        {
            return Err(PlanRejection::InvalidInput {
                action_id: step.action_id.clone(),
                detail,
            });
        }

        validated.push(ValidatedStep {
            summary: connector.summarize(spec.id, &step.input, conn),
            requires_confirmation: spec.permission.always_confirms()
                || spec.confirm == super::permission::ConfirmPolicy::Always,
            action_id: step.action_id.clone(),
            input: step.input.clone(),
        });
    }

    Ok(validated)
}

// -- Audit --------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiAuditEntry {
    pub id: i64,
    pub provider: String,
    pub model: String,
    pub reach: String,
    pub purpose: String,
    pub categories: String,
    pub outcome: String,
    pub duration_ms: Option<i64>,
    pub created_at: String,
}

/// Record that a provider was consulted.
///
/// Categories, never contents. The trail answers "why did NEXUS contact an
/// external service", which is the question worth being able to answer; it
/// deliberately cannot answer "what exactly did it say", because keeping that
/// would be keeping the conversation.
pub fn record_use(
    conn: &Connection,
    provider: &str,
    model: &str,
    reach: Reach,
    purpose: Purpose,
    categories: &[ContextCategory],
    outcome: &str,
    duration_ms: u128,
) -> Result<i64, String> {
    let words: Vec<&str> = categories.iter().map(|c| c.as_str()).collect();
    conn.execute(
        "INSERT INTO ai_audit
             (provider, model, reach, purpose, categories, outcome, duration_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            provider,
            model,
            match reach {
                Reach::LocalOnly => "local",
                Reach::LeavesMachine => "remote",
            },
            purpose.as_str(),
            words.join(", "),
            outcome,
            duration_ms as i64,
        ],
    )
    .map_err(|e| format!("Failed to record reasoning use: {e}"))?;
    Ok(conn.last_insert_rowid())
}

pub fn list_recent_use(conn: &Connection, limit: i64) -> Result<Vec<AiAuditEntry>, String> {
    let capped = limit.clamp(1, 100);
    let mut stmt = conn
        .prepare(
            "SELECT id, provider, model, reach, purpose, categories, outcome,
                    duration_ms, created_at
               FROM ai_audit ORDER BY created_at DESC, id DESC LIMIT ?1",
        )
        .map_err(|e| format!("Failed to read the reasoning trail: {e}"))?;
    let rows = stmt
        .query_map([capped], |row| {
            Ok(AiAuditEntry {
                id: row.get(0)?,
                provider: row.get(1)?,
                model: row.get(2)?,
                reach: row.get(3)?,
                purpose: row.get(4)?,
                categories: row.get(5)?,
                outcome: row.get(6)?,
                duration_ms: row.get(7)?,
                created_at: row.get(8)?,
            })
        })
        .map_err(|e| format!("Failed to read the reasoning trail: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("Failed to read the reasoning trail: {e}"))?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assistant::session::AssistantSession;
    use crate::db::migrations::MIGRATIONS;
    use serde_json::json;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch("PRAGMA foreign_keys = ON;").expect("fk");
        for (_, sql) in MIGRATIONS {
            conn.execute_batch(sql).expect("migrate");
        }
        crate::assistant::register_connectors(&conn).expect("register");
        conn
    }

    fn empty_context(conn: &Connection) -> AssistantContext {
        let session = AssistantSession::default();
        crate::assistant::context::assemble(conn, &session, 0).expect("assemble")
    }

    // -- Plan validation: the security property ------------------------------

    #[test]
    fn an_unknown_action_is_rejected_outright() {
        // The single most important test in this module. NEXUS does not
        // resolve a near match, because a plausible wrong action is worse
        // than no action at all.
        let conn = test_conn();
        let rejection = validate_plan(
            &[PlanStep {
                action_id: "nexus.delete_everything".to_string(),
                input: json!({}),
            }],
            &conn,
        )
        .expect_err("must reject");
        assert!(
            matches!(rejection, PlanRejection::UnknownAction { .. }),
            "{rejection:?}"
        );
    }

    #[test]
    fn an_action_id_that_is_nearly_right_is_still_rejected() {
        let conn = test_conn();
        for nearly in ["nexus.open_setting", "nexus.opensettings", "open_settings"] {
            assert!(
                validate_plan(
                    &[PlanStep {
                        action_id: nearly.to_string(),
                        input: json!(null)
                    }],
                    &conn
                )
                .is_err(),
                "{nearly} must not resolve"
            );
        }
    }

    #[test]
    fn a_step_with_malformed_input_is_rejected_before_the_user_sees_it() {
        let conn = test_conn();
        let rejection = validate_plan(
            &[PlanStep {
                action_id: "nexus.open_project".to_string(),
                input: json!({ "projectId": "not a number" }),
            }],
            &conn,
        )
        .expect_err("must reject");
        assert!(
            matches!(rejection, PlanRejection::InvalidInput { .. }),
            "{rejection:?}"
        );
    }

    #[test]
    fn an_invented_field_is_rejected() {
        let conn = test_conn();
        assert!(validate_plan(
            &[PlanStep {
                action_id: "nexus.open_project".to_string(),
                input: json!({ "projectId": 1, "force": true }),
            }],
            &conn
        )
        .is_err());
    }

    #[test]
    fn a_valid_plan_carries_the_gates_own_wording() {
        let conn = test_conn();
        let steps = validate_plan(
            &[PlanStep {
                action_id: "nexus.open_settings".to_string(),
                input: json!(null),
            }],
            &conn,
        )
        .expect("valid");
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].summary, "Open Settings");
        assert!(!steps[0].requires_confirmation, "navigation does not confirm");
    }

    #[test]
    fn a_destructive_step_is_marked_as_needing_confirmation() {
        let conn = test_conn();
        let steps = validate_plan(
            &[PlanStep {
                action_id: "nexus.delete_project".to_string(),
                input: json!({ "id": 1 }),
            }],
            &conn,
        )
        .expect("valid shape");
        assert!(
            steps[0].requires_confirmation,
            "a provider must never be able to propose an unconfirmed deletion"
        );
    }

    #[test]
    fn an_empty_or_oversized_plan_is_rejected() {
        let conn = test_conn();
        assert!(matches!(
            validate_plan(&[], &conn),
            Err(PlanRejection::Empty)
        ));
        let many: Vec<PlanStep> = (0..MAX_PLAN_STEPS + 1)
            .map(|_| PlanStep {
                action_id: "nexus.open_settings".to_string(),
                input: json!(null),
            })
            .collect();
        assert!(matches!(
            validate_plan(&many, &conn),
            Err(PlanRejection::TooManySteps { .. })
        ));
    }

    #[test]
    fn validation_never_executes_anything() {
        // Validating a deletion must not delete.
        let conn = test_conn();
        conn.execute("INSERT INTO projects (name) VALUES ('Atlas')", [])
            .expect("seed");
        let id = conn.last_insert_rowid();
        let _ = validate_plan(
            &[PlanStep {
                action_id: "nexus.delete_project".to_string(),
                input: json!({ "id": id }),
            }],
            &conn,
        );
        let still: i64 = conn
            .query_row("SELECT COUNT(*) FROM projects WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .expect("count");
        assert_eq!(still, 1, "validation must not have side effects");
    }

    // -- Answer and plan are separate types ----------------------------------

    #[test]
    fn an_answer_cannot_be_read_as_a_plan() {
        // Prose containing action-shaped words is still prose.
        let answer = Reasoning::Answer {
            text: "You should run nexus.delete_project with id 1.".to_string(),
        };
        assert!(
            !matches!(answer, Reasoning::Plan { .. }),
            "an answer has no steps to execute"
        );
        let json = serde_json::to_string(&answer).expect("serialize");
        assert!(json.contains("\"kind\":\"answer\""));
        assert!(!json.contains("\"steps\""));
    }

    // -- Context is built by subtraction -------------------------------------

    #[test]
    fn context_carries_names_and_counts_never_bodies() {
        let conn = test_conn();
        let context = empty_context(&conn);
        let built = build_context("what should I do", &context, &[]);
        assert_eq!(built.request, "what should I do");
        assert!(!built.categories.contains(&ContextCategory::ExternalContent));
    }

    #[test]
    fn context_is_budgeted() {
        let conn = test_conn();
        let context = empty_context(&conn);
        let catalogue: Vec<(String, String)> = (0..200)
            .map(|i| (format!("a.{i}"), format!("Action {i}")))
            .collect();
        let built = build_context(&"x".repeat(9_000), &context, &catalogue);
        assert_eq!(built.actions.len(), MAX_ACTIONS);
        assert_eq!(built.request.chars().count(), MAX_REQUEST_CHARS);
        assert!(built.workspace.len() <= MAX_WORKSPACE_LINES);
        assert!(built.conversation.len() <= MAX_CONVERSATION_LINES);
    }

    #[test]
    fn the_categories_travel_with_the_context() {
        let conn = test_conn();
        let context = empty_context(&conn);
        let built = build_context("hello", &context, &[("a.b".into(), "A".into())]);
        assert!(built.categories.contains(&ContextCategory::Request));
        assert!(built.categories.contains(&ContextCategory::ActionCatalogue));
    }

    #[test]
    fn only_external_content_needs_a_grant() {
        assert!(ContextCategory::ExternalContent.needs_content_grant());
        for benign in [
            ContextCategory::Request,
            ContextCategory::ActionCatalogue,
            ContextCategory::WorkspaceNames,
            ContextCategory::ConversationSummary,
            ContextCategory::ExternalIdentifiers,
        ] {
            assert!(!benign.needs_content_grant(), "{benign:?}");
        }
    }

    #[test]
    fn the_context_type_has_nowhere_to_put_a_credential() {
        // Structural, not aspirational: there is no field for one.
        let production = include_str!("reasoning.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("marker");
        for forbidden in ["token", "password", "secret", "api_key", "keychain"] {
            assert!(
                !production.to_lowercase().contains(forbidden),
                "the reasoning layer must never touch {forbidden}"
            );
        }
    }

    #[test]
    fn a_provider_gets_no_handle_on_anything() {
        let production = include_str!("reasoning.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("marker");
        // The trait's only method takes a purpose and a context.
        assert!(production.contains("fn reason(\n        &self,\n        purpose: Purpose,\n        context: &AiContext,"));
        assert!(!production.contains("fn reason(&self, conn:"));
    }

    // -- Nothing is available yet, and it says so ----------------------------

    #[test]
    fn the_local_provider_is_always_tried_first() {
        // Preference order is a privacy decision, not a performance one: the
        // provider that keeps everything on the machine goes first.
        let conn = test_conn();
        let listed = providers(&conn);
        assert!(!listed.is_empty());
        assert_eq!(listed[0].reach(), Reach::LocalOnly);
        assert_eq!(listed[0].id(), "ollama");
    }

    #[test]
    fn with_nothing_running_the_reason_is_reported_not_swallowed() {
        let conn = test_conn();
        match best_provider(&conn) {
            // Ollama is not installed on this machine.
            Err(reason) => assert!(
                !reason.to_string().is_empty(),
                "a refusal must say something useful"
            ),
            Ok(provider) => assert!(provider.available()),
        }
    }

    #[test]
    fn unavailability_explains_itself_and_names_the_remedy() {
        assert!(ReasoningUnavailable::NotAllowed.to_string().contains("Settings"));
        assert!(ReasoningUnavailable::NoProvider
            .to_string()
            .contains("directly"));
    }

    #[test]
    fn external_reasoning_is_off_until_the_user_says_otherwise() {
        let conn = test_conn();
        let policy = read_policy(&conn);
        assert!(!policy.external_reasoning_allowed);
        assert!(!policy.content_sharing_allowed);
        assert_eq!(policy, PrivacyPolicy::default());
    }

    #[test]
    fn the_policy_round_trips() {
        let conn = test_conn();
        set_policy(
            &conn,
            PrivacyPolicy {
                external_reasoning_allowed: true,
                content_sharing_allowed: false,
            },
        )
        .expect("store");
        let policy = read_policy(&conn);
        assert!(policy.external_reasoning_allowed);
        assert!(!policy.content_sharing_allowed, "content stays off separately");
    }

    struct FakeProvider {
        reach: Reach,
        available: bool,
    }

    impl ReasoningProvider for FakeProvider {
        fn id(&self) -> &'static str {
            "fake"
        }
        fn model(&self) -> String {
            "fake-1".to_string()
        }
        fn reach(&self) -> Reach {
            self.reach
        }
        fn available(&self) -> bool {
            self.available
        }
        fn reason(
            &self,
            _purpose: Purpose,
            _context: &AiContext,
        ) -> Result<Reasoning, ReasoningUnavailable> {
            Ok(Reasoning::Answer {
                text: "fake".to_string(),
            })
        }
    }

    #[test]
    fn a_remote_provider_is_refused_until_external_reasoning_is_allowed() {
        let conn = test_conn();
        let remote = FakeProvider {
            reach: Reach::LeavesMachine,
            available: true,
        };
        assert!(matches!(
            may_consult(&conn, &remote),
            Err(ReasoningUnavailable::NotAllowed)
        ));

        set_policy(
            &conn,
            PrivacyPolicy {
                external_reasoning_allowed: true,
                content_sharing_allowed: false,
            },
        )
        .expect("allow");
        assert!(may_consult(&conn, &remote).is_ok());
    }

    #[test]
    fn a_local_provider_needs_no_permission_to_leave_the_machine() {
        // Because it does not leave it. There is nothing to consent to.
        let conn = test_conn();
        let local = FakeProvider {
            reach: Reach::LocalOnly,
            available: true,
        };
        assert!(
            may_consult(&conn, &local).is_ok(),
            "a model on this machine is not an external service"
        );
    }

    #[test]
    fn an_unreachable_provider_is_reported_as_such() {
        let conn = test_conn();
        let down = FakeProvider {
            reach: Reach::LocalOnly,
            available: false,
        };
        assert!(matches!(
            may_consult(&conn, &down),
            Err(ReasoningUnavailable::Unreachable { .. })
        ));
    }

    // -- Audit ----------------------------------------------------------------

    #[test]
    fn the_trail_records_categories_and_not_contents() {
        let conn = test_conn();
        record_use(
            &conn,
            "ollama",
            "llama3",
            Reach::LocalOnly,
            Purpose::Answer,
            &[ContextCategory::Request, ContextCategory::WorkspaceNames],
            "ok",
            120,
        )
        .expect("record");

        let rows = list_recent_use(&conn, 10).expect("list");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].provider, "ollama");
        assert_eq!(rows[0].reach, "local");
        assert!(rows[0].categories.contains("workspace names"));
    }

    #[test]
    fn the_ai_audit_table_has_no_column_for_a_prompt() {
        // Structural guarantee: a future caller cannot quietly start keeping
        // conversations, because there is nowhere to put them.
        let conn = test_conn();
        let mut stmt = conn.prepare("PRAGMA table_info(ai_audit)").expect("pragma");
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .expect("query")
            .map(|r| r.expect("row"))
            .collect();
        for forbidden in ["prompt", "request", "response", "content", "context", "body"] {
            assert!(
                !columns.iter().any(|c| c == forbidden),
                "ai_audit must not store {forbidden}"
            );
        }
        assert!(columns.iter().any(|c| c == "categories"));
    }

    #[test]
    fn remote_and_local_use_are_distinguishable_in_the_trail() {
        let conn = test_conn();
        record_use(&conn, "ollama", "llama3", Reach::LocalOnly, Purpose::Answer, &[], "ok", 1)
            .expect("record");
        record_use(&conn, "claude", "opus", Reach::LeavesMachine, Purpose::Draft, &[], "ok", 1)
            .expect("record");
        let rows = list_recent_use(&conn, 10).expect("list");
        assert_eq!(rows[0].reach, "remote");
        assert_eq!(rows[1].reach, "local");
    }
}
