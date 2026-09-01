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
//! **No shell, ever.** Launching is a fixed argument vector, and there is
//! still no `ide.run_task`. Running a build means executing a command, NEXUS
//! has nowhere vetted to keep one, and shipping it would mean either
//! inventing a command or accepting one from a caller. The second is the
//! escape hatch this architecture exists to avoid, and that has not changed.
//!
//! **Dictation is a different shape, which is why it is allowed.**
//! `ide.type_prompt` types what you said into the box you have focused and
//! stops. `ide.submit_prompt` presses Return, as a separate `Execute` action
//! with its own confirmation. NEXUS never composes a command, never stores
//! one, and never decides what the words mean; it moves your sentence to
//! your editor and the editor decides. The two steps exist so a misheard
//! sentence is visible on screen before anything acts on it.
//!
//! **Which box receives the text is settled by putting the cursor there.**
//! Electron publishes no `AXFocusedUIElement` unless a screen reader is
//! running, so NEXUS cannot *read* where focus is. It can *place* it: the
//! Claude Code extension registers "Claude Code: Focus input", and running
//! that through the Command Palette is unconditional. The keyboard shortcut
//! is not -- `Cmd+Escape` is bound to focus *and* blur, chosen by whether a
//! code editor has focus, so sending it blind closes Claude half the time.
//!
//! Where the extension is missing, dictation is refused rather than
//! attempted. A palette with no matching command stays open, and the user's
//! sentence would be typed into it; the confirmed Return after that would
//! run whatever it had matched. That is somebody's spoken words executed as
//! an editor command, and no amount of confirmation makes it acceptable.

use rusqlite::Connection;
use serde::Deserialize;

