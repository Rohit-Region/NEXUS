//! NEXUS-012: the connector interface.
//!
//! NEXUS decides *what* should happen. A connector decides *how* that
//! particular application performs it. The Assistant Core never names a
//! specific application, and a connector never decides whether it is allowed
//! to act; that belongs to the gate.
//!
//! `capabilities()` is runtime rather than compile-time, and that is the
//! lesson NEXUS-011 taught: the shipped default voice was one the speech API
//! did not actually expose on this machine, even though another macOS
//! subsystem listed it. Connectors have the same problem in a larger form. An
//! IDE may not be installed; a CLI may not be authenticated. A connector
//! reports what it can do *here*, and the UI offers only that.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::action::{ActionError, ActionSpec};
use super::referent::ReferentKind;

/// Whether a connector can be used right now, and if not, why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectorStatus {
    /// Present and usable.
    Ready,
    /// Nothing has been set up yet. Distinct from NeedsAuth: there is no
    /// account to authenticate, so the remedy is configuration, not sign-in.
    Unconfigured,
    /// Usable, but some capabilities are missing on this machine.
    Degraded,
    /// Needs credentials or authorisation before anything will work.
    NeedsAuth,
    /// Turned off by the user.
    Disabled,
    /// The underlying application or tool is not present.
    Unavailable,
}

/// What a connector can actually do on this machine, right now.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    /// Action ids that will work. A subset of `actions()`.
    pub available: Vec<String>,
    /// Action ids that exist but cannot run here, with the reason, so the UI
    /// can explain rather than simply fail.
    pub unavailable: Vec<UnavailableAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnavailableAction {
    pub action_id: String,
    pub reason: String,
}

/// Everything a connector needs while performing one action.
///
/// Holds the database connection rather than the Tauri state wrapper, so a
/// connector cannot take the lock itself, and cannot hold it across anything
/// it should not. Deliberately a struct rather than a bare `&Connection`:
/// later connectors will need more here, and widening a struct does not
/// touch every call site.
///
/// It carries no approval flag. Whether the user approved is the gate's
/// business, settled before dispatch; a connector that could see it might be
/// tempted to make its own decision.
pub struct ExecCtx<'a> {
    pub conn: &'a Connection,
}

/// An action a bare "yes" may run next, and what to run it with.
///
/// It carries an input because the next step is not always argument-free.
/// Pressing send in WhatsApp needs nothing; submitting a prompt needs to
/// know which editor, and a follow-up that had to guess would be picking an
/// application to type into, which is the wrong kind of guess.
///
/// The input is built by the connector from the action that just ran, never
/// by whoever says "yes". A bare affirmation supplies no arguments and
/// cannot redirect this anywhere.
#[derive(Debug, Clone, PartialEq)]
pub struct FollowUp {
    pub action_id: &'static str,
    pub input: serde_json::Value,
}

/// A way out of a failure, offered by the connector that hit it.
///
/// NEXUS-026. Connectors already write good failure text: the Accessibility
/// sentence names the exact settings pane to open. What was missing is that
/// the sentence is a dead end, read out to somebody whose hands are busy.
/// A remedy turns it into a question with an answer.
///
/// It is deliberately the same shape as [`FollowUp`], and reaches the user
/// through the same mechanism, because "shall I open that for you?" is the
/// same kind of offer as "shall I send it?". Nothing new is invented, and
/// the remedy still passes the gate like anything else: the user is offered
/// a fix, never given one.
#[derive(Debug, Clone, PartialEq)]
pub struct Remedy {
    /// Spoken aloud, so it ends in a question a person answers with "yes".
    pub prompt: String,
    pub action_id: &'static str,
    pub input: serde_json::Value,
}

/// Something worth remembering, as the connector describes it.
///
/// The gate fills in the id, the turn and the source; a connector only says
/// what the thing is and what it is called.
#[derive(Debug, Clone, PartialEq)]
pub struct ReferentDraft {
    pub kind: ReferentKind,
    pub display_name: String,
    /// Enough to act on it later. By convention a row id lives under `id`.
    pub metadata: serde_json::Value,
}

