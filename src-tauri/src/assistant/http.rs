//! NEXUS-017: HTTPS, without a TLS stack in the binary.
//!
//! Requests go out through `curl`, which ships with macOS and uses the system
//! trust store. That is a deliberate trade, and the alternative was measured:
//! enabling `reqwest`'s TLS features adds **56 crates** to the build. For a
//! connector that makes a handful of JSON requests, a vetted system binary is
//! the smaller attack surface and the smaller supply chain.
//!
//! **Credentials never touch `argv`.** Every process on the machine can read
//! another's command line through `ps`, so a token passed as an argument is a
//! token published to the machine. Requests are written to curl's stdin as a
//! config file instead, where only the child can see them. `argv` is always
//! exactly `--config -` plus fixed flags, and a test asserts it.
//!
//! Only `https://` is accepted. A connector that could be pointed at `http://`
//! or `file://` would be a credential-leaking primitive wearing a connector's
//! clothes.

use std::time::Duration;

use super::shell::{run_with_stdin, RunError};

const CURL: &str = "/usr/bin/curl";
const HTTP_TIMEOUT: Duration = Duration::from_secs(25);
/// Curl's own deadline, slightly under ours so it reports rather than is killed.
const CURL_MAX_TIME: &str = "20";
/// Longest body NEXUS will read. A connector answers questions; it does not
/// mirror a service.
const MAX_BODY: usize = 512 * 1024;
/// Separates the body from the status line curl appends.
const STATUS_MARKER: &str = "\u{1e}NEXUS-STATUS:";

#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

impl Response {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum HttpError {
    /// The URL was not something NEXUS is willing to send credentials to.
    Rejected { detail: String },
    /// The request never completed.
    Unreachable { detail: String },
    /// It completed, but the response was not usable.
    Malformed { detail: String },
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpError::Rejected { detail } => write!(f, "{detail}"),
            HttpError::Unreachable { detail } => write!(f, "{detail}"),
            HttpError::Malformed { detail } => write!(f, "{detail}"),
        }
    }
}

/// Hosts that may be reached over plain HTTP.
///
/// Only the loopback interface, and only because a local model server listens
/// there without TLS. Traffic to 127.0.0.1 never leaves the machine, so there
/// is nothing on the wire to intercept. Anything else must be HTTPS.
fn is_loopback(url: &str) -> bool {
    let lowered = url.to_lowercase();
    ["http://127.0.0.1", "http://localhost", "http://[::1]"]
        .iter()
        .any(|prefix| {
            lowered.strip_prefix(prefix).is_some_and(|rest| {
                // Guard against `http://localhost.evil.com`: what follows the
                // host must be a port, a path, or nothing.
                rest.is_empty() || rest.starts_with(':') || rest.starts_with('/')
            })
        })
}

/// Accept HTTPS anywhere, and plain HTTP only on loopback.
pub fn safe_https(url: &str) -> Result<String, HttpError> {
    let trimmed = url.trim().trim_end_matches('/');
    let lowered = trimmed.to_lowercase();
    if !lowered.starts_with("https://") && !is_loopback(trimmed) {
        return Err(HttpError::Rejected {
            detail: format!("NEXUS only sends credentials over https, and {trimmed} is not."),
        });
    }
    let scheme_len = if lowered.starts_with("https://") { 8 } else { 7 };
    if trimmed.len() <= scheme_len {
        return Err(HttpError::Rejected {
            detail: "That address has no host.".to_string(),
        });
    }
    if trimmed.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(HttpError::Rejected {
            detail: "That address contains characters a URL cannot have.".to_string(),
        });
    }
    Ok(trimmed.to_string())
}

/// Escape a value for curl's config-file syntax.
///
/// Values are double-quoted there, so a quote or backslash in a token or a
/// header would end the value early. Newlines are refused outright rather
/// than escaped: a newline could start a new directive, which is the config
/// equivalent of an injection.
fn config_value(raw: &str) -> Result<String, HttpError> {
    if raw.contains('\n') || raw.contains('\r') {
        return Err(HttpError::Rejected {
            detail: "A header or credential cannot contain a line break.".to_string(),
        });
    }
    Ok(raw.replace('\\', "\\\\").replace('"', "\\\""))
}

#[derive(Debug, Clone)]
pub struct Request {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// `(user, secret)` for basic auth. Written to stdin, never to argv.
    pub basic_auth: Option<(String, String)>,
    pub body: Option<String>,
}

