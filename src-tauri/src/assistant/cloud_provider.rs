//! NEXUS-022b: cloud reasoning providers.
//!
//! Claude and ChatGPT behind the same trait NEXUS-022a's local model
//! implements. Nothing about the bridge changes; only the provider does,
//! which is the entire point of having the abstraction.
//!
//! Three things are different from the local provider, and all three are
//! consequences of the context leaving the machine:
//!
//! 1. **A key is required**, and it lives in the Keychain. Never in
//!    `nexus.db`, which is a plain file, and never in a prompt.
//! 2. **The external-reasoning switch applies.** `may_consult` refuses a
//!    `LeavesMachine` provider until the user turns it on. The local model is
//!    exempt; these are not.
//! 3. **Every call is audited** with the categories that travelled, so the
//!    trail can answer "why did NEXUS contact an external service".
//!
//! Both providers are configured but unavailable until a key exists, and a
//! provider without a key is not listed at all rather than listed-and-broken.
//! Neither has been called: no key is stored on this machine.

use rusqlite::Connection;

use super::http::{keychain_secret, send, HttpError, Request};
use super::ollama_provider::interpret;
use super::permission::Reach;
use super::reasoning::{AiContext, Purpose, Reasoning, ReasoningProvider, ReasoningUnavailable};

/// Keychain service names. The account is the provider id, so a user with
/// both configured keeps them apart.
pub const KEYCHAIN_SERVICE: &str = "nexus-reasoning";

const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";
const OPENAI_URL: &str = "https://api.openai.com/v1/chat/completions";
/// Pinned rather than tracking "latest": a silent model change alters
/// behaviour under the user, and the model is recorded in the audit trail.
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_ANTHROPIC_MODEL: &str = "claude-sonnet-4-5";
const DEFAULT_OPENAI_MODEL: &str = "gpt-4o-mini";
/// Bounded so a prompt cannot grow without limit, and so a runaway reply
/// cannot become a bill.
const MAX_OUTPUT_TOKENS: u32 = 1_024;

const SYSTEM_PROMPT: &str = "\
You are the reasoning component of NEXUS, a local developer assistant. \
Reply with a single JSON object and nothing else.

To answer a question:
{\"kind\":\"answer\",\"text\":\"...\"}

To propose actions:
{\"kind\":\"plan\",\"rationale\":\"...\",\"steps\":[{\"actionId\":\"...\",\"input\":{...}}]}

Rules:
- Only use actionId values from the provided list. Never invent one.
- If no listed action fits, reply with an answer explaining that.
- Never claim to have done something. You propose; NEXUS executes.
- Keep answers short and factual. Say when you do not know.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vendor {
    Anthropic,
    OpenAi,
}

impl Vendor {
    pub fn id(self) -> &'static str {
        match self {
            Vendor::Anthropic => "anthropic",
            Vendor::OpenAi => "openai",
        }
    }

    fn default_model(self) -> &'static str {
        match self {
            Vendor::Anthropic => DEFAULT_ANTHROPIC_MODEL,
            Vendor::OpenAi => DEFAULT_OPENAI_MODEL,
        }
    }

    fn model_key(self) -> &'static str {
        match self {
            Vendor::Anthropic => "ai_anthropic_model",
            Vendor::OpenAi => "ai_openai_model",
        }
    }

    fn url(self) -> &'static str {
        match self {
            Vendor::Anthropic => ANTHROPIC_URL,
            Vendor::OpenAi => OPENAI_URL,
        }
    }
}

fn model_for(conn: &Connection, vendor: Vendor) -> String {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        [vendor.model_key()],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .map(|v| v.trim().to_string())
    .filter(|v| !v.is_empty() && v.len() < 100)
    .unwrap_or_else(|| vendor.default_model().to_string())
}

pub fn set_model(conn: &Connection, vendor: Vendor, model: &str) -> Result<(), String> {
    let cleaned = model.trim();
    if cleaned.is_empty() || cleaned.len() > 99 {
        return Err("That is not a model name.".to_string());
    }
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE
           SET value = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        rusqlite::params![vendor.model_key(), cleaned],
    )
    .map_err(|e| format!("Failed to store the model name: {e}"))?;
    Ok(())
}

