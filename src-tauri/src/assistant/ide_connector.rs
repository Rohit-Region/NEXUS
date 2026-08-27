//! NEXUS-016: the IDE connector.
//!
//! **The registry is the source of truth.** This connector holds no opinion
//! about which editor you use: it reads `ides.executable_path` and launches
//! whatever is there. Adding an editor is a registry row, not a code change,
//! and the Assistant Core never learns that IntelliJ exists.
//!
//! The one place a specific application is named is [`KNOWN_IDES`], used only
//! by `ide.discover` to answer "what is installed on this Mac that you might
//! want to register". That is a suggestion, not a dependency: discovery
//! finding nothing leaves every other action working, and an editor missing
//! from the list can still be registered by hand.
//!
//! **No shell, ever.** Launching is a fixed argument vector, and there is no
//! `ide.run_task` in this milestone. Running a build or a test suite means
//! executing a command, and NEXUS has nowhere to store a vetted one yet.
//! Shipping it would mean either inventing a command or accepting one from a
//! caller, and the second is the escape hatch this architecture exists to
//! avoid.

use rusqlite::Connection;
use serde::Deserialize;

use super::action::{ActionError, ActionSpec};
use super::connector::{
    Capabilities, Connector, ConnectorStatus, ExecCtx, ReferentDraft, UnavailableAction,
};
use super::permission::{ConfirmPolicy, Permission, Reach};
use super::referent::ReferentKind;
use super::shell::{run, RunError, DEFAULT_TIMEOUT};
use crate::db::ides::list_ides;

pub const CONNECTOR_ID: &str = "ide";

/// Editors NEXUS can recognise on disk, for `ide.discover` only.
///
/// `(display name, app bundle, launcher inside the bundle)`. The launcher is
/// preferred because a bundle path has to go through `open`, which cannot
/// pass a line number.
const KNOWN_IDES: &[(&str, &str, &str)] = &[
    (
        "IntelliJ IDEA",
        "/Applications/IntelliJ IDEA.app",
        "Contents/MacOS/idea",
    ),
    (
        "Visual Studio Code",
        "/Applications/Visual Studio Code.app",
        "Contents/Resources/app/bin/code",
    ),
    (
        "Cursor",
        "/Applications/Cursor.app",
        "Contents/Resources/app/bin/cursor",
    ),
    ("Zed", "/Applications/Zed.app", "Contents/MacOS/cli"),
    (
        "PyCharm",
        "/Applications/PyCharm.app",
        "Contents/MacOS/pycharm",
    ),
    (
        "WebStorm",
        "/Applications/WebStorm.app",
        "Contents/MacOS/webstorm",
    ),
];

const fn spec(
    id: &'static str,
    summary: &'static str,
    permission: Permission,
) -> ActionSpec {
    ActionSpec {
        id,
        connector_id: CONNECTOR_ID,
        summary,
        permission,
        confirm: ConfirmPolicy::Never,
        reach: Reach::LocalOnly,
        reversible: true,
    }
}

pub const ACTIONS: &[ActionSpec] = &[
    spec("ide.discover", "Find editors installed on this Mac", Permission::Read),
    spec("ide.list", "List the editors you have registered", Permission::Read),
    spec("ide.status", "Check whether an editor is running", Permission::Read),
    spec("ide.open_project", "Open a project in an editor", Permission::Interact),
    spec("ide.open_file", "Open a file in an editor", Permission::Interact),
    spec("ide.focus", "Bring an editor to the front", Permission::Interact),
];

