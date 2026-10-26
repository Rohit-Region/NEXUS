//! Avetta service versions, read from the page you already have open.
//!
//! # Why it works this way
//!
//! `int-app.avetta.com/serviceVersions.html?env=kai` is behind Avetta SSO:
//! every version endpoint answers `302 -> /login` without a session, and the
//! login is a token/SSO handshake, not a form a stored password can drive. So
//! NEXUS does not authenticate. It reads the page **you** are logged into.
//!
//! The page is an Elm app that fetches the versions client-side and renders
//! them. Chrome's AppleScript `execute javascript` bridge returns
//! synchronously and will not await a `fetch`, so NEXUS cannot re-issue those
//! calls itself. It does not need to: by the time you are looking at the
//! page, the browser has already made the authenticated requests and put the
//! answers in the DOM. NEXUS reads the rendered result out of that one tab.
//!
//! # What it does not do
//!
//! It runs JavaScript in a **named** tab, the Avetta one, found by URL, never
//! in whatever tab happens to be focused. A card that refreshes on a timer
//! must not fire script into your bank tab because it was in front. And it
//! reads: it scrapes text, clicks nothing, submits nothing.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::action::{ActionError, ActionSpec};
use super::connector::{
    Capabilities, Connector, ConnectorStatus, ExecCtx, UnavailableAction,
};
use super::permission::{ConfirmPolicy, Permission, Reach};
use super::shell::{osascript, RunError};

pub const CONNECTOR_ID: &str = "kai";

/// The host that serves the versions page, matched against open tabs.
const AVETTA_HOST: &str = "int-app.avetta.com";

const fn spec(
    id: &'static str,
    summary: &'static str,
    permission: Permission,
    confirm: ConfirmPolicy,
) -> ActionSpec {
    ActionSpec {
        id,
        connector_id: CONNECTOR_ID,
        summary,
        permission,
        confirm,
        // Reading a page NEXUS did not open, over a session it did not
        // create. Nothing of NEXUS's own leaves the machine.
        reach: Reach::LocalOnly,
        reversible: true,
    }
}

pub const ACTIONS: &[ActionSpec] = &[spec(
    "kai.service_versions",
    // Named for what people ask: "what version is KAI on", "service versions".
    "Check the KAI service versions",
    // Read, not Execute, even though it runs a script. The browser
    // connector's read_page is Execute because it runs *arbitrary* JavaScript
    // handed in as input, which can do anything. This runs one fixed,
    // input-free line, document.body.innerText, against a tab the user chose
    // to open. That is the same risk as reading unread mail, and it is rated
    // the same: Read, never confirmed, so a card can refresh without a prompt
    // on every tick.
    Permission::Read,
    ConfirmPolicy::Never,
)];

/// One service and the version it is running.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceVersion {
    pub service: String,
    pub version: String,
}

/// Which environment column to read. Optional: the card sends one, and when it
/// does not, the tab's own `?env=` (or KAI) is used.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct VersionsInput {
    env: Option<String>,
}

/// A Chromium browser NEXUS can read a tab out of.
///
/// Both Chrome and Edge expose the same AppleScript `execute javascript`
/// bridge, so the scrape is identical; only the application name differs. Edge
/// is here because it is a common default browser in a Microsoft-managed
/// tenant, and "Open & sign in" opens the page in whatever the default is. If
/// the versions page is signed into in Edge, NEXUS has to be able to read Edge,
/// or it reports "not open" about a page that is plainly open.
struct Browser {
    /// The AppleScript application name.
    app: &'static str,
    /// Where it installs, to tell present from absent without launching it.
    path: &'static str,
}

const BROWSERS: &[Browser] = &[
    Browser {
        app: "Google Chrome",
        path: "/Applications/Google Chrome.app",
    },
    Browser {
        app: "Microsoft Edge",
        path: "/Applications/Microsoft Edge.app",
    },
];

/// Those actually installed, in preference order.
fn installed_browsers() -> Vec<&'static Browser> {
    BROWSERS
        .iter()
        .filter(|b| std::path::Path::new(b.path).exists())
        .collect()
}

fn any_browser_installed() -> bool {
    !installed_browsers().is_empty()
}

