//! NEXUS-014: turning something the user said into something NEXUS can do.
//!
//! This is the escalation ladder, and every rung of it here is deterministic.
//! No provider is involved, and none is reachable from this module. When
//! NEXUS-019 adds reasoning it becomes one more rung *below* these, reached
//! only when they all decline.
//!
//! The order is the point:
//!
//! 1. **Local answer.** "Show my blocked tasks" is a question a query
//!    answers. Sending it to a model would be slower, less accurate, and
//!    would stop working on a train.
//! 2. **Known command.** The NEXUS-010 matcher already resolves spoken
//!    phrases to registry ids, and it is reused verbatim rather than
//!    reimplemented: one matching contract, one place to fix it.
//! 3. **Decline, with a reason.** Saying "I can't do that yet" is a correct
//!    answer. Guessing is not.
//!
//! The command registry arrives from the caller rather than being duplicated
//! here, exactly as the voice path already does it, so src/lib/commands.ts
//! stays the single source of truth for what NEXUS can do.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::voice::intent::{resolve_voice_intent, VoiceCommandSpec};

use super::connector::ReferentDraft;
use super::context::{work_context, WorkContext};
use super::reasoning::{
    build_context, validate_plan, AiContext, Purpose, Reasoning, ValidatedStep,
};
use super::referent::ReferentKind;
use super::session::SessionSnapshot;

/// One thing NEXUS could do, when several match.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Choice {
    /// The registry id from src/lib/commands.ts, so the caller can map it
    /// through the same bridge the palette uses.
    pub command_id: String,
    pub label: String,
}

/// What NEXUS made of the request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AssistantReply {
    /// Answered from local data. `cited` names what the answer was built
    /// from, because attribution NEXUS can actually make is worth more than
    /// a claim of correctness it cannot.
    #[serde(rename_all = "camelCase")]
    Answer { text: String, cited: Vec<String> },
    /// One clear thing to do.
    ///
    /// Carries the *registry* id, not an action id, and is not executed here.
    /// Only the palette bridge knows how a registry id maps to a typed
    /// action, and duplicating that mapping would create a second copy to
    /// drift. The caller maps it, then runs it through the gate, which is
    /// where permission and confirmation live.
    #[serde(rename_all = "camelCase")]
    Action { command_id: String, summary: String },
    /// Several things matched. NEXUS names them rather than picking.
    #[serde(rename_all = "camelCase")]
    Choices { candidates: Vec<Choice> },
    /// Nothing matched, and NEXUS says so plainly.
    #[serde(rename_all = "camelCase")]
    Unresolved { reason: String },
    /// A reasoning provider proposed steps, every one of which was validated
    /// against the action registry. Nothing has run: each step still goes
    /// through the gate, with its own permission and confirmation.
    #[serde(rename_all = "camelCase")]
    Proposal {
        steps: Vec<ValidatedStep>,
        rationale: String,
    },
}

/// A reply, plus what it put into the conversation.
///
/// An answer that lists things must register them, or "do the first one"
/// has nothing to count through. Keeping that here rather than in the
/// command layer means the code that knows the ids is the code that files
/// them.
#[derive(Debug, Clone)]
pub struct Response {
    pub reply: AssistantReply,
    pub referents: Vec<ReferentDraft>,
    /// True when the referents were rendered as a numbered list the user can
    /// count through. A pair of unrelated mentions is not a list.
    pub rendered_as_list: bool,
}

impl Response {
    fn plain(reply: AssistantReply) -> Self {
        Response {
            reply,
            referents: Vec::new(),
            rendered_as_list: false,
        }
    }
}

/// Lowercase, strip punctuation, collapse whitespace.
fn normalize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .map(|s| s.to_string())
        .collect()
}

fn has_any(tokens: &[String], words: &[&str]) -> bool {
    words.iter().any(|w| tokens.iter().any(|t| t == w))
}

fn plural(count: i64, singular: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {singular}s")
    }
}

