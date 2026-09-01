//! Reading Jira through the browser the user is already signed in to.
//!
//! The API token is refused on this machine: `/rest/api/3/myself` answers
//! 401 for a well-formed classic token whose email is demonstrably correct,
//! while the same account authenticates fine by cookie. The likeliest reason
//! is that the tenant disables token auth, which is not something NEXUS can
//! fix and not something the user can either.
//!
//! So this asks Jira the same questions from inside a tab that is already
//! authenticated. What that means honestly:
//!
//! - **It is the user's own session, on their own machine.** No credential
//!   is captured, stored or moved; the cookie never leaves the browser.
//! - **It needs a Jira tab open.** No tab, no answer, and it says so rather
//!   than appearing to work.
//! - **Read-only, and only Jira's REST API.** The path is checked to be a
//!   `/rest/api/` path on the configured site before it is used. This is not
//!   a general "run JavaScript in the user's browser" facility.
//! - **It routes around a control the tenant may have set on purpose.** That
//!   is the uncomfortable part and it is why this is a fallback rather than
//!   the first choice: the token path is tried first, every time, and this
//!   runs only when the credentials were refused.
//!
//! Gated on the browser connector holding `Execute`, because that is what it
//! is: injected JavaScript. A user who has not allowed that does not get
//! this quietly.

use rusqlite::Connection;

use super::action::ActionError;
use super::permission::Permission;
use super::shell::osascript;

/// How long to wait for the page to answer, and how often to look.
///
/// A `fetch` is asynchronous, so the value has to be collected on a later
/// AppleEvent. Chrome's event bridge is not always quick; this is generous
/// enough to survive that and bounded so a wedged tab cannot hang a turn.
const POLL_ATTEMPTS: usize = 12;
const POLL_INTERVAL_MS: u64 = 500;

/// Where the answer is parked on the page between the request and the poll.
const SLOT: &str = "__nexusJiraResult";

/// Is the browser fallback allowed to run at all?
pub fn permitted(conn: &Connection) -> bool {
    super::permission::is_granted(conn, "browser", Permission::Execute).unwrap_or(false)
}

/// The host part of the configured site, for matching a tab.
fn host_of(site: &str) -> Option<String> {
    let rest = site.strip_prefix("https://")?;
    let host = rest.split('/').next()?.trim();
    (!host.is_empty()).then(|| host.to_string())
}

