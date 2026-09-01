//! NEXUS-012: the Assistant action layer.
//!
//! One rule holds this together: **[`execute_action`] is the only way an
//! action runs.** Voice, the command palette, a dashboard button and, later,
//! a reasoning provider's plan all converge on this single function. There is
//! no second path, which is what makes "voice cannot bypass permissions" a
//! structural fact rather than a promise.
//!
//! The design is a direct generalisation of something NEXUS already does.
//! `nexus_voice_start` re-reads the `voice_enabled` preference in Rust before
//! touching the microphone, with the note that enforcing it only in the UI
//! would leave the invariant unguarded. This module applies that reasoning to
//! everything NEXUS can do.

pub mod action;
pub mod approval;
pub mod claude_provider;
pub mod wa_contacts;
pub mod audit;
pub mod browser_connector;
pub mod calendar;
pub mod cloud_provider;
pub mod connector;
pub mod context;
pub mod converse;
pub mod github_connector;
pub mod http;
pub mod ide_connector;
pub mod jira_browser;
pub mod jira_connector;
pub mod msgraph;
pub mod nexus_connector;
pub mod notification_connector;
pub mod ollama_provider;
pub mod outlook_connector;
pub mod parametric;
pub mod permission;
pub mod proactive;
pub mod reasoning;
pub mod referent;
pub mod session;
pub mod shell;
pub mod suggestions;
pub mod system_connector;
pub mod teams_connector;
pub mod weather_connector;
pub mod whatsapp_connector;

use std::time::Instant;

use rusqlite::Connection;
use serde::Serialize;

use action::{ActionError, ActionOutcome, ActionRequest, ActionSpec};
use approval::{ApprovalStore, APPROVAL_TTL};
use audit::Outcome;
use browser_connector::BrowserConnector;
use connector::{Capabilities, Connector, ConnectorStatus, ExecCtx};
use github_connector::GithubConnector;
use ide_connector::IdeConnector;
use jira_connector::JiraConnector;
use nexus_connector::NexusConnector;
use notification_connector::NotificationConnector;
use outlook_connector::OutlookConnector;
use permission::{ConfirmPolicy, Permission};
use session::{AssistantSession, AssistantState, TurnInput};
use system_connector::SystemConnector;
use teams_connector::TeamsConnector;
use weather_connector::WeatherConnector;
use whatsapp_connector::WhatsappConnector;

/// NEXUS-013: emitted whenever assistant state changes, so the UI never has
/// to poll. Mirrors the voice event channel's naming.
pub const EVENT_ASSISTANT_STATE: &str = "nexus://assistant/state";

/// Every connector NEXUS knows about.
///
/// A slice of trait objects rather than an enum, so NEXUS-013 adds a
/// connector by adding a line here and nothing else. The Assistant Core never
/// names a specific application.
pub fn connectors() -> Vec<&'static dyn Connector> {
    vec![
        &NexusConnector,
        &BrowserConnector,
        &IdeConnector,
        &GithubConnector,
        &JiraConnector,
        &TeamsConnector,
        &WhatsappConnector,
        &WeatherConnector,
        &SystemConnector,
        &OutlookConnector,
        &NotificationConnector,
    ]
}

/// The connector that owns an action id, if any.
///
/// NEXUS-026 needs it to check that a remedy points at something real before
/// offering it: an offer the user says yes to and that then fails with
/// "unknown action" is worse than not offering at all.
pub fn connector_for(action_id: &str) -> Option<&'static dyn Connector> {
    connectors()
        .into_iter()
        .find(|c| c.spec(action_id).is_some())
}

/// Make sure every connector in the code has a row in the database.
///
/// Called at startup rather than written as a migration, so adding a
/// connector is a line in `connectors()` and nothing else. Migrations are for
/// schema; a connector list is not schema.
///
/// Deliberately `INSERT OR IGNORE`: it registers what is new and never
/// touches what is there, so a connector the user disabled stays disabled and
/// a grant they revoked stays revoked. New connectors arrive with **no
/// grants at all** and are inert until the user allows them in Settings. The
/// local `nexus` connector is the exception, seeded by migration 002, because
/// it is NEXUS acting on the user's own workspace.
pub fn register_connectors(conn: &Connection) -> Result<(), String> {
    for c in connectors() {
        conn.execute(
            "INSERT OR IGNORE INTO connectors (connector_id, display_name)
             VALUES (?1, ?2)",
            rusqlite::params![c.id(), c.display_name()],
        )
        .map_err(|e| format!("Failed to register connector {}: {e}", c.id()))?;
    }
    Ok(())
}

fn find_action(action_id: &str) -> Option<(&'static dyn Connector, &'static ActionSpec)> {
    connectors()
        .into_iter()
        .find_map(|c| c.spec(action_id).map(|spec| (c, spec)))
}