/// Read the rendered version rows out of the Avetta tab.
///
/// The scrape is deliberately loose. The page is a third party's and its
/// markup can change under us; what does not change is that a version reads
/// as a dotted number next to a name. So this pulls every `name … x.y.z`
/// pairing out of the visible text and lets the parser below be the contract,
/// not a brittle CSS selector.
fn read_versions_script(env: &str, browser: &str) -> String {
    // The page is a matrix: services down the side, one column per environment
    // (CFG, CN, ... KAI, ... TRN) across the top. Reading a whole row and
    // taking the first version found returned the wrong environment, usually
    // the leftmost column. So this finds the column whose header matches the
    // wanted environment and reads only that cell per row, emitting
    // "service<TAB>value" lines. The value can be a version ("2026.09.09") or
    // a branch ("main", "norm-otelv2"); both are what is deployed there, so
    // both are shown. A "-" means nothing is deployed and is skipped.
    // `innerText` skips the page's hidden legacy rows. If the header is missing
    // it falls back to the first value cell, so a layout change degrades rather
    // than blanks.
    //
    // Built by string replacement, not `format!`: the JS is full of braces and
    // `format!`'s brace-doubling turns a function body into invalid syntax,
    // which is exactly the bug that made this return nothing.
    const TEMPLATE: &str = "(function(){var rows=document.querySelectorAll('tr');if(!rows.length)return '';var TAB=String.fromCharCode(9),NL=String.fromCharCode(10);var head=Array.from(rows[0].querySelectorAll('th,td')).map(function(c){return c.innerText.trim().toUpperCase();});var idx=head.indexOf('__ENV__');var out=[];for(var i=1;i<rows.length;i++){var cells=Array.from(rows[i].querySelectorAll('td,th'));if(!cells.length)continue;var name=cells[0].innerText.trim();if(!name)continue;var val='';if(idx>0&&idx<cells.length){val=cells[idx].innerText.trim();}else{for(var j=1;j<cells.length;j++){var t=cells[j].innerText.trim();if(t&&t!=='-'){val=t;break;}}}if(!val||val==='-')continue;out.push(name+TAB+val);}return out.join(NL);})()";

    // The env is our own constant, uppercased, so no injection concern; still,
    // keep it to letters and digits so it can never break out of the quotes.
    let safe: String = env
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_uppercase();
    let js = TEMPLATE.replace("__ENV__", &safe);

    // Find the Avetta tab and run the scrape in it inside ONE script, using the
    // tab object rather than a window/tab index looked up by a separate call.
    // The old two-step (list the tabs, then `execute tab N of window M`) raced:
    // a tab closing or moving between the two left N pointing at the wrong tab
    // or none, which is the "Can't get tab 10 of window 2, Invalid index"
    // error. Iterating and executing on the matched tab here cannot go stale.
    let mut s = String::new();
    // Do not launch a closed browser just to read it. If the app is not already
    // running, there is no tab to scrape, so return empty rather than opening a
    // window the user never asked for.
    s.push_str("tell application \"System Events\"\n");
    s.push_str(&format!("if not (exists process \"{browser}\") then return \"\"\n"));
    s.push_str("end tell\n");
    s.push_str(&format!("tell application \"{browser}\"\n"));
    s.push_str("set out to \"\"\n");
    s.push_str("repeat with w in windows\n");
    s.push_str("repeat with t in tabs of w\n");
    s.push_str("if (URL of t) contains \"");
    s.push_str(AVETTA_HOST);
    s.push_str("\" then\n");
    s.push_str("set out to (execute t javascript \"");
    s.push_str(&js);
    s.push_str("\")\n");
    s.push_str("exit repeat\n");
    s.push_str("end if\n");
    s.push_str("end repeat\n");
    s.push_str("if out is not \"\" then exit repeat\n");
    s.push_str("end repeat\n");
    s.push_str("return out\n");
    s.push_str("end tell");
    s
}/// Turn the page's visible text into service/version pairs.
///
/// A line counts when it contains a version-shaped token: two or more dot
/// separated numbers, optionally with a v prefix or a build suffix. The
/// service is whatever text sits on the line before it, cleaned of the
/// version itself. Lines with no version are structure, not data, and are
/// dropped.
pub fn parse_versions(text: &str) -> Vec<ServiceVersion> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }

        // The scrape emits one "service<TAB>value" line per row, already
        // narrowed to the wanted environment's column. The value is whatever
        // that column holds: a version, or a branch name, both of which are
        // shown as-is.
        if let Some((service, value)) = line.split_once('\t') {
            let service = service.trim();
            let value = value.trim();
            if !service.is_empty() && !value.is_empty() && value != "-" {
                out.push(ServiceVersion {
                    service: service.to_string(),
                    version: value.to_string(),
                });
            }
            continue;
        }

        // Fallback for a line with no tab ("branding 2.0.11"): pull the
        // version-shaped token out and keep the rest as the name.
        let Some(version) = find_version(line) else {
            continue;
        };
        let service = line.replacen(&version, "", 1);
        let service = service
            .trim_matches(|c: char| !c.is_alphanumeric())
            .trim()
            .to_string();
        if service.is_empty() {
            continue;
        }
        out.push(ServiceVersion { service, version });
    }
    out
}

