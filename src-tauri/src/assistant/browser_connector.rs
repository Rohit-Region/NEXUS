//! NEXUS-015: the Chrome connector.
//!
//! Three tiers, deliberately separated because their risk profiles are
//! nothing alike:
//!
//! | Tier | Mechanism | Costs the user |
//! |------|-----------|----------------|
//! | 1 | `/usr/bin/open` | nothing |
//! | 2 | AppleScript | a one-time macOS Automation prompt |
//! | 3 | `execute javascript` | a manual Chrome toggle, and it is `Execute` |
//!
//! **The DevTools Protocol is deliberately not used.** Launching Chrome with
//! `--remote-debugging-port` hands *any* local process full control of the
//! browser, cookies and authenticated sessions included, with no
//! authentication on the port. Coordinate-based UI automation is rejected for
//! the reason NEXUS-009 established: synthetic input can leak into other
//! applications.
//!
//! Every argument reaches AppleScript as a value, never as script text, so a
//! URL containing quotes or an embedded `do shell script` is inert.

use rusqlite::Connection;
use serde::Deserialize;

use super::action::{ActionError, ActionSpec};
use super::connector::{Capabilities, Connector, ConnectorStatus, ExecCtx, UnavailableAction};
use super::permission::{ConfirmPolicy, Permission, Reach};
use super::shell::{osascript, run, RunError, DEFAULT_TIMEOUT};

pub const CONNECTOR_ID: &str = "browser";

/// The browser NEXUS drives. Chrome specifically, because the scripting
/// vocabulary below is Chrome's; Safari's differs.
const CHROME_APP: &str = "Google Chrome";
const CHROME_PATH: &str = "/Applications/Google Chrome.app";

/// Where `browser.search` sends a query. A constant rather than a preference
/// so the URL NEXUS builds is always inspectable in the audit trail.
const SEARCH_URL: &str = "https://www.google.com/search?q=";

/// Longest page text handed back. A page is not a document store, and this
/// value eventually becomes part of a prompt.
const PAGE_TEXT_CAP: usize = 8_000;
/// Longest list read back before it stops being an answer and becomes a dump.
const RESULT_LIST_CAP: usize = 6;
/// Longest single dictation. A prompt box is not a document editor, and a
/// runaway transcript should not be pasted somewhere by accident.
const MAX_DICTATION: usize = 2_000;

const fn spec(
    id: &'static str,
    summary: &'static str,
    permission: Permission,
    confirm: ConfirmPolicy,
    reversible: bool,
) -> ActionSpec {
    ActionSpec {
        id,
        connector_id: CONNECTOR_ID,
        summary,
        permission,
        confirm,
        // Opening a URL reaches the network, but NEXUS is not the one
        // transmitting: the browser is, as it would if the user typed it.
        // What matters here is that NEXUS sends nothing of its own.
        reach: Reach::LocalOnly,
        reversible,
    }
}

pub const ACTIONS: &[ActionSpec] = &[
    spec(
        "browser.open_url",
        "Open a page",
        Permission::Interact,
        ConfirmPolicy::Never,
        true,
    ),
    spec(
        "browser.search",
        "Search the web",
        Permission::Interact,
        ConfirmPolicy::Never,
        true,
    ),
    spec(
        "browser.list_tabs",
        "List open tabs",
        Permission::Read,
        ConfirmPolicy::Never,
        true,
    ),
    spec(
        "browser.activate_tab",
        "Switch to a tab",
        Permission::Interact,
        ConfirmPolicy::Never,
        true,
    ),
    spec(
        "browser.focus_tab",
        "Switch to a tab by name",
        Permission::Interact,
        ConfirmPolicy::Never,
        true,
    ),
    spec(
        "browser.navigate",
        "Navigate the current tab",
        Permission::Interact,
        ConfirmPolicy::Never,
        true,
    ),
    spec(
        "browser.close_tab",
        "Close a tab",
        Permission::Write,
        ConfirmPolicy::Always,
        false,
    ),
    spec(
        "browser.read_page",
        "Read the current page",
        Permission::Execute,
        ConfirmPolicy::Always,
        true,
    ),
    spec(
        "browser.click",
        "Click something on the page",
        Permission::Execute,
        ConfirmPolicy::Always,
        false,
    ),
    spec(
        "browser.type",
        "Type into the page",
        Permission::Execute,
        ConfirmPolicy::Always,
        false,
    ),
    spec(
        "browser.type_here",
        "Type into the box you have focused",
        Permission::Execute,
        ConfirmPolicy::Always,
        false,
    ),
];

