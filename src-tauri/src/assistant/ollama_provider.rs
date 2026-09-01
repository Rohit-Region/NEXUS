//! NEXUS-022a: the local reasoning provider.
//!
//! Ollama on `127.0.0.1`. This is the provider NEXUS should reach for first,
//! and the reason is not performance: **nothing leaves the machine**, so the
//! privacy question does not arise. `may_consult` exempts a local provider
//! from the external-reasoning switch for exactly that reason.
//!
//! It costs no new dependency either. `reqwest` with TLS would have added 56
//! crates; loopback needs no TLS at all, so this talks to Ollama through the
//! same `curl` transport every other connector uses.
//!
//! **Installation is never automatic.** If Ollama is not running, NEXUS says
//! so and carries on doing everything it can do without a model. Downloading
//! a several-gigabyte model on a user's behalf is not a thing an assistant
//! should decide by itself.
//!
//! One rule governs parsing, and it is the safe direction: **output that
//! cannot be understood degrades to an answer, never to a plan.** A model
//! that returns prose, or malformed JSON, or something inventive, produces
//! text for the user to read. It can never accidentally produce steps.

use std::time::Duration;

use rusqlite::Connection;

use super::http::{safe_https, send, HttpError, Request};
use super::permission::Reach;
use super::reasoning::{
    AiContext, PlanStep, Purpose, Reasoning, ReasoningProvider, ReasoningUnavailable,
};

pub const PROVIDER_ID: &str = "ollama";

/// Where Ollama listens by default.
const BASE_URL: &str = "http://127.0.0.1:11434";
/// Used when the user has not chosen. Small enough to run on a laptop.
pub const DEFAULT_MODEL: &str = "llama3.2";
const KEY_MODEL: &str = "ai_local_model";
/// A local model on a laptop is not fast. Long enough to be useful, short
/// enough that the UI does not appear frozen.
const REASON_TIMEOUT: Duration = Duration::from_secs(90);

/// The instruction the model is given.
///
/// Deliberately narrow: it is told the vocabulary it may use and told to stay
/// inside it. That is belt to the braces of `validate_plan`, which rejects
/// anything outside the registry regardless of what the prompt said.
pub const SYSTEM_PROMPT: &str = "\
You are the reasoning component of NEXUS, a local developer assistant. \
Reply with a single JSON object and nothing else.

