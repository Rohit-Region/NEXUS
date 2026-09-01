//! NEXUS-012: pending approvals.
//!
//! Approvals live in memory and expire. They are deliberately **not**
//! persisted and do not survive a restart.
//!
//! The alternative, writing them to the database so nothing is lost, builds a
//! queue of pre-approved actions that can fire after a crash, an update, or a
//! night away from the desk. "NEXUS deleted something I approved yesterday"
//! is a far worse outcome than "NEXUS forgot a draft". Drafts can be
//! re-proposed for free; unwanted actions cannot be recalled.
//!
//! A token binds to one action id AND the exact input it was shown for. That
//! is what stops "Edit" in the confirmation UI from becoming a way to approve
//! one thing and perform another: changing the input invalidates the token.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long the user has to answer before the request lapses. Long enough to
/// read and think, short enough that an unattended machine goes quiet.
pub const APPROVAL_TTL: Duration = Duration::from_secs(300);

/// Bound so a caller that requests approvals in a loop and never answers
/// cannot grow the map without limit. The oldest are dropped first.
const MAX_PENDING: usize = 32;

#[derive(Debug, Clone)]
struct Pending {
    action_id: String,
    /// The exact serialised input the summary was rendered from. Compared
    /// verbatim rather than hashed: an equality check on the canonical form
    /// has no collisions to reason about.
    input: String,
    summary: String,
    issued_at: Instant,
}

/// In-memory approval store, held as Tauri managed state.
pub struct ApprovalStore {
    pending: Mutex<HashMap<u64, Pending>>,
    next_token: AtomicU64,
}

impl Default for ApprovalStore {
    fn default() -> Self {
        ApprovalStore {
            pending: Mutex::new(HashMap::new()),
            // Starts at 1 so 0 is never a valid token, which makes an
            // uninitialised field obvious rather than accidentally valid.
            next_token: AtomicU64::new(1),
        }
    }
}

impl ApprovalStore {
    /// Record a request awaiting the user, returning the token to quote back.
    pub fn issue(&self, action_id: &str, input: &serde_json::Value, summary: &str) -> u64 {
        let token = self.next_token.fetch_add(1, Ordering::SeqCst);
        let entry = Pending {
            action_id: action_id.to_string(),
            input: canonical(input),
            summary: summary.to_string(),
            issued_at: Instant::now(),
        };

        let mut map = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, p| p.issued_at.elapsed() < APPROVAL_TTL);
        if map.len() >= MAX_PENDING {
            if let Some(&oldest) = map
                .iter()
                .min_by_key(|(_, p)| p.issued_at)
                .map(|(token, _)| token)
            {
                map.remove(&oldest);
            }
        }
        map.insert(token, entry);
        token
    }

    /// Redeem a token for one specific action and input.
    ///
    /// Single use: a redeemed token is removed, so a replayed approval cannot
    /// perform the action twice. Every rejection says why, because "invalid"
    /// alone leaves the user unable to tell an expiry from a mismatch.
    pub fn redeem(
        &self,
        token: u64,
        action_id: &str,
        input: &serde_json::Value,
    ) -> Result<String, String> {
        let mut map = self.pending.lock().unwrap_or_else(|e| e.into_inner());

        let entry = match map.remove(&token) {
            Some(entry) => entry,
            None => return Err("it was already used, or NEXUS restarted".to_string()),
        };

        if entry.issued_at.elapsed() >= APPROVAL_TTL {
            return Err("it expired".to_string());
        }
        if entry.action_id != action_id {
            return Err("it was issued for a different action".to_string());
        }
        if entry.input != canonical(input) {
            return Err("the details changed after you approved it".to_string());
        }
        Ok(entry.summary)
    }

    /// Drop a request the user declined. Cancelling is not an error.
    pub fn cancel(&self, token: u64) {
        let mut map = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        map.remove(&token);
    }

    /// How many requests are still awaiting an answer, expiries excluded.
    pub fn pending_count(&self) -> usize {
        let map = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        map.values()
            .filter(|p| p.issued_at.elapsed() < APPROVAL_TTL)
            .count()
    }
}