/// One integration.
pub trait Connector: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;

    /// Every action this connector defines. Static: the set of things a
    /// connector can do does not change at runtime, only whether each one
    /// currently works.
    fn actions(&self) -> &'static [ActionSpec];

    fn capabilities(&self, conn: &Connection) -> Capabilities;

    fn status(&self, conn: &Connection) -> ConnectorStatus;

    /// Render the one-line sentence the approval prompt and audit row show.
    ///
    /// Separate from `dispatch` because it runs *before* the user is asked,
    /// so the prompt can name the actual target: "Delete project Atlas"
    /// rather than "Delete project 41". Must not change anything.
    fn summarize(&self, action_id: &str, input: &serde_json::Value, conn: &Connection) -> String;

    /// What this action just put into the conversation.
    ///
    /// NEXUS-013. Called after a successful dispatch so follow-ups like
    /// "open the PR" have something to resolve against. It lives on the
    /// connector rather than in the Assistant Core because only the
    /// connector knows the shape of its own output, and the core is
    /// forbidden from knowing about specific applications.
    ///
    /// Returning nothing is the right answer for most actions. Navigation
    /// that opens a named thing is worth remembering; navigation to a screen
    /// is not, and something just deleted certainly is not.
    fn observe(
        &self,
        _action_id: &str,
        _input: &serde_json::Value,
        _output: &serde_json::Value,
        _conn: &Connection,
    ) -> Vec<ReferentDraft> {
        Vec::new()
    }

    /// Check that an input would deserialise, without performing anything.
    ///
    /// NEXUS-019 needs this so a reasoning provider's plan can be validated
    /// before the user is shown it: rejecting a malformed step at the point
    /// it is proposed is much better than surfacing it as an offer and
    /// failing at execution.
    ///
    /// The default accepts everything, which is safe because the real check
    /// still happens inside `dispatch`. A connector that overrides this moves
    /// its rejection earlier; one that does not is no less strict, only less
    /// helpful.
    fn validate_input(
        &self,
        _action_id: &str,
        _input: &serde_json::Value,
    ) -> Result<(), ActionError> {
        Ok(())
    }

    /// Turn this action's output into something worth reading aloud.
    ///
    /// NEXUS-015 defect E. Without it the assistant showed the action's
    /// *summary* ("List open tabs") and discarded the tabs themselves, so a
    /// working action looked like nothing had happened.
    ///
    /// Lives on the connector because only the connector knows the shape of
    /// its own payload. Returning None falls back to the summary, which is
    /// right for actions whose result is the navigation itself.
    fn describe_result(&self, _action_id: &str, _output: &serde_json::Value) -> Option<String> {
        None
    }

    /// Actions that can be run with no input at all.
    ///
    /// NEXUS-015 defect C. The deterministic resolver may offer these
    /// directly, because a bare phrase can supply everything they need.
    /// Actions requiring a URL, an id or a message are deliberately absent:
    /// matching one and then failing on a missing field is worse than not
    /// matching it, since the user gets an error instead of an offer.
    fn zero_input_actions(&self) -> &'static [&'static str] {
        &[]
    }

    /// The action a bare "yes" should run after this one succeeded.
    ///
    /// Some actions genuinely leave a question on screen. `compose_message`
    /// puts a draft in front of the user and the only sensible next word is
    /// "send it", but a bare affirmation names no action, so before this the
    /// deterministic tiers matched nothing and the reply was discarded as
    /// overheard speech.
    ///
    /// It lives here rather than in the Assistant Core for the usual reason:
    /// only the connector knows that one of its actions is a step rather
    /// than an ending. Returning `None` is right for almost everything.
    ///
    /// The follow-up is an offer, not an approval. It still passes the gate,
    /// so an action with `ConfirmPolicy::Always` is confirmed exactly as it
    /// would be from the palette. "Yes" chooses what to do; it does not
    /// answer the question of whether to do it.
    fn follow_up(
        &self,
        _action_id: &str,
        _input: &serde_json::Value,
        _output: &serde_json::Value,
    ) -> Option<FollowUp> {
        None
    }

    /// What the user could do about this failure, if anything.
    ///
    /// NEXUS-026. Only the connector knows what a failure of its own actually
    /// means, so only it can say what would fix it. The core must never infer
    /// one: a guessed remedy that does not work is worse than none, because
    /// the user spends their attention on it.
    ///
    /// Returning `None` is right for most failures. A network that is down
    /// has no remedy NEXUS can offer, and saying so plainly is the honest
    /// answer.
    fn remedy(&self, _action_id: &str, _error: &ActionError) -> Option<Remedy> {
        None
    }

    /// Perform the action. Reached only through the gate.
    fn dispatch(
        &self,
        action_id: &str,
        input: serde_json::Value,
        ctx: &ExecCtx<'_>,
    ) -> Result<serde_json::Value, ActionError>;

    /// Look up one of this connector's specs.
    fn spec(&self, action_id: &str) -> Option<&'static ActionSpec> {
        self.actions().iter().find(|spec| spec.id == action_id)
    }
}