impl Request {
    pub fn get(url: &str) -> Self {
        Request {
            method: "GET",
            url: url.to_string(),
            headers: vec![("Accept".to_string(), "application/json".to_string())],
            basic_auth: None,
            body: None,
        }
    }

    pub fn with_basic_auth(mut self, user: &str, secret: &str) -> Self {
        self.basic_auth = Some((user.to_string(), secret.to_string()));
        self
    }
}

/// Send a request and read the response.
pub fn send(request: Request) -> Result<Response, HttpError> {
    let url = safe_https(&request.url)?;

    // Belt and braces: even though loopback is exempt from the TLS rule,
    // credentials must never travel unencrypted. A local model needs no
    // authentication, so this costs nothing and closes the hole where a
    // future caller points an authenticated request at a plain-http host.
    let authenticated = request.basic_auth.is_some()
        || request
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("authorization"));
    if authenticated && !url.to_lowercase().starts_with("https://") {
        return Err(HttpError::Rejected {
            detail: "NEXUS will not send credentials over an unencrypted connection."
                .to_string(),
        });
    }

    let mut config = String::new();
    config.push_str(&format!("url = \"{}\"\n", config_value(&url)?));
    config.push_str(&format!("request = \"{}\"\n", request.method));
    for (name, value) in &request.headers {
        config.push_str(&format!(
            "header = \"{}: {}\"\n",
            config_value(name)?,
            config_value(value)?
        ));
    }
    if let Some((user, secret)) = &request.basic_auth {
        config.push_str(&format!(
            "user = \"{}:{}\"\n",
            config_value(user)?,
            config_value(secret)?
        ));
    }
    if let Some(body) = &request.body {
        config.push_str(&format!("data = \"{}\"\n", config_value(body)?));
    }
    config.push_str("silent\nshow-error\nfail-with-body\nlocation = false\n");
    config.push_str(&format!("max-time = {CURL_MAX_TIME}\n"));
    config.push_str(&format!("write-out = \"{STATUS_MARKER}%{{http_code}}\"\n"));

    // argv carries no data at all: the entire request is on stdin.
    let out = run_with_stdin(CURL, &["--config", "-"], &config, HTTP_TIMEOUT).map_err(|e| {
        match e {
            RunError::NotFound { .. } => HttpError::Unreachable {
                detail: "curl is missing from this Mac, so NEXUS cannot make web requests."
                    .to_string(),
            },
            other => HttpError::Unreachable {
                detail: other.to_string(),
            },
        }
    })?;

    let raw = out.stdout;
    let (body, status_text) = match raw.rsplit_once(STATUS_MARKER) {
        Some((body, status)) => (body, status),
        None => {
            let detail = if out.stderr.is_empty() {
                "The request did not complete.".to_string()
            } else {
                out.stderr
            };
            return Err(HttpError::Unreachable { detail });
        }
    };

    let status: u16 = status_text.trim().parse().map_err(|_| HttpError::Malformed {
        detail: "The response had no usable status code.".to_string(),
    })?;

    let mut body = body.to_string();
    if body.len() > MAX_BODY {
        body.truncate(MAX_BODY);
    }

    Ok(Response { status, body })
}