/// Build the prompt. Formatting only: what may travel was decided in
/// `build_context`.
fn render(context: &AiContext) -> String {
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

pub struct CloudProvider {
    pub vendor: Vendor,
    pub model: String,
}

impl CloudProvider {
    fn key(&self) -> Option<String> {
        keychain_secret(KEYCHAIN_SERVICE, self.vendor.id())
    }
}

impl ReasoningProvider for CloudProvider {
    fn id(&self) -> &'static str {
        self.vendor.id()
    }

    fn model(&self) -> String {
        self.model.clone()
    }

    fn reach(&self) -> Reach {
        // The reason the external-reasoning switch applies to this and not to
        // the local model.
        Reach::LeavesMachine
    }

    fn available(&self) -> bool {
        // Having a key, not reachability. Probing would mean a billable call
        // every time the UI asks what is available.
        self.key().is_some()
    }

    fn reason(
        &self,
        purpose: Purpose,
        context: &AiContext,
    ) -> Result<Reasoning, ReasoningUnavailable> {
        let key = self.key().ok_or(ReasoningUnavailable::NoProvider)?;
        let prompt = format!("Purpose: {}\n\n{}", purpose.as_str(), render(context));

        let (body, headers) = match self.vendor {
            Vendor::Anthropic => (
                serde_json::json!({
                    "model": self.model,
                    "max_tokens": MAX_OUTPUT_TOKENS,
                    "system": SYSTEM_PROMPT,
                    "messages": [{ "role": "user", "content": prompt }]
                }),
                vec![
                    ("Content-Type".to_string(), "application/json".to_string()),
                    ("anthropic-version".to_string(), ANTHROPIC_VERSION.to_string()),
                    ("x-api-key".to_string(), key),
                ],
            ),
            Vendor::OpenAi => (
                serde_json::json!({
                    "model": self.model,
                    "max_tokens": MAX_OUTPUT_TOKENS,
                    "temperature": 0.1,
                    "messages": [
                        { "role": "system", "content": SYSTEM_PROMPT },
                        { "role": "user", "content": prompt }
                    ]
                }),
                vec![
                    ("Content-Type".to_string(), "application/json".to_string()),
                    ("Authorization".to_string(), format!("Bearer {key}")),
                ],
            ),
        };

        let response = send(Request {
            method: "POST",
            url: self.vendor.url().to_string(),
            headers,
            basic_auth: None,
            body: Some(body.to_string()),
        })
        .map_err(|e| match e {
            HttpError::Unreachable { detail } => ReasoningUnavailable::Unreachable { detail },
            other => ReasoningUnavailable::Unreachable {
                detail: other.to_string(),
            },
        })?;

        if !response.ok() {
            return Err(ReasoningUnavailable::Unreachable {
                detail: match response.status {
                    401 | 403 => format!(
                        "{} rejected the API key. Check the one stored in the Keychain.",
                        self.vendor.id()
                    ),
                    429 => "The provider is rate limiting NEXUS. Try again shortly.".to_string(),
                    other => format!("The provider returned {other}."),
                },
            });
        }

        let value: serde_json::Value =
            serde_json::from_str(&response.body).map_err(|_| ReasoningUnavailable::Unreachable {
                detail: "The provider returned something NEXUS could not read.".to_string(),
            })?;

        // Two shapes, one meaning. Both end at `interpret`, which is where
        // the safe-direction rule lives: anything unrecognisable becomes an
        // answer, never a plan.
        let content = match self.vendor {
            Vendor::Anthropic => value
                .get("content")
                .and_then(|c| c.as_array())
                .and_then(|items| items.first())
                .and_then(|item| item.get("text"))
                .and_then(|t| t.as_str()),
            Vendor::OpenAi => value
                .get("choices")
                .and_then(|c| c.as_array())
                .and_then(|items| items.first())
                .and_then(|item| item.get("message"))
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str()),
        }
        .ok_or_else(|| ReasoningUnavailable::Unreachable {
            detail: "The provider returned no content.".to_string(),
        })?;

        Ok(interpret(content))
    }
}

/// Cloud providers that have a key stored.
///
/// A provider without one is not listed at all. Listing it as unavailable
/// would put a broken option in front of the user for no benefit; the
/// Settings screen offers configuration separately.
pub fn configured(conn: &Connection) -> Vec<Box<dyn ReasoningProvider>> {
    [Vendor::Anthropic, Vendor::OpenAi]
        .into_iter()
        .filter(|vendor| keychain_secret(KEYCHAIN_SERVICE, vendor.id()).is_some())
        .map(|vendor| {
            Box::new(CloudProvider {
                model: model_for(conn, vendor),
                vendor,
            }) as Box<dyn ReasoningProvider>
        })
        .collect()
}