// -- Typed inputs -------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UrlInput {
    url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QueryInput {
    query: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TabRef {
    window_index: u32,
    tab_index: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TabQuery {
    query: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SelectorInput {
    selector: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DictateInput {
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TypeInput {
    selector: String,
    text: String,
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

/// Accept only URLs a browser should be told to open.
///
/// `javascript:` would execute in whatever page is focused, which is a code
/// path dressed as a navigation. `file:` reads the disk. `data:` can carry a
/// whole document. None of them belongs behind an `Interact` permission, and
/// a URL is the one field a reasoning provider is most likely to compose.
pub fn safe_url(url: &str) -> Result<String, ActionError> {
    let trimmed = url.trim();
    let lowered = trimmed.to_lowercase();
    if lowered.starts_with("http://") || lowered.starts_with("https://") {
        if trimmed.contains(char::is_whitespace) {
            return Err(ActionError::InvalidInput {
                detail: "A URL cannot contain spaces.".to_string(),
            });
        }
        return Ok(trimmed.to_string());
    }
    Err(ActionError::InvalidInput {
        detail: format!(
            "NEXUS only opens http and https addresses, and that one is not: {trimmed}"
        ),
    })
}

/// Percent-encode a search query for a URL.
///
/// Hand-rolled against the unreserved set from RFC 3986 rather than adding a
/// crate for nine lines. Everything outside it is escaped, so the result is
/// always a well-formed query component.
fn encode(query: &str) -> String {
    let mut out = String::with_capacity(query.len() * 3);
    for byte in query.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn chrome_installed() -> bool {
    std::path::Path::new(CHROME_PATH).exists()
}

/// Turn a subprocess failure into something the user can act on.
fn from_run_error(err: RunError) -> ActionError {
    ActionError::Failed {
        detail: err.to_string(),
    }
}

/// AppleScript failed. Its messages name the cause but never the remedy, so
/// the remedy is added here, exactly as NEXUS-010 had to for Dictation.
fn from_script_failure(stderr: &str) -> ActionError {
    let detail = if stderr.contains("-1743") || stderr.to_lowercase().contains("not allowed") {
        "macOS has not given NEXUS permission to control Chrome. Grant it in \
         System Settings, Privacy & Security, Automation."
            .to_string()
    } else if stderr.contains("-609") || stderr.contains("Connection is invalid") {
        "Chrome did not accept the connection. Make sure Chrome is running, \
         then try again."
            .to_string()
    } else if stderr.to_lowercase().contains("javascript") {
        "Chrome is refusing to run JavaScript from Apple Events. Turn it on in \
         Chrome under View, Developer, Allow JavaScript from Apple Events."
            .to_string()
    } else if stderr.is_empty() {
        "Chrome did not respond.".to_string()
    } else {
        stderr.to_string()
    };
    ActionError::Failed { detail }
}

/// Record and field separators for tab listings.
///
/// Control characters rather than tabs or pipes, because a page title can
/// contain either and a split on the wrong byte silently corrupts the list.
const RECORD_SEP: char = '\u{1e}';
const FIELD_SEP: char = '\u{1f}';

fn list_tabs_script() -> Vec<&'static str> {
    vec![
        "set out to \"\"",
        "set fs to (character id 31)",
        "set rs to (character id 30)",
        "tell application \"Google Chrome\"",
        "repeat with w from 1 to (count of windows)",
        "repeat with t from 1 to (count of tabs of window w)",
        "set theTab to tab t of window w",
        "set out to out & w & fs & t & fs & (URL of theTab) & fs & (title of theTab) & rs",
        "end repeat",
        "end repeat",
        "end tell",
        "return out",
    ]
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TabInfo {
    window_index: u32,
    tab_index: u32,
    url: String,
    title: String,
}

fn parse_tabs(raw: &str) -> Vec<TabInfo> {
    raw.split(RECORD_SEP)
        .filter(|record| !record.trim().is_empty())
        .filter_map(|record| {
            let fields: Vec<&str> = record.split(FIELD_SEP).collect();
            if fields.len() < 4 {
                return None;
            }
            Some(TabInfo {
                window_index: fields[0].trim().parse().ok()?,
                tab_index: fields[1].trim().parse().ok()?,
                url: fields[2].to_string(),
                title: fields[3].trim().to_string(),
            })
        })
        .collect()
}

/// Does this tab match what the user said?
///
/// **Every word of the query must appear somewhere in the title or URL**, in
/// any order, ignoring case. Substring matching was the obvious rule and it
/// was wrong twice over: "player zero" is not a substring of "PlayerZero",
/// and "clone ui repo" is not a substring of "Clone the UI repo". Words are
/// what people actually remember about a tab.
///
/// Matching per word rather than per token also means a word can land inside
/// a longer one, which is what makes "player" find "PlayerZero".
pub fn tab_matches(query: &str, title: &str, url: &str) -> bool {
    let haystack = format!("{} {}", title.to_lowercase(), url.to_lowercase());
    let words: Vec<&str> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() {
        return false;
    }
    words.iter().all(|w| haystack.contains(&w.to_lowercase()))
}

pub struct BrowserConnector;

impl Connector for BrowserConnector {
    fn id(&self) -> &'static str {
        CONNECTOR_ID
    }

    fn display_name(&self) -> &'static str {
        "Chrome"
    }

    fn actions(&self) -> &'static [ActionSpec] {
        ACTIONS
    }

    fn capabilities(&self, _conn: &Connection) -> Capabilities {
        if !chrome_installed() {
            return Capabilities {
                available: Vec::new(),
                unavailable: ACTIONS
                    .iter()
                    .map(|spec| UnavailableAction {
                        action_id: spec.id.to_string(),
                        reason: "Chrome is not installed on this Mac.".to_string(),
                    })
                    .collect(),
            };
        }

        // Tier 3 is listed as available rather than probed. Probing means
        // running a script against Chrome on every capability read, which
        // raises permission prompts NEXUS did not need to raise. The failure
        // path names the toggle instead, which is the same information at the
        // moment it is actually useful.
        Capabilities {
            available: ACTIONS.iter().map(|spec| spec.id.to_string()).collect(),
            unavailable: Vec::new(),
        }
    }

    fn status(&self, _conn: &Connection) -> ConnectorStatus {
        if chrome_installed() {
            ConnectorStatus::Ready
        } else {
            ConnectorStatus::Unavailable
        }
    }

    fn summarize(&self, action_id: &str, input: &serde_json::Value, _conn: &Connection) -> String {
        match action_id {
            "browser.open_url" | "browser.navigate" => {
                match serde_json::from_value::<UrlInput>(input.clone()) {
                    Ok(target) => format!("Open {} in Chrome", target.url),
                    Err(_) => "Open a page in Chrome".to_string(),
                }
            }
            "browser.search" => match serde_json::from_value::<QueryInput>(input.clone()) {
                Ok(q) => format!("Search the web for \"{}\"", q.query),
                Err(_) => "Search the web".to_string(),
            },
            "browser.type_here" => match serde_json::from_value::<DictateInput>(input.clone()) {
                Ok(d) => format!("Type into the focused box: \"{}\"", d.text.trim()),
                Err(_) => "Type into the focused box".to_string(),
            },
            "browser.focus_tab" => match serde_json::from_value::<TabQuery>(input.clone()) {
                Ok(q) => format!("Switch to the \"{}\" tab", q.query),
                Err(_) => "Switch to a tab".to_string(),
            },
            "browser.close_tab" => match serde_json::from_value::<TabRef>(input.clone()) {
                Ok(t) => format!(
                    "Close tab {} in Chrome window {}",
                    t.tab_index, t.window_index
                ),
                Err(_) => "Close a Chrome tab".to_string(),
            },
            "browser.click" => match serde_json::from_value::<SelectorInput>(input.clone()) {
                Ok(s) => format!("Click \"{}\" on the current page", s.selector),
                Err(_) => "Click something on the current page".to_string(),
            },
            "browser.type" => match serde_json::from_value::<TypeInput>(input.clone()) {
                Ok(t) => format!("Type into \"{}\" on the current page", t.selector),
                Err(_) => "Type into the current page".to_string(),
            },
            other => ACTIONS
                .iter()
                .find(|spec| spec.id == other)
                .map(|spec| spec.summary.to_string())
                .unwrap_or_else(|| other.to_string()),
        }
    }

    fn validate_input(
        &self,
        action_id: &str,
        input: &serde_json::Value,
    ) -> Result<(), ActionError> {
        match action_id {
            "browser.open_url" | "browser.navigate" => {
                let target: UrlInput = parse(input.clone())?;
                safe_url(&target.url).map(|_| ())
            }
            "browser.search" => parse::<QueryInput>(input.clone()).map(|_| ()),
            "browser.activate_tab" | "browser.close_tab" => {
                parse::<TabRef>(input.clone()).map(|_| ())
            }
            "browser.click" => parse::<SelectorInput>(input.clone()).map(|_| ()),
            "browser.type" => parse::<TypeInput>(input.clone()).map(|_| ()),
            _ => Ok(()),
        }
    }

    fn describe_result(&self, action_id: &str, output: &serde_json::Value) -> Option<String> {
        match action_id {
            "browser.list_tabs" => {
                let tabs = output.get("tabs")?.as_array()?;
                if tabs.is_empty() {
                    return Some("Chrome has no tabs open.".to_string());
                }
                // Titles, not URLs: a title is what the user recognises, and
                // a list of URLs is unreadable aloud.
                let named: Vec<String> = tabs
                    .iter()
                    .filter_map(|t| t.get("title").and_then(|v| v.as_str()))
                    .filter(|t| !t.trim().is_empty())
                    .take(RESULT_LIST_CAP)
                    .map(|t| t.chars().take(60).collect::<String>())
                    .collect();
                let more = tabs.len().saturating_sub(named.len());
                let mut text = format!(
                    "{} tab{} open: {}",
                    tabs.len(),
                    if tabs.len() == 1 { "" } else { "s" },
                    named.join(", ")
                );
                if more > 0 {
                    text.push_str(&format!(", and {more} more"));
                }
                text.push('.');
                Some(text)
            }
            "browser.read_page" => {
                let text = output.get("text")?.as_str()?;
                let first: String = text.lines().take(3).collect::<Vec<_>>().join(" ");
                Some(format!(
                    "{}...",
                    first.chars().take(280).collect::<String>()
                ))
            }
            "browser.open_url" | "browser.search" => {
                Some(format!("Opened {}.", output.get("url")?.as_str()?))
            }
            "browser.type_here" => Some("Typed it in.".to_string()),
            "browser.focus_tab" => {
                let title: String = output.get("title")?.as_str()?.chars().take(70).collect();
                let others = output.get("alsoMatched")?.as_u64().unwrap_or(0);
                Some(if others > 0 {
                    // Saying so matters: with four PlayerZero tabs open, "the
                    // first match" is not obviously the one they meant.
                    format!("Switched to {title}. {others} other tabs also matched.")
                } else {
                    format!("Switched to {title}.")
                })
            }
            _ => None,
        }
    }

    fn zero_input_actions(&self) -> &'static [&'static str] {
        &["browser.list_tabs", "browser.read_page"]
    }

    fn dispatch(
        &self,
        action_id: &str,
        input: serde_json::Value,
        _ctx: &ExecCtx<'_>,
    ) -> Result<serde_json::Value, ActionError> {
        if !chrome_installed() {
            return Err(ActionError::Failed {
                detail: "Chrome is not installed on this Mac.".to_string(),
            });
        }

        match action_id {
            // Tier 1: no automation permission needed at all.
            "browser.open_url" => {
                let target: UrlInput = parse(input)?;
                let url = safe_url(&target.url)?;
                let out = run("/usr/bin/open", &["-a", CHROME_APP, &url], DEFAULT_TIMEOUT)
                    .map_err(from_run_error)?;
                if !out.success {
                    return Err(from_script_failure(&out.stderr));
                }
                json(serde_json::json!({ "url": url }))
            }

            "browser.search" => {
                let q: QueryInput = parse(input)?;
                if q.query.trim().is_empty() {
                    return Err(ActionError::InvalidInput {
                        detail: "There was nothing to search for.".to_string(),
                    });
                }
                let url = format!("{SEARCH_URL}{}", encode(q.query.trim()));
                let out = run("/usr/bin/open", &["-a", CHROME_APP, &url], DEFAULT_TIMEOUT)
                    .map_err(from_run_error)?;
                if !out.success {
                    return Err(from_script_failure(&out.stderr));
                }
                json(serde_json::json!({ "url": url }))
            }

            // Tier 2: AppleScript.
            "browser.list_tabs" => {
                let out = osascript(&list_tabs_script(), &[]).map_err(from_run_error)?;
                if !out.success {
                    return Err(from_script_failure(&out.stderr));
                }
                json(serde_json::json!({ "tabs": parse_tabs(&out.stdout) }))
            }

            // Switching by name rather than by index, because nobody knows
            // which number their Jira tab is, and a spoken index would be a
            // guess the moment a tab is opened or closed.
            "browser.focus_tab" => {
                let target: TabQuery = parse(input)?;
                let query = target.query.trim().to_string();
                if query.is_empty() {
                    return Err(ActionError::InvalidInput {
                        detail: "There was no tab name to look for.".to_string(),
                    });
                }

                // Listed first, then matched in Rust. Doing the matching in
                // AppleScript meant a substring test that could not be tested
                // and got the rule wrong; here it is a pure function with
                // cases pinned by unit tests.
                let listing = osascript(&list_tabs_script(), &[]).map_err(from_run_error)?;
                if !listing.success {
                    return Err(from_script_failure(&listing.stderr));
                }

                let tabs = parse_tabs(&listing.stdout);
                let hits: Vec<&TabInfo> = tabs
                    .iter()
                    .filter(|tab| tab_matches(&query, &tab.title, &tab.url))
                    .collect();

                let chosen = match hits.first() {
                    Some(tab) => *tab,
                    None => {
                        return Err(ActionError::Failed {
                            detail: format!("No open tab matches \"{query}\"."),
                        })
                    }
                };

                let out = osascript(
                    &[
                        "set w to (item 1 of argv) as integer",
                        "set t to (item 2 of argv) as integer",
                        "tell application \"Google Chrome\"",
                        "set active tab index of window w to t",
                        "set index of window w to 1",
                        "activate",
                        "end tell",
                        "return \"ok\"",
                    ],
                    &[
                        &chosen.window_index.to_string(),
                        &chosen.tab_index.to_string(),
                    ],
                )
                .map_err(from_run_error)?;

                if !out.success {
                    return Err(from_script_failure(&out.stderr));
                }
                json(serde_json::json!({
                    "title": chosen.title,
                    "url": chosen.url,
                    "query": query,
                    "alsoMatched": hits.len().saturating_sub(1),
                }))
            }

            "browser.activate_tab" => {
                let target: TabRef = parse(input)?;
                let out = osascript(
                    &[
                        "set w to (item 1 of argv) as integer",
                        "set t to (item 2 of argv) as integer",
                        "tell application \"Google Chrome\"",
                        "set active tab index of window w to t",
                        "set index of window w to 1",
                        "activate",
                        "end tell",
                        "return \"ok\"",
                    ],
                    &[
                        &target.window_index.to_string(),
                        &target.tab_index.to_string(),
                    ],
                )
                .map_err(from_run_error)?;
                if !out.success {
                    return Err(from_script_failure(&out.stderr));
                }
                json(serde_json::json!({
                    "windowIndex": target.window_index,
                    "tabIndex": target.tab_index
                }))
            }

            "browser.navigate" => {
                let target: UrlInput = parse(input)?;
                let url = safe_url(&target.url)?;
                let out = osascript(
                    &[
                        "tell application \"Google Chrome\"",
                        "set URL of active tab of front window to (item 1 of argv)",
                        "end tell",
                        "return \"ok\"",
                    ],
                    &[&url],
                )
                .map_err(from_run_error)?;
                if !out.success {
                    return Err(from_script_failure(&out.stderr));
                }
                json(serde_json::json!({ "url": url }))
            }

            "browser.close_tab" => {
                let target: TabRef = parse(input)?;
                let out = osascript(
                    &[
                        "set w to (item 1 of argv) as integer",
                        "set t to (item 2 of argv) as integer",
                        "tell application \"Google Chrome\" to close tab t of window w",
                        "return \"ok\"",
                    ],
                    &[
                        &target.window_index.to_string(),
                        &target.tab_index.to_string(),
                    ],
                )
                .map_err(from_run_error)?;
                if !out.success {
                    return Err(from_script_failure(&out.stderr));
                }
                json(serde_json::json!({ "closed": true }))
            }

            // Tier 3: JavaScript in the page. Execute-level, always confirmed.
            "browser.read_page" => {
                let out = osascript(
                    &[
                        "tell application \"Google Chrome\"",
                        "set r to execute active tab of front window javascript \
                         \"document.title + '\\n' + document.body.innerText\"",
                        "end tell",
                        "return r",
                    ],
                    &[],
                )
                .map_err(from_run_error)?;
                if !out.success {
                    return Err(from_script_failure(&out.stderr));
                }
                let mut text = out.stdout;
                let truncated = text.chars().count() > PAGE_TEXT_CAP;
                if truncated {
                    text = text.chars().take(PAGE_TEXT_CAP).collect();
                }
                json(serde_json::json!({ "text": text, "truncated": truncated }))
            }

            "browser.click" => {
                let target: SelectorInput = parse(input)?;
                let out = osascript(
                    &[
                        "set sel to (item 1 of argv)",
                        "tell application \"Google Chrome\"",
                        "set r to execute active tab of front window javascript \
                         (\"(function(s){var e=document.querySelector(s);\
                         if(!e){return 'missing';}e.click();return 'clicked';})\
                         (\" & quoted form of sel & \")\")",
                        "end tell",
                        "return r",
                    ],
                    &[&target.selector],
                )
                .map_err(from_run_error)?;
                if !out.success {
                    return Err(from_script_failure(&out.stderr));
                }
                if out.stdout == "missing" {
                    return Err(ActionError::Failed {
                        detail: format!("Nothing on the page matches \"{}\".", target.selector),
                    });
                }
                json(serde_json::json!({ "clicked": true }))
            }

            // Dictation into whatever the user has already focused.
            //
            // `browser.type` needs a CSS selector, which nobody says out
            // loud. This needs none: the user clicks the box, then speaks,
            // and the focused element is a fact rather than a guess. If
            // nothing is focused it falls back to the page's only editable
            // field, and refuses when that is ambiguous rather than picking.
            "browser.type_here" => {
                let target: DictateInput = parse(input)?;
                let text = target.text.trim();
                if text.is_empty() {
                    return Err(ActionError::InvalidInput {
                        detail: "There was nothing to type.".to_string(),
                    });
                }
                if text.chars().count() > MAX_DICTATION {
                    return Err(ActionError::InvalidInput {
                        detail: format!("That is longer than {MAX_DICTATION} characters."),
                    });
                }

                // Encoded as a JSON string literal in Rust, which is also a
                // valid JavaScript string literal. The text therefore reaches
                // the page as data and can never become syntax, however many
                // quotes or backslashes are in it.
                let literal = serde_json::to_string(text).map_err(|e| ActionError::Failed {
                    detail: format!("Could not encode the text: {e}"),
                })?;

                let out = osascript(
                    &[
                        "set t to (item 1 of argv)",
                        "tell application \"Google Chrome\"",
                        "set r to execute active tab of front window javascript \
                         (\"(function(t){\
                         var a=document.activeElement;\
                         var ed=function(e){return e&&(e.isContentEditable||\
                           e.tagName==='TEXTAREA'||(e.tagName==='INPUT'&&\
                           /^(text|search|email|url|tel|)$/i.test(e.type||'')));};\
                         if(!ed(a)){\
                           var c=[].slice.call(document.querySelectorAll(\
                             'textarea,input[type=text],input:not([type]),[contenteditable=true]'))\
                             .filter(function(e){var r=e.getBoundingClientRect();\
                               return r.width>0&&r.height>0&&!e.disabled&&!e.readOnly;});\
                           if(c.length!==1){return c.length?'ambiguous':'nofield';}\
                           a=c[0];a.focus();\
                         }\
                         if(a.isContentEditable){\
                           document.execCommand('insertText',false,t);\
                         }else{\
                           var p=Object.getPrototypeOf(a);\
                           var d=Object.getOwnPropertyDescriptor(p,'value');\
                           if(d&&d.set){d.set.call(a,(a.value||'')+t);}else{a.value=(a.value||'')+t;}\
                           a.dispatchEvent(new Event('input',{bubbles:true}));\
                         }\
                         return 'typed';})(\" & t & \")\")",
                        "end tell",
                        "return r",
                    ],
                    &[&literal],
                )
                .map_err(from_run_error)?;

                if !out.success {
                    return Err(from_script_failure(&out.stderr));
                }
                match out.stdout.as_str() {
                    "typed" => json(serde_json::json!({ "typed": true, "text": text })),
                    "ambiguous" => Err(ActionError::Failed {
                        detail: "This page has several text boxes. Click the one you want \
                                 first, then say it again."
                            .to_string(),
                    }),
                    _ => Err(ActionError::Failed {
                        detail: "There is no text box on this page to type into.".to_string(),
                    }),
                }
            }

            "browser.type" => {
                let target: TypeInput = parse(input)?;
                let out = osascript(
                    &[
                        "set sel to (item 1 of argv)",
                        "set val to (item 2 of argv)",
                        "tell application \"Google Chrome\"",
                        "set r to execute active tab of front window javascript \
                         (\"(function(s,v){var e=document.querySelector(s);\
                         if(!e){return 'missing';}e.focus();e.value=v;\
                         e.dispatchEvent(new Event('input',{bubbles:true}));\
                         return 'typed';})(\" & quoted form of sel & \",\" & \
                         quoted form of val & \")\")",
                        "end tell",
                        "return r",
                    ],
                    &[&target.selector, &target.text],
                )
                .map_err(from_run_error)?;
                if !out.success {
                    return Err(from_script_failure(&out.stderr));
                }
                if out.stdout == "missing" {
                    return Err(ActionError::Failed {
                        detail: format!("Nothing on the page matches \"{}\".", target.selector),
                    });
                }
                json(serde_json::json!({ "typed": true }))
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

    // -- URL safety, the field most likely to be composed for NEXUS ----------

    #[test]
    fn http_and_https_are_accepted() {
        assert!(safe_url("https://example.com/a?b=1").is_ok());
        assert!(safe_url("http://localhost:3000").is_ok());
        assert!(safe_url("  https://example.com  ").is_ok());
    }

    #[test]
    fn a_javascript_url_is_refused() {
        // A code path dressed as a navigation. It must not sit behind an
        // Interact permission.
        for hostile in [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            "  javascript:fetch('/x')",
        ] {
            assert!(safe_url(hostile).is_err(), "{hostile} must be refused");
        }
    }

    #[test]
    fn file_and_data_urls_are_refused() {
        for hostile in [
            "file:///etc/passwd",
            "data:text/html,<script>alert(1)</script>",
            "chrome://settings",
            "about:blank",
        ] {
            assert!(safe_url(hostile).is_err(), "{hostile} must be refused");
        }
    }

    #[test]
    fn a_url_with_whitespace_is_refused() {
        assert!(safe_url("https://example.com/a b").is_err());
    }

    #[test]
    fn the_refusal_says_what_is_allowed() {
        let err = safe_url("ftp://example.com").expect_err("must refuse");
        assert!(format!("{err:?}").contains("http"), "{err:?}");
    }

    // -- Query encoding -------------------------------------------------------

    #[test]
    fn a_query_is_percent_encoded() {
        assert_eq!(encode("hello world"), "hello+world");
        assert_eq!(encode("a&b=c"), "a%26b%3Dc");
        assert_eq!(encode("safe-_.~"), "safe-_.~");
    }

    #[test]
    fn a_query_cannot_escape_the_url() {
        // The property that matters: nothing survives that could start a new
        // parameter or a new URL.
        let encoded = encode("x&redirect=https://evil.example#frag");
        assert!(!encoded.contains('&'));
        assert!(!encoded.contains('#'));
        assert!(!encoded.contains(':') || encoded.contains("%3A"));
    }

    #[test]
    fn a_multibyte_query_is_encoded_by_byte() {
        let encoded = encode("நெக்சஸ்");
        assert!(encoded.starts_with('%'), "{encoded}");
        assert!(encoded.chars().all(|c| c.is_ascii()));
    }

    // -- Tab parsing ----------------------------------------------------------

    #[test]
    fn tabs_parse_from_the_control_separated_listing() {
        let raw = format!(
            "1{FIELD_SEP}1{FIELD_SEP}https://a.example{FIELD_SEP}Alpha{RECORD_SEP}\
             1{FIELD_SEP}2{FIELD_SEP}https://b.example{FIELD_SEP}Beta{RECORD_SEP}"
        );
        let tabs = parse_tabs(&raw);
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[1].tab_index, 2);
        assert_eq!(tabs[1].title, "Beta");
    }

    #[test]
    fn a_title_containing_a_tab_or_pipe_does_not_corrupt_the_listing() {
        // The reason the separators are control characters.
        let raw =
            format!("1{FIELD_SEP}1{FIELD_SEP}https://a.example{FIELD_SEP}A | B\tC{RECORD_SEP}");
        let tabs = parse_tabs(&raw);
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].title, "A | B\tC");
    }

    // -- Tab matching, against the titles that actually broke it -----------

    /// The four PlayerZero tabs that were open when this rule was wrong.
    const REAL_TITLES: [&str; 4] = [
        "Add scenario to simulation playlist | PlayerZero",
        "Clone the UI repo | PlayerZero",
        "Help Creating a PPT | PlayerZero",
        "Radius mismatch after clearing filters | PlayerZero",
    ];

    #[test]
    fn two_words_find_a_one_word_name() {
        // "player zero" is not a substring of "PlayerZero". Per-word matching
        // is what makes this work, and substring matching is what broke it.
        assert!(tab_matches("player zero", REAL_TITLES[0], ""));
        assert!(tab_matches("PLAYER ZERO", REAL_TITLES[1], ""));
        assert!(tab_matches("playerzero", REAL_TITLES[2], ""));
    }

    #[test]
    fn dropped_filler_words_no_longer_break_the_match() {
        // The resolver strips "the", so the query arrives as "clone ui repo"
        // while the title still says "Clone the UI repo". Word matching does
        // not care about the gap; substring matching did.
        assert!(tab_matches("clone ui repo", REAL_TITLES[1], ""));
        assert!(tab_matches("clone repo", REAL_TITLES[1], ""));
    }

    #[test]
    fn word_order_does_not_matter() {
        assert!(tab_matches("repo clone", REAL_TITLES[1], ""));
        assert!(tab_matches("ppt creating help", REAL_TITLES[2], ""));
    }

    #[test]
    fn a_word_that_is_not_there_fails_the_match() {
        // All words must appear, or "clone the android repo" would happily
        // match the UI one.
        assert!(!tab_matches("clone android repo", REAL_TITLES[1], ""));
        assert!(!tab_matches("jira", REAL_TITLES[0], ""));
    }

    #[test]
    fn the_url_counts_as_well_as_the_title() {
        assert!(tab_matches(
            "atlassian",
            "PROJ-1069",
            "https://your-team.atlassian.net/browse/PROJ-1069"
        ));
        assert!(tab_matches(
            "proj 1069",
            "Some issue",
            "https://your-team.atlassian.net/browse/PROJ-1069"
        ));
    }

    #[test]
    fn a_specific_query_narrows_to_one_of_several() {
        // Four tabs share "PlayerZero"; naming the distinguishing words picks
        // exactly one.
        let hits: Vec<&str> = REAL_TITLES
            .iter()
            .copied()
            .filter(|t| tab_matches("help creating a ppt", t, ""))
            .collect();
        assert_eq!(hits, vec![REAL_TITLES[2]]);
    }

    #[test]
    fn a_shared_word_matches_all_of_them() {
        let hits = REAL_TITLES
            .iter()
            .filter(|t| tab_matches("playerzero", t, ""))
            .count();
        assert_eq!(hits, 4, "the caller is told how many others matched");
    }

    #[test]
    fn an_empty_query_matches_nothing() {
        assert!(!tab_matches("", REAL_TITLES[0], ""));
        assert!(!tab_matches("   ", REAL_TITLES[0], ""));
        assert!(!tab_matches("!!!", REAL_TITLES[0], ""));
    }

    #[test]
    fn a_malformed_record_is_skipped_not_fatal() {
        let raw = format!("garbage{RECORD_SEP}1{FIELD_SEP}1{FIELD_SEP}https://a{FIELD_SEP}A");
        assert_eq!(parse_tabs(&raw).len(), 1);
    }

    #[test]
    fn an_empty_listing_yields_no_tabs() {
        assert!(parse_tabs("").is_empty());
        assert!(parse_tabs("   ").is_empty());
    }

    // -- Registry consistency -------------------------------------------------

    #[test]
    fn javascript_injection_is_execute_level_and_always_confirms() {
        // Tier 3 can do anything the page can. It must never sit at Interact.
        for id in ["browser.read_page", "browser.click", "browser.type"] {
            let spec = ACTIONS.iter().find(|s| s.id == id).expect(id);
            assert_eq!(spec.permission, Permission::Execute, "{id}");
            assert_eq!(spec.confirm, ConfirmPolicy::Always, "{id}");
        }
    }

    #[test]
    fn reading_is_read_and_navigating_is_interact() {
        let listing = ACTIONS
            .iter()
            .find(|s| s.id == "browser.list_tabs")
            .expect("present");
        assert_eq!(listing.permission, Permission::Read);
        for id in ["browser.open_url", "browser.search", "browser.navigate"] {
            let spec = ACTIONS.iter().find(|s| s.id == id).expect(id);
            assert_eq!(spec.permission, Permission::Interact, "{id}");
        }
    }

    #[test]
    fn action_ids_are_unique_and_namespaced() {
        let mut seen = std::collections::HashSet::new();
        for spec in ACTIONS {
            assert!(seen.insert(spec.id), "duplicate {}", spec.id);
            assert!(spec.id.starts_with("browser."), "{}", spec.id);
            assert_eq!(spec.connector_id, CONNECTOR_ID);
        }
    }

    #[test]
    fn the_devtools_protocol_is_never_reached_for() {
        let production = include_str!("browser_connector.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("the file must keep its test module marker");
        // Named in the module doc as rejected; must not appear as an argument.
        assert!(
            !production.contains("--remote-debugging-port="),
            "the DevTools protocol must not be used"
        );
        assert!(
            !production.contains("cliclick") && !production.contains("CGEvent"),
            "coordinate automation must not be used"
        );
    }

    #[test]
    fn permission_errors_name_the_remedy_not_just_the_code() {
        // Apple's messages say what failed, never what to do about it.
        let denied = from_script_failure("execution error: Not allowed (-1743)");
        assert!(format!("{denied:?}").contains("Automation"), "{denied:?}");

        let js = from_script_failure("Chrome cannot run javascript from apple events");
        assert!(format!("{js:?}").contains("Allow JavaScript"), "{js:?}");

        let closed = from_script_failure("Connection is invalid. (-609)");
        assert!(format!("{closed:?}").contains("running"), "{closed:?}");
    }
}