/// A connector as the Settings and Activity views see it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorView {
    pub id: String,
    pub display_name: String,
    pub status: ConnectorStatus,
    pub enabled: bool,
    pub capabilities: Capabilities,
    pub actions: Vec<ActionSpec>,
    pub granted: Vec<Permission>,
    /// Levels this connector's actions actually use. Offering a toggle for a
    /// level no action needs invites the user to grant something for nothing.
    pub required_levels: Vec<Permission>,
    /// The connector's stored configuration: endpoints and account names.
    ///
    /// Safe to show: `set_connector_config` refuses any key that looks like
    /// a secret, so credentials live in the Keychain and never here. Null
    /// when the connector has never been configured.
    pub config: serde_json::Value,
}

pub fn list_connectors(conn: &Connection) -> Result<Vec<ConnectorView>, String> {
    let mut out = Vec::new();
    for c in connectors() {
        let mut required: Vec<Permission> =
            c.actions().iter().map(|spec| spec.permission).collect();
        required.sort();
        required.dedup();

        out.push(ConnectorView {
            id: c.id().to_string(),
            display_name: c.display_name().to_string(),
            status: if is_enabled(conn, c.id())? {
                c.status(conn)
            } else {
                ConnectorStatus::Disabled
            },
            enabled: is_enabled(conn, c.id())?,
            capabilities: c.capabilities(conn),
            actions: c.actions().to_vec(),
            granted: permission::granted_levels(conn, c.id())?,
            required_levels: required,
            config: read_config_json(conn, c.id()),
        });
    }
    Ok(out)
}

/// A connector's stored configuration, or null when it has none.
fn read_config_json(conn: &Connection, connector_id: &str) -> serde_json::Value {
    conn.query_row(
        "SELECT config_json FROM connectors WHERE connector_id = ?1",
        [connector_id],
        |row| row.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
    .and_then(|raw| serde_json::from_str(&raw).ok())
    .unwrap_or(serde_json::Value::Null)
}

fn is_enabled(conn: &Connection, connector_id: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT enabled FROM connectors WHERE connector_id = ?1",
        [connector_id],
        |row| row.get::<_, i64>(0),
    )
    .map(|v| v != 0)
    // A connector with no row has never been registered, which is a denial
    // rather than an error: the gate refuses and the audit records why.
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(false),
        other => Err(format!("Failed to read connector: {other}")),
    })
}

/// Store a connector's configuration.
///
/// Configuration only: site addresses, account names, endpoints. **Never
/// secrets.** Tokens belong in the Keychain, because `nexus.db` is a plain
/// file and a token in it is a token on disk in the clear.
pub fn set_connector_config(
    conn: &Connection,
    connector_id: &str,
    config: &serde_json::Value,
) -> Result<(), String> {
    // A refusal rather than a redaction: silently dropping a field the caller
    // asked to store is worse than telling them it does not belong here.
    if let Some(object) = config.as_object() {
        for key in object.keys() {
            let lowered = key.to_lowercase();
            if [
                "token",
                "password",
                "secret",
                "apikey",
                "api_key",
                "credential",
            ]
            .iter()
            .any(|banned| lowered.contains(banned))
            {
                return Err(format!(
                    "\"{key}\" looks like a secret. Store it in the Keychain, not in NEXUS."
                ));
            }
        }
    }

    let changed = conn
        .execute(
            "UPDATE connectors
                SET config_json = ?2,
                    updated_at  = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
              WHERE connector_id = ?1",
            rusqlite::params![connector_id, config.to_string()],
        )
        .map_err(|e| format!("Failed to store configuration: {e}"))?;
    if changed == 0 {
        return Err(format!("Unknown connector: {connector_id}"));
    }
    Ok(())
}

pub fn set_connector_enabled(
    conn: &Connection,
    connector_id: &str,
    enabled: bool,
) -> Result<(), String> {
    let changed = conn
        .execute(
            "UPDATE connectors
                SET enabled = ?2,
                    updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
              WHERE connector_id = ?1",
            rusqlite::params![connector_id, enabled as i64],
        )
        .map_err(|e| format!("Failed to update connector: {e}"))?;
    if changed == 0 {
        return Err(format!("Unknown connector: {connector_id}"));
    }
    Ok(())
}