use super::action::{ActionError, ActionSpec};
use super::connector::{
    Capabilities, Connector, ConnectorStatus, ExecCtx, FollowUp, ReferentDraft, Remedy,
    UnavailableAction,
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

const fn spec(id: &'static str, summary: &'static str, permission: Permission) -> ActionSpec {
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
    spec(
        "ide.discover",
        "Find editors installed on this Mac",
        Permission::Read,
    ),
    spec(
        "ide.list",
        "List the editors you have registered",
        Permission::Read,
    ),
    spec(
        "ide.status",
        "Check whether an editor is running",
        Permission::Read,
    ),
    spec(
        "ide.open_project",
        "Open a project in an editor",
        Permission::Interact,
    ),
    spec(
        "ide.open_file",
        "Open a file in an editor",
        Permission::Interact,
    ),
    spec(
        "ide.focus",
        "Bring an editor to the front",
        Permission::Interact,
    ),
    ActionSpec {
        // Dictating into an editor. Types and stops: the text lands in
        // whatever box has focus and nothing runs until `ide.submit_prompt`
        // is confirmed separately, which is what makes a misheard sentence
        // recoverable by looking at it.
        id: "ide.type_prompt",
        connector_id: CONNECTOR_ID,
        // Names Claude, which it may now do: the action focuses Claude's
        // prompt box itself rather than typing into whatever happened to
        // have focus, and refuses outright where it cannot find it.
        summary: "Type a prompt into Claude in an editor",
        permission: Permission::Write,
        confirm: ConfirmPolicy::Always,
        reach: Reach::LocalOnly,
        // The text is on screen and can be cleared by hand.
        reversible: true,
    },
    ActionSpec {
        id: "ide.submit_prompt",
        connector_id: CONNECTOR_ID,
        summary: "Run what is typed in the editor",
        // `Execute` rather than `Write`, and the distinction is the whole
        // point of splitting this in two: typing puts characters on screen,
        // pressing Return hands them to something that acts on them.
        permission: Permission::Execute,
        confirm: ConfirmPolicy::Always,
        reach: Reach::LocalOnly,
        // Whatever it starts is already started.
        reversible: false,
    },
];

/// The command the Claude Code extension registers for putting the cursor in
/// its prompt box, exactly as it appears in the Command Palette.
///
/// **The palette rather than the keyboard shortcut, and the reason matters.**
/// The extension binds `Cmd+Escape` to *two* commands: `claude-vscode.focus`
/// when a code editor has focus, and `claude-vscode.blur` when it does not.
/// Sending that key blind is a coin flip that closes Claude as often as it
/// opens it. Running the command by name carries no such condition.
const CLAUDE_FOCUS_COMMAND: &str = "Claude Code: Focus input";

/// The command that starts a fresh conversation.
///
/// Also palette-only, and for a second reason on top of the first: its
/// `Cmd+N` binding is gated on `claudeCode.enableNewConversationShortcut`,
/// which ships **off**. The shortcut is therefore dead on a default install,
/// while the command itself carries no condition at all.
const CLAUDE_NEW_CHAT_COMMAND: &str = "Claude Code: New Conversation";

/// How long the Command Palette is given to open, filter, and close.
///
/// Generous on purpose. Every one of these is a race NEXUS cannot observe
/// the result of: Electron publishes no accessibility tree, so a palette
/// that had not finished filtering when Return arrived would run whichever
/// command was highlighted at that moment. Waiting is the only instrument
/// available, so it is set well past what the editor needs rather than at
/// the edge of it.
const PALETTE_SETTLE: &str = "delay 0.7";
const COMMAND_SETTLE: &str = "delay 1.0";

/// Where each editor keeps its extensions, so NEXUS can tell whether Claude
/// Code is actually installed there.
///
/// **This check is load-bearing, not a nicety.** Without it, an editor
/// without the extension leaves the Command Palette open with no match, and
/// the dictated sentence is typed into the palette instead of into Claude.
/// The next confirmed Return would then run whatever the palette had matched
/// by then, which is somebody's spoken words executed as an editor command.
/// Refusing early is the only honest option.
///
/// Keyed by process name, which is what the bundle resolves to. An editor
/// absent from this list is not refused for being unknown; it simply has no
/// known place to look.
const EXTENSION_DIRS: &[(&str, &str)] = &[("Code", ".vscode"), ("Cursor", ".cursor")];

/// Whether the Claude Code extension is installed for this editor.
///
/// `None` means NEXUS has no way to tell for that editor, which is different
/// from `Some(false)` and is treated differently by the caller.
fn claude_installed(process: &str) -> Option<bool> {
    let dir = EXTENSION_DIRS
        .iter()
        .find(|(name, _)| *name == process)
        .map(|(_, dir)| *dir)?;
    let home = std::env::var("HOME").ok()?;
    let extensions = std::path::Path::new(&home).join(dir).join("extensions");
    Some(
        std::fs::read_dir(extensions)
            .map(|entries| {
                entries.filter_map(|e| e.ok()).any(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with("anthropic.claude-code")
                })
            })
            .unwrap_or(false),
    )
}

/// Longest dictated prompt NEXUS will type.
///
/// Synthetic typing is slow and cannot be interrupted once it starts, so an
/// accidental paragraph is a wait the user cannot escape.
const MAX_PROMPT: usize = 2_000;