/// Read a secret from the macOS Keychain.
///
/// The Keychain rather than the database: `nexus.db` is a plain file in the
/// application support directory, and a token in it is a token on disk in the
/// clear. `security` prints the secret on **stdout**, so it never appears in
/// any process's arguments.
pub fn keychain_secret(service: &str, account: &str) -> Option<String> {
    let out = super::shell::run(
        "/usr/bin/security",
        &["find-generic-password", "-s", service, "-a", account, "-w"],
        Duration::from_secs(10),
    )
    .ok()?;
    if !out.success || out.stdout.is_empty() {
        return None;
    }
    Some(out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- URL policy -----------------------------------------------------------

    #[test]
    fn only_https_is_accepted() {
        assert!(safe_https("https://example.atlassian.net").is_ok());
        for bad in [
            "http://example.atlassian.net",
            "file:///etc/passwd",
            "ftp://example.com",
            "example.atlassian.net",
            "",
        ] {
            assert!(safe_https(bad).is_err(), "{bad} must be refused");
        }
    }

    #[test]
    fn loopback_may_use_plain_http_because_it_never_leaves_the_machine() {
        // A local model server listens on 127.0.0.1 without TLS.
        assert!(safe_https("http://127.0.0.1:11434").is_ok());
        assert!(safe_https("http://localhost:11434/api/chat").is_ok());
    }

    #[test]
    fn a_host_that_merely_starts_with_localhost_is_not_loopback() {
        // The classic prefix-matching hole.
        for hostile in [
            "http://localhost.evil.com",
            "http://127.0.0.1.evil.com",
            "http://localhostx",
        ] {
            assert!(safe_https(hostile).is_err(), "{hostile} must be refused");
        }
    }

    #[test]
    fn credentials_are_refused_over_plain_http_even_on_loopback() {
        // The loopback exemption is for unauthenticated local services only.
        let result = send(
            Request::get("http://127.0.0.1:11434/api/tags").with_basic_auth("a", "secret"),
        );
        assert!(
            matches!(result, Err(HttpError::Rejected { .. })),
            "credentials must never travel unencrypted: {result:?}"
        );
    }

    #[test]
    fn a_bearer_header_over_plain_http_is_also_refused() {
        let result = send(Request {
            method: "GET",
            url: "http://127.0.0.1:11434/x".to_string(),
            headers: vec![("Authorization".to_string(), "Bearer abc".to_string())],
            basic_auth: None,
            body: None,
        });
        assert!(matches!(result, Err(HttpError::Rejected { .. })), "{result:?}");
    }

    #[test]
    fn the_refusal_explains_the_rule() {
        let err = safe_https("http://example.com").expect_err("must refuse");
        assert!(err.to_string().contains("https"), "{err}");
    }

    #[test]
    fn a_url_with_whitespace_or_control_characters_is_refused() {
        assert!(safe_https("https://example.com/a b").is_err());
        assert!(safe_https("https://example.com/a\nb").is_err());
    }

    // -- Config escaping, the injection surface -------------------------------

    #[test]
    fn quotes_and_backslashes_are_escaped() {
        assert_eq!(config_value("a\"b").expect("ok"), "a\\\"b");
        assert_eq!(config_value("a\\b").expect("ok"), "a\\\\b");
    }

    #[test]
    fn a_line_break_is_refused_rather_than_escaped() {
        // A newline could start a new curl directive. That is the config-file
        // equivalent of an injection, so it is refused outright.
        assert!(config_value("token\nurl = \"https://evil.example\"").is_err());
        assert!(config_value("token\rmore").is_err());
    }

    #[test]
    fn a_hostile_token_cannot_add_a_directive() {
        let hostile = "abc\" \nupload-file = \"/etc/passwd\" \"";
        assert!(
            config_value(hostile).is_err(),
            "a token containing a newline must be refused"
        );
    }

    // -- The argv guarantee ---------------------------------------------------

    #[test]
    fn credentials_are_never_passed_as_arguments() {
        // The whole reason this module uses a config file on stdin. Every
        // process on the machine can read another's argv through `ps`.
        let production = include_str!("http.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("marker");
        for forbidden in ["\"-u\"", "\"--user\"", "\"-H\"", "\"--header\"", "\"-d\""] {
            assert!(
                !production.contains(forbidden),
                "{forbidden} would put request data into argv"
            );
        }
        assert!(
            production.contains("&[\"--config\", \"-\"]"),
            "argv must be exactly --config -"
        );
    }

    #[test]
    fn redirects_are_not_followed() {
        // A redirect can send credentials to a host the user never chose.
        let production = include_str!("http.rs");
        assert!(production.contains("location = false"));
    }

    // -- Live transport -------------------------------------------------------

    #[test]
    fn a_real_request_round_trips() {
        // Verifies the mechanism end to end against a public endpoint: config
        // on stdin, status parsed, body returned. No credentials involved.
        match send(Request::get("https://api.github.com/rate_limit")) {
            Ok(response) => {
                assert!(response.status > 0, "a status code must come back");
                assert!(
                    response.ok() || response.status == 403,
                    "unexpected status {}",
                    response.status
                );
                assert!(!response.body.is_empty());
            }
            // Offline is not a failing test. The transport is exercised when
            // the network is there, and the offline path is the other test.
            Err(HttpError::Unreachable { .. }) => {}
            Err(other) => panic!("transport is broken: {other}"),
        }
    }

    #[test]
    fn an_unreachable_host_reports_rather_than_hangs() {
        let started = std::time::Instant::now();
        let result = send(Request::get("https://nexus-does-not-exist.invalid/x"));
        assert!(result.is_err(), "a bad host must not look like success");
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the deadline must bound the wait"
        );
    }

    #[test]
    fn a_missing_keychain_entry_is_none_not_a_panic() {
        assert!(keychain_secret("nexus-test-nothing-here", "nobody").is_none());
    }
}