/// A stable string for a JSON value.
///
/// `serde_json::Value` orders object keys deterministically (BTreeMap without
/// the preserve_order feature), so serialising twice gives the same bytes and
/// equality on the result is equality on the value.
fn canonical(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| String::from("\u{0}unserialisable"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_token_redeems_once_for_the_action_and_input_it_was_issued_for() {
        let store = ApprovalStore::default();
        let input = json!({ "id": 7 });
        let token = store.issue("nexus.delete_task", &input, "Delete task Ship it");

        let summary = store
            .redeem(token, "nexus.delete_task", &input)
            .expect("first redemption succeeds");
        assert_eq!(summary, "Delete task Ship it");

        assert!(
            store.redeem(token, "nexus.delete_task", &input).is_err(),
            "a replayed token must not perform the action twice"
        );
    }

    #[test]
    fn changing_the_input_invalidates_the_token() {
        // The reason Edit cannot become a way to approve one thing and do
        // another.
        let store = ApprovalStore::default();
        let token = store.issue("nexus.delete_task", &json!({ "id": 7 }), "Delete task 7");
        let err = store
            .redeem(token, "nexus.delete_task", &json!({ "id": 8 }))
            .expect_err("must reject");
        assert!(err.contains("details changed"), "unhelpful reason: {err}");
    }

    #[test]
    fn a_token_is_not_valid_for_a_different_action() {
        let store = ApprovalStore::default();
        let input = json!({ "id": 7 });
        let token = store.issue("nexus.delete_task", &input, "Delete task 7");
        assert!(store.redeem(token, "nexus.delete_project", &input).is_err());
    }

    #[test]
    fn key_order_does_not_affect_redemption() {
        let store = ApprovalStore::default();
        let issued = json!({ "projectId": 1, "title": "Ship" });
        let token = store.issue("nexus.create_task", &issued, "Create task Ship");
        let reordered = json!({ "title": "Ship", "projectId": 1 });
        assert!(
            store.redeem(token, "nexus.create_task", &reordered).is_ok(),
            "the same value written differently is still the same value"
        );
    }

    #[test]
    fn an_unknown_token_is_rejected_and_says_why() {
        let store = ApprovalStore::default();
        let err = store
            .redeem(999, "nexus.delete_task", &json!({}))
            .expect_err("must reject");
        assert!(
            err.contains("already used") || err.contains("restarted"),
            "{err}"
        );
    }

    #[test]
    fn zero_is_never_a_valid_token() {
        let store = ApprovalStore::default();
        store.issue("nexus.delete_task", &json!({}), "x");
        assert!(store.redeem(0, "nexus.delete_task", &json!({})).is_err());
    }

    #[test]
    fn tokens_are_never_reused_within_a_session() {
        let store = ApprovalStore::default();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            let token = store.issue("nexus.delete_task", &json!({}), "x");
            assert!(seen.insert(token), "token {token} was issued twice");
        }
    }

    #[test]
    fn cancelling_removes_the_request() {
        let store = ApprovalStore::default();
        let input = json!({ "id": 1 });
        let token = store.issue("nexus.delete_task", &input, "Delete task 1");
        assert_eq!(store.pending_count(), 1);
        store.cancel(token);
        assert_eq!(store.pending_count(), 0);
        assert!(store.redeem(token, "nexus.delete_task", &input).is_err());
    }

    #[test]
    fn cancelling_an_unknown_token_is_harmless() {
        let store = ApprovalStore::default();
        store.cancel(12345);
        assert_eq!(store.pending_count(), 0);
    }

    #[test]
    fn the_pending_map_is_bounded() {
        // A caller that requests approvals and never answers must not be able
        // to grow this without limit.
        let store = ApprovalStore::default();
        for i in 0..(MAX_PENDING * 3) {
            store.issue("nexus.delete_task", &json!({ "id": i }), "x");
        }
        assert!(
            store.pending_count() <= MAX_PENDING,
            "unbounded pending approvals: {}",
            store.pending_count()
        );
    }

    #[test]
    fn the_newest_request_survives_eviction() {
        let store = ApprovalStore::default();
        for i in 0..(MAX_PENDING * 2) {
            store.issue("nexus.delete_task", &json!({ "id": i }), "x");
        }
        let last = json!({ "id": MAX_PENDING * 2 });
        let token = store.issue("nexus.delete_task", &last, "newest");
        assert!(
            store.redeem(token, "nexus.delete_task", &last).is_ok(),
            "eviction must drop the oldest, never the request just made"
        );
    }

    #[test]
    fn nothing_is_written_to_disk() {
        // Asserts the design rather than the behaviour: this module must have
        // no database or filesystem reach at all.
        //
        // Scoped to the source above the test module, because the list of
        // forbidden words is itself part of this file. Guard tests that read
        // their own source will happily match themselves.
        let whole = include_str!("approval.rs");
        let production = whole
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("this file must keep its test module marker");

        for forbidden in ["Connection", "rusqlite", "std::fs", "File::", "OpenOptions"] {
            assert!(
                !production.contains(forbidden),
                "approvals must never be persisted, found {forbidden}"
            );
        }
    }
}