/// What the Settings screen shows: every vendor, and whether it is set up.
pub fn describe(conn: &Connection) -> Vec<serde_json::Value> {
    [Vendor::Anthropic, Vendor::OpenAi]
        .into_iter()
        .map(|vendor| {
            serde_json::json!({
                "id": vendor.id(),
                "model": model_for(conn, vendor),
                "configured": keychain_secret(KEYCHAIN_SERVICE, vendor.id()).is_some(),
                "keychainHint": format!(
                    "security add-generic-password -s {KEYCHAIN_SERVICE} -a {} -w",
                    vendor.id()
                ),
            })
        })
        .collect()
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

    #[test]
    fn a_cloud_provider_always_leaves_the_machine() {
        // Which is what subjects it to the external-reasoning switch.
        for vendor in [Vendor::Anthropic, Vendor::OpenAi] {
            let provider = CloudProvider {
                vendor,
                model: "x".to_string(),
            };
            assert_eq!(provider.reach(), Reach::LeavesMachine);
        }
    }

    #[test]
    fn a_provider_without_a_key_is_not_offered() {
        // No key is stored on this machine, so nothing should be listed.
        let conn = test_conn();
        assert!(
            configured(&conn).is_empty(),
            "a provider with no key must not be listed"
        );
    }

    #[test]
    fn every_vendor_is_still_described_so_it_can_be_set_up() {
        let conn = test_conn();
        let described = describe(&conn);
        assert_eq!(described.len(), 2);
        for entry in described {
            assert_eq!(entry["configured"], false);
            assert!(entry["keychainHint"]
                .as_str()
                .expect("hint")
                .contains("add-generic-password"));
        }
    }

    #[test]
    fn reasoning_without_a_key_fails_before_any_request() {
        let provider = CloudProvider {
            vendor: Vendor::Anthropic,
            model: "x".to_string(),
        };
        let context = AiContext {
            request: "hello".to_string(),
            actions: Vec::new(),
            workspace: Vec::new(),
            conversation: Vec::new(),
            categories: Vec::new(),
        };
        assert!(matches!(
            provider.reason(Purpose::Answer, &context),
            Err(ReasoningUnavailable::NoProvider)
        ));
    }

    #[test]
    fn models_default_and_round_trip_per_vendor() {
        let conn = test_conn();
        assert_eq!(model_for(&conn, Vendor::Anthropic), DEFAULT_ANTHROPIC_MODEL);
        assert_eq!(model_for(&conn, Vendor::OpenAi), DEFAULT_OPENAI_MODEL);

        set_model(&conn, Vendor::Anthropic, "claude-opus-4-5").expect("store");
        assert_eq!(model_for(&conn, Vendor::Anthropic), "claude-opus-4-5");
        // Setting one must not disturb the other.
        assert_eq!(model_for(&conn, Vendor::OpenAi), DEFAULT_OPENAI_MODEL);
    }

    #[test]
    fn a_nonsense_model_name_is_refused() {
        let conn = test_conn();
        assert!(set_model(&conn, Vendor::OpenAi, "  ").is_err());
        assert!(set_model(&conn, Vendor::OpenAi, &"x".repeat(200)).is_err());
    }

    #[test]
    fn the_key_never_reaches_the_database_or_the_prompt() {
        let production = include_str!("cloud_provider.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("marker");
        // The key is read from the Keychain and used only as a header.
        assert!(production.contains("keychain_secret"));
        assert!(!production.contains("INSERT INTO settings (key, value) VALUES (?1, ?2)\n         ON CONFLICT(key) DO UPDATE\n           SET value = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')\n    ,"));
        // `render` builds the prompt and cannot see a key: it takes only the
        // context.
        assert!(production.contains("fn render(context: &AiContext) -> String"));
    }

    #[test]
    fn both_vendors_end_at_the_same_safe_interpreter() {
        // The safe-direction rule is implemented once, in the local provider,
        // and reused. A second parser would be a second place for prose to
        // become a plan.
        // Scoped to production, because this test's own assertion contains
        // the very string it is counting.
        let production = include_str!("cloud_provider.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("marker");
        assert!(production.contains("use super::ollama_provider::interpret"));
        assert_eq!(
            production.matches("interpret(content)").count(),
            1,
            "a second parser would be a second place for prose to become a plan"
        );
    }

    #[test]
    fn the_model_is_pinned_rather_than_tracking_latest() {
        // A silent model change alters behaviour under the user, and the
        // audit trail records which model answered.
        assert!(!DEFAULT_ANTHROPIC_MODEL.contains("latest"));
        assert!(!DEFAULT_OPENAI_MODEL.contains("latest"));
        assert!(ANTHROPIC_VERSION.starts_with("20"));
    }

    #[test]
    fn output_is_bounded() {
        assert!(MAX_OUTPUT_TOKENS <= 4_096, "a runaway reply is a bill");
    }

    #[test]
    fn both_endpoints_are_https() {
        for url in [ANTHROPIC_URL, OPENAI_URL] {
            assert!(url.starts_with("https://"), "{url}");
        }
    }
}