// -- Typed inputs -------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IdeRef {
    ide_id: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TypePrompt {
    ide_id: i64,
    text: String,
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

    let executable = entry
        .executable_path
        .clone()
        .ok_or_else(|| ActionError::Failed {
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
/// The name the running process actually has, read from the bundle.
///
/// **Not the bundle's own name, which is what this used to guess.** The
/// bundle is "Visual Studio Code.app" and the process is `Code`; IntelliJ's
/// is `idea`. Deriving the process name from the folder therefore reported
/// VS Code as not running while it was plainly open, and any check built on
/// that comparison was answering about a process that does not exist.
///
/// `CFBundleExecutable` is the authoritative answer, it is what macOS itself
/// uses, and it matches both `pgrep` and the name System Events reports for
/// the frontmost process. Falling back to the bundle name keeps the old
/// behaviour for anything without a readable plist rather than failing.
fn process_name(executable: &str) -> String {
    let bundle = executable
        .split("/Contents/")
        .next()
        .unwrap_or(executable)
        .to_string();

    let plist = format!("{bundle}/Contents/Info.plist");
    if let Ok(out) = run(
        "/usr/bin/defaults",
        &["read", &plist, "CFBundleExecutable"],
        DEFAULT_TIMEOUT,
    ) {
        if out.success && !out.stdout.trim().is_empty() {
            return out.stdout.trim().to_string();
        }
    }

    bundle
        .rsplit('/')
        .next()
        .map(|app| app.trim_end_matches(".app").to_string())
        .unwrap_or_else(|| bundle.clone())
}

/// Registered editors that are actually open, in registry order.
///
/// The resolver needs this because dictation has to land in a specific
/// editor and NEXUS will not guess between two that are both running. With
/// one open the answer is obvious; with several the user is asked. Uses
/// `pgrep`, so checking cannot raise an Automation prompt.
pub fn running_editors(conn: &Connection) -> Vec<(i64, String)> {
    list_ides(conn, true)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| {
            let executable = entry.executable_path?;
            is_running(&process_name(&executable)).then_some((entry.id, entry.name))
        })
        .collect()
}

/// A prompt worth typing.
///
/// Empty is rejected rather than typed as nothing: an empty dictation is a
/// microphone that heard silence, and confirming "type nothing" wastes the
/// user's confirmation on a no-op.
fn check_prompt(raw: &str) -> Result<String, ActionError> {
    let text = raw.trim();
    if text.is_empty() {
        return Err(ActionError::InvalidInput {
            detail: "There was nothing to type.".to_string(),
        });
    }
    if text.chars().count() > MAX_PROMPT {
        return Err(ActionError::InvalidInput {
            detail: format!("That prompt is longer than {MAX_PROMPT} characters."),
        });
    }
    Ok(text.to_string())
}

/// Raise an editor, confirm it is really in front, and act on it.
///
/// **What this can and cannot promise, stated plainly, because the gap
/// matters to whoever is dictating into it.**
///
/// It *can* guarantee the keystrokes reach the editor you named. The
/// activation, the frontmost check and the keystrokes are one script, so no
/// other window can take focus between the check and the typing and collect
/// the text instead. That is the same lesson the WhatsApp connector learned
/// the expensive way.
///
/// It *cannot* guarantee which box inside that editor receives them. VS Code
/// and Cursor are Electron applications and publish no `AXFocusedUIElement`
/// unless a screen reader is running, so there is no supported way to see
/// whether the Claude panel, the file, or the find bar has focus. NEXUS types
/// into whatever you focused, exactly as `browser.type_here` does, and the
/// action's summary says so rather than naming a panel it cannot find.
///
/// The text is passed as an argument, never interpolated into the script.
/// A dictated sentence containing a quote would otherwise close the string
/// and the rest would be read as AppleScript.
fn drive_editor(executable: &str, lines: &[&str], args: &[&str]) -> Result<(), ActionError> {
    let process = process_name(executable);
    let process = process.as_str();
    let mut script = vec![
        "tell application \"System Events\"",
        "set n to name of first application process whose frontmost is true",
        "if n is not (item 1 of argv) then return \"not-front:\" & n",
    ];
    script.extend_from_slice(lines);
    script.push("end tell");
    script.push("return \"done\"");

    let mut argv = vec![process];
    argv.extend_from_slice(args);

    // Raising the window is a separate, ordinary launch rather than part of
    // the script: `open -a` is what `ide.focus` already uses, it needs no
    // Automation permission, and it works for an application that is not
    // running yet. The bundle comes from the registry row, so it is a path
    // already on disk rather than anything a caller supplied.
    let bundle = executable
        .split("/Contents/")
        .next()
        .unwrap_or(executable)
        .to_string();
    let _ = run("/usr/bin/open", &["-a", &bundle], DEFAULT_TIMEOUT);
    // Raising a window is not instant. Without the wait the frontmost check
    // reads the application on its way out and refuses a keystroke that
    // would have been fine a moment later.
    let _ = run("/bin/sleep", &["0.4"], DEFAULT_TIMEOUT);

    let out = crate::assistant::shell::osascript(&script, &argv).map_err(|e| {
        ActionError::Failed {
            detail: format!("Could not reach the editor: {e}"),
        }
    })?;

    if !out.success {
        let detail = if out.stderr.contains("not allowed") || out.stderr.contains("1002") {
            "NEXUS is not allowed to send keystrokes. Turn it on in System Settings \
             > Privacy & Security > Accessibility, then try again."
                .to_string()
        } else {
            format!("The editor refused: {}", out.stderr.trim())
        };
        return Err(ActionError::Failed { detail });
    }

    match out.stdout.trim() {
        "done" => Ok(()),
        other => Err(ActionError::Failed {
            detail: format!(
                "{} was in front instead of the editor, so nothing was typed. \
                 Bring the editor to the front and say it again.",
                other.strip_prefix("not-front:").unwrap_or(other)
            ),
        }),
    }
}

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
            "No editor is registered yet. Run discovery, then add one in the Registry.".to_string();
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
            Ok(rows) if rows.iter().any(|e| e.executable_path.is_some()) => ConnectorStatus::Ready,
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
            "ide.type_prompt" => match serde_json::from_value::<TypePrompt>(input.clone()) {
                Ok(t) => {
                    let ide = ide_name(t.ide_id).unwrap_or_else(|| "an editor".to_string());
                    // The text itself, because this is the last point at
                    // which a misheard sentence can be caught, and a summary
                    // that hid it would make the confirmation worthless.
                    format!(
                        "Type into Claude in {ide}: \"{}\"",
                        t.text.trim().chars().take(140).collect::<String>()
                    )
                }
                Err(_) => "Type into an editor".to_string(),
            },
            "ide.submit_prompt" => match serde_json::from_value::<IdeRef>(input.clone())
                .ok()
                .and_then(|r| ide_name(r.ide_id))
            {
                Some(ide) => format!("Run what is typed in {ide}"),
                None => "Run what is typed in the editor".to_string(),
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
            "ide.focus" | "ide.status" | "ide.submit_prompt" => {
                parse::<IdeRef>(input.clone()).map(|_| ())
            }
            "ide.type_prompt" => {
                let target = parse::<TypePrompt>(input.clone())?;
                check_prompt(&target.text).map(|_| ())
            }
            _ => Ok(()),
        }
    }

    fn describe_result(&self, action_id: &str, output: &serde_json::Value) -> Option<String> {
        match action_id {
            "ide.discover" => {
                let found = output.get("found")?.as_array()?;
                if found.is_empty() {
                    return Some("No editors found in Applications.".to_string());
                }
                let names: Vec<String> = found
                    .iter()
                    .filter_map(|e| {
                        let name = e.get("name")?.as_str()?;
                        let registered = e.get("registered")?.as_bool().unwrap_or(false);
                        Some(if registered {
                            format!("{name} (registered)")
                        } else {
                            name.to_string()
                        })
                    })
                    .collect();
                Some(format!("Found {}.", names.join(", ")))
            }
            "ide.list" => {
                let ides = output.get("ides")?.as_array()?;
                if ides.is_empty() {
                    return Some("No editors are registered yet.".to_string());
                }
                let names: Vec<&str> = ides
                    .iter()
                    .filter_map(|e| e.get("name").and_then(|v| v.as_str()))
                    .collect();
                Some(format!("Registered: {}.", names.join(", ")))
            }
            "ide.open_project" => Some(format!(
                "Opened {} in {}.",
                output.get("project")?.as_str()?,
                output.get("ide")?.as_str()?
            )),
            "ide.status" => Some(format!(
                "{} is {}.",
                output.get("name")?.as_str()?,
                if output.get("running")?.as_bool().unwrap_or(false) {
                    "running"
                } else {
                    "not running"
                }
            )),
            _ => None,
        }
    }

    fn zero_input_actions(&self) -> &'static [&'static str] {
        // `type_prompt` is absent because it needs the text, and
        // `submit_prompt` because it needs to know which editor: matching
        // either from a bare phrase and then failing on a missing field is
        // worse than not matching it. `submit_prompt` is reached through the
        // follow-up below, which supplies neither, so it is offered only
        // where the editor is already known.
        &["ide.discover", "ide.list"]
    }

    /// Text sitting in an editor is a question, and "yes" is an answer to it.
    ///
    /// This is what makes dictation two steps rather than one: the sentence
    /// lands on screen where it can be read, and nothing runs until the user
    /// looks at it and agrees.
    /// The Accessibility grant, which is the only failure here with a fix
    /// NEXUS can point at. A network problem or a chat that moved has no
    /// remedy worth offering, and offering one anyway spends the user's
    /// attention on something that will not work.
    fn remedy(&self, _action_id: &str, error: &ActionError) -> Option<Remedy> {
        let detail = match error {
            ActionError::Failed { detail } => detail,
            _ => return None,
        };
        detail.contains("not allowed to send keystrokes").then(|| Remedy {
            prompt: "NEXUS is not allowed to send keystrokes. Shall I open \
                     Accessibility settings?"
                .to_string(),
            action_id: "system.open_settings_pane",
            input: serde_json::json!({ "pane": "accessibility" }),
        })
    }

    fn follow_up(
        &self,
        action_id: &str,
        input: &serde_json::Value,
        _output: &serde_json::Value,
    ) -> Option<FollowUp> {
        if action_id != "ide.type_prompt" {
            return None;
        }
        // The same editor the text went into, taken from the action that
        // just ran. Not from whoever says "yes", and not from whatever
        // happens to be in front by then.
        let typed: TypePrompt = serde_json::from_value(input.clone()).ok()?;
        Some(FollowUp {
            action_id: "ide.submit_prompt",
            input: serde_json::json!({ "ideId": typed.ide_id }),
        })
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
                let rows =
                    list_ides(ctx.conn, false).map_err(|detail| ActionError::Failed { detail })?;
                json(serde_json::json!({ "ides": rows }))
            }

            "ide.status" => {
                let target: IdeRef = parse(input)?;
                let ide = resolve_ide(ctx.conn, target.ide_id)?;
                json(serde_json::json!({
                    "name": ide.name,
                    "running": is_running(&process_name(&ide.executable))
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

            "ide.type_prompt" => {
                let target: TypePrompt = parse(input)?;
                let ide = resolve_ide(ctx.conn, target.ide_id)?;
                let text = check_prompt(&target.text)?;
                let process = process_name(&ide.executable);

                match claude_installed(&process) {
                    Some(true) => {}
                    Some(false) => {
                        return Err(ActionError::Failed {
                            detail: format!(
                                "Claude Code is not installed in {}. Install the \
                                 extension there, or dictate into an editor that \
                                 has it.",
                                ide.name
                            ),
                        })
                    }
                    None => {
                        return Err(ActionError::Failed {
                            detail: format!(
                                "NEXUS does not know where {} keeps its extensions, \
                                 so it cannot find Claude's prompt box. Only VS Code \
                                 and Cursor are supported for dictation.",
                                ide.name
                            ),
                        })
                    }
                }

                // Focus first, then type. Two things NEXUS could not promise
                // an hour ago it can promise now: the text goes to Claude
                // rather than to whatever box happened to have focus, and
                // the palette is closed again before a single character of
                // the user's sentence is typed.
                //
                // `keystroke` rather than a paste, throughout: the clipboard
                // belongs to the user, and an assistant that silently
                // replaces what is on it loses them something they may still
                // need.
                drive_editor(
                    &ide.executable,
                    &[
                        // Escape first, so a palette or dialog somebody left
                        // open does not swallow the sequence below.
                        "key code 53",
                        "delay 0.3",
                        // 1. A fresh conversation. Opens Claude if it is not
                        //    already open, which is why there is no separate
                        //    step for that.
                        "key code 35 using {command down, shift down}",
                        PALETTE_SETTLE,
                        "keystroke (item 2 of argv)",
                        PALETTE_SETTLE,
                        "key code 36",
                        COMMAND_SETTLE,
                        // 2. Put the cursor in the box. Belt and braces: a
                        //    new conversation usually focuses its own input,
                        //    but "usually" is not something to type
                        //    somebody's sentence into.
                        "key code 35 using {command down, shift down}",
                        PALETTE_SETTLE,
                        "keystroke (item 3 of argv)",
                        PALETTE_SETTLE,
                        "key code 36",
                        COMMAND_SETTLE,
                        // 3. Only now the user's words.
                        "keystroke (item 4 of argv)",
                    ],
                    &[CLAUDE_NEW_CHAT_COMMAND, CLAUDE_FOCUS_COMMAND, &text],
                )?;

                json(serde_json::json!({
                    "typed": text,
                    "into": ide.name,
                    // NEXUS did not run this. Saying so in the payload keeps
                    // every caller honest about what happened.
                    "submitted": false
                }))
            }

            "ide.submit_prompt" => {
                let target: IdeRef = parse(input)?;
                let ide = resolve_ide(ctx.conn, target.ide_id)?;
                // Return rather than a click: finding a send button needs the
                // accessibility tree, which these editors do not publish.
                drive_editor(&ide.executable, &["key code 36"], &[])?;
                json(serde_json::json!({ "submitted": true, "in": ide.name }))
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
        let id = seed_ide(
            &conn,
            "UI-TEST-IDE",
            Some("/Applications/NotReal.app"),
            true,
        );
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
        assert!(
            format!("{err:?}").contains("not a project folder"),
            "{err:?}"
        );
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
            assert!(
                !first.registered,
                "nothing is registered in a fresh database"
            );
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
            caps.unavailable
                .iter()
                .any(|u| u.action_id == "ide.open_project"),
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
    fn no_action_takes_a_command_to_run() {
        // The rule has not moved, only what satisfies it. NEXUS still has
        // nowhere vetted to keep a build command, so it still refuses to
        // accept one: `ide.run_task` would mean taking a command string from
        // a caller, which is the escape hatch this architecture exists to
        // avoid.
        //
        // Dictation is a different shape and that is why it is allowed. The
        // text goes on screen, the user reads it, and Return is a second
        // confirmed action. NEXUS never composes a command, never stores
        // one, and never decides what the typed words mean; the editor does.
        assert!(
            !ACTIONS.iter().any(|s| s.id == "ide.run_task"),
            "ide.run_task must not ship without somewhere to configure it"
        );

        for spec in ACTIONS {
            if spec.permission == Permission::Execute {
                assert_eq!(
                    spec.id, "ide.submit_prompt",
                    "the only Execute here presses Return on text already on \
                     screen; anything else must justify itself"
                );
                assert_eq!(spec.confirm, ConfirmPolicy::Always);
                assert!(!spec.reversible, "whatever it starts is already started");
            }
        }
    }

    #[test]
    fn typing_and_running_are_separate_confirmed_steps() {
        // The property that makes dictation recoverable: a misheard sentence
        // is visible on screen before anything acts on it. Collapsing these
        // into one action would remove the only moment the user can catch it.
        let typing = ACTIONS
            .iter()
            .find(|s| s.id == "ide.type_prompt")
            .expect("typing must exist");
        let running = ACTIONS
            .iter()
            .find(|s| s.id == "ide.submit_prompt")
            .expect("running must exist");

        assert_eq!(typing.permission, Permission::Write);
        assert_eq!(running.permission, Permission::Execute);
        assert_eq!(typing.confirm, ConfirmPolicy::Always);
        assert_eq!(running.confirm, ConfirmPolicy::Always);
        assert!(typing.reversible, "typed text can be cleared by hand");
        assert!(!running.reversible);

        // And the second step is offered by the first, so "yes" reaches it
        // without the user having to name the editor twice.
        let offer = IdeConnector
            .follow_up(
                "ide.type_prompt",
                &serde_json::json!({ "ideId": 7, "text": "run the tests" }),
                &serde_json::Value::Null,
            )
            .expect("typing must offer the run that follows it");
        assert_eq!(offer.action_id, "ide.submit_prompt");
        assert_eq!(
            offer.input,
            serde_json::json!({ "ideId": 7 }),
            "the follow-up must target the editor the text went into, not \
             whatever happens to be in front when the user says yes"
        );
    }

    #[test]
    fn dictation_focuses_claude_before_it_types_anything() {
        // The ordering is the safety property. Typing first and focusing
        // afterwards would put the user's sentence wherever the cursor
        // happened to be, which for an editor is somebody's source file.
        let production = include_str!("ide_connector.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("the file must keep its test module marker");
        // The dispatch arm, not the validation arm that shares its name:
        // `rfind` takes the last, which is the one that does the work.
        let at = production
            .rfind("\"ide.type_prompt\" => {")
            .expect("the dictation arm must exist");
        let dictation = production[at..]
            .split_once("\"ide.submit_prompt\" => {")
            .map(|(body, _)| body)
            .expect("the dictation arm must be followed by the submit arm");

        let new_chat = dictation
            .find("item 2 of argv")
            .expect("a fresh conversation must be opened");
        let focus = dictation
            .find("item 3 of argv")
            .expect("the focus command must be typed into the palette");
        let types = dictation
            .find("item 4 of argv")
            .expect("the prompt must be typed");
        assert!(new_chat < focus, "open the chat before focusing its box");
        assert!(focus < types, "focus Claude before typing the sentence");

        // And each palette is dismissed by running its command, not left
        // open for a later Return to act on.
        assert!(
            dictation.matches("key code 36").count() >= 2,
            "both palette commands must actually be run"
        );
    }

    #[test]
    fn an_editor_without_claude_is_refused_rather_than_typed_into() {
        // A palette with no matching command stays open, and the sentence
        // would land in it. NEXUS must not start the sequence at all.
        assert_eq!(claude_installed("Terminal"), None, "unknown editor");
        assert!(
            EXTENSION_DIRS.iter().any(|(name, _)| *name == "Code"),
            "VS Code must be resolvable, or dictation refuses on this Mac"
        );
        assert!(EXTENSION_DIRS.iter().any(|(name, _)| *name == "Cursor"));
    }

    #[test]
    fn a_dictated_prompt_is_never_interpolated_into_the_script() {
        // A sentence containing a quote would otherwise close the AppleScript
        // string and the remainder would be read as code. The text travels as
        // an argument, so the script itself is fixed.
        let production = include_str!("ide_connector.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("the file must keep its test module marker");
        assert!(
            production.contains("keystroke (item 4 of argv)"),
            "the typed text must reach AppleScript as an argument"
        );
        assert!(
            !production.contains("keystroke \\\"{"),
            "no formatted string may become part of the script"
        );
    }

    #[test]
    fn an_empty_dictation_is_refused_rather_than_typed() {
        for empty in ["", "   ", "\n"] {
            assert!(
                check_prompt(empty).is_err(),
                "a microphone that heard silence must not spend a confirmation"
            );
        }
        assert_eq!(check_prompt("  run the tests  ").unwrap(), "run the tests");
        assert!(check_prompt(&"x".repeat(MAX_PROMPT + 1)).is_err());
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
            assert!(
                !core.contains(named),
                "the core must not know about {named}"
            );
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