/// The single dispatch path.
///
/// The order of the checks is deliberate. Identity before permission, because
/// an unknown action has no permission to check. Permission before
/// confirmation, because there is no point asking the user to approve
/// something NEXUS would refuse anyway. Confirmation before deserialisation,
/// because the summary the user approved was rendered from the raw input, and
/// re-deriving it after a transformation would let the two drift apart.
pub fn execute_action(
    conn: &Connection,
    approvals: &ApprovalStore,
    session: &AssistantSession,
    request: ActionRequest,
) -> Result<ActionOutcome, ActionError> {
    // NEXUS-013. Taking the session here rather than leaving it to the caller
    // makes "every action moves assistant state" a guarantee instead of
    // something each call site has to remember. A turn opened by the UI is
    // continued; an action arriving with no turn in flight opens its own.
    if session.current_turn() == 0 || !session.state().is_busy() {
        session.begin_turn(TurnInput::Ui {
            action_id: request.action_id.clone(),
        });
    }

    // 1. Identity. An unknown id is rejected outright: NEXUS never guesses a
    //    nearest match, because a plausible wrong action is worse than none.
    let (connector, spec) = match find_action(&request.action_id) {
        Some(found) => found,
        None => {
            return Err(ActionError::UnknownAction {
                action_id: request.action_id,
            })
        }
    };

    let summary = connector.summarize(spec.id, &request.input, conn);

    // Refusals are audited on a best-effort basis. A failure to write history
    // must not become a way to make the gate fail open.
    let refuse = |error: ActionError| -> ActionError {
        if error.is_auditable() {
            let _ = audit::refusal(
                conn,
                spec.id,
                spec.connector_id,
                spec.permission,
                &summary,
                error.label(),
            );
        }
        session.advance(
            AssistantState::Failed,
            Some(summary.clone()),
            Some(error.to_string()),
        );
        error
    };

    // 2. The connector must be registered and switched on.
    match is_enabled(conn, spec.connector_id) {
        Ok(true) => {}
        Ok(false) => {
            return Err(refuse(ActionError::ConnectorDisabled {
                connector_id: spec.connector_id.to_string(),
            }))
        }
        Err(detail) => return Err(refuse(ActionError::Failed { detail })),
    }

    // 3. The standing grant.
    match permission::is_granted(conn, spec.connector_id, spec.permission) {
        Ok(true) => {}
        Ok(false) => {
            return Err(refuse(ActionError::NotPermitted {
                connector_id: spec.connector_id.to_string(),
                level: spec.permission,
            }))
        }
        Err(detail) => return Err(refuse(ActionError::Failed { detail })),
    }

    // 4. Per-invocation confirmation.
    //
    //    `always_confirms()` is consulted alongside the spec's own policy, so
    //    a Write-or-above action cannot ship without confirmation even if its
    //    spec says otherwise. The policy can tighten the rule, never loosen it.
    let needs_confirmation =
        spec.confirm == ConfirmPolicy::Always || spec.permission.always_confirms();

    let approved = if needs_confirmation {
        match request.approval {
            None => {
                let token = approvals.issue(spec.id, &request.input, &summary);
                session.advance(
                    AssistantState::AwaitingConfirmation,
                    Some(summary.clone()),
                    None,
                );
                // Not audited: this is the middle of a conversation, not an
                // outcome. Auditing here would double-count every approval.
                return Err(ActionError::NeedsApproval {
                    token,
                    summary,
                    permission: spec.permission,
                    reversible: spec.reversible,
                    expires_in_ms: APPROVAL_TTL.as_millis() as u64,
                });
            }
            Some(token) => match approvals.redeem(token, spec.id, &request.input) {
                Ok(_) => true,
                Err(reason) => return Err(refuse(ActionError::InvalidApproval { reason })),
            },
        }
    } else {
        false
    };

    // 5. Open the audit row BEFORE dispatch, so an action that never returns
    //    still leaves a trace.
    let audit_id = audit::begin(
        conn,
        spec.id,
        spec.connector_id,
        spec.permission,
        &summary,
        approved,
    )
    .map_err(|detail| ActionError::Failed { detail })?;

    // 6. Dispatch. Deserialisation into the action's typed input happens
    //    inside the connector, and is the only validation there is.
    session.advance(AssistantState::Executing, Some(summary.clone()), None);
    let started = Instant::now();
    let input = request.input;
    let result = connector.dispatch(spec.id, input.clone(), &ExecCtx { conn });
    let elapsed = started.elapsed().as_millis();

    // 7. Close the row either way.
    match result {
        Ok(output) => {
            let _ = audit::finish(conn, audit_id, Outcome::Succeeded, None, elapsed);
            // What this action put into the conversation, so "open the PR"
            // has something to resolve against later. The connector decides;
            // the core only files it.
            for draft in connector.observe(spec.id, &input, &output, conn) {
                session.remember(
                    draft.kind,
                    &draft.display_name,
                    spec.connector_id,
                    draft.metadata,
                );
            }
            // What the action found, in the connector's own words. Recorded
            // on the turn too, so the conversation shows the answer rather
            // than only the intent.
            let detail = connector.describe_result(spec.id, &output);

            // Whether this action left a question on screen. Set or cleared
            // on every success, never left standing: an offer that outlived
            // the action it followed would let a much later "yes" run
            // something the user had stopped thinking about.
            match connector
                .follow_up(spec.id, &input, &output)
                .and_then(|next| connector.spec(next.action_id).map(|spec| (next, spec)))
            {
                Some((next, next_spec)) => session.offer_follow_up(
                    next_spec.id,
                    next.input,
                    &format!(
                        "Say yes to {}, or no to leave it.",
                        next_spec.summary.to_lowercase()
                    ),
                ),
                None => session.clear_follow_up(),
            }

            session.advance(
                AssistantState::Completed,
                Some(detail.clone().unwrap_or_else(|| summary.clone())),
                None,
            );
            Ok(ActionOutcome {
                action_id: spec.id.to_string(),
                output,
                summary,
                detail,
                audit_id,
            })
        }
        Err(error) => {
            // A failed offer is not still open. Clearing it here means a
            // "yes" after a failure re-asks rather than silently retrying
            // something that just did not work.
            session.clear_follow_up();

            // NEXUS-026. What the user could do about it, if the connector
            // knows. Offered through the same follow-up mechanism a draft
            // uses, so "yes" reaches it with no new vocabulary and no new
            // surface: a fix is offered, never applied.
            if let Some(remedy) = connector.remedy(spec.id, &error) {
                if connector_for(remedy.action_id).is_some() {
                    session.offer_follow_up(
                        remedy.action_id,
                        remedy.input.clone(),
                        &remedy.prompt,
                    );
                }
            }
            // The reason, not the class. `label()` renders every dispatch
            // failure as the string "failed", which is already what the
            // `outcome` column says, so the row carried no information about
            // what actually went wrong: a missing Accessibility grant and a
            // focus race were indistinguishable in the trail. Refusals still
            // record their label, because there the class *is* the reason.
            let _ = audit::finish(
                conn,
                audit_id,
                Outcome::Failed,
                Some(&error.to_string()),
                elapsed,
            );
            session.advance(
                AssistantState::Failed,
                Some(summary.clone()),
                Some(error.to_string()),
            );
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrations::MIGRATIONS;
    use serde_json::json;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch("PRAGMA foreign_keys = ON;").expect("fk");
        for (_, sql) in MIGRATIONS {
            conn.execute_batch(sql).expect("migrate");
        }
        register_connectors(&conn).expect("register");
        conn
    }

    fn seed_project(conn: &Connection, name: &str) -> i64 {
        conn.execute("INSERT INTO projects (name) VALUES (?1)", [name])
            .expect("seed");
        conn.last_insert_rowid()
    }

    fn seed_task(conn: &Connection, project_id: i64, title: &str) -> i64 {
        conn.execute(
            "INSERT INTO tasks (project_id, title) VALUES (?1, ?2)",
            rusqlite::params![project_id, title],
        )
        .expect("seed");
        conn.last_insert_rowid()
    }

    fn request(action_id: &str, input: serde_json::Value) -> ActionRequest {
        ActionRequest {
            action_id: action_id.to_string(),
            input,
            approval: None,
        }
    }

    // -- Registry consistency -------------------------------------------------

    #[test]
    fn confirm_policy_never_contradicts_permission() {
        // The rule the whole gate rests on: nothing at Write or above may
        // ship without confirmation. This is the test that stops a future
        // action from quietly opting out.
        for c in connectors() {
            for spec in c.actions() {
                if spec.permission.always_confirms() {
                    assert_eq!(
                        spec.confirm,
                        ConfirmPolicy::Always,
                        "{} is {} but does not always confirm",
                        spec.id,
                        spec.permission.as_str()
                    );
                }
            }
        }
    }

    #[test]
    fn action_ids_are_unique_across_all_connectors() {
        let mut seen = std::collections::HashSet::new();
        for c in connectors() {
            for spec in c.actions() {
                assert!(
                    seen.insert(spec.id),
                    "{} is defined by more than one connector",
                    spec.id
                );
            }
        }
    }

    #[test]
    fn every_action_belongs_to_the_connector_that_lists_it() {
        for c in connectors() {
            for spec in c.actions() {
                assert_eq!(spec.connector_id, c.id(), "{} is misfiled", spec.id);
            }
        }
    }

    // -- Gate: identity -------------------------------------------------------

    #[test]
    fn an_unknown_action_is_rejected_with_no_nearest_match() {
        let conn = test_conn();
        let approvals = ApprovalStore::default();
        let session = AssistantSession::default();
        // Deliberately close to a real id. NEXUS must not "help".
        let err = execute_action(
            &conn,
            &approvals,
            &session,
            request("nexus.open_setting", json!(null)),
        )
        .expect_err("must reject");
        assert!(matches!(err, ActionError::UnknownAction { .. }), "{err:?}");
    }

    #[test]
    fn an_action_from_an_unregistered_connector_is_refused() {
        let conn = test_conn();
        let approvals = ApprovalStore::default();
        let session = AssistantSession::default();
        // A namespace no connector claims. Deliberately not a real
        // connector's id: those resolve and are refused by the *grant*, which
        // is a different code path entirely.
        let err = execute_action(
            &conn,
            &approvals,
            &session,
            request("salesforce.create_lead", json!({})),
        )
        .expect_err("must reject");
        assert!(matches!(err, ActionError::UnknownAction { .. }), "{err:?}");
    }

    // -- Gate: grants ---------------------------------------------------------

    #[test]
    fn revoking_a_grant_blocks_the_next_attempt() {
        let conn = test_conn();
        let approvals = ApprovalStore::default();
        let session = AssistantSession::default();

        execute_action(
            &conn,
            &approvals,
            &session,
            request("nexus.open_settings", json!(null)),
        )
        .expect("permitted while granted");

        permission::set_grant(&conn, "nexus", Permission::Interact, false).expect("revoke");

        let err = execute_action(
            &conn,
            &approvals,
            &session,
            request("nexus.open_settings", json!(null)),
        )
        .expect_err("must refuse once revoked");
        assert!(
            matches!(err, ActionError::NotPermitted { level, .. } if level == Permission::Interact),
            "{err:?}"
        );
    }

    #[test]
    fn disabling_a_connector_blocks_every_one_of_its_actions() {
        let conn = test_conn();
        let approvals = ApprovalStore::default();
        let session = AssistantSession::default();
        set_connector_enabled(&conn, "nexus", false).expect("disable");

        let err = execute_action(
            &conn,
            &approvals,
            &session,
            request("nexus.open_overview", json!(null)),
        )
        .expect_err("must refuse");
        assert!(
            matches!(err, ActionError::ConnectorDisabled { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn a_refusal_is_written_to_the_audit_trail() {
        let conn = test_conn();
        let approvals = ApprovalStore::default();
        let session = AssistantSession::default();
        permission::set_grant(&conn, "nexus", Permission::Interact, false).expect("revoke");

        let _ = execute_action(
            &conn,
            &approvals,
            &session,
            request("nexus.open_projects", json!(null)),
        );

        let rows = audit::list_recent(&conn, 10).expect("audit");
        assert_eq!(rows.len(), 1, "a refusal must be recorded");
        assert_eq!(rows[0].outcome, "refused");
        assert_eq!(rows[0].error.as_deref(), Some("not-permitted"));
    }

    // -- Gate: confirmation ---------------------------------------------------

    #[test]
    fn a_write_action_cannot_run_without_an_approval_token() {
        let conn = test_conn();
        let approvals = ApprovalStore::default();
        let session = AssistantSession::default();
        let project = seed_project(&conn, "Atlas");

        let err = execute_action(
            &conn,
            &approvals,
            &session,
            request(
                "nexus.create_task",
                json!({ "projectId": project, "title": "Ship it" }),
            ),
        )
        .expect_err("must ask first");
        assert!(matches!(err, ActionError::NeedsApproval { .. }), "{err:?}");

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
            .expect("count");
        assert_eq!(count, 0, "nothing may happen before approval");
    }

    #[test]
    fn a_write_action_runs_once_the_token_is_presented() {
        let conn = test_conn();
        let approvals = ApprovalStore::default();
        let session = AssistantSession::default();
        let project = seed_project(&conn, "Atlas");
        let input = json!({ "projectId": project, "title": "Ship it" });

        let token = match execute_action(
            &conn,
            &approvals,
            &session,
            request("nexus.create_task", input.clone()),
        ) {
            Err(ActionError::NeedsApproval { token, .. }) => token,
            other => panic!("expected an approval request, got {other:?}"),
        };

        let outcome = execute_action(
            &conn,
            &approvals,
            &session,
            ActionRequest {
                action_id: "nexus.create_task".to_string(),
                input,
                approval: Some(token),
            },
        )
        .expect("approved action runs");

        assert_eq!(outcome.output["title"], "Ship it");
        assert!(outcome.summary.contains("Atlas"), "{}", outcome.summary);
    }

    #[test]
    fn an_approved_action_cannot_be_replayed() {
        let conn = test_conn();
        let approvals = ApprovalStore::default();
        let session = AssistantSession::default();
        let project = seed_project(&conn, "Atlas");
        let task = seed_task(&conn, project, "Ship it");
        let input = json!({ "id": task });

        let token = match execute_action(
            &conn,
            &approvals,
            &session,
            request("nexus.delete_task", input.clone()),
        ) {
            Err(ActionError::NeedsApproval { token, .. }) => token,
            other => panic!("expected an approval request, got {other:?}"),
        };
        let approved = ActionRequest {
            action_id: "nexus.delete_task".to_string(),
            input,
            approval: Some(token),
        };
        execute_action(&conn, &approvals, &session, approved).expect("first run");

        let replay = execute_action(
            &conn,
            &approvals,
            &session,
            ActionRequest {
                action_id: "nexus.delete_task".to_string(),
                input: json!({ "id": task }),
                approval: Some(token),
            },
        )
        .expect_err("a replayed token must not run again");
        assert!(
            matches!(replay, ActionError::InvalidApproval { .. }),
            "{replay:?}"
        );
    }

    #[test]
    fn editing_the_target_after_approval_invalidates_the_token() {
        // The reason Edit cannot become a way to approve one thing and do
        // another.
        let conn = test_conn();
        let approvals = ApprovalStore::default();
        let session = AssistantSession::default();
        let project = seed_project(&conn, "Atlas");
        let keep = seed_task(&conn, project, "Keep me");
        let target = seed_task(&conn, project, "Delete me");

        let token = match execute_action(
            &conn,
            &approvals,
            &session,
            request("nexus.delete_task", json!({ "id": target })),
        ) {
            Err(ActionError::NeedsApproval { token, .. }) => token,
            other => panic!("expected an approval request, got {other:?}"),
        };

        let err = execute_action(
            &conn,
            &approvals,
            &session,
            ActionRequest {
                action_id: "nexus.delete_task".to_string(),
                input: json!({ "id": keep }),
                approval: Some(token),
            },
        )
        .expect_err("must reject a swapped target");
        assert!(
            matches!(err, ActionError::InvalidApproval { .. }),
            "{err:?}"
        );

        let still_there: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks WHERE id = ?1", [keep], |r| {
                r.get(0)
            })
            .expect("count");
        assert_eq!(still_there, 1, "the swapped-in row must survive");
    }

    #[test]
    fn a_fabricated_token_is_refused_and_audited() {
        let conn = test_conn();
        let approvals = ApprovalStore::default();
        let session = AssistantSession::default();
        let project = seed_project(&conn, "Atlas");
        let task = seed_task(&conn, project, "Ship it");

        let err = execute_action(
            &conn,
            &approvals,
            &session,
            ActionRequest {
                action_id: "nexus.delete_task".to_string(),
                input: json!({ "id": task }),
                approval: Some(4242),
            },
        )
        .expect_err("must refuse");
        assert!(
            matches!(err, ActionError::InvalidApproval { .. }),
            "{err:?}"
        );

        let rows = audit::list_recent(&conn, 10).expect("audit");
        assert_eq!(rows[0].outcome, "refused");
        assert_eq!(rows[0].error.as_deref(), Some("invalid-approval"));
    }

    #[test]
    fn an_approval_request_is_not_audited() {
        // It is the middle of a conversation, not an outcome. Auditing it
        // would double-count every confirmed action.
        let conn = test_conn();
        let approvals = ApprovalStore::default();
        let session = AssistantSession::default();
        let project = seed_project(&conn, "Atlas");

        let _ = execute_action(
            &conn,
            &approvals,
            &session,
            request(
                "nexus.create_task",
                json!({ "projectId": project, "title": "x" }),
            ),
        );
        assert!(audit::list_recent(&conn, 10).expect("audit").is_empty());
    }

    #[test]
    fn permission_is_checked_before_the_user_is_asked_to_approve() {
        // No point asking someone to authorise something NEXUS would refuse.
        let conn = test_conn();
        let approvals = ApprovalStore::default();
        let session = AssistantSession::default();
        let project = seed_project(&conn, "Atlas");
        permission::set_grant(&conn, "nexus", Permission::Write, false).expect("revoke");

        let err = execute_action(
            &conn,
            &approvals,
            &session,
            request(
                "nexus.create_task",
                json!({ "projectId": project, "title": "x" }),
            ),
        )
        .expect_err("must refuse");
        assert!(matches!(err, ActionError::NotPermitted { .. }), "{err:?}");
        assert_eq!(
            approvals.pending_count(),
            0,
            "no prompt should have been issued"
        );
    }

    // -- Interact path --------------------------------------------------------

    #[test]
    fn an_interact_action_runs_without_a_prompt() {
        let conn = test_conn();
        let approvals = ApprovalStore::default();
        let session = AssistantSession::default();
        let outcome = execute_action(
            &conn,
            &approvals,
            &session,
            request("nexus.open_registry", json!(null)),
        )
        .expect("no prompt for navigation");
        assert_eq!(outcome.output["screen"], "registry");
        assert_eq!(approvals.pending_count(), 0);
    }

    #[test]
    fn a_result_is_described_not_just_summarised() {
        // Defect E: the assistant showed "List open tabs" and discarded the
        // tabs, so a working action looked like nothing had happened.
        let conn = test_conn();
        let approvals = ApprovalStore::default();
        let session = AssistantSession::default();
        let project = seed_project(&conn, "Atlas");

        let outcome = execute_action(
            &conn,
            &approvals,
            &session,
            request("nexus.open_project", json!({ "projectId": project })),
        )
        .expect("run");

        // nexus.* navigation has no result worth reading, so it falls back to
        // the summary rather than inventing detail.
        assert!(outcome.detail.is_none());
        assert!(outcome.summary.contains("Atlas"));
    }

    #[test]
    fn every_connector_describing_a_result_returns_something_readable() {
        // A description that is empty or that echoes the raw payload is worse
        // than none, because the caller would show it instead of the summary.
        for connector in connectors() {
            for action_id in connector.zero_input_actions() {
                if let Some(text) =
                    connector.describe_result(action_id, &json!({ "unexpected": true }))
                {
                    assert!(!text.trim().is_empty(), "{action_id} described nothing");
                    assert!(!text.contains('{'), "{action_id} leaked raw json");
                }
            }
        }
    }

    #[test]
    fn the_turn_records_what_was_found() {
        // The conversation should show the answer, not only the intent.
        let conn = test_conn();
        let approvals = ApprovalStore::default();
        let session = AssistantSession::default();
        execute_action(
            &conn,
            &approvals,
            &session,
            request("nexus.open_overview", json!(null)),
        )
        .expect("run");

        let snapshot = session.snapshot(0);
        assert!(
            snapshot.turns.last().expect("a turn").summary.is_some(),
            "the turn must carry what happened"
        );
    }

    #[test]
    fn a_successful_action_is_audited_as_succeeded() {
        let conn = test_conn();
        let approvals = ApprovalStore::default();
        let session = AssistantSession::default();
        execute_action(
            &conn,
            &approvals,
            &session,
            request("nexus.open_overview", json!(null)),
        )
        .expect("run");

        let rows = audit::list_recent(&conn, 10).expect("audit");
        assert_eq!(rows[0].outcome, "succeeded");
        assert_eq!(rows[0].action_id, "nexus.open_overview");
        assert!(!rows[0].approved, "navigation is not an approved action");
    }

    #[test]
    fn a_failing_action_is_audited_as_failed() {
        let conn = test_conn();
        let approvals = ApprovalStore::default();
        let session = AssistantSession::default();
        let err = execute_action(
            &conn,
            &approvals,
            &session,
            request("nexus.open_project", json!({ "projectId": "not a number" })),
        )
        .expect_err("must fail");
        assert!(matches!(err, ActionError::InvalidInput { .. }), "{err:?}");

        let rows = audit::list_recent(&conn, 10).expect("audit");
        assert_eq!(rows[0].outcome, "failed");
        // The reason, not the class. `outcome` already says it failed, so a
        // label here would repeat that and say nothing about what went
        // wrong: the trail could not tell a missing permission apart from a
        // focus race. Refusals keep their labels, where the class *is* the
        // reason.
        let reason = rows[0].error.clone().expect("a reason");
        assert!(reason.contains("malformed"), "{reason}");
        assert!(reason.contains("not a number"), "{reason}");
    }

    // -- Connector view -------------------------------------------------------

    #[test]
    fn the_connector_view_offers_only_levels_its_actions_use() {
        let conn = test_conn();
        let views = list_connectors(&conn).expect("list");
        let nexus = views
            .iter()
            .find(|v| v.id == "nexus")
            .expect("nexus present");

        assert!(
            !nexus.required_levels.contains(&Permission::Execute),
            "offering a toggle for a level no action needs invites granting it for nothing"
        );
        assert!(nexus.required_levels.contains(&Permission::Interact));
        assert!(nexus.required_levels.contains(&Permission::Destructive));
    }

    #[test]
    fn a_new_connector_arrives_inert() {
        // The rule that makes adding a connector safe: registration creates
        // the row, never the permission. Chrome can do nothing until the user
        // says so in Settings.
        let conn = test_conn();
        let approvals = ApprovalStore::default();
        let session = AssistantSession::default();

        let browser = list_connectors(&conn)
            .expect("list")
            .into_iter()
            .find(|v| v.id == "browser")
            .expect("browser is registered");
        assert!(browser.enabled, "registered");
        assert!(
            browser.granted.is_empty(),
            "a new connector must arrive with no grants, got {:?}",
            browser.granted
        );

        let err = execute_action(
            &conn,
            &approvals,
            &session,
            request("browser.list_tabs", json!(null)),
        )
        .expect_err("must refuse before it is allowed");
        assert!(matches!(err, ActionError::NotPermitted { .. }), "{err:?}");
    }

    #[test]
    fn registration_never_restores_something_the_user_turned_off() {
        let conn = test_conn();
        set_connector_enabled(&conn, "browser", false).expect("disable");
        permission::set_grant(&conn, "nexus", Permission::Destructive, false).expect("revoke");

        register_connectors(&conn).expect("re-register");

        let views = list_connectors(&conn).expect("list");
        let browser = views.iter().find(|v| v.id == "browser").expect("present");
        assert!(!browser.enabled, "a disabled connector must stay disabled");
        assert!(
            !permission::is_granted(&conn, "nexus", Permission::Destructive).expect("read"),
            "a revoked grant must stay revoked"
        );
    }

    #[test]
    fn every_connector_in_code_has_a_row_after_registration() {
        let conn = test_conn();
        let registered: Vec<String> = list_connectors(&conn)
            .expect("list")
            .into_iter()
            .map(|v| v.id)
            .collect();
        for c in connectors() {
            assert!(
                registered.contains(&c.id().to_string()),
                "{} was never registered",
                c.id()
            );
        }
    }

    #[test]
    fn a_disabled_connector_reports_disabled_rather_than_ready() {
        let conn = test_conn();
        set_connector_enabled(&conn, "nexus", false).expect("disable");
        let views = list_connectors(&conn).expect("list");
        let nexus = views.iter().find(|v| v.id == "nexus").expect("present");
        assert_eq!(nexus.status, ConnectorStatus::Disabled);
        assert!(!nexus.enabled);
    }

    #[test]
    fn a_secret_is_refused_from_connector_configuration() {
        // The Keychain is the only place a token goes. Refusing loudly beats
        // storing it quietly or dropping it silently.
        let conn = test_conn();
        for field in ["token", "apiToken", "password", "clientSecret", "API_KEY"] {
            let config = json!({ "site": "https://x.atlassian.net", field: "hunter2" });
            let err =
                set_connector_config(&conn, "jira", &config).expect_err("must refuse {field}");
            assert!(err.contains("Keychain"), "{err}");
        }
    }

    #[test]
    fn ordinary_configuration_is_stored() {
        let conn = test_conn();
        set_connector_config(
            &conn,
            "jira",
            &json!({ "site": "https://x.atlassian.net", "email": "a@b.com" }),
        )
        .expect("stores");
        let stored: String = conn
            .query_row(
                "SELECT config_json FROM connectors WHERE connector_id = 'jira'",
                [],
                |r| r.get(0),
            )
            .expect("read");
        assert!(stored.contains("atlassian"));
    }

    #[test]
    fn enabling_an_unknown_connector_is_an_error() {
        let conn = test_conn();
        assert!(set_connector_enabled(&conn, "salesforce", true).is_err());
    }

    // -- The single-path guarantee -------------------------------------------

    /// The source above this file's test module. Guard tests that read their
    /// own source will otherwise match the very strings they forbid.
    fn production_source(whole: &str) -> &str {
        whole
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("the file must keep its test module marker")
    }

    #[test]
    fn the_gate_is_the_only_caller_of_dispatch() {
        // The structural claim the whole design rests on. If a second call
        // site appears, permissions and audit become advisory.
        let gate = production_source(include_str!("mod.rs"));
        assert_eq!(
            gate.matches(".dispatch(").count(),
            1,
            "execute_action must hold the only dispatch call site"
        );
    }

    #[test]
    fn the_command_layer_cannot_reach_a_connector_directly() {
        let commands = include_str!("../commands/mod.rs");
        assert!(
            !commands.contains(".dispatch("),
            "IPC must go through execute_action, never straight to a connector"
        );
        assert!(
            !commands.contains("NexusConnector"),
            "the command layer must not name a connector"
        );
    }

    #[test]
    fn the_assistant_core_names_no_specific_application() {
        // NEXUS decides what; connectors decide how. The core has to *name*
        // each connector module in order to register it, so that wiring is
        // excluded; what must never appear is application knowledge in the
        // logic itself.
        let core = production_source(include_str!("mod.rs"));
        let logic: String = core
            .lines()
            .filter(|line| {
                let t = line.trim();
                !t.starts_with("use ")
                    && !t.starts_with("pub mod ")
                    && !t.starts_with("&")
                    && !t.starts_with("//")
            })
            .collect::<Vec<_>>()
            .join("\n");

        for application in [
            "IntelliJ",
            "Chrome",
            "Teams",
            "WhatsApp",
            "osascript",
            "curl",
            "msteams",
            "graph.microsoft",
        ] {
            assert!(
                !logic.contains(application),
                "the core's logic must not know about {application}"
            );
        }
    }

    #[test]
    fn the_core_never_branches_on_a_connector_id() {
        // Registration names connectors; behaviour must not. A `match` on
        // "github" here would be the moment the abstraction stopped paying.
        let core = production_source(include_str!("mod.rs"));
        for id in [
            "\"github\"",
            "\"jira\"",
            "\"teams\"",
            "\"browser\"",
            "\"ide\"",
        ] {
            assert!(
                !core.contains(id),
                "the core must not test for the connector id {id}"
            );
        }
    }
}



