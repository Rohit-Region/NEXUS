//! NEXUS-012: the typed action vocabulary.
//!
//! An action is the only unit of work NEXUS performs on the user's behalf.
//! Everything that can happen has a stable id, a declared permission level,
//! and a typed input; nothing reaches a connector by any other route.
//!
//! Input types are plain serde structs rather than runtime JSON schemas. The
//! struct is the schema, the compiler checks it, and deserialisation is the
//! validation. Every input type carries `deny_unknown_fields`, so a caller
//! that invents a field is rejected rather than silently ignored. That
//! matters most for a reasoning provider, which will one day be producing
//! these payloads.

use serde::{Deserialize, Serialize};

use super::permission::{ConfirmPolicy, Permission, Reach};

/// The static description of one thing NEXUS can do.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionSpec {
    /// Stable and namespaced: "nexus.open_settings". Never reused, never
    /// renamed; a plan that names an unknown id is rejected outright.
    pub id: &'static str,
    pub connector_id: &'static str,
    /// One line, present tense, written for the approval prompt.
    pub summary: &'static str,
    pub permission: Permission,
    pub confirm: ConfirmPolicy,
    pub reach: Reach,
    /// Whether the user can undo it themselves afterwards. Surfaced in the
    /// approval prompt, because "this cannot be undone" changes the answer.
    pub reversible: bool,
}

/// A request to perform one action.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionRequest {
    pub action_id: String,
    /// Deserialised into the action's own input type at dispatch time.
    #[serde(default)]
    pub input: serde_json::Value,
    /// Present on the second call, once the user has approved.
    #[serde(default)]
    pub approval: Option<u64>,
}

/// What happened, on success.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionOutcome {
    pub action_id: String,
    /// The action's typed output, serialised. For navigation actions this is
    /// a directive the UI applies; NEXUS does not move the user itself.
    pub output: serde_json::Value,
    /// The same sentence the approval prompt showed, so the caller can report
    /// what happened without rebuilding it.
    pub summary: String,
    /// What the action actually found, described by the connector. None when
    /// the result is the navigation itself and there is nothing to read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub audit_id: i64,
}

/// Everything that can go wrong, as data rather than a string.
///
/// `NeedsApproval` is deliberately an error rather than a success variant:
/// nothing ran, and a caller that ignores errors must not mistake it for
/// completion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ActionError {
    #[serde(rename_all = "camelCase")]
    UnknownAction { action_id: String },
    #[serde(rename_all = "camelCase")]
    ConnectorDisabled { connector_id: String },
    #[serde(rename_all = "camelCase")]
    NotPermitted {
        connector_id: String,
        level: Permission,
    },
    #[serde(rename_all = "camelCase")]
    NeedsApproval {
        token: u64,
        summary: String,
        permission: Permission,
        reversible: bool,
        expires_in_ms: u64,
    },
    #[serde(rename_all = "camelCase")]
    InvalidApproval { reason: String },
    #[serde(rename_all = "camelCase")]
    InvalidInput { detail: String },
    #[serde(rename_all = "camelCase")]
    Failed { detail: String },
}

impl ActionError {
    /// A short label for the audit row. Never the full detail: an error
    /// string can quote input, and audit rows are long-lived.
    pub fn label(&self) -> &'static str {
        match self {
            ActionError::UnknownAction { .. } => "unknown-action",
            ActionError::ConnectorDisabled { .. } => "connector-disabled",
            ActionError::NotPermitted { .. } => "not-permitted",
            ActionError::NeedsApproval { .. } => "needs-approval",
            ActionError::InvalidApproval { .. } => "invalid-approval",
            ActionError::InvalidInput { .. } => "invalid-input",
            ActionError::Failed { .. } => "failed",
        }
    }

    /// Whether this outcome should be written to the audit trail.
    ///
    /// `NeedsApproval` is not a refusal, it is the middle of a conversation.
    /// Auditing it would double-count every confirmed action.
    pub fn is_auditable(&self) -> bool {
        !matches!(self, ActionError::NeedsApproval { .. })
    }
}

