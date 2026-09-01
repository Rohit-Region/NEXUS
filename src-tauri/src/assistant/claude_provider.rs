//! Claude Code as a reasoning provider.
//!
//! The escalation rung has never actually worked on this machine: Ollama is
//! not installed, so every phrase the deterministic ladder declined ended in
//! "ollama is not responding". Claude Code is already here, already signed
//! in, and answers on stdout.
//!
//! Why a subprocess rather than the API:
//!
//! - **No key to store.** It uses the CLI's existing session, so NEXUS never
//!   holds a credential and there is nothing to leak from the database.
//! - **No new dependency.** A fixed argument vector through the same shell
//!   helper every connector uses. No HTTP client, no SDK.
//! - **The prompt goes in on stdin**, so it never appears in `ps` output.
//!   Context can carry project and task names; those are not for the process
//!   list.
//!
//! **This provider leaves the machine.** It is `Reach::LeavesMachine` and is
//! therefore gated by the external-reasoning switch exactly like the cloud
//! providers, which is the honest classification: the prompt reaches
//! Anthropic. Ollama stays first in the order, because a local model is the
//! one that raises no privacy question at all.

use std::time::Duration;

use super::permission::Reach;
use super::reasoning::{
    AiContext, Purpose, Reasoning, ReasoningProvider, ReasoningUnavailable,
};
use super::shell::run_with_stdin;

pub const PROVIDER_ID: &str = "claude-code";

/// Where the CLI usually lives. Checked in order; the first that exists wins.
///
/// `PATH` is deliberately not consulted: this runs a fixed program, and a
/// `PATH` lookup would let whatever is earliest on it decide what NEXUS
/// executes.
const CANDIDATE_PATHS: [&str; 4] = [
    "/opt/homebrew/bin/claude",
    "/usr/local/bin/claude",
    "/usr/bin/claude",
    "/opt/homebrew/opt/claude/bin/claude",
];

/// Long enough for a considered answer, short enough that the panel does not
/// look frozen. The deterministic tiers have already declined by this point,
/// so the user is waiting on this and nothing else.
const REASON_TIMEOUT: Duration = Duration::from_secs(60);

/// The binary, if it is installed.
fn binary() -> Option<&'static str> {
    CANDIDATE_PATHS
        .into_iter()
        .find(|path| std::path::Path::new(path).exists())
}

pub struct ClaudeProvider;

impl ReasoningProvider for ClaudeProvider {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn model(&self) -> String {
        // The CLI picks the model from the user's own configuration. Naming
        // one here would be a claim NEXUS cannot keep.
        "claude-code".to_string()
    }

    fn reach(&self) -> Reach {
        // The prompt reaches Anthropic. Saying otherwise would exempt it
        // from the switch that exists precisely for this.
        Reach::LeavesMachine
    }

    fn available(&self) -> bool {
        binary().is_some()
    }

    fn reason(
        &self,
        purpose: Purpose,
        context: &AiContext,
    ) -> Result<Reasoning, ReasoningUnavailable> {
        let program = binary().ok_or_else(|| ReasoningUnavailable::Unreachable {
            detail: "Claude Code is not installed. Install it, or start Ollama, \
                     or NEXUS will keep to what it can work out locally."
                .to_string(),
        })?;

        // The same instruction the local model gets, so the two providers
        // are held to one contract and `interpret` can parse either.
        let prompt = format!(
            "{}\n\nPurpose: {}\n\n{}",
            super::ollama_provider::SYSTEM_PROMPT,
            purpose.as_str(),
            super::ollama_provider::render(context)
        );

        // `-p` prints one reply and exits. No session is kept, so nothing
        // NEXUS asks accumulates into a conversation the user cannot see.
        let out = run_with_stdin(
            program,
            &["-p", "--output-format", "text"],
            &prompt,
            REASON_TIMEOUT,
        )
        .map_err(|e| ReasoningUnavailable::Unreachable {
            detail: format!("Claude Code did not answer: {e}"),
        })?;

        if !out.success {
            let detail = out.stderr.trim();
            return Err(ReasoningUnavailable::Unreachable {
                detail: if detail.is_empty() {
                    "Claude Code exited without answering.".to_string()
                } else if detail.contains("login") || detail.contains("auth") {
                    "Claude Code is not signed in. Run `claude` in a terminal once."
                        .to_string()
                } else {
                    format!("Claude Code: {detail}")
                },
            });
        }

        // Same rule as the local provider, and the safe direction: anything
        // that cannot be read as a plan becomes an answer. A model can never
        // accidentally produce steps.
        Ok(super::ollama_provider::interpret(&out.stdout))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn production() -> &'static str {
        include_str!("claude_provider.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("marker")
    }

    #[test]
    fn it_is_honest_about_leaving_the_machine() {
        // Classifying this as local would exempt it from the external
        // reasoning switch, which is the one control standing between a
        // user's project names and a network request.
        assert_eq!(ClaudeProvider.reach(), Reach::LeavesMachine);
    }

    #[test]
    fn the_prompt_never_reaches_the_process_list() {
        // Context carries project and task names. Passed as an argument they
        // would be readable by every process on the machine.
        assert!(production().contains("run_with_stdin"));
        assert!(
            !production().contains("&[\"-p\", &prompt"),
            "the prompt must not be an argument"
        );
    }

    #[test]
    fn the_binary_is_never_resolved_through_path() {
        // A PATH lookup lets whatever is earliest on it decide what NEXUS
        // executes. Every candidate here is an absolute path.
        assert!(CANDIDATE_PATHS.iter().all(|p| p.starts_with('/')));
        // Matched as a program being run, not as the English word, which
        // this comment would otherwise trip over.
        assert!(!production().contains("\"which\""));
        assert!(!production().contains("/usr/bin/env"));
    }

    #[test]
    fn no_session_is_kept_between_questions() {
        // `--continue` or `--resume` would accumulate NEXUS's questions into
        // a conversation the user never opened and cannot see.
        for forbidden in ["--continue", "--resume", "-c\""] {
            assert!(!production().contains(forbidden), "found {forbidden}");
        }
    }

    #[test]
    fn being_absent_is_reported_not_panicked() {
        // The whole point of a provider list is that a missing one degrades.
        if binary().is_none() {
            assert!(!ClaudeProvider.available());
        } else {
            assert!(ClaudeProvider.available());
        }
    }
}