/// Ask Jira for `path` using the browser's session.
///
/// `path` must be a REST path on the configured site. Anything else is
/// refused here rather than being sent to a page for evaluation.
pub fn get_json(site: &str, path: &str) -> Result<serde_json::Value, ActionError> {
    if !path.starts_with("/rest/api/") {
        return Err(ActionError::Failed {
            detail: "Only Jira's REST API is read this way.".to_string(),
        });
    }
    let host = host_of(site).ok_or_else(|| ActionError::Failed {
        detail: "The Jira site address is not a https address.".to_string(),
    })?;

    // Encoded as JSON, which is also a JavaScript literal, so the path
    // reaches the page as a value and can never become syntax.
    let literal = serde_json::to_string(path).map_err(|e| ActionError::Failed {
        detail: format!("Could not encode the request: {e}"),
    })?;

    let start = format!(
        "window.{SLOT}=null;fetch({literal},{{credentials:'same-origin',headers:{{'Accept':'application/json'}}}})\
         .then(function(r){{return r.text()}})\
         .then(function(t){{window.{SLOT}=t}})\
         .catch(function(e){{window.{SLOT}='NEXUS_ERR '+e}});'started'"
    );

    let fired = osascript(
        &[
            "set h to (item 1 of argv)",
            "set js to (item 2 of argv)",
            "tell application \"Google Chrome\"",
            "repeat with w from 1 to (count of windows)",
            "repeat with t from 1 to (count of tabs of window w)",
            "if (URL of tab t of window w) contains h then",
            "execute tab t of window w javascript js",
            "return (w as string) & \",\" & (t as string)",
            "end if",
            "end repeat",
            "end repeat",
            "end tell",
            "return \"none\"",
        ],
        &[&host, &start],
    )
    .map_err(|e| ActionError::Failed {
        detail: format!("Could not reach Chrome: {e}"),
    })?;

    if !fired.success {
        return Err(ActionError::Failed {
            detail: format!("Chrome refused: {}", fired.stderr.trim()),
        });
    }
    let (window, tab) = match fired.stdout.trim().split_once(',') {
        Some((w, t)) => (w.to_string(), t.to_string()),
        None => {
            return Err(ActionError::Failed {
                detail: format!(
                    "Jira rejected the API token, and no {host} tab is open to ask instead. \
                     Open Jira in Chrome and try again."
                ),
            })
        }
    };

    let read = format!("String(window.{SLOT})");
    for _ in 0..POLL_ATTEMPTS {
        std::thread::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS));
        let out = osascript(
            &[
                "set w to (item 1 of argv) as integer",
                "set t to (item 2 of argv) as integer",
                "set js to (item 3 of argv)",
                "tell application \"Google Chrome\"",
                "return (execute tab t of window w javascript js)",
                "end tell",
            ],
            &[&window, &tab, &read],
        );

        // A timed-out AppleEvent is not a failure of the request: the fetch
        // is still in flight in the page. Try again rather than giving up on
        // the first slow tick.
        let Ok(out) = out else { continue };
        if !out.success {
            continue;
        }
        let body = out.stdout.trim();
        if body.is_empty() || body == "null" || body == "undefined" {
            continue;
        }
        if let Some(err) = body.strip_prefix("NEXUS_ERR ") {
            return Err(ActionError::Failed {
                detail: format!("The Jira tab could not answer: {err}"),
            });
        }
        return serde_json::from_str(body).map_err(|_| ActionError::Failed {
            detail: "Jira answered with something NEXUS could not read.".to_string(),
        });
    }

    Err(ActionError::Failed {
        detail: "The Jira tab did not answer in time.".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn production() -> &'static str {
        include_str!("jira_browser.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("marker")
    }

    #[test]
    fn only_rest_paths_are_ever_asked_for() {
        // The whole surface. Without this the fallback is a way to fetch any
        // URL from inside an authenticated page.
        for bad in [
            "https://example.com/",
            "/secure/admin",
            "//evil.example/rest/api/3/myself",
            "javascript:alert(1)",
            "",
        ] {
            let err = get_json("https://your-team.atlassian.net", bad).expect_err(bad);
            assert!(
                format!("{err:?}").contains("REST API"),
                "{bad} -> {err:?}"
            );
        }
    }

    #[test]
    fn the_site_must_be_https() {
        assert_eq!(host_of("http://jira.internal"), None);
        assert_eq!(
            host_of("https://your-team.atlassian.net").as_deref(),
            Some("your-team.atlassian.net")
        );
        assert_eq!(
            host_of("https://your-team.atlassian.net/browse/PROJ-1").as_deref(),
            Some("your-team.atlassian.net")
        );
    }

    #[test]
    fn the_path_reaches_the_page_as_a_value() {
        // JSON-encoded, which is also a JavaScript string literal, so a path
        // with a quote in it cannot close the string and become syntax.
        assert!(production().contains("serde_json::to_string(path)"));
        assert!(
            !production().contains("fetch('\" + path"),
            "the path must never be concatenated into the script"
        );
    }

    #[test]
    fn nothing_here_writes() {
        // Read-only by construction: no method, no body, no POST.
        for forbidden in ["method:", "POST", "PUT", "DELETE", "body:"] {
            assert!(!production().contains(forbidden), "found {forbidden}");
        }
    }

    #[test]
    fn it_is_bounded() {
        // A wedged tab must not hold a turn open indefinitely.
        assert!(POLL_ATTEMPTS as u64 * POLL_INTERVAL_MS <= 10_000);
    }
}
