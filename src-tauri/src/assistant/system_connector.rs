//! NEXUS-015 defect H: opening the applications you already have.
//!
//! "Switch to Slack" is the most ordinary thing to ask a desktop assistant,
//! and until now NEXUS could only open editors, because those are the ones
//! the registry knows about.
//!
//! The safety property here is worth stating, because "launch an app by
//! name" sounds like arbitrary execution and must not be: **NEXUS only ever
//! launches something it has already found on disk.** A name is matched
//! against the applications actually installed, and the *discovered path* is
//! what gets executed. A name that matches nothing is refused. There is no
//! path in this file by which a caller-supplied string reaches the system as
//! a command.
//!
//! It is also why this is `Interact` and not `Execute`: bringing an installed
//! application to the front is what the Dock does.

use rusqlite::Connection;
use serde::Deserialize;

use super::action::{ActionError, ActionSpec};
use super::connector::{Capabilities, Connector, ConnectorStatus, ExecCtx};
use super::permission::{ConfirmPolicy, Permission, Reach};
use super::shell::{run, DEFAULT_TIMEOUT};

pub const CONNECTOR_ID: &str = "system";

/// Where macOS keeps applications. Only the top level of each: a bundle
/// nested inside another bundle is a component, not something to launch.
const APP_DIRS: &[&str] = &[
    "/Applications",
    "/Applications/Utilities",
    "/System/Applications",
    "/System/Applications/Utilities",
];

/// Longest list read back before it stops being an answer.
const LIST_CAP: usize = 12;

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        id: "system.list_apps",
        connector_id: CONNECTOR_ID,
        summary: "List the applications installed here",
        permission: Permission::Read,
        confirm: ConfirmPolicy::Never,
        reach: Reach::LocalOnly,
        reversible: true,
    },
    ActionSpec {
        id: "system.open_app",
        connector_id: CONNECTOR_ID,
        summary: "Open or switch to an application",
        permission: Permission::Interact,
        confirm: ConfirmPolicy::Never,
        reach: Reach::LocalOnly,
        reversible: true,
    },
    ActionSpec {
        // NEXUS-026. The thing a remedy points at: showing the user the
        // exact pane rather than reciting a path through System Settings at
        // somebody whose hands are busy.
        id: "system.open_settings_pane",
        connector_id: CONNECTOR_ID,
        summary: "Open a System Settings pane",
        // Showing a settings pane is what a link does. It changes nothing,
        // and the user still has to grant whatever they came to grant.
        permission: Permission::Interact,
        confirm: ConfirmPolicy::Never,
        reach: Reach::LocalOnly,
        reversible: true,
    },
];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AppRef {
    name: String,
}

/// Every application installed, as (display name, full path).
///
/// Read from disk on each call. Caching would mean an app installed while
/// NEXUS is running stays invisible until a restart, which is exactly the
/// kind of staleness that makes an assistant feel broken.
pub fn installed_apps() -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = Vec::new();
    for dir in APP_DIRS {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(name) if name.ends_with(".app") => name.trim_end_matches(".app").to_string(),
                _ => continue,
            };
            if let Some(full) = path.to_str() {
                found.push((name, full.to_string()));
            }
        }
    }
    found.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    found.dedup_by(|a, b| a.0 == b.0);
    found
}

/// Find an installed application by spoken name.
///
/// Exact match first, then a whole-word prefix, and only then a contains.
/// Ordered so "Notes" cannot be beaten by "Notes Plus" simply because the
/// latter was read from disk first.
pub fn find_app(query: &str) -> Option<(String, String)> {
    let wanted = query.trim().to_lowercase();
    if wanted.is_empty() {
        return None;
    }
    let apps = installed_apps();

    apps.iter()
        .find(|(name, _)| name.to_lowercase() == wanted)
        .or_else(|| {
            apps.iter()
                .find(|(name, _)| name.to_lowercase().starts_with(&wanted))
        })
        .or_else(|| {
            apps.iter()
                .find(|(name, _)| name.to_lowercase().contains(&wanted))
        })
        .cloned()
}

/// Settings panes NEXUS is allowed to open, by name.
///
/// **An allowlist, for the same reason `find_app` only launches paths it
/// discovered.** `x-apple.systempreferences:` takes an arbitrary bundle
/// identifier and anchor, and a caller-supplied one is a URL built from
/// somebody's speech. These are the panes a remedy can name, and nothing
/// else reaches `open`.
const SETTINGS_PANES: &[(&str, &str, &str)] = &[
    (
        "accessibility",
        "Privacy & Security > Accessibility",
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
    ),
    (
        "microphone",
        "Privacy & Security > Microphone",
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone",
    ),
    (
        "speech",
        "Privacy & Security > Speech Recognition",
        "x-apple.systempreferences:com.apple.preference.security?Privacy_SpeechRecognition",
    ),
    (
        "full-disk-access",
        "Privacy & Security > Full Disk Access",
        "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles",
    ),
    (
        "automation",
        "Privacy & Security > Automation",
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Automation",
    ),
];