// -- Typed inputs -------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IdeRef {
    ide_id: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OpenProject {
    ide_id: i64,
    project_id: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OpenFile {
    ide_id: i64,
    path: String,
    #[serde(default)]
    line: Option<u32>,
}

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

/// A registered editor, resolved and checked.
#[derive(Debug)]
struct ResolvedIde {
    name: String,
    executable: String,
}

fn resolve_ide(conn: &Connection, ide_id: i64) -> Result<ResolvedIde, ActionError> {
    let entry = list_ides(conn, false)
        .map_err(|detail| ActionError::Failed { detail })?
        .into_iter()
        .find(|e| e.id == ide_id)
        .ok_or_else(|| ActionError::Failed {
            detail: format!("No editor is registered with id {ide_id}."),
        })?;

    if !entry.enabled {
        return Err(ActionError::Failed {
            detail: format!("{} is turned off in the registry.", entry.name),
        });
    }

    let executable = entry.executable_path.clone().ok_or_else(|| ActionError::Failed {
        detail: format!(
            "{} has no executable path. Add one in the Registry so NEXUS knows what to launch.",
            entry.name
        ),
    })?;

    if !std::path::Path::new(&executable).exists() {
        return Err(ActionError::Failed {
            detail: format!(
                "{} is registered at {executable}, but nothing is there. Fix the path in the Registry.",
                entry.name
            ),
        });
    }

    Ok(ResolvedIde {
        name: entry.name,
        executable,
    })
}

/// Only absolute paths that exist.
///
/// A relative path would resolve against whatever directory NEXUS happens to
/// have been launched from, which is not a thing the user can reason about.
fn checked_path(path: &str, want_dir: bool) -> Result<String, ActionError> {
    let trimmed = path.trim();
    if !trimmed.starts_with('/') {
        return Err(ActionError::InvalidInput {
            detail: format!("NEXUS needs a full path, and {trimmed} is not one."),
        });
    }
    let candidate = std::path::Path::new(trimmed);
    if !candidate.exists() {
        return Err(ActionError::Failed {
            detail: format!("There is nothing at {trimmed}."),
        });
    }
    if want_dir && !candidate.is_dir() {
        return Err(ActionError::InvalidInput {
            detail: format!("{trimmed} is a file, not a project folder."),
        });
    }
    Ok(trimmed.to_string())
}

/// Launch an editor against a target.
///
/// A `.app` bundle has to go through `open`, which cannot pass a line number;
/// a launcher binary takes arguments directly. Preferring the launcher is why
/// discovery reports one.
fn launch(ide: &ResolvedIde, target: &str, line: Option<u32>) -> Result<(), ActionError> {
    let is_bundle = ide.executable.ends_with(".app");

    let result = if is_bundle {
        run(
            "/usr/bin/open",
            &["-a", &ide.executable, target],
            DEFAULT_TIMEOUT,
        )
    } else if let Some(line) = line {
        // Both JetBrains and VS Code accept a line, with different flags.
        // Chosen by what the launcher is called rather than by editor brand,
        // so a rename in the registry cannot break it.
        let launcher = ide.executable.rsplit('/').next().unwrap_or("");
        let line_text = line.to_string();
        if launcher.starts_with("code") || launcher.starts_with("cursor") {
            run(
                &ide.executable,
                &["--goto", &format!("{target}:{line_text}")],
                DEFAULT_TIMEOUT,
            )
        } else {
            run(
                &ide.executable,
                &["--line", &line_text, target],
                DEFAULT_TIMEOUT,
            )
        }
    } else {
        run(&ide.executable, &[target], DEFAULT_TIMEOUT)
    };

    match result {
        Ok(out) if out.success => Ok(()),
        Ok(out) => Err(ActionError::Failed {
            detail: if out.stderr.is_empty() {
                format!("{} would not open {target}.", ide.name)
            } else {
                out.stderr
            },
        }),
        Err(RunError::NotFound { .. }) => Err(ActionError::Failed {
            detail: format!(
                "{} is registered at {}, but it could not be started.",
                ide.name, ide.executable
            ),
        }),
        Err(other) => Err(ActionError::Failed {
            detail: other.to_string(),
        }),
    }
}

/// Whether a process with this name is running.
///
/// `pgrep` rather than AppleScript: it needs no Automation permission, so
/// checking whether an editor is open cannot raise a prompt.
fn is_running(name: &str) -> bool {
    match run("/usr/bin/pgrep", &["-qx", name], DEFAULT_TIMEOUT) {
        Ok(out) => out.success,
        Err(_) => false,
    }
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Discovered {
    name: String,
    app_path: String,
    /// The launcher binary, when the bundle ships one. Preferred for
    /// registration because it can take a line number.
    executable_path: String,
    /// True when this editor already has a registry row pointing at it.
    registered: bool,
}

fn discover(conn: &Connection) -> Vec<Discovered> {
    let registered: Vec<String> = list_ides(conn, false)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| e.executable_path)
        .collect();

    KNOWN_IDES
        .iter()
        .filter(|(_, app, _)| std::path::Path::new(app).exists())
        .map(|(name, app, launcher)| {
            let full = format!("{app}/{launcher}");
            let executable_path = if std::path::Path::new(&full).exists() {
                full
            } else {
                // No launcher in the bundle: registering the bundle still
                // works, it just cannot carry a line number.
                (*app).to_string()
            };
            Discovered {
                name: (*name).to_string(),
                app_path: (*app).to_string(),
                registered: registered.iter().any(|p| p == &executable_path),
                executable_path,
            }
        })
        .collect()
}

fn project_path(conn: &Connection, project_id: i64) -> Option<(String, Option<String>)> {
    conn.query_row(
        "SELECT name, repository_path FROM projects WHERE id = ?1",
        [project_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
    )
    .ok()
}

pub struct IdeConnector;

impl Connector for IdeConnector {
    fn id(&self) -> &'static str {
        CONNECTOR_ID
    }

    fn display_name(&self) -> &'static str {
        "Editors"
    }

    fn actions(&self) -> &'static [ActionSpec] {
        ACTIONS
    }

    fn capabilities(&self, conn: &Connection) -> Capabilities {
        let has_registered = list_ides(conn, true)
            .map(|rows| rows.iter().any(|e| e.executable_path.is_some()))
            .unwrap_or(false);

        if has_registered {
            return Capabilities {
                available: ACTIONS.iter().map(|s| s.id.to_string()).collect(),
                unavailable: Vec::new(),
            };
        }

        // Discovery and listing still work with nothing registered; that is
        // precisely how the user finds out what to register.
        let reason =
            "No editor is registered yet. Run discovery, then add one in the Registry."
                .to_string();
        Capabilities {
            available: vec!["ide.discover".to_string(), "ide.list".to_string()],
            unavailable: ACTIONS
                .iter()
                .filter(|s| s.id != "ide.discover" && s.id != "ide.list")
                .map(|s| UnavailableAction {
                    action_id: s.id.to_string(),
                    reason: reason.clone(),
                })
                .collect(),
        }
    }

    fn status(&self, conn: &Connection) -> ConnectorStatus {
        match list_ides(conn, true) {
            Ok(rows) if rows.iter().any(|e| e.executable_path.is_some()) => {
                ConnectorStatus::Ready
            }
            // Not `Unavailable`: discovery works, and that is the way out.
            _ => ConnectorStatus::Degraded,
        }
    }

    fn summarize(&self, action_id: &str, input: &serde_json::Value, conn: &Connection) -> String {
        let ide_name = |id: i64| {
            list_ides(conn, false)
                .ok()
                .and_then(|rows| rows.into_iter().find(|e| e.id == id).map(|e| e.name))
        };

        match action_id {
            "ide.open_project" => match serde_json::from_value::<OpenProject>(input.clone()) {
                Ok(target) => {
                    let project = project_path(conn, target.project_id)
                        .map(|(name, _)| name)
                        .unwrap_or_else(|| "a project".to_string());
                    match ide_name(target.ide_id) {
                        Some(ide) => format!("Open {project} in {ide}"),
                        None => format!("Open {project} in an editor"),
                    }
                }
                Err(_) => "Open a project in an editor".to_string(),
            },
            "ide.open_file" => match serde_json::from_value::<OpenFile>(input.clone()) {
                Ok(target) => {
                    let file = target.path.rsplit('/').next().unwrap_or(&target.path);
                    match ide_name(target.ide_id) {
                        Some(ide) => format!("Open {file} in {ide}"),
                        None => format!("Open {file}"),
                    }
                }
                Err(_) => "Open a file in an editor".to_string(),
            },
            "ide.focus" | "ide.status" => match serde_json::from_value::<IdeRef>(input.clone())
                .ok()
                .and_then(|r| ide_name(r.ide_id))
            {
                Some(ide) => {
                    if action_id == "ide.focus" {
                        format!("Bring {ide} to the front")
                    } else {
                        format!("Check whether {ide} is running")
                    }
                }
                None => "Check an editor".to_string(),
            },
            other => ACTIONS
                .iter()
                .find(|s| s.id == other)
                .map(|s| s.summary.to_string())
                .unwrap_or_else(|| other.to_string()),
        }
    }

    fn observe(
        &self,
        action_id: &str,
        input: &serde_json::Value,
        _output: &serde_json::Value,
        conn: &Connection,
    ) -> Vec<ReferentDraft> {
        // Opening a project puts it into the conversation, so "what am I
        // working on" and "the project" both find it afterwards.
        if action_id != "ide.open_project" {
            return Vec::new();
        }
        serde_json::from_value::<OpenProject>(input.clone())
            .ok()
            .and_then(|target| {
                project_path(conn, target.project_id).map(|(name, _)| ReferentDraft {
                    kind: ReferentKind::Project,
                    display_name: name,
                    metadata: serde_json::json!({ "id": target.project_id }),
                })
            })
            .into_iter()
            .collect()
    }

    fn validate_input(
        &self,
        action_id: &str,
        input: &serde_json::Value,
    ) -> Result<(), ActionError> {
        match action_id {
            "ide.open_project" => parse::<OpenProject>(input.clone()).map(|_| ()),
            "ide.open_file" => parse::<OpenFile>(input.clone()).map(|_| ()),
            "ide.focus" | "ide.status" => parse::<IdeRef>(input.clone()).map(|_| ()),
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
            "ide.discover" => json(serde_json::json!({ "found": discover(ctx.conn) })),

            "ide.list" => {
                let rows = list_ides(ctx.conn, false)
                    .map_err(|detail| ActionError::Failed { detail })?;
                json(serde_json::json!({ "ides": rows }))
            }

            "ide.status" => {
                let target: IdeRef = parse(input)?;
                let ide = resolve_ide(ctx.conn, target.ide_id)?;
                // The process name is the bundle name, not the launcher's.
                let process = ide
                    .executable
                    .split("/Contents/")
                    .next()
                    .and_then(|app| app.rsplit('/').next())
                    .map(|app| app.trim_end_matches(".app").to_string())
                    .unwrap_or_else(|| ide.name.clone());
                json(serde_json::json!({
                    "name": ide.name,
                    "running": is_running(&process)
                }))
            }

            "ide.focus" => {
                let target: IdeRef = parse(input)?;
                let ide = resolve_ide(ctx.conn, target.ide_id)?;
                let app = ide
                    .executable
                    .split("/Contents/")
                    .next()
                    .unwrap_or(&ide.executable)
                    .to_string();
                let out = run("/usr/bin/open", &["-a", &app], DEFAULT_TIMEOUT).map_err(|e| {
                    ActionError::Failed {
                        detail: e.to_string(),
                    }
                })?;
                if !out.success {
                    return Err(ActionError::Failed {
                        detail: format!("{} would not come to the front.", ide.name),
                    });
                }
                json(serde_json::json!({ "focused": ide.name }))
            }

            "ide.open_project" => {
                let target: OpenProject = parse(input)?;
                let ide = resolve_ide(ctx.conn, target.ide_id)?;
                let (name, path) = project_path(ctx.conn, target.project_id).ok_or_else(|| {
                    ActionError::Failed {
                        detail: format!("No project with id {}.", target.project_id),
                    }
                })?;
                let path = path.ok_or_else(|| ActionError::Failed {
                    detail: format!(
                        "{name} has no repository path. Add one so NEXUS knows what to open."
                    ),
                })?;
                let checked = checked_path(&path, true)?;
                launch(&ide, &checked, None)?;
                json(serde_json::json!({
                    "project": name,
                    "ide": ide.name,
                    "path": checked
                }))
            }

            "ide.open_file" => {
                let target: OpenFile = parse(input)?;
                let ide = resolve_ide(ctx.conn, target.ide_id)?;
                let checked = checked_path(&target.path, false)?;
                launch(&ide, &checked, target.line)?;
                json(serde_json::json!({
                    "ide": ide.name,
                    "path": checked,
                    "line": target.line
                }))
            }

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

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch("PRAGMA foreign_keys = ON;").expect("fk");
        for (_, sql) in MIGRATIONS {
            conn.execute_batch(sql).expect("migrate");
        }
        conn
    }

    fn seed_ide(conn: &Connection, name: &str, path: Option<&str>, enabled: bool) -> i64 {
        conn.execute(
            "INSERT INTO ides (name, ide_type, executable_path, enabled)
             VALUES (?1, 'editor', ?2, ?3)",
            rusqlite::params![name, path, enabled as i64],
        )
        .expect("seed");
        conn.last_insert_rowid()
    }

    // -- The registry is the source of truth ---------------------------------

    #[test]
    fn an_unregistered_editor_cannot_be_launched() {
        let conn = test_conn();
        let err = resolve_ide(&conn, 999).expect_err("must fail");
        assert!(format!("{err:?}").contains("999"), "{err:?}");
    }

    #[test]
    fn a_disabled_editor_is_refused_and_says_why() {
        let conn = test_conn();
        let id = seed_ide(&conn, "Old Editor", Some("/usr/bin/true"), false);
        let err = resolve_ide(&conn, id).expect_err("must fail");
        assert!(format!("{err:?}").contains("turned off"), "{err:?}");
    }

    #[test]
    fn an_editor_with_no_path_names_the_fix() {
        let conn = test_conn();
        let id = seed_ide(&conn, "Pathless", None, true);
        let err = resolve_ide(&conn, id).expect_err("must fail");
        assert!(format!("{err:?}").contains("Registry"), "{err:?}");
    }

    #[test]
    fn an_editor_registered_at_a_missing_path_is_caught_before_launching() {
        // The exact state this machine is in: registry rows pointing at
        // paths that do not exist.
        let conn = test_conn();
        let id = seed_ide(&conn, "UI-TEST-IDE", Some("/Applications/NotReal.app"), true);
        let err = resolve_ide(&conn, id).expect_err("must fail");
        assert!(format!("{err:?}").contains("nothing is there"), "{err:?}");
    }

    #[test]
    fn a_registered_editor_that_exists_resolves() {
        let conn = test_conn();
        let id = seed_ide(&conn, "Echo", Some("/bin/echo"), true);
        let resolved = resolve_ide(&conn, id).expect("resolves");
        assert_eq!(resolved.executable, "/bin/echo");
    }

    // -- Path checking --------------------------------------------------------

    #[test]
    fn a_relative_path_is_refused() {
        let err = checked_path("src/main.rs", false).expect_err("must refuse");
        assert!(format!("{err:?}").contains("full path"), "{err:?}");
    }

    #[test]
    fn a_path_that_does_not_exist_is_refused() {
        assert!(checked_path("/definitely/not/here", false).is_err());
    }

    #[test]
    fn a_file_offered_as_a_project_folder_is_refused() {
        let err = checked_path("/bin/echo", true).expect_err("must refuse");
        assert!(format!("{err:?}").contains("not a project folder"), "{err:?}");
    }

    #[test]
    fn an_existing_directory_and_file_are_accepted() {
        assert!(checked_path("/tmp", true).is_ok());
        assert!(checked_path("/bin/echo", false).is_ok());
    }

    // -- Discovery ------------------------------------------------------------

    #[test]
    fn discovery_reports_only_editors_that_are_actually_installed() {
        let conn = test_conn();
        let found = discover(&conn);
        for entry in &found {
            assert!(
                std::path::Path::new(&entry.app_path).exists(),
                "{} was reported but is not installed",
                entry.name
            );
        }
    }

    #[test]
    fn discovery_prefers_a_launcher_over_the_bundle() {
        // A bundle path has to go through `open`, which cannot pass a line
        // number. The launcher is what makes ide.open_file useful.
        let conn = test_conn();
        for entry in discover(&conn) {
            if entry.executable_path != entry.app_path {
                assert!(
                    entry.executable_path.starts_with(&entry.app_path),
                    "{} points outside its bundle",
                    entry.name
                );
                assert!(std::path::Path::new(&entry.executable_path).exists());
            }
        }
    }

    #[test]
    fn discovery_marks_what_is_already_registered() {
        let conn = test_conn();
        let found = discover(&conn);
        if let Some(first) = found.first() {
            assert!(!first.registered, "nothing is registered in a fresh database");
            seed_ide(&conn, &first.name, Some(&first.executable_path), true);
            let again = discover(&conn);
            assert!(
                again.iter().any(|e| e.registered),
                "a registered editor must be marked"
            );
        }
    }

    #[test]
    fn discovery_works_with_nothing_registered() {
        let conn = test_conn();
        let caps = IdeConnector.capabilities(&conn);
        assert!(caps.available.contains(&"ide.discover".to_string()));
        assert!(caps.available.contains(&"ide.list".to_string()));
        assert!(
            caps.unavailable.iter().any(|u| u.action_id == "ide.open_project"),
            "opening needs an editor, and the reason should say so"
        );
    }

    #[test]
    fn the_connector_is_degraded_not_unavailable_without_an_editor() {
        // Degraded, because discovery is the way out of that state.
        let conn = test_conn();
        assert_eq!(IdeConnector.status(&conn), ConnectorStatus::Degraded);
        seed_ide(&conn, "Echo", Some("/bin/echo"), true);
        assert_eq!(IdeConnector.status(&conn), ConnectorStatus::Ready);
    }

    // -- Registry consistency -------------------------------------------------

    #[test]
    fn no_action_runs_a_command() {
        // The rule: launching an editor is not the same as running a build,
        // and NEXUS has nowhere vetted to keep a build command yet.
        assert!(
            !ACTIONS.iter().any(|s| s.id == "ide.run_task"),
            "ide.run_task must not ship without somewhere to configure it"
        );
        for spec in ACTIONS {
            assert_ne!(
                spec.permission,
                Permission::Execute,
                "{} must not be Execute in this milestone",
                spec.id
            );
        }
    }

    #[test]
    fn no_shell_is_used_to_launch() {
        let production = include_str!("ide_connector.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("the file must keep its test module marker");
        for forbidden in ["/bin/sh", "/bin/bash", "sh\", \"-c"] {
            assert!(!production.contains(forbidden), "found {forbidden}");
        }
    }

    #[test]
    fn the_assistant_core_is_never_told_which_editor() {
        // Application knowledge belongs in the connector. The core must stay
        // ignorant, which its own guard test also checks from the other side.
        let core = include_str!("mod.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("marker");
        for named in ["IntelliJ", "Visual Studio Code", "Cursor", "PyCharm"] {
            assert!(!core.contains(named), "the core must not know about {named}");
        }
    }

    #[test]
    fn action_ids_are_unique_and_namespaced() {
        let mut seen = std::collections::HashSet::new();
        for spec in ACTIONS {
            assert!(seen.insert(spec.id), "duplicate {}", spec.id);
            assert!(spec.id.starts_with("ide."), "{}", spec.id);
            assert_eq!(spec.connector_id, CONNECTOR_ID);
        }
    }

    #[test]
    fn opening_a_project_puts_it_into_the_conversation() {
        let conn = test_conn();
        conn.execute(
            "INSERT INTO projects (name, repository_path) VALUES ('Atlas', '/tmp')",
            [],
        )
        .expect("seed");
        let project = conn.last_insert_rowid();
        let ide = seed_ide(&conn, "Echo", Some("/bin/echo"), true);

        let drafts = IdeConnector.observe(
            "ide.open_project",
            &serde_json::json!({ "ideId": ide, "projectId": project }),
            &serde_json::json!({}),
            &conn,
        );
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].display_name, "Atlas");
        assert_eq!(drafts[0].kind, ReferentKind::Project);
    }
}
