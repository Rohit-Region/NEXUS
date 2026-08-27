//! NEXUS-013: assistant state and the conversation.
//!
//! One session, held in memory, bounded, and gone on restart. It is the
//! answer to "what is NEXUS doing right now, and what have we been talking
//! about" and nothing more.
//!
//! **Listening is not stored here.** The microphone already has a single
//! source of truth in `voice::is_listening()`, and a second copy would drift
//! the first time a session ended in an unexpected order. `Session::state`
//! derives it instead.
//!
//! Nothing in this module is persisted. A conversation that survived a
//! restart would be a transcript on disk, which is exactly the thing the
//! voice milestones spent their effort not creating.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::referent::{Referent, ReferentKind, RenderedList, Resolution};

/// Turns kept for context. Enough for a real exchange, small enough that
/// "bounded" means something.
const MAX_TURNS: usize = 24;
/// Referents kept addressable. Older ones fall out of the conversation the
/// way they fall out of a person's working memory.
const MAX_REFERENTS: usize = 60;
const MAX_LISTS: usize = 10;

/// What NEXUS is doing.
///
/// `Listening` is derived, never assigned: see the module note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AssistantState {
    Idle,
    Listening,
    Thinking,
    AwaitingConfirmation,
    Executing,
    Completed,
    Failed,
    Cancelled,
}

impl AssistantState {
    /// Whether NEXUS is mid-turn. Used to decide whether a new input
    /// interrupts something.
    pub fn is_busy(self) -> bool {
        matches!(
            self,
            AssistantState::Thinking
                | AssistantState::AwaitingConfirmation
                | AssistantState::Executing
        )
    }

    /// Whether this is a resting state a turn can finish in.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            AssistantState::Completed | AssistantState::Failed | AssistantState::Cancelled
        )
    }
}

/// How a turn started.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "camelCase")]
pub enum TurnInput {
    #[serde(rename_all = "camelCase")]
    Voice { text: String },
    #[serde(rename_all = "camelCase")]
    Text { text: String },
    /// A button, a palette entry, an accepted suggestion.
    #[serde(rename_all = "camelCase")]
    Ui { action_id: String },
}

/// One exchange.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    pub id: u64,
    pub input: TurnInput,
    pub state: AssistantState,
    /// What NEXUS did or said, in one line. The same sentence the audit
    /// trail records for an action.
    pub summary: Option<String>,
    /// Set when the turn ended in `Failed`.
    pub error: Option<String>,
}

/// The whole conversation, as the UI reads it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshot {
    pub state: AssistantState,
    pub turns: Vec<Turn>,
    pub referents: Vec<Referent>,
    pub lists: Vec<RenderedList>,
    /// Approvals still waiting. Read from the approval store, not stored here.
    pub pending_approvals: usize,
}

/// The live session. Tauri managed state.
pub struct AssistantSession {
    inner: Mutex<Inner>,
    next_turn: AtomicU64,
    next_referent: AtomicU64,
    next_list: AtomicU64,
}

#[derive(Default)]
struct Inner {
    state: AssistantState,
    turns: VecDeque<Turn>,
    referents: VecDeque<Referent>,
    lists: VecDeque<RenderedList>,
    current_turn: Option<u64>,
}

impl Default for AssistantState {
    fn default() -> Self {
        AssistantState::Idle
    }
}

impl Default for AssistantSession {
    fn default() -> Self {
        AssistantSession {
            inner: Mutex::new(Inner::default()),
            // All three start at 1, so 0 is never a valid id and an
            // uninitialised field is obvious rather than accidentally valid.
            next_turn: AtomicU64::new(1),
            next_referent: AtomicU64::new(1),
            next_list: AtomicU64::new(1),
        }
    }
}

impl AssistantSession {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// What NEXUS is doing, with listening derived from the microphone.
    ///
    /// An explicit state always wins: if a turn is executing, NEXUS is
    /// executing even if the mic somehow reopened. Only rest defers to voice.
    pub fn state(&self) -> AssistantState {
        let state = self.lock().state;
        match state {
            AssistantState::Idle if super::super::voice::is_listening() => {
                AssistantState::Listening
            }
            other => other,
        }
    }