pub fn pane(key: &str) -> Option<(&'static str, &'static str)> {
    SETTINGS_PANES
        .iter()
        .find(|(name, _, _)| *name == key)
        .map(|(_, label, url)| (*label, *url))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PaneRef {
    /// A key from `SETTINGS_PANES`. Never a URL.
    pane: String,
}

pub struct SystemConnector;

impl Connector for SystemConnector {
    fn id(&self) -> &'static str {
        CONNECTOR_ID
    }

    fn display_name(&self) -> &'static str {
        "Applications"
    }

    fn actions(&self) -> &'static [ActionSpec] {
        ACTIONS
    }

    fn capabilities(&self, _conn: &Connection) -> Capabilities {
        Capabilities {
            available: ACTIONS.iter().map(|s| s.id.to_string()).collect(),
            unavailable: Vec::new(),
        }
    }

    fn status(&self, _conn: &Connection) -> ConnectorStatus {
        ConnectorStatus::Ready
    }

    fn zero_input_actions(&self) -> &'static [&'static str] {
        &["system.list_apps"]
    }

    fn summarize(&self, action_id: &str, input: &serde_json::Value, _conn: &Connection) -> String {
        match action_id {
            "system.open_settings_pane" => {
                match serde_json::from_value::<PaneRef>(input.clone())
                    .ok()
                    .and_then(|r| pane(&r.pane))
                {
                    Some((label, _)) => format!("Open {label}"),
                    None => "Open a System Settings pane".to_string(),
                }
            }
            "system.open_app" => match serde_json::from_value::<AppRef>(input.clone()) {
                Ok(target) => match find_app(&target.name) {
                    Some((name, _)) => format!("Open {name}"),
                    None => format!("Open {}", target.name),
                },
                Err(_) => "Open an application".to_string(),
            },
            other => ACTIONS
                .iter()
                .find(|s| s.id == other)
                .map(|s| s.summary.to_string())
                .unwrap_or_else(|| other.to_string()),
        }
    }

    fn describe_result(&self, action_id: &str, output: &serde_json::Value) -> Option<String> {
        match action_id {
            "system.open_settings_pane" => Some(format!(
                "{} is open. Turn NEXUS on there, then say it again.",
                output.get("pane")?.as_str()?
            )),
            "system.open_app" => Some(format!("Opened {}.", output.get("name")?.as_str()?)),
            "system.list_apps" => {
                let apps = output.get("apps")?.as_array()?;
                let names: Vec<&str> = apps
                    .iter()
                    .filter_map(|a| a.as_str())
                    .take(LIST_CAP)
                    .collect();
                let more = apps.len().saturating_sub(names.len());
                let mut text = format!("{} installed: {}", apps.len(), names.join(", "));
                if more > 0 {
                    text.push_str(&format!(", and {more} more"));
                }
                text.push('.');
                Some(text)
            }
            _ => None,
        }
    }

    fn dispatch(
        &self,
        action_id: &str,
        input: serde_json::Value,
        _ctx: &ExecCtx<'_>,
    ) -> Result<serde_json::Value, ActionError> {
        match action_id {
            "system.list_apps" => {
                let apps: Vec<String> =
                    installed_apps().into_iter().map(|(name, _)| name).collect();
                Ok(serde_json::json!({ "apps": apps }))
            }

            "system.open_settings_pane" => {
                let target: PaneRef =
                    serde_json::from_value(input).map_err(|e| ActionError::InvalidInput {
                        detail: e.to_string(),
                    })?;
                // The allowlist is the safety property, and it is the same
                // one `open_app` has: what reaches `open` is a constant from
                // this file, never a string a caller supplied.
                let (label, url) = pane(&target.pane).ok_or_else(|| ActionError::InvalidInput {
                    detail: format!("NEXUS does not open a settings pane called \"{}\".", target.pane),
                })?;

                let out = run("/usr/bin/open", &[url], DEFAULT_TIMEOUT).map_err(|e| {
                    ActionError::Failed {
                        detail: e.to_string(),
                    }
                })?;
                if !out.success {
                    return Err(ActionError::Failed {
                        detail: format!("System Settings would not open {label}."),
                    });
                }
                Ok(serde_json::json!({ "pane": label }))
            }

            "system.open_app" => {
                let target: AppRef =
                    serde_json::from_value(input).map_err(|e| ActionError::InvalidInput {
                        detail: e.to_string(),
                    })?;

                // The safety property: a name that matches nothing installed
                // is refused, and the *discovered path* is what runs. The
                // caller's string never reaches the system.
                let (name, path) = find_app(&target.name).ok_or_else(|| ActionError::Failed {
                    detail: format!("There is no application called \"{}\" here.", target.name),
                })?;

                let out = run("/usr/bin/open", &["-a", &path], DEFAULT_TIMEOUT).map_err(|e| {
                    ActionError::Failed {
                        detail: e.to_string(),
                    }
                })?;
                if !out.success {
                    return Err(ActionError::Failed {
                        detail: format!("{name} would not open."),
                    });
                }
                Ok(serde_json::json!({ "name": name, "path": path }))
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

    #[test]
    fn applications_are_discovered_from_disk() {
        let apps = installed_apps();
        assert!(!apps.is_empty(), "a Mac has applications");
        for (name, path) in &apps {
            assert!(!name.ends_with(".app"), "{name} kept its extension");
            assert!(std::path::Path::new(path).exists(), "{path} does not exist");
        }
    }

    /// An application that is present on any Mac and lives in a directory
    /// this connector actually scans.
    ///
    /// Not Finder: it sits in CoreServices, which is deliberately not
    /// scanned, because that directory is full of components rather than
    /// things a person would ask to open.
    fn an_installed_app() -> (String, String) {
        installed_apps()
            .into_iter()
            .next()
            .expect("a Mac has at least one application")
    }

    #[test]
    fn a_known_application_is_found_by_name() {
        let (name, path) = an_installed_app();
        let found = find_app(&name).expect("the app just discovered is findable");
        assert_eq!(found.0, name);
        assert_eq!(found.1, path);
    }

    #[test]
    fn matching_ignores_case_and_surrounding_space() {
        let (name, _) = an_installed_app();
        assert!(find_app(&name.to_lowercase()).is_some(), "{name}");
        assert!(find_app(&name.to_uppercase()).is_some(), "{name}");
        assert!(find_app(&format!("  {name}  ")).is_some(), "{name}");
    }

    #[test]
    fn a_prefix_is_enough() {
        let (name, _) = an_installed_app();
        if name.chars().count() > 4 {
            let prefix: String = name.chars().take(4).collect();
            assert!(find_app(&prefix).is_some(), "{prefix} should find {name}");
        }
    }

    #[test]
    fn components_are_not_offered_as_applications() {
        // CoreServices is full of bundles nobody asks to open by name.
        let apps = installed_apps();
        assert!(
            apps.iter().all(|(_, path)| !path.contains("CoreServices")),
            "internal components must not be listed"
        );
    }

    #[test]
    fn an_exact_name_beats_a_longer_one_that_contains_it() {
        // Ordering matters: "Notes" must not resolve to "Notes Plus" simply
        // because that was read from disk first.
        let apps = installed_apps();
        if let Some((exact, _)) = apps.iter().find(|(n, _)| {
            apps.iter()
                .any(|(other, _)| other != n && other.to_lowercase().contains(&n.to_lowercase()))
        }) {
            assert_eq!(
                find_app(exact).expect("found").0,
                *exact,
                "an exact match must win"
            );
        }
    }

    #[test]
    fn a_name_that_matches_nothing_is_refused() {
        assert!(find_app("DefinitelyNotInstalledXYZ").is_none());
        assert!(find_app("").is_none());
        assert!(find_app("   ").is_none());
    }

    #[test]
    fn a_caller_string_never_reaches_the_system() {
        // The property that keeps this from being arbitrary execution.
        for hostile in [
            "../../bin/sh",
            "Finder; rm -rf /",
            "$(whoami)",
            "/System/Applications/Utilities/Terminal.app; echo x",
        ] {
            match find_app(hostile) {
                None => {}
                Some((_, path)) => assert!(
                    std::path::Path::new(&path).exists() && path.ends_with(".app"),
                    "{hostile} resolved to something that is not an installed app: {path}"
                ),
            }
        }
    }

    #[test]
    fn opening_an_application_is_interact_not_execute() {
        // Bringing an installed app to the front is what the Dock does.
        let open = ACTIONS
            .iter()
            .find(|s| s.id == "system.open_app")
            .expect("present");
        assert_eq!(open.permission, Permission::Interact);
        assert_eq!(open.reach, Reach::LocalOnly);
        assert_eq!(open.confirm, ConfirmPolicy::Never);
    }

    #[test]
    fn no_shell_and_no_raw_name_is_executed() {
        let production = include_str!("system_connector.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("marker");
        assert!(!production.contains("/bin/sh"), "no shell");
        assert!(
            !production.contains("&target.name]") && !production.contains("&[\"-a\", &target.name"),
            "the caller's name must never be executed"
        );
        assert!(
            production.contains("&[\"-a\", &path]"),
            "the discovered path runs"
        );
    }

    #[test]
    fn action_ids_are_unique_and_namespaced() {
        let mut seen = std::collections::HashSet::new();
        for spec in ACTIONS {
            assert!(seen.insert(spec.id), "duplicate {}", spec.id);
            assert!(spec.id.starts_with("system."), "{}", spec.id);
            assert_eq!(spec.connector_id, CONNECTOR_ID);
        }
    }
}
