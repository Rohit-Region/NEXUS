//! NEXUS-012: the local connector.
//!
//! NEXUS acting on its own workspace. It exists first because it makes the
//! abstraction answerable against code that already works: every action here
//! delegates to a `db::` function that has been shipping since NEXUS-003, so
//! a bug in this file is a bug in the action layer and nowhere else.
//!
//! Navigation actions do not navigate. They return a directive the shell
//! applies. Keeping that as a returned value rather than a side effect is
//! what lets a navigation be gated, audited and reported like anything else.

use rusqlite::Connection;
use serde::Deserialize;

use crate::db::projects::delete_project;
use crate::db::tasks::{delete_task, insert_task, CreateTaskInput};

use super::action::{ActionError, ActionSpec, DeletedOutput, NavigateOutput};
use super::connector::{Capabilities, Connector, ConnectorStatus, ExecCtx, ReferentDraft};
use super::permission::{ConfirmPolicy, Permission, Reach};
use super::referent::ReferentKind;

pub const CONNECTOR_ID: &str = "nexus";

/// Shorthand for the many actions that only tell the shell where to go.
const fn navigate(id: &'static str, summary: &'static str) -> ActionSpec {
    ActionSpec {
        id,
        connector_id: CONNECTOR_ID,
        summary,
        permission: Permission::Interact,
        confirm: ConfirmPolicy::Never,
        reach: Reach::LocalOnly,
        reversible: true,
    }
}

pub const ACTIONS: &[ActionSpec] = &[
    navigate("nexus.open_overview", "Open Overview"),
    navigate("nexus.open_projects", "Open Projects"),
    navigate("nexus.open_registry", "Open Registry"),
    navigate("nexus.open_settings", "Open Settings"),
    navigate("nexus.open_project", "Open a project"),
    navigate("nexus.new_project", "Start a new project"),
    navigate("nexus.new_task", "Start a new task"),
    ActionSpec {
        // NEXUS-028. Confirmed, like every other write, and here the
        // confirmation earns its keep twice: it is also NEXUS repeating back
        // what it heard before agreeing to bring it up later.
        id: "nexus.remember",
        connector_id: CONNECTOR_ID,
        summary: "Remember something for later",
        permission: Permission::Write,
        confirm: ConfirmPolicy::Always,
        reach: Reach::LocalOnly,
        reversible: true,
    },
    ActionSpec {
        id: "nexus.list_commitments",
        connector_id: CONNECTOR_ID,
        summary: "List what you said you would do",
        permission: Permission::Read,
        confirm: ConfirmPolicy::Never,
        reach: Reach::LocalOnly,
        reversible: true,
    },
    ActionSpec {
        id: "nexus.settle_commitment",
        connector_id: CONNECTOR_ID,
        summary: "Mark something done or drop it",
        permission: Permission::Write,
        confirm: ConfirmPolicy::Always,
        reach: Reach::LocalOnly,
        reversible: true,
    },
    ActionSpec {
        id: "nexus.create_task",
        connector_id: CONNECTOR_ID,
        summary: "Create a task",
        permission: Permission::Write,
        confirm: ConfirmPolicy::Always,
        reach: Reach::LocalOnly,
        // Reversible in the sense that matters to someone reading the prompt:
        // the task can be deleted afterwards.
        reversible: true,
    },
    ActionSpec {
        id: "nexus.delete_task",
        connector_id: CONNECTOR_ID,
        summary: "Delete a task",
        permission: Permission::Destructive,
        confirm: ConfirmPolicy::Always,
        reach: Reach::LocalOnly,
        reversible: false,
    },
    ActionSpec {
        id: "nexus.delete_project",
        connector_id: CONNECTOR_ID,
        summary: "Delete a project and all of its tasks",
        permission: Permission::Destructive,
        confirm: ConfirmPolicy::Always,
        reach: Reach::LocalOnly,
        reversible: false,
    },
];