    /// Begin a turn. Returns its id.
    pub fn begin_turn(&self, input: TurnInput) -> u64 {
        let id = self.next_turn.fetch_add(1, Ordering::SeqCst);
        let mut inner = self.lock();
        inner.turns.push_back(Turn {
            id,
            input,
            state: AssistantState::Thinking,
            summary: None,
            error: None,
        });
        while inner.turns.len() > MAX_TURNS {
            inner.turns.pop_front();
        }
        inner.state = AssistantState::Thinking;
        inner.current_turn = Some(id);
        id
    }

    /// Move the open turn along. Terminal states close it.
    pub fn advance(&self, state: AssistantState, summary: Option<String>, error: Option<String>) {
        let mut inner = self.lock();
        inner.state = state;
        let current = inner.current_turn;
        if let Some(turn) = current.and_then(|id| inner.turns.iter_mut().find(|t| t.id == id)) {
            turn.state = state;
            if summary.is_some() {
                turn.summary = summary;
            }
            if error.is_some() {
                turn.error = error;
            }
        }
        if state.is_terminal() {
            inner.current_turn = None;
        }
    }

    /// Return to rest. Called once the UI has shown a terminal state, so a
    /// finished turn does not leave NEXUS looking permanently "completed".
    pub fn settle(&self) {
        let mut inner = self.lock();
        if inner.state.is_terminal() {
            inner.state = AssistantState::Idle;
        }
    }

    /// The turn a caller should attribute new referents to.
    pub fn current_turn(&self) -> u64 {
        let inner = self.lock();
        inner
            .current_turn
            .or_else(|| inner.turns.back().map(|t| t.id))
            .unwrap_or(0)
    }

    /// Register something NEXUS just mentioned. Returns its referent id.
    ///
    /// Only callers speaking on NEXUS's behalf should use this: a referent
    /// the user introduced is not one NEXUS can act on.
    pub fn remember(
        &self,
        kind: ReferentKind,
        display_name: &str,
        source: &str,
        metadata: serde_json::Value,
    ) -> u64 {
        let turn = self.current_turn();
        let id = self.next_referent.fetch_add(1, Ordering::SeqCst);
        let mut inner = self.lock();
        inner.referents.push_back(Referent {
            id,
            kind,
            display_name: display_name.to_string(),
            source: source.to_string(),
            metadata,
            turn,
        });
        while inner.referents.len() > MAX_REFERENTS {
            inner.referents.pop_front();
        }
        id
    }

    /// Record a list NEXUS rendered, in the order the user saw it.
    ///
    /// The only thing an ordinal may index into. An empty list is not
    /// recorded: counting through nothing is not a thing the user can mean.
    pub fn remember_list(&self, items: Vec<u64>) -> Option<u64> {
        if items.is_empty() {
            return None;
        }
        let turn = self.current_turn();
        let id = self.next_list.fetch_add(1, Ordering::SeqCst);
        let mut inner = self.lock();
        inner.lists.push_back(RenderedList { id, turn, items });
        while inner.lists.len() > MAX_LISTS {
            inner.lists.pop_front();
        }
        Some(id)
    }

    /// Resolve a phrase against the conversation. Deterministic; no provider.
    pub fn resolve(&self, phrase: &str) -> Resolution {
        let inner = self.lock();
        let referents: Vec<Referent> = inner.referents.iter().cloned().collect();
        let lists: Vec<RenderedList> = inner.lists.iter().cloned().collect();
        drop(inner);
        super::referent::resolve(phrase, &referents, &lists)
    }

    pub fn snapshot(&self, pending_approvals: usize) -> SessionSnapshot {
        let state = self.state();
        let inner = self.lock();
        SessionSnapshot {
            state,
            turns: inner.turns.iter().cloned().collect(),
            referents: inner.referents.iter().cloned().collect(),
            lists: inner.lists.iter().cloned().collect(),
            pending_approvals,
        }
    }

    /// Abandon the open turn. Cancelling is a normal outcome, not an error.
    pub fn cancel(&self) {
        self.advance(AssistantState::Cancelled, None, None);
    }