/// Count tasks by status across the workspace.
fn count_by_status(conn: &Connection, status: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM tasks WHERE status = ?1",
        [status],
        |row| row.get::<_, i64>(0),
    )
    .unwrap_or(0)
}

/// Tasks in a status, capped so an answer stays an answer.
///
/// Returns ids alongside labels: an answer that names things has to be able
/// to register them, or the follow-up cannot refer to them.
fn tasks_by_status(conn: &Connection, status: &str, limit: i64) -> Vec<(i64, String)> {
    let mut stmt = match conn.prepare(
        "SELECT t.id, t.title, p.name
           FROM tasks t JOIN projects p ON p.id = t.project_id
          WHERE t.status = ?1
          ORDER BY t.updated_at DESC, t.id DESC
          LIMIT ?2",
    ) {
        Ok(stmt) => stmt,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map(rusqlite::params![status, limit], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            format!(
                "{} ({})",
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?
            ),
        ))
    });
    match rows {
        Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// Longest list NEXUS will read back before it stops being an answer.
const ANSWER_LIST_CAP: i64 = 5;

/// Try to answer from local data alone.
///
/// A fixed set of patterns, not an intent classifier. Each one is a question
/// with a factual answer NEXUS already holds, which is exactly the set worth
/// keeping away from a model.
fn local_answer(tokens: &[String], conn: &Connection, work: &WorkContext) -> Option<Response> {
    // "What am I working on?" / "What is my current project?"
    if has_any(tokens, &["working", "current", "active"])
        && has_any(tokens, &["on", "project", "task"])
    {
        return Some(Response::plain(match (&work.current_project, &work.current_task) {
            (Some(project), Some(task)) => AssistantReply::Answer {
                text: format!(
                    "You're on {}, most recently {} ({}). It has {} open and {}.",
                    project.name,
                    task.title,
                    task.status,
                    project.open_tasks,
                    plural(project.blocked_tasks, "blocked task")
                ),
                cited: vec![project.name.clone(), task.title.clone()],
            },
            (Some(project), None) => AssistantReply::Answer {
                text: format!(
                    "You're on {}, with {} open and {}.",
                    project.name,
                    project.open_tasks,
                    plural(project.blocked_tasks, "blocked task")
                ),
                cited: vec![project.name.clone()],
            },
            _ => AssistantReply::Answer {
                text: "Nothing yet this session. Open a project and I'll keep track."
                    .to_string(),
                cited: Vec::new(),
            },
        }));
    }

    // "Show my blocked tasks."
    for (status, words) in [
        ("blocked", &["blocked"][..]),
        ("open", &["open"][..]),
        ("done", &["done", "finished", "completed"][..]),
    ] {
        if has_any(tokens, words) && has_any(tokens, &["task", "tasks", "todo", "todos"]) {
            let total = count_by_status(conn, status);
            if total == 0 {
                return Some(Response::plain(AssistantReply::Answer {
                    text: format!("Nothing is {status}."),
                    cited: Vec::new(),
                }));
            }
            let rows = tasks_by_status(conn, status, ANSWER_LIST_CAP);
            let titles: Vec<String> = rows.iter().map(|(_, label)| label.clone()).collect();
            let shown = titles.len() as i64;
            let mut text = format!("{}: {}", plural(total, "task"), titles.join(", "));
            if total > shown {
                text.push_str(&format!(", and {} more", total - shown));
            }
            text.push('.');
            return Some(Response {
                reply: AssistantReply::Answer {
                    text,
                    cited: titles,
                },
                // Registered in the order the user reads them, which is what
                // makes "do the first one" a position rather than a guess.
                referents: rows
                    .into_iter()
                    .map(|(id, label)| ReferentDraft {
                        kind: ReferentKind::Task,
                        display_name: label,
                        metadata: serde_json::json!({ "id": id }),
                    })
                    .collect(),
                rendered_as_list: true,
            });
        }
    }

    None
}

/// Resolve a request against everything NEXUS can do without help.
///
/// `commands` and `project_names` come from the caller so the registry has
/// one definition. `snapshot` supplies work context; referent resolution is
/// handled separately, before this is reached.
pub fn respond(
    text: &str,
    conn: &Connection,
    snapshot: &SessionSnapshot,
    commands: &[VoiceCommandSpec],
    project_names: &[String],
) -> Response {
    let tokens = normalize(text);
    if tokens.is_empty() {
        return Response::plain(AssistantReply::Unresolved {
            reason: "There was nothing to act on.".to_string(),
        });
    }

    // 1. Local data first. A question a query answers should never travel.
    let work = work_context(conn, snapshot);
    if let Some(answer) = local_answer(&tokens, conn, &work) {
        return answer;
    }

    // 2. The NEXUS-010 matcher, reused rather than reimplemented.
    let intent = resolve_voice_intent(text, commands, project_names);

    match intent.command_ids.len() {
        1 => {
            let command_id = intent.command_ids[0].clone();
            let label = commands
                .iter()
                .find(|c| c.id == command_id)
                .map(|c| c.label.clone())
                .unwrap_or_else(|| command_id.clone());
            Response::plain(AssistantReply::Action {
                summary: label.clone(),
                command_id,
            })
        }
        0 => {
            // A project name with no command around it means "open it".
            if let Some(name) = intent.project_name.clone() {
                return Response::plain(AssistantReply::Action {
                    command_id: format!("open-project:{name}"),
                    summary: format!("Open project {name}"),
                });
            }
            escalate(text, conn, snapshot, commands)
        }
        _ => Response::plain(AssistantReply::Choices {
            candidates: intent
                .command_ids
                .iter()
                .take(5)
                .map(|id| Choice {
                    label: commands
                        .iter()
                        .find(|c| &c.id == id)
                        .map(|c| c.label.clone())
                        .unwrap_or_else(|| id.clone()),
                    command_id: id.clone(),
                })
                .collect(),
        }),
    }
}

/// Rung 3: ask a reasoning provider, if there is one and it is allowed.
///
/// Reached only when a local answer and the deterministic matcher have both
/// declined. With no provider configured this returns a plain refusal that
/// names the reason, which is the behaviour NEXUS has today: the deterministic
/// tiers keep working with no network and no model.
fn escalate(
    text: &str,
    conn: &Connection,
    snapshot: &SessionSnapshot,
    commands: &[VoiceCommandSpec],
) -> Response {
    // Local first, then any configured cloud provider, with the privacy
    // switch applied per provider rather than globally.
    let provider = match super::reasoning::best_provider(conn) {
        Ok(provider) => provider,
        Err(unavailable) => {
            return Response::plain(AssistantReply::Unresolved {
                reason: unavailable.to_string(),
            })
        }
    };

    // Built by subtraction, and budgeted. Everything a provider can ever see
    // is decided in `build_context`.
    let context: AiContext = match super::context::assemble(conn, &Default::default(), 0) {
        Ok(assembled) => build_context(text, &assembled, &catalogue(commands)),
        Err(_) => build_context(text, &empty_assistant_context(snapshot), &catalogue(commands)),
    };

    let started = std::time::Instant::now();
    let outcome = provider.reason(Purpose::Plan, &context);

    // Categories, never contents. This is the row that answers "why did NEXUS
    // contact a provider", and it is written whether or not the call worked.
    let _ = super::reasoning::record_use(
        conn,
        provider.id(),
        &provider.model(),
        provider.reach(),
        Purpose::Plan,
        &context.categories,
        if outcome.is_ok() { "ok" } else { "failed" },
        started.elapsed().as_millis(),
    );

    match outcome {
        Ok(Reasoning::Answer { text }) => Response::plain(AssistantReply::Answer {
            // Marked as generated where it is rendered: NEXUS attributes, it
            // does not vouch.
            text,
            cited: Vec::new(),
        }),
        Ok(Reasoning::Plan { steps, rationale }) => match validate_plan(&steps, conn) {
            Ok(validated) => Response::plain(AssistantReply::Proposal {
                steps: validated,
                rationale,
            }),
            // A plan NEXUS cannot express in its own vocabulary is refused,
            // never approximated.
            Err(rejection) => Response::plain(AssistantReply::Unresolved {
                reason: rejection.to_string(),
            }),
        },
        Err(unavailable) => Response::plain(AssistantReply::Unresolved {
            reason: unavailable.to_string(),
        }),
    }
}

/// Action ids and summaries, which is all of the catalogue a provider needs.
fn catalogue(commands: &[VoiceCommandSpec]) -> Vec<(String, String)> {
    commands
        .iter()
        .map(|c| (c.id.clone(), c.label.clone()))
        .collect()
}

/// Used when context assembly fails: better a thin context than none.
fn empty_assistant_context(snapshot: &SessionSnapshot) -> super::context::AssistantContext {
    super::context::AssistantContext {
        session: snapshot.clone(),
        work: WorkContext {
            current_project: None,
            current_task: None,
        },
        recent_actions: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assistant::session::AssistantSession;
    use crate::db::migrations::MIGRATIONS;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch("PRAGMA foreign_keys = ON;").expect("fk");
        for (_, sql) in MIGRATIONS {
            conn.execute_batch(sql).expect("migrate");
        }
        conn
    }

    fn seed(conn: &Connection) -> i64 {
        conn.execute("INSERT INTO projects (name) VALUES ('Atlas')", [])
            .expect("seed");
        let project = conn.last_insert_rowid();
        for (title, status) in [
            ("Wire the gate", "blocked"),
            ("Ship referents", "blocked"),
            ("Write the spec", "open"),
        ] {
            conn.execute(
                "INSERT INTO tasks (project_id, title, status) VALUES (?1, ?2, ?3)",
                rusqlite::params![project, title, status],
            )
            .expect("seed task");
        }
        project
    }

    fn registry() -> Vec<VoiceCommandSpec> {
        vec![
            VoiceCommandSpec {
                id: "nav-settings".to_string(),
                label: "Go to Settings".to_string(),
                keywords: vec!["settings".into(), "preferences".into()],
            },
            VoiceCommandSpec {
                id: "nav-projects".to_string(),
                label: "Go to Projects".to_string(),
                keywords: vec!["projects".into(), "workspace".into()],
            },
            VoiceCommandSpec {
                id: "create-project".to_string(),
                label: "New Project".to_string(),
                keywords: vec!["new".into(), "create".into(), "project".into()],
            },
        ]
    }

    fn empty_snapshot() -> SessionSnapshot {
        AssistantSession::default().snapshot(0)
    }

    fn answer_text(response: &Response) -> &str {
        match &response.reply {
            AssistantReply::Answer { text, .. } => text,
            other => panic!("expected a local answer, got {other:?}"),
        }
    }

    // -- Rung 1: local answers never escalate --------------------------------

    #[test]
    fn blocked_tasks_are_answered_from_the_database() {
        let conn = test_conn();
        seed(&conn);
        let reply = respond(
            "show my blocked tasks",
            &conn,
            &empty_snapshot(),
            &registry(),
            &["Atlas".to_string()],
        );
        let text = answer_text(&reply);
        assert!(text.contains("2 tasks"), "{text}");
        assert!(text.contains("Wire the gate"), "{text}");
    }

    #[test]
    fn a_status_with_nothing_in_it_says_so() {
        let conn = test_conn();
        seed(&conn);
        let reply = respond("show done tasks", &conn, &empty_snapshot(), &registry(), &[]);
        assert_eq!(answer_text(&reply), "Nothing is done.");
    }

    #[test]
    fn a_long_list_is_capped_and_says_how_many_are_left() {
        let conn = test_conn();
        let project = seed(&conn);
        for i in 0..10 {
            conn.execute(
                "INSERT INTO tasks (project_id, title, status) VALUES (?1, ?2, 'blocked')",
                rusqlite::params![project, format!("Extra {i}")],
            )
            .expect("seed");
        }
        let reply = respond("blocked tasks", &conn, &empty_snapshot(), &registry(), &[]);
        let text = answer_text(&reply);
        assert!(text.contains("12 tasks"), "{text}");
        assert!(text.contains("more"), "{text}");
    }

    #[test]
    fn what_am_i_working_on_uses_the_conversation() {
        let conn = test_conn();
        let project = seed(&conn);
        let session = AssistantSession::default();
        session.begin_turn(crate::assistant::session::TurnInput::Text {
            text: "open atlas".to_string(),
        });
        session.remember(
            crate::assistant::referent::ReferentKind::Project,
            "Atlas",
            "nexus",
            serde_json::json!({ "id": project }),
        );

        let reply = respond(
            "what am I working on",
            &conn,
            &session.snapshot(0),
            &registry(),
            &["Atlas".to_string()],
        );
        let text = answer_text(&reply);
        assert!(text.contains("Atlas"), "{text}");
        assert!(text.contains("2 blocked tasks"), "{text}");
    }

    #[test]
    fn with_nothing_open_it_says_so_rather_than_inventing() {
        let conn = test_conn();
        seed(&conn);
        let reply = respond(
            "what am I working on",
            &conn,
            &empty_snapshot(),
            &registry(),
            &[],
        );
        assert!(answer_text(&reply).contains("Nothing yet"), "{reply:?}");
    }

    #[test]
    fn an_answer_that_lists_things_registers_them_in_reading_order() {
        // The producer that makes ordinals live: without this, "do the first
        // one" has nothing to count through.
        let conn = test_conn();
        seed(&conn);
        let response = respond("blocked tasks", &conn, &empty_snapshot(), &registry(), &[]);
        assert!(response.rendered_as_list);
        assert_eq!(response.referents.len(), 2);
        assert!(response
            .referents
            .iter()
            .all(|d| d.metadata.get("id").is_some()));

        // The property that matters is not which task is first, it is that
        // the referents are in the same order the user reads them. An ordinal
        // means a position on screen.
        let text = answer_text(&response);
        let positions: Vec<usize> = response
            .referents
            .iter()
            .map(|d| text.find(&d.display_name).expect("named in the answer"))
            .collect();
        assert!(
            positions.windows(2).all(|w| w[0] < w[1]),
            "referent order must match reading order: {positions:?} in {text:?}"
        );
    }

    #[test]
    fn a_single_fact_is_not_a_rendered_list() {
        // Two unrelated mentions are not something the user counted through.
        let conn = test_conn();
        seed(&conn);
        let response = respond("open settings", &conn, &empty_snapshot(), &registry(), &[]);
        assert!(!response.rendered_as_list);
        assert!(response.referents.is_empty());
    }

    #[test]
    fn an_empty_status_registers_nothing() {
        let conn = test_conn();
        seed(&conn);
        let response = respond("done tasks", &conn, &empty_snapshot(), &registry(), &[]);
        assert!(response.referents.is_empty());
        assert!(!response.rendered_as_list);
    }

    #[test]
    fn a_local_answer_cites_what_it_was_built_from() {
        // Attribution NEXUS can actually make, rather than a claim of
        // correctness it cannot.
        let conn = test_conn();
        seed(&conn);
        match respond("blocked tasks", &conn, &empty_snapshot(), &registry(), &[]).reply {
            AssistantReply::Answer { cited, .. } => {
                assert!(!cited.is_empty(), "an answer from data should cite it");
            }
            other => panic!("expected an answer, got {other:?}"),
        }
    }

    // -- Rung 2: the existing matcher ----------------------------------------

    #[test]
    fn a_known_command_resolves_without_touching_the_database_answer_path() {
        let conn = test_conn();
        seed(&conn);
        match respond("open settings", &conn, &empty_snapshot(), &registry(), &[]).reply {
            AssistantReply::Action { command_id, .. } => {
                assert_eq!(command_id, "nav-settings");
            }
            other => panic!("expected an action, got {other:?}"),
        }
    }

    #[test]
    fn the_reply_carries_a_registry_id_never_a_resolved_action() {
        // Only the palette bridge knows how a registry id maps to an action,
        // and duplicating that here would create a second mapping to drift.
        let production = include_str!("converse.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("the file must keep its test module marker");
        assert!(
            !production.contains("nexus.open") && !production.contains("action_id"),
            "the resolver must not name typed actions"
        );
    }

    #[test]
    fn a_bare_project_name_means_open_it() {
        let conn = test_conn();
        seed(&conn);
        match respond("atlas", &conn, &empty_snapshot(), &registry(), &["Atlas".to_string()]).reply {
            AssistantReply::Action { summary, .. } => {
                assert!(summary.contains("Atlas"), "{summary}");
            }
            other => panic!("expected an action, got {other:?}"),
        }
    }

    // -- Rung 3: declining ----------------------------------------------------

    #[test]
    fn an_unknown_request_escalates_and_then_declines_with_a_reason() {
        // The full ladder: no local answer, no command match, no provider.
        // The refusal names why rather than shrugging.
        let conn = test_conn();
        let reply = respond(
            "explain the difference between oauth and api keys",
            &conn,
            &empty_snapshot(),
            &registry(),
            &[],
        );
        match &reply.reply {
            AssistantReply::Unresolved { reason } => {
                assert!(
                    reason.contains("reasoning provider"),
                    "the refusal should say what is missing: {reason}"
                );
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn everything_deterministic_still_works_with_no_provider() {
        // The property the whole ladder exists to protect: NEXUS on a train.
        let conn = test_conn();
        seed(&conn);

        for phrase in ["show my blocked tasks", "open settings", "what am I working on"] {
            assert!(
                !matches!(
                    respond(phrase, &conn, &empty_snapshot(), &registry(), &[]).reply,
                    AssistantReply::Unresolved { .. }
                ),
                "{phrase} must not need a provider"
            );
        }
    }

    #[test]
    fn empty_input_is_handled() {
        let conn = test_conn();
        assert!(matches!(
            respond("", &conn, &empty_snapshot(), &registry(), &[]).reply,
            AssistantReply::Unresolved { .. }
        ));
        assert!(matches!(
            respond("   ", &conn, &empty_snapshot(), &registry(), &[]).reply,
            AssistantReply::Unresolved { .. }
        ));
    }

    #[test]
    fn this_module_never_talks_to_a_network_itself() {
        // Since NEXUS-019 this file owns the escalation, so it names the
        // reasoning layer. What it must never do is reach a service directly:
        // a provider is called through the trait, and the trait is the only
        // thing that knows how to reach one. Anything else would be a second
        // network path outside the audit.
        let production = include_str!("converse.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("the file must keep its test module marker");
        for forbidden in [
            "reqwest", "curl", "http://", "https://", "openai", "anthropic",
            "ollama", "api_key", "Bearer",
        ] {
            assert!(
                !production.contains(forbidden),
                "resolution must not reach a service directly, found {forbidden}"
            );
        }
    }

    #[test]
    fn the_deterministic_rungs_run_before_any_escalation() {
        // Order is the whole design. A question a query answers must never
        // travel, so a local answer has to win even when a provider exists.
        let production = include_str!("converse.rs");
        let local_at = production.find("if let Some(answer) = local_answer").expect("rung 1");
        let matcher_at = production.find("let intent = resolve_voice_intent").expect("rung 2");
        let escalate_at = production.find("escalate(text, conn").expect("rung 3");
        assert!(local_at < matcher_at, "the local answer must come first");
        assert!(matcher_at < escalate_at, "the matcher must precede escalation");
    }

    #[test]
    fn replies_serialise_as_a_tagged_union() {
        let conn = test_conn();
        seed(&conn);
        let json = serde_json::to_string(
            &respond("blocked tasks", &conn, &empty_snapshot(), &registry(), &[]).reply,
        )
        .expect("serialize");
        assert!(json.contains("\"kind\":\"answer\""), "{json}");
    }
}