// -- Typed inputs -------------------------------------------------------------
//
// `deny_unknown_fields` on every one of them. A caller that invents a field
// is rejected rather than silently ignored, which is the property that makes
// a generated plan safe to deserialise later.

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProjectRef {
    project_id: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RowRef {
    id: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Remember {
    /// Verbatim. NEXUS repeats what was said, not its summary of it.
    what: String,
    /// Minutes from now. Absent means someday: recorded, never raised, and
    /// visible where the user can act on it themselves.
    #[serde(default)]
    due_in_minutes: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Settle {
    id: i64,
    /// `done` or `dropped`. Validated in the store, not here, so the rule
    /// lives in one place.
    state: String,
}

/// How far "later" pushes a commitment.
///
/// Long enough to finish what interrupted you, short enough that "later"
/// still means today.
const DEFER_MINUTES: i64 = 30;

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateTask {
    project_id: i64,
    title: String,
    #[serde(default)]
    description: Option<String>,
}

/// Deserialise into the action's own input type.
///
/// This IS the validation step. There is no separate schema to keep in step
/// with the struct, and the error names the field, which is what a reasoning
/// provider needs in order to correct itself.
fn parse<T: serde::de::DeserializeOwned>(input: serde_json::Value) -> Result<T, ActionError> {
    serde_json::from_value(input).map_err(|e| ActionError::InvalidInput {
        detail: e.to_string(),
    })
}

fn json<T: serde::Serialize>(value: T) -> Result<serde_json::Value, ActionError> {
    serde_json::to_value(value).map_err(|e| ActionError::Failed {
        detail: format!("Could not encode the result: {e}"),
    })
}

// -- Naming for the approval prompt -------------------------------------------

fn project_name(conn: &Connection, id: i64) -> Option<String> {
    conn.query_row("SELECT name FROM projects WHERE id = ?1", [id], |row| {
        row.get::<_, String>(0)
    })
    .ok()
}

fn task_title(conn: &Connection, id: i64) -> Option<String> {
    conn.query_row("SELECT title FROM tasks WHERE id = ?1", [id], |row| {
        row.get::<_, String>(0)
    })
    .ok()
}

/// How many tasks a project deletion would take with it.
///
/// `tasks.project_id` cascades, so deleting a project is quietly a bulk
/// delete. The prompt says so rather than letting the user find out.
fn task_count(conn: &Connection, project_id: i64) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM tasks WHERE project_id = ?1",
        [project_id],
        |row| row.get::<_, i64>(0),
    )
    .unwrap_or(0)
}

pub struct NexusConnector;

impl Connector for NexusConnector {
    fn id(&self) -> &'static str {
        CONNECTOR_ID
    }

    fn display_name(&self) -> &'static str {
        "NEXUS Workspace"
    }

    fn actions(&self) -> &'static [ActionSpec] {
        ACTIONS
    }

    fn capabilities(&self, _conn: &Connection) -> Capabilities {
        // The local workspace is always fully available: its dependency is
        // the database, and without that NEXUS has not started.
        Capabilities {
            available: ACTIONS.iter().map(|spec| spec.id.to_string()).collect(),
            unavailable: Vec::new(),
        }
    }

    fn status(&self, _conn: &Connection) -> ConnectorStatus {
        ConnectorStatus::Ready
    }

    fn summarize(&self, action_id: &str, input: &serde_json::Value, conn: &Connection) -> String {
        // Names, not row ids. "Delete project 41" tells the user nothing they
        // can check; "Delete project Atlas and its 6 tasks" does.
        match action_id {
            "nexus.open_project" => match serde_json::from_value::<ProjectRef>(input.clone())
                .ok()
                .and_then(|p| project_name(conn, p.project_id))
            {
                Some(name) => format!("Open project {name}"),
                None => "Open a project".to_string(),
            },
            "nexus.new_task" => match serde_json::from_value::<ProjectRef>(input.clone())
                .ok()
                .and_then(|p| project_name(conn, p.project_id))
            {
                Some(name) => format!("Start a new task in {name}"),
                None => "Start a new task".to_string(),
            },
            "nexus.remember" => match serde_json::from_value::<Remember>(input.clone()) {
                // The user's own words in the prompt, because that is the
                // thing being agreed to and a paraphrase would hide a
                // mishearing at the one moment it can still be caught.
                Ok(r) => match r.due_in_minutes {
                    Some(mins) => format!("Remind you to \"{}\" in {mins} minutes", r.what.trim()),
                    None => format!("Remember \"{}\"", r.what.trim()),
                },
                Err(_) => "Remember something".to_string(),
            },
            "nexus.settle_commitment" => match serde_json::from_value::<Settle>(input.clone()) {
                Ok(s) if s.state == "done" => "Mark that done".to_string(),
                Ok(s) if s.state == "later" => {
                    format!("Bring that back in {DEFER_MINUTES} minutes")
                }
                Ok(_) => "Drop that".to_string(),
                Err(_) => "Settle a commitment".to_string(),
            },
            "nexus.create_task" => match serde_json::from_value::<CreateTask>(input.clone()) {
                Ok(task) => match project_name(conn, task.project_id) {
                    Some(name) => format!("Create task \"{}\" in {name}", task.title),
                    None => format!("Create task \"{}\"", task.title),
                },
                Err(_) => "Create a task".to_string(),
            },
            "nexus.delete_task" => match serde_json::from_value::<RowRef>(input.clone())
                .ok()
                .and_then(|r| task_title(conn, r.id))
            {
                Some(title) => format!("Delete task \"{title}\""),
                None => "Delete a task".to_string(),
            },
            "nexus.delete_project" => match serde_json::from_value::<RowRef>(input.clone()) {
                Ok(row) => match project_name(conn, row.id) {
                    Some(name) => {
                        let tasks = task_count(conn, row.id);
                        if tasks == 1 {
                            format!("Delete project {name} and its 1 task")
                        } else if tasks > 1 {
                            format!("Delete project {name} and its {tasks} tasks")
                        } else {
                            format!("Delete project {name}")
                        }
                    }
                    None => "Delete a project".to_string(),
                },
                Err(_) => "Delete a project".to_string(),
            },
            other => ACTIONS
                .iter()
                .find(|spec| spec.id == other)
                .map(|spec| spec.summary.to_string())
                .unwrap_or_else(|| other.to_string()),
        }
    }

    fn observe(
        &self,
        action_id: &str,
        input: &serde_json::Value,
        output: &serde_json::Value,
        conn: &Connection,
    ) -> Vec<ReferentDraft> {
        // A project the user is now looking at is worth remembering; the
        // Settings screen is not. Something just deleted certainly is not.
        let project_draft = |id: i64| {
            project_name(conn, id).map(|name| ReferentDraft {
                kind: ReferentKind::Project,
                display_name: name,
                metadata: serde_json::json!({ "id": id }),
            })
        };

        match action_id {
            "nexus.open_project" | "nexus.new_task" => {
                serde_json::from_value::<ProjectRef>(input.clone())
                    .ok()
                    .and_then(|p| project_draft(p.project_id))
                    .into_iter()
                    .collect()
            }
            "nexus.create_task" => {
                let mut drafts = Vec::new();
                if let (Some(id), Some(title)) = (
                    output.get("id").and_then(|v| v.as_i64()),
                    output.get("title").and_then(|v| v.as_str()),
                ) {
                    drafts.push(ReferentDraft {
                        kind: ReferentKind::Task,
                        display_name: title.to_string(),
                        metadata: serde_json::json!({ "id": id }),
                    });
                }
                if let Some(draft) = output
                    .get("projectId")
                    .and_then(|v| v.as_i64())
                    .and_then(project_draft)
                {
                    drafts.push(draft);
                }
                drafts
            }
            _ => Vec::new(),
        }
    }

    fn validate_input(
        &self,
        action_id: &str,
        input: &serde_json::Value,
    ) -> Result<(), ActionError> {
        match action_id {
            "nexus.open_project" | "nexus.new_task" => {
                parse::<ProjectRef>(input.clone()).map(|_| ())
            }
            "nexus.create_task" => parse::<CreateTask>(input.clone()).map(|_| ()),
            "nexus.delete_task" | "nexus.delete_project" => {
                parse::<RowRef>(input.clone()).map(|_| ())
            }
            _ => Ok(()),
        }
    }

    fn dispatch(
        &self,
        action_id: &str,
        input: serde_json::Value,
        ctx: &ExecCtx<'_>,
    ) -> Result<serde_json::Value, ActionError> {
        match action_id {
            "nexus.remember" => {
                let target: Remember =
                    serde_json::from_value(input).map_err(|e| ActionError::InvalidInput {
                        detail: e.to_string(),
                    })?;
                let due = target.due_in_minutes.map(|m| unix_now() + m * 60);
                let made = crate::db::commitments::create(ctx.conn, &target.what, due)
                    .map_err(|detail| ActionError::Failed { detail })?;
                Ok(serde_json::json!({ "id": made.id, "what": made.what, "dueAt": made.due_at }))
            }

            "nexus.list_commitments" => {
                let open = crate::db::commitments::list(ctx.conn, true)
                    .map_err(|detail| ActionError::Failed { detail })?;
                Ok(serde_json::json!({ "commitments": open }))
            }

            "nexus.settle_commitment" => {
                let target: Settle =
                    serde_json::from_value(input).map_err(|e| ActionError::InvalidInput {
                        detail: e.to_string(),
                    })?;
                // "later" is the user saying yes, not now. It is not a state
                // a commitment rests in, so it moves the time instead and
                // clears `raised_at`: without that, a commitment raised once
                // could never come back and "later" would quietly mean never.
                if target.state == "later" {
                    crate::db::commitments::defer(ctx.conn, target.id, unix_now() + DEFER_MINUTES * 60)
                        .map_err(|detail| ActionError::Failed { detail })?;
                    return Ok(serde_json::json!({ "id": target.id, "state": "later" }));
                }
                crate::db::commitments::set_state(ctx.conn, target.id, &target.state)
                    .map_err(|detail| ActionError::Failed { detail })?;
                Ok(serde_json::json!({ "id": target.id, "state": target.state }))
            }

            "nexus.open_overview" => json(NavigateOutput::screen("overview")),
            "nexus.open_projects" => json(NavigateOutput::screen("projects")),
            "nexus.open_registry" => json(NavigateOutput::screen("registry")),
            "nexus.open_settings" => json(NavigateOutput::screen("settings")),

            "nexus.open_project" => {
                let target: ProjectRef = parse(input)?;
                json(NavigateOutput {
                    screen: "project-detail".to_string(),
                    project_id: Some(target.project_id),
                    intent: None,
                })
            }

            "nexus.new_project" => json(NavigateOutput {
                screen: "projects".to_string(),
                project_id: None,
                intent: Some("create-project".to_string()),
            }),

            "nexus.new_task" => {
                let target: ProjectRef = parse(input)?;
                json(NavigateOutput {
                    screen: "project-detail".to_string(),
                    project_id: Some(target.project_id),
                    intent: Some("create-task".to_string()),
                })
            }

            "nexus.create_task" => {
                let task: CreateTask = parse(input)?;
                let created = insert_task(
                    ctx.conn,
                    &CreateTaskInput {
                        project_id: task.project_id,
                        title: task.title,
                        description: task.description,
                        status: None,
                    },
                )
                .map_err(|detail| ActionError::Failed { detail })?;
                json(created)
            }

            "nexus.delete_task" => {
                let target: RowRef = parse(input)?;
                delete_task(ctx.conn, target.id)
                    .map_err(|detail| ActionError::Failed { detail })?;
                json(DeletedOutput { id: target.id })
            }

            "nexus.delete_project" => {
                let target: RowRef = parse(input)?;
                delete_project(ctx.conn, target.id)
                    .map_err(|detail| ActionError::Failed { detail })?;
                json(DeletedOutput { id: target.id })
            }

            // Unreachable through the gate, which checks the spec first.
            // Handled anyway so adding a spec without a handler fails loudly
            // rather than doing nothing.
            other => Err(ActionError::UnknownAction {
                action_id: other.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrations::MIGRATIONS;
    use serde_json::json as j;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch("PRAGMA foreign_keys = ON;").expect("fk");
        for (_, sql) in MIGRATIONS {
            conn.execute_batch(sql).expect("migrate");
        }
        conn
    }

    fn seed_project(conn: &Connection, name: &str) -> i64 {
        conn.execute("INSERT INTO projects (name) VALUES (?1)", [name])
            .expect("seed project");
        conn.last_insert_rowid()
    }

    fn seed_task(conn: &Connection, project_id: i64, title: &str) -> i64 {
        conn.execute(
            "INSERT INTO tasks (project_id, title) VALUES (?1, ?2)",
            rusqlite::params![project_id, title],
        )
        .expect("seed task");
        conn.last_insert_rowid()
    }

    fn ctx(conn: &Connection) -> ExecCtx<'_> {
        ExecCtx { conn }
    }

    #[test]
    fn every_spec_has_a_handler() {
        // Guards the gap the catch-all arm exists to expose: a spec added
        // without a handler would otherwise be a silent no-op.
        for spec in ACTIONS {
            // A fresh workspace per action, so a destructive one cannot
            // remove the fixture the next needs.
            let conn = test_conn();
            let project = seed_project(&conn, "Atlas");
            let task = seed_task(&conn, project, "Ship it");

            let input = match spec.id {
                "nexus.open_project" | "nexus.new_task" => j!({ "projectId": project }),
                "nexus.create_task" => j!({ "projectId": project, "title": "From test" }),
                "nexus.delete_task" => j!({ "id": task }),
                "nexus.delete_project" => j!({ "id": project }),
                "nexus.remember" => j!({ "what": "call the dentist", "dueInMinutes": 30 }),
                "nexus.settle_commitment" => {
                    let made = crate::db::commitments::create(&conn, "x", None).expect("seed");
                    j!({ "id": made.id, "state": "done" })
                }
                _ => j!(null),
            };

            let result = NexusConnector.dispatch(spec.id, input, &ctx(&conn));
            assert!(
                result.is_ok(),
                "{} has no working handler: {:?}",
                spec.id,
                result.err()
            );
        }
    }

    #[test]
    fn navigation_returns_a_directive_and_changes_nothing() {
        let conn = test_conn();
        let before = task_count(&conn, 1);
        let out = NexusConnector
            .dispatch("nexus.open_settings", j!(null), &ctx(&conn))
            .expect("dispatch");
        assert_eq!(out, j!({ "screen": "settings" }));
        assert_eq!(task_count(&conn, 1), before);
    }

    #[test]
    fn opening_a_project_carries_the_id_through() {
        let conn = test_conn();
        let out = NexusConnector
            .dispatch("nexus.open_project", j!({ "projectId": 41 }), &ctx(&conn))
            .expect("dispatch");
        assert_eq!(out, j!({ "screen": "project-detail", "projectId": 41 }));
    }

    #[test]
    fn an_unknown_field_is_rejected_rather_than_ignored() {
        // The property that makes a generated plan safe to deserialise: a
        // caller that invents a field gets told, not silently obeyed.
        let conn = test_conn();
        let err = NexusConnector
            .dispatch(
                "nexus.open_project",
                j!({ "projectId": 1, "force": true }),
                &ctx(&conn),
            )
            .expect_err("must reject");
        assert!(matches!(err, ActionError::InvalidInput { .. }), "{err:?}");
    }

    #[test]
    fn a_wrongly_typed_field_is_rejected() {
        let conn = test_conn();
        let err = NexusConnector
            .dispatch("nexus.delete_task", j!({ "id": "seven" }), &ctx(&conn))
            .expect_err("must reject");
        assert!(matches!(err, ActionError::InvalidInput { .. }), "{err:?}");
    }

    #[test]
    fn a_missing_required_field_is_rejected() {
        let conn = test_conn();
        let err = NexusConnector
            .dispatch("nexus.create_task", j!({ "projectId": 1 }), &ctx(&conn))
            .expect_err("must reject");
        assert!(matches!(err, ActionError::InvalidInput { .. }), "{err:?}");
    }

    #[test]
    fn creating_a_task_delegates_to_the_existing_insert() {
        let conn = test_conn();
        let project = seed_project(&conn, "Atlas");
        let out = NexusConnector
            .dispatch(
                "nexus.create_task",
                j!({ "projectId": project, "title": "Ship it" }),
                &ctx(&conn),
            )
            .expect("dispatch");
        assert_eq!(out["title"], "Ship it");
        assert_eq!(task_count(&conn, project), 1);
    }

    #[test]
    fn creating_a_task_for_a_missing_project_fails_at_the_foreign_key() {
        let conn = test_conn();
        let err = NexusConnector
            .dispatch(
                "nexus.create_task",
                j!({ "projectId": 999, "title": "Orphan" }),
                &ctx(&conn),
            )
            .expect_err("must fail");
        assert!(matches!(err, ActionError::Failed { .. }), "{err:?}");
    }

    #[test]
    fn deleting_a_project_takes_its_tasks_with_it() {
        let conn = test_conn();
        let project = seed_project(&conn, "Atlas");
        seed_task(&conn, project, "One");
        seed_task(&conn, project, "Two");

        NexusConnector
            .dispatch("nexus.delete_project", j!({ "id": project }), &ctx(&conn))
            .expect("dispatch");
        assert_eq!(task_count(&conn, project), 0);
    }

    #[test]
    fn the_summary_names_the_target_not_its_row_id() {
        let conn = test_conn();
        let project = seed_project(&conn, "Atlas");
        let summary =
            NexusConnector.summarize("nexus.delete_project", &j!({ "id": project }), &conn);
        assert!(summary.contains("Atlas"), "{summary}");
        assert!(
            !summary.contains(&project.to_string()),
            "row id leaked into the prompt: {summary}"
        );
    }

    #[test]
    fn deleting_a_project_warns_how_many_tasks_go_with_it() {
        // The cascade is invisible in the UI, so the prompt has to say it.
        let conn = test_conn();
        let project = seed_project(&conn, "Atlas");
        seed_task(&conn, project, "One");
        seed_task(&conn, project, "Two");
        let summary =
            NexusConnector.summarize("nexus.delete_project", &j!({ "id": project }), &conn);
        assert!(summary.contains("2 tasks"), "{summary}");

        let empty = seed_project(&conn, "Beta");
        let plain = NexusConnector.summarize("nexus.delete_project", &j!({ "id": empty }), &conn);
        assert!(!plain.contains("task"), "{plain}");

        let single = seed_project(&conn, "Gamma");
        seed_task(&conn, single, "Only");
        let one = NexusConnector.summarize("nexus.delete_project", &j!({ "id": single }), &conn);
        assert!(one.contains("1 task") && !one.contains("1 tasks"), "{one}");
    }

    #[test]
    fn a_summary_for_a_deleted_row_degrades_instead_of_panicking() {
        let conn = test_conn();
        let summary = NexusConnector.summarize("nexus.delete_task", &j!({ "id": 999 }), &conn);
        assert_eq!(summary, "Delete a task");
    }

    #[test]
    fn a_summary_for_malformed_input_degrades_to_the_static_text() {
        let conn = test_conn();
        let summary = NexusConnector.summarize("nexus.create_task", &j!({ "nonsense": 1 }), &conn);
        assert_eq!(summary, "Create a task");
    }

    #[test]
    fn every_destructive_action_is_marked_irreversible() {
        for spec in ACTIONS {
            if spec.permission == Permission::Destructive {
                assert!(
                    !spec.reversible,
                    "{} is destructive but claims to be reversible",
                    spec.id
                );
            }
        }
    }

    #[test]
    fn action_ids_are_unique_and_namespaced() {
        let mut seen = std::collections::HashSet::new();
        for spec in ACTIONS {
            assert!(seen.insert(spec.id), "duplicate action id {}", spec.id);
            assert!(
                spec.id.starts_with("nexus."),
                "{} is not namespaced to its connector",
                spec.id
            );
            assert_eq!(spec.connector_id, CONNECTOR_ID);
        }
    }

    #[test]
    fn the_local_connector_never_leaves_the_machine() {
        for spec in ACTIONS {
            assert_eq!(
                spec.reach,
                Reach::LocalOnly,
                "{} claims to leave the machine",
                spec.id
            );
        }
    }
}