    /// Forget everything. The user's "start again".
    pub fn clear(&self) {
        let mut inner = self.lock();
        inner.turns.clear();
        inner.referents.clear();
        inner.lists.clear();
        inner.current_turn = None;
        inner.state = AssistantState::Idle;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text(input: &str) -> TurnInput {
        TurnInput::Text {
            text: input.to_string(),
        }
    }

    #[test]
    fn a_new_session_is_idle_and_empty() {
        let session = AssistantSession::default();
        assert_eq!(session.state(), AssistantState::Idle);
        let snapshot = session.snapshot(0);
        assert!(snapshot.turns.is_empty());
        assert!(snapshot.referents.is_empty());
    }

    #[test]
    fn a_turn_walks_from_thinking_to_completed() {
        let session = AssistantSession::default();
        session.begin_turn(text("open settings"));
        assert_eq!(session.state(), AssistantState::Thinking);

        session.advance(AssistantState::Executing, None, None);
        assert_eq!(session.state(), AssistantState::Executing);

        session.advance(
            AssistantState::Completed,
            Some("Opening Settings".to_string()),
            None,
        );
        assert_eq!(session.state(), AssistantState::Completed);

        let snapshot = session.snapshot(0);
        assert_eq!(snapshot.turns.len(), 1);
        assert_eq!(snapshot.turns[0].summary.as_deref(), Some("Opening Settings"));
    }

    #[test]
    fn a_terminal_state_closes_the_turn() {
        let session = AssistantSession::default();
        let id = session.begin_turn(text("do a thing"));
        assert_eq!(session.current_turn(), id);
        session.advance(AssistantState::Completed, None, None);

        // Further referents attach to the closed turn rather than a phantom
        // open one, which keeps recency ordering meaningful.
        session.remember(ReferentKind::Task, "Ship it", "nexus", json!({}));
        assert_eq!(session.snapshot(0).referents[0].turn, id);
    }

    #[test]
    fn settle_returns_to_idle_only_from_a_terminal_state() {
        let session = AssistantSession::default();
        session.begin_turn(text("x"));
        session.settle();
        assert_eq!(
            session.state(),
            AssistantState::Thinking,
            "settling must not abandon a turn in flight"
        );

        session.advance(AssistantState::Completed, None, None);
        session.settle();
        assert_eq!(session.state(), AssistantState::Idle);
    }

    #[test]
    fn cancelling_ends_the_turn_without_an_error() {
        let session = AssistantSession::default();
        session.begin_turn(text("delete everything"));
        session.cancel();
        assert_eq!(session.state(), AssistantState::Cancelled);
        assert!(session.snapshot(0).turns[0].error.is_none());
    }

    #[test]
    fn a_failure_records_its_reason() {
        let session = AssistantSession::default();
        session.begin_turn(text("open the thing"));
        session.advance(
            AssistantState::Failed,
            None,
            Some("not permitted".to_string()),
        );
        assert_eq!(
            session.snapshot(0).turns[0].error.as_deref(),
            Some("not permitted")
        );
    }

    #[test]
    fn busy_and_terminal_partition_the_states() {
        for state in [
            AssistantState::Thinking,
            AssistantState::AwaitingConfirmation,
            AssistantState::Executing,
        ] {
            assert!(state.is_busy(), "{state:?}");
            assert!(!state.is_terminal(), "{state:?}");
        }
        for state in [
            AssistantState::Completed,
            AssistantState::Failed,
            AssistantState::Cancelled,
        ] {
            assert!(state.is_terminal(), "{state:?}");
            assert!(!state.is_busy(), "{state:?}");
        }
        assert!(!AssistantState::Idle.is_busy());
        assert!(!AssistantState::Idle.is_terminal());
    }

    // -- Bounds ---------------------------------------------------------------

    #[test]
    fn turns_referents_and_lists_are_all_bounded() {
        let session = AssistantSession::default();
        for i in 0..(MAX_TURNS * 2) {
            session.begin_turn(text(&format!("turn {i}")));
            session.advance(AssistantState::Completed, None, None);
        }
        for i in 0..(MAX_REFERENTS * 2) {
            session.remember(ReferentKind::Task, &format!("task {i}"), "nexus", json!({}));
        }
        for i in 0..(MAX_LISTS * 2) {
            session.remember_list(vec![i as u64 + 1]);
        }
        let snapshot = session.snapshot(0);
        assert_eq!(snapshot.turns.len(), MAX_TURNS);
        assert_eq!(snapshot.referents.len(), MAX_REFERENTS);
        assert_eq!(snapshot.lists.len(), MAX_LISTS);
    }

    #[test]
    fn the_oldest_falls_out_first() {
        let session = AssistantSession::default();
        for i in 0..(MAX_TURNS + 3) {
            session.begin_turn(text(&format!("turn {i}")));
            session.advance(AssistantState::Completed, None, None);
        }
        let snapshot = session.snapshot(0);
        assert_eq!(
            snapshot.turns.last().expect("some turns").input,
            text(&format!("turn {}", MAX_TURNS + 2))
        );
    }

    #[test]
    fn ids_are_never_reused() {
        let session = AssistantSession::default();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            assert!(seen.insert(session.remember(
                ReferentKind::Task,
                "t",
                "nexus",
                json!({})
            )));
        }
    }