impl std::fmt::Display for ActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActionError::UnknownAction { action_id } => {
                write!(f, "NEXUS has no action called {action_id}.")
            }
            ActionError::ConnectorDisabled { connector_id } => {
                write!(f, "The {connector_id} connector is turned off.")
            }
            ActionError::NotPermitted {
                connector_id,
                level,
            } => write!(
                f,
                "{connector_id} is not allowed to {} on your behalf. Enable it in Settings.",
                level.as_str()
            ),
            ActionError::NeedsApproval { summary, .. } => {
                write!(f, "Waiting for you to approve: {summary}")
            }
            ActionError::InvalidApproval { reason } => {
                write!(f, "That approval is no longer valid: {reason}")
            }
            ActionError::InvalidInput { detail } => {
                write!(f, "That request was malformed: {detail}")
            }
            ActionError::Failed { detail } => write!(f, "{detail}"),
        }
    }
}

// -- Shared output shapes -----------------------------------------------------

/// Where the UI should go next.
///
/// NEXUS does not navigate; it says where to go and the shell obeys. Keeping
/// that as a returned value rather than a side effect is what lets a
/// navigation action be audited, approved and replayed like any other.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NavigateOutput {
    pub screen: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
}

impl NavigateOutput {
    pub fn screen(screen: &str) -> Self {
        NavigateOutput {
            screen: screen.to_string(),
            project_id: None,
            intent: None,
        }
    }
}

/// A row that no longer exists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletedOutput {
    pub id: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_serialise_as_a_tagged_union() {
        let json = serde_json::to_string(&ActionError::UnknownAction {
            action_id: "nexus.explode".to_string(),
        })
        .expect("serialize");
        assert!(json.contains("\"kind\":\"unknownAction\""), "{json}");
        assert!(json.contains("\"actionId\""), "{json}");
    }

    #[test]
    fn needs_approval_carries_what_the_prompt_must_show() {
        let err = ActionError::NeedsApproval {
            token: 7,
            summary: "Delete project Atlas".to_string(),
            permission: Permission::Destructive,
            reversible: false,
            expires_in_ms: 300_000,
        };
        let json = serde_json::to_string(&err).expect("serialize");
        for field in [
            "token",
            "summary",
            "permission",
            "reversible",
            "expiresInMs",
        ] {
            assert!(json.contains(field), "missing {field} in {json}");
        }
    }

    #[test]
    fn needs_approval_is_not_audited_but_every_other_outcome_is() {
        let pending = ActionError::NeedsApproval {
            token: 1,
            summary: String::new(),
            permission: Permission::Write,
            reversible: true,
            expires_in_ms: 0,
        };
        assert!(
            !pending.is_auditable(),
            "auditing a prompt would double-count the action"
        );
        assert!(ActionError::Failed {
            detail: "boom".to_string()
        }
        .is_auditable());
        assert!(ActionError::NotPermitted {
            connector_id: "nexus".to_string(),
            level: Permission::Write,
        }
        .is_auditable());
    }

    #[test]
    fn error_labels_are_stable_and_free_of_detail() {
        let err = ActionError::Failed {
            detail: "project Atlas at /Users/someone/secret".to_string(),
        };
        assert_eq!(err.label(), "failed");
        assert!(
            !err.label().contains("secret"),
            "audit labels must not carry detail"
        );
    }

    #[test]
    fn a_request_deserialises_without_input_or_approval() {
        let request: ActionRequest =
            serde_json::from_str(r#"{"actionId":"nexus.open_settings"}"#).expect("deserialize");
        assert_eq!(request.action_id, "nexus.open_settings");
        assert!(request.approval.is_none());
        assert!(request.input.is_null());
    }

    #[test]
    fn navigate_output_omits_absent_fields() {
        let json = serde_json::to_string(&NavigateOutput::screen("settings")).expect("serialize");
        assert_eq!(json, r#"{"screen":"settings"}"#);
    }

    #[test]
    fn error_messages_name_the_remedy_not_just_the_problem() {
        let err = ActionError::NotPermitted {
            connector_id: "nexus".to_string(),
            level: Permission::Write,
        };
        let shown = err.to_string();
        assert!(shown.contains("Settings"), "no remedy offered: {shown}");
    }
}