To answer a question, reply exactly:
{\"kind\":\"answer\",\"text\":\"...\"}

To propose actions, reply exactly:
{\"kind\":\"plan\",\"rationale\":\"...\",\"steps\":[{\"actionId\":\"...\",\"input\":{...}}]}

Rules:
- Only use actionId values from the provided list. Never invent one.
- If no listed action fits, reply with an answer explaining that.
- Never claim to have done something. You propose; NEXUS executes.
- Keep answers short and factual. Say when you do not know.";

fn model_name(conn: &Connection) -> String {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        [KEY_MODEL],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .map(|v| v.trim().to_string())
    .filter(|v| !v.is_empty() && v.len() < 100)
    .unwrap_or_else(|| DEFAULT_MODEL.to_string())
}

pub fn set_model(conn: &Connection, model: &str) -> Result<(), String> {
    let cleaned = model.trim();
    if cleaned.is_empty() || cleaned.len() > 99 {
        return Err("That is not a model name.".to_string());
    }
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE
           SET value = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        rusqlite::params![KEY_MODEL, cleaned],
    )
    .map_err(|e| format!("Failed to store the model name: {e}"))?;
    Ok(())
}

/// Render the context as the prompt the model sees.
///
/// Everything here came out of `build_context`, which is where the decision
/// about what may travel is made. This only formats.
pub fn render(context: &AiContext) -> String {
    let mut out = String::new();
    out.push_str("Actions you may propose:\n");
    if context.actions.is_empty() {
        out.push_str("  (none)\n");
    }
    for (id, summary) in &context.actions {
        out.push_str(&format!("  {id} - {summary}\n"));
    }
    if !context.workspace.is_empty() {
        out.push_str("\nWhat NEXUS knows right now:\n");
        for line in &context.workspace {
            out.push_str(&format!("  {line}\n"));
        }
    }
    if !context.conversation.is_empty() {
        out.push_str("\nWhat NEXUS said earlier:\n");
        for line in &context.conversation {
            out.push_str(&format!("  {line}\n"));
        }
    }
    out.push_str(&format!("\nRequest: {}\n", context.request));
    out
}

/// Pull the first balanced JSON object out of a reply.
///
/// Models wrap JSON in prose, fences and apologies. This finds the object
/// without a parser, tracking string state so a brace inside a string does
/// not end the scan early.
fn extract_json(raw: &str) -> Option<&str> {
    let bytes = raw.as_bytes();
    let start = raw.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for index in start..bytes.len() {
        let ch = bytes[index] as char;
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&raw[start..=index]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Turn a model's reply into reasoning.
///
/// The safe direction: anything not recognisably a plan becomes an answer.
/// Text can be read; only a plan can propose an action, so ambiguity must
/// never resolve towards one.
pub fn interpret(raw: &str) -> Reasoning {
    let fallback = || Reasoning::Answer {
        text: raw.trim().to_string(),
    };

    let json = match extract_json(raw) {
        Some(json) => json,
        None => return fallback(),
    };
    let value: serde_json::Value = match serde_json::from_str(json) {
        Ok(value) => value,
        Err(_) => return fallback(),
    };

    match value.get("kind").and_then(|v| v.as_str()) {
        Some("plan") => {
            let steps: Vec<PlanStep> = value
                .get("steps")
                .and_then(|v| v.as_array())
                .map(|rows| {
                    rows.iter()
                        .filter_map(|step| {
                            Some(PlanStep {
                                action_id: step
                                    .get("actionId")
                                    .and_then(|v| v.as_str())?
                                    .to_string(),
                                input: step
                                    .get("input")
                                    .cloned()
                                    .unwrap_or(serde_json::Value::Null),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();

            // A plan with no usable steps is not a plan. Falling back to an
            // answer keeps the failure visible instead of silently empty.
            if steps.is_empty() {
                return fallback();
            }
            Reasoning::Plan {
                steps,
                rationale: value
                    .get("rationale")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            }
        }
        Some("answer") => Reasoning::Answer {
            text: value
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or(raw)
                .trim()
                .to_string(),
        },
        // Unrecognised shape. Text, never steps.
        _ => fallback(),
    }
}

/// Is Ollama listening?
fn probe() -> bool {
    match send(Request::get(&format!("{BASE_URL}/api/tags"))) {
        Ok(response) => response.ok(),
        Err(_) => false,
    }
}

/// Models Ollama actually has, for the Settings picker.
pub fn installed_models() -> Vec<String> {
    let response = match send(Request::get(&format!("{BASE_URL}/api/tags"))) {
        Ok(response) if response.ok() => response,
        _ => return Vec::new(),
    };
    serde_json::from_str::<serde_json::Value>(&response.body)
        .ok()
        .and_then(|v| v.get("models").and_then(|m| m.as_array()).cloned())
        .map(|models| {
            models
                .iter()
                .filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

pub struct OllamaProvider {
    pub model: String,
}

impl OllamaProvider {
    pub fn from_settings(conn: &Connection) -> Self {
        OllamaProvider {
            model: model_name(conn),
        }
    }
}

impl ReasoningProvider for OllamaProvider {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn model(&self) -> String {
        self.model.clone()
    }

    fn reach(&self) -> Reach {
        // The whole reason to prefer it.
        Reach::LocalOnly
    }

    fn available(&self) -> bool {
        probe()
    }

    fn reason(
        &self,
        purpose: Purpose,
        context: &AiContext,
    ) -> Result<Reasoning, ReasoningUnavailable> {
        let body = serde_json::json!({
            "model": self.model,
            "stream": false,
            "options": { "temperature": 0.1 },
            "messages": [
                { "role": "system", "content": SYSTEM_PROMPT },
                {
                    "role": "user",
                    "content": format!("Purpose: {}\n\n{}", purpose.as_str(), render(context))
                }
            ]
        })
        .to_string();

        let response = send(Request {
            method: "POST",
            url: format!("{BASE_URL}/api/chat"),
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            basic_auth: None,
            body: Some(body),
        })
        .map_err(|e| match e {
            HttpError::Unreachable { .. } => ReasoningUnavailable::Unreachable {
                detail: "Ollama is not running. Start it, or install it from ollama.com."
                    .to_string(),
            },
            other => ReasoningUnavailable::Unreachable {
                detail: other.to_string(),
            },
        })?;

        if !response.ok() {
            return Err(ReasoningUnavailable::Unreachable {
                detail: if response.status == 404 {
                    format!(
                        "Ollama does not have the model \"{}\". Pull it with: ollama pull {}",
                        self.model, self.model
                    )
                } else {
                    format!("Ollama returned {}.", response.status)
                },
            });
        }

        let content = serde_json::from_str::<serde_json::Value>(&response.body)
            .ok()
            .and_then(|v| {
                v.get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_str())
                    .map(String::from)
            })
            .ok_or_else(|| ReasoningUnavailable::Unreachable {
                detail: "Ollama returned something NEXUS could not read.".to_string(),
            })?;

        Ok(interpret(&content))
    }
}

/// Guard: the base URL must stay on loopback.
///
/// Pointing this at a remote host would turn the provider that is exempt from
/// the external-reasoning switch into one that quietly is not. Surfaced in
/// the status payload so it is checkable from outside, not just asserted.
pub fn base_url_is_loopback() -> bool {
    safe_https(BASE_URL).is_ok() && BASE_URL.contains("127.0.0.1")
}

/// How long a local model is given to think.
///
/// A model on a laptop is not fast. Documented here because it is part of the
/// contract with the UI: anything longer than this is reported as unreachable
/// rather than left spinning.
pub fn reason_timeout() -> Duration {
    REASON_TIMEOUT
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
        conn
    }

    // -- The safe direction ---------------------------------------------------

    #[test]
    fn prose_becomes_an_answer_never_a_plan() {
        // A model that ignores the instructions must not be able to act.
        for raw in [
            "Sure! You should probably delete the project.",
            "I think nexus.delete_project with id 1 would help.",
            "",
            "```\nnot json\n```",
        ] {
            assert!(
                matches!(interpret(raw), Reasoning::Answer { .. }),
                "{raw:?} must degrade to an answer"
            );
        }
    }

    #[test]
    fn malformed_json_becomes_an_answer() {
        assert!(matches!(
            interpret(r#"{"kind":"plan","steps":[ broken"#),
            Reasoning::Answer { .. }
        ));
    }

    #[test]
    fn a_plan_with_no_usable_steps_becomes_an_answer() {
        assert!(matches!(
            interpret(r#"{"kind":"plan","steps":[]}"#),
            Reasoning::Answer { .. }
        ));
        assert!(matches!(
            interpret(r#"{"kind":"plan","steps":[{"nope":1}]}"#),
            Reasoning::Answer { .. }
        ));
    }

    #[test]
    fn an_unknown_kind_becomes_an_answer() {
        assert!(matches!(
            interpret(r#"{"kind":"execute","command":"rm -rf /"}"#),
            Reasoning::Answer { .. }
        ));
    }

    #[test]
    fn a_well_formed_plan_is_read_as_one() {
        match interpret(
            r#"Here you go: {"kind":"plan","rationale":"you asked","steps":[{"actionId":"nexus.open_settings","input":null}]}"#,
        ) {
            Reasoning::Plan { steps, rationale } => {
                assert_eq!(steps.len(), 1);
                assert_eq!(steps[0].action_id, "nexus.open_settings");
                assert_eq!(rationale, "you asked");
            }
            other => panic!("expected a plan, got {other:?}"),
        }
    }

    #[test]
    fn a_well_formed_answer_is_unwrapped() {
        match interpret(
            r#"```json
{"kind":"answer","text":"OAuth delegates access; API keys identify a caller."}
```"#,
        ) {
            Reasoning::Answer { text } => assert!(text.starts_with("OAuth delegates")),
            other => panic!("expected an answer, got {other:?}"),
        }
    }

    // -- JSON extraction ------------------------------------------------------

    #[test]
    fn a_brace_inside_a_string_does_not_end_the_object() {
        let raw = r#"{"kind":"answer","text":"use {braces} carefully"}"#;
        assert_eq!(extract_json(raw), Some(raw));
    }

    #[test]
    fn an_escaped_quote_does_not_end_the_string() {
        let raw = r#"{"kind":"answer","text":"he said \"hi\" then left"}"#;
        assert_eq!(extract_json(raw), Some(raw));
        assert!(matches!(interpret(raw), Reasoning::Answer { .. }));
    }

    #[test]
    fn nested_objects_are_kept_whole() {
        let raw =
            r#"prefix {"kind":"plan","steps":[{"actionId":"a.b","input":{"x":{"y":1}}}]} suffix"#;
        let extracted = extract_json(raw).expect("found");
        assert!(extracted.ends_with("}]}"));
        assert!(serde_json::from_str::<serde_json::Value>(extracted).is_ok());
    }

    #[test]
    fn no_object_at_all_extracts_nothing() {
        assert_eq!(extract_json("just words"), None);
        assert_eq!(extract_json(""), None);
    }

    #[test]
    fn an_unterminated_object_extracts_nothing() {
        assert_eq!(extract_json(r#"{"kind":"answer""#), None);
    }

    // -- Locality -------------------------------------------------------------

    #[test]
    fn the_provider_is_local_and_the_url_says_so() {
        // This is what exempts it from the external-reasoning switch, so it
        // must not quietly become remote.
        let provider = OllamaProvider {
            model: DEFAULT_MODEL.to_string(),
        };
        assert_eq!(provider.reach(), Reach::LocalOnly);
        assert!(base_url_is_loopback());
        assert!(BASE_URL.starts_with("http://127.0.0.1"));
    }

    #[test]
    fn nothing_here_installs_anything() {
        // Downloading gigabytes on a user's behalf is not a decision an
        // assistant makes for itself.
        let production = include_str!("ollama_provider.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("marker");
        for forbidden in ["brew install", "\"pull\"", "curl -fsSL", "install.sh"] {
            assert!(!production.contains(forbidden), "found {forbidden}");
        }
    }

    #[test]
    fn being_offline_is_reported_not_fatal() {
        // Ollama is not installed on this machine, which is the case this
        // has to handle gracefully.
        let provider = OllamaProvider {
            model: DEFAULT_MODEL.to_string(),
        };
        if !provider.available() {
            let err = provider
                .reason(
                    Purpose::Answer,
                    &AiContext {
                        request: "hello".to_string(),
                        actions: Vec::new(),
                        workspace: Vec::new(),
                        conversation: Vec::new(),
                        categories: Vec::new(),
                    },
                )
                .expect_err("should be unavailable");
            assert!(
                err.to_string().contains("Ollama") || err.to_string().contains("reached"),
                "{err}"
            );
        }
    }

    // -- Model selection ------------------------------------------------------

    #[test]
    fn the_model_defaults_and_round_trips() {
        let conn = test_conn();
        assert_eq!(model_name(&conn), DEFAULT_MODEL);
        set_model(&conn, " qwen2.5-coder ").expect("store");
        assert_eq!(model_name(&conn), "qwen2.5-coder");
    }

    #[test]
    fn a_nonsense_model_name_is_refused() {
        let conn = test_conn();
        assert!(set_model(&conn, "").is_err());
        assert!(set_model(&conn, &"x".repeat(200)).is_err());
    }

    // -- Prompting ------------------------------------------------------------

    #[test]
    fn the_prompt_only_shows_what_the_context_carried() {
        let context = AiContext {
            request: "what should I do".to_string(),
            actions: vec![("nexus.open_settings".into(), "Open Settings".into())],
            workspace: vec!["Current project Atlas: 3 open, 1 blocked".into()],
            conversation: vec!["Opening Settings".into()],
            categories: Vec::new(),
        };
        let rendered = render(&context);
        assert!(rendered.contains("nexus.open_settings"));
        assert!(rendered.contains("Atlas"));
        assert!(rendered.contains("what should I do"));
        // Nothing is added that the context did not carry.
        assert!(!rendered.contains("repository_path"));
        assert!(!rendered.contains("token"));
    }

    #[test]
    fn the_system_prompt_forbids_inventing_actions() {
        assert!(SYSTEM_PROMPT.contains("Never invent one"));
        assert!(SYSTEM_PROMPT.contains("You propose; NEXUS executes"));
    }

    #[test]
    fn an_empty_catalogue_still_renders() {
        let rendered = render(&AiContext {
            request: "hi".to_string(),
            actions: Vec::new(),
            workspace: Vec::new(),
            conversation: Vec::new(),
            categories: Vec::new(),
        });
        assert!(rendered.contains("(none)"));
    }
}