    #[test]
    fn zero_is_never_a_valid_id() {
        let session = AssistantSession::default();
        assert_ne!(session.begin_turn(text("x")), 0);
        assert_ne!(session.remember(ReferentKind::Task, "t", "nexus", json!({})), 0);
        assert_ne!(session.remember_list(vec![1]), Some(0));
    }

    #[test]
    fn an_empty_list_is_not_recorded() {
        // Counting through nothing is not something the user can mean.
        let session = AssistantSession::default();
        assert_eq!(session.remember_list(vec![]), None);
        assert!(session.snapshot(0).lists.is_empty());
    }

    // -- Resolution through the session ---------------------------------------

    #[test]
    fn the_session_resolves_against_what_it_remembered() {
        let session = AssistantSession::default();
        session.begin_turn(text("what needs my attention"));
        let pr = session.remember(
            ReferentKind::PullRequest,
            "PR #8792",
            "github",
            json!({ "number": 8792 }),
        );
        let issue = session.remember(ReferentKind::JiraIssue, "KAI-515", "jira", json!({}));
        session.remember_list(vec![pr, issue]);
        session.advance(AssistantState::Completed, None, None);

        match session.resolve("open the PR") {
            Resolution::Resolved { referent } => {
                assert_eq!(referent.display_name, "PR #8792");
                assert_eq!(referent.metadata["number"], 8792);
            }
            other => panic!("expected the PR, got {other:?}"),
        }
        match session.resolve("do the second one") {
            Resolution::Resolved { referent } => assert_eq!(referent.display_name, "KAI-515"),
            other => panic!("expected the second item, got {other:?}"),
        }
    }

    #[test]
    fn clearing_forgets_the_conversation() {
        let session = AssistantSession::default();
        session.begin_turn(text("x"));
        session.remember(ReferentKind::Task, "Ship it", "nexus", json!({}));
        session.remember_list(vec![1]);

        session.clear();

        let snapshot = session.snapshot(0);
        assert!(snapshot.turns.is_empty());
        assert!(snapshot.referents.is_empty());
        assert!(snapshot.lists.is_empty());
        assert_eq!(snapshot.state, AssistantState::Idle);
        assert!(matches!(
            session.resolve("open the task"),
            Resolution::Unresolved { .. }
        ));
    }

    // -- The no-duplication rule ---------------------------------------------

    #[test]
    fn listening_is_never_stored_only_derived() {
        // The microphone has one source of truth. A second copy would drift
        // the first time a voice session ended in an unexpected order.
        let production = include_str!("session.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("the file must keep its test module marker");

        assert!(
            !production.contains("state = AssistantState::Listening"),
            "Listening must never be assigned"
        );
        assert!(
            production.contains("voice::is_listening()"),
            "Listening must be derived from the microphone's own flag"
        );
    }

    #[test]
    fn nothing_here_reaches_the_database() {
        let production = include_str!("session.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("the file must keep its test module marker");
        for forbidden in ["rusqlite", "Connection", "std::fs", "File::"] {
            assert!(
                !production.contains(forbidden),
                "the conversation must never be persisted, found {forbidden}"
            );
        }
    }

    #[test]
    fn a_snapshot_serialises_as_camel_case() {
        let session = AssistantSession::default();
        session.begin_turn(TurnInput::Voice {
            text: "open settings".to_string(),
        });
        let json = serde_json::to_string(&session.snapshot(2)).expect("serialize");
        assert!(json.contains("\"pendingApprovals\":2"), "{json}");
        assert!(json.contains("\"source\":\"voice\""), "{json}");
        assert!(json.contains("\"state\":\"thinking\""), "{json}");
    }
}