/// The first version-shaped token in a line, if any.
///
/// "Version-shaped" is intentionally narrow: at least two dot separated
/// numeric groups, so a lone year or a single integer is not mistaken for a
/// version. A leading `v` and a trailing `-build.7` style suffix are kept.
fn find_version(line: &str) -> Option<String> {
    for token in line.split(|c: char| c.is_whitespace() || c == '(' || c == ')') {
        let trimmed = token.trim_matches(|c: char| c == ',' || c == ';' || c == ':');
        let core = trimmed.strip_prefix('v').unwrap_or(trimmed);
        let dot_groups: Vec<&str> = core.split('.').collect();
        if dot_groups.len() >= 2
            && dot_groups.len() <= 5
            && dot_groups
                .iter()
                .take(3)
                .all(|g| !g.is_empty() && g.chars().take_while(|c| c.is_ascii_digit()).count() > 0)
            && core.chars().next().is_some_and(|c| c.is_ascii_digit())
        {
            return Some(trimmed.to_string());
        }
    }
    None
}

fn map_run_error(err: RunError) -> ActionError {
    ActionError::Failed {
        detail: err.to_string(),
    }
}

pub struct KaiConnector;

impl Connector for KaiConnector {
    fn id(&self) -> &'static str {
        CONNECTOR_ID
    }

    fn display_name(&self) -> &'static str {
        "KAI"
    }

    fn actions(&self) -> &'static [ActionSpec] {
        ACTIONS
    }

    fn capabilities(&self, _conn: &Connection) -> Capabilities {
        if any_browser_installed() {
            Capabilities {
                available: ACTIONS.iter().map(|s| s.id.to_string()).collect(),
                unavailable: Vec::new(),
            }
        } else {
            Capabilities {
                available: Vec::new(),
                unavailable: ACTIONS
                    .iter()
                    .map(|s| UnavailableAction {
                        action_id: s.id.to_string(),
                        reason: "Neither Chrome nor Edge is installed on this Mac.".to_string(),
                    })
                    .collect(),
            }
        }
    }

    fn status(&self, _conn: &Connection) -> ConnectorStatus {
        // Ready means "could read if asked", which needs a supported browser and
        // nothing else. Whether the Avetta tab is open is a per-call answer, not
        // a standing status, and is reported by the action rather than the dot.
        if any_browser_installed() {
            ConnectorStatus::Ready
        } else {
            ConnectorStatus::Unavailable
        }
    }

    fn summarize(&self, _action_id: &str, _input: &serde_json::Value, _conn: &Connection) -> String {
        "Read the KAI service versions from the open Avetta tab".to_string()
    }

    fn describe_result(&self, _action_id: &str, output: &serde_json::Value) -> Option<String> {
        let versions = output.get("versions")?.as_array()?;
        if versions.is_empty() {
            return Some("No service versions were on the page.".to_string());
        }
        Some(format!("{} services reported their version.", versions.len()))
    }

    fn dispatch(
        &self,
        action_id: &str,
        input: serde_json::Value,
        _ctx: &ExecCtx<'_>,
    ) -> Result<serde_json::Value, ActionError> {
        if action_id != "kai.service_versions" {
            return Err(ActionError::Failed {
                detail: format!("KAI has no action {action_id}."),
            });
        }
        let browsers = installed_browsers();
        if browsers.is_empty() {
            return Err(ActionError::Failed {
                detail: "Neither Chrome nor Edge is installed on this Mac.".to_string(),
            });
        }

        // The card names the environment it wants, defaulting to KAI.
        let env = serde_json::from_value::<VersionsInput>(input)
            .ok()
            .and_then(|i| i.env)
            .filter(|e| !e.trim().is_empty())
            .unwrap_or_else(|| "kai".to_string());

        // Try each installed browser until one has the Avetta tab with rendered
        // versions. The default browser is where "Open & sign in" lands, and
        // that is Edge as often as Chrome, so reading only one would miss a page
        // that is plainly open in the other. A browser that is not running, has
        // no such tab, or refuses the script (Apple Events JavaScript turned
        // off) just falls through to the next.
        let mut script_error: Option<String> = None;
        for browser in &browsers {
            let out = match osascript(&[&read_versions_script(&env, browser.app)], &[]) {
                Ok(out) => out,
                Err(e) => return Err(map_run_error(e)),
            };
            if !out.success {
                // Keep the reason in case every browser fails; the usual cause
                // is "Allow JavaScript from Apple Events" being off.
                script_error = Some(out.stderr.clone());
                continue;
            }
            let versions = parse_versions(&out.stdout);
            if !versions.is_empty() {
                return Ok(serde_json::json!({
                    "env": env,
                    "versions": versions,
                }));
            }
        }

        // Nothing readable in any browser. If a script actually errored, the
        // cause is usually a permission, and it is worth naming which one.
        // Browser-neutral on purpose: the page may be open in Chrome or Edge,
        // and the fix (the same View > Developer menu item) applies to both.
        if let Some(stderr) = script_error.filter(|s| !s.trim().is_empty()) {
            let low = stderr.to_lowercase();
            let detail = if low.contains("javascript") {
                "Your browser is blocking JavaScript from Apple Events, so NEXUS cannot read the \
                 versions. Turn it on in the browser that has the page open (Chrome or Edge): \
                 View menu > Developer > Allow JavaScript from Apple Events, then refresh."
                    .to_string()
            } else if low.contains("not allowed") || stderr.contains("-1743") {
                "macOS has not given NEXUS permission to control the browser. Grant it in System \
                 Settings > Privacy & Security > Automation, then refresh."
                    .to_string()
            } else {
                stderr.trim().to_string()
            };
            return Err(ActionError::Failed { detail });
        }
        Err(ActionError::Failed {
            detail: format!(
                "Open {AVETTA_HOST} and sign in (in Chrome or Edge), then refresh. If it is \
                 already open, let the page finish loading."
            ),
        })
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_semver_is_found_and_its_service_named() {
        let v = parse_versions("united-verification-service 3.14.2");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].service, "united-verification-service");
        assert_eq!(v[0].version, "3.14.2");
    }

    #[test]
    fn a_v_prefix_and_build_suffix_survive() {
        let v = parse_versions("branding  v2.0.11-build.47");
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].version, "v2.0.11-build.47");
        assert_eq!(v[0].service, "branding");
    }

    #[test]
    fn a_service_name_with_a_number_in_it_stays_whole() {
        // The trap: stripping the version must not also strip the 2 in uvs2.
        let v = parse_versions("uvs2 1.4.0");
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].service, "uvs2");
        assert_eq!(v[0].version, "1.4.0");
    }

    #[test]
    fn structural_lines_without_a_version_are_dropped() {
        let text = "Service Versions\nKAI environment\nHeader Row\n";
        assert!(parse_versions(text).is_empty());
    }

    #[test]
    fn a_lone_year_or_integer_is_not_a_version() {
        // "Updated 2026" and "Region 12" must not read as versions: a
        // version needs at least two dotted numeric groups.
        assert!(find_version("Updated 2026").is_none());
        assert!(find_version("Region 12 online").is_none());
        assert!(find_version("process 4.2").is_some());
    }

    #[test]
    fn a_tab_separated_row_is_service_and_value() {
        // The scrape emits "service<TAB>value" for the chosen env column.
        let v = parse_versions("account-management-gateway\t2026.09.09");
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].service, "account-management-gateway");
        assert_eq!(v[0].version, "2026.09.09");
    }

    #[test]
    fn a_branch_name_in_the_column_is_shown_verbatim() {
        // The KAI column sometimes holds a branch, not a version. It is what
        // is deployed there, so it is shown as-is rather than filtered out.
        let v = parse_versions("recalc\tnorm-otelv2");
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].service, "recalc");
        assert_eq!(v[0].version, "norm-otelv2");
    }

    #[test]
    fn an_undeployed_dash_row_is_dropped() {
        assert!(parse_versions("ruleengine\t-").is_empty());
        assert!(parse_versions("job\t").is_empty());
    }

    #[test]
    fn a_real_kai_column_scrape_parses() {
        // Lines as the scrape would emit them from the real matrix, KAI column.
        let scraped = "account-management-gateway\t2026.09.09\n\
                       admin\t2026.09.17\n\
                       recalc\tnorm-noOtel\n\
                       ruleengine\t-\n\
                       process-domain\t-";
        let v = parse_versions(scraped);
        assert_eq!(v.len(), 3, "{v:?}");
        assert_eq!(v[0].service, "account-management-gateway");
        assert_eq!(v[0].version, "2026.09.09");
        assert_eq!(v[2].version, "norm-noOtel");
    }

    #[test]
    fn a_tabless_line_falls_back_to_token_parsing() {
        let v = parse_versions("branding 2.0.11");
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].service, "branding");
        assert_eq!(v[0].version, "2.0.11");
    }

    #[test]
    fn this_connector_only_reads() {
        let production = include_str!("kai_connector.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("test marker");
        // Comments describe what it does not do ("submits nothing"), so the
        // scan is of code lines only, with slash-slash lines removed.
        let code: String = production
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for write in [".click(", ".submit(", "removeEvent", ".value =", "type_here"] {
            assert!(!code.contains(write), "KAI must only read, found {write}");
        }
    }
}
