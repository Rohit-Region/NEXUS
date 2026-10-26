//! Bank SMS, fetched from the user's own Google Sheet.
//!
//! The phone forwards ICICI and HDFC messages to a Google Apps Script web app
//! in the user's account (`docs/bank-sms/Code.gs`), which stores them as rows.
//! This reads the new rows back.
//!
//! # The link and its key
//!
//! The web app URL is not secret on its own; the `k` key is. They are stored
//! apart: the URL in the settings table, the key in the macOS Keychain, so the
//! database file never holds anything that can read the user's bank messages.
//! The key never appears in an error, a log line, or anything returned to the
//! frontend.
//!
//! # The one redirect this follows
//!
//! Apps Script answers every request with a 302 to a one-time URL on
//! `script.googleusercontent.com`. The HTTP layer never follows redirects, so
//! this makes the second request itself, and only after checking the target
//! is exactly that Google host over HTTPS. The target carries no key: it is
//! Google's own short-lived reference to the response.

use serde::Deserialize;

use super::http::{send_with_timeout, Request};

const SCRIPT_PREFIX: &str = "https://script.google.com/macros/s/";
const CONTENT_HOST: &str = "https://script.googleusercontent.com/";
const KEY_LEN: usize = 64;
/// Apps Script can take several seconds to start a cold script.
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(40);

/// The web app URL and its key, split from the link the user pastes:
/// `https://script.google.com/macros/s/<id>/exec?k=<64 hex>`.
pub fn split_link(raw: &str) -> Result<(String, String), String> {
    let link = raw.trim();
    let (base, query) = link
        .split_once('?')
        .ok_or("The link has no key. It should end in ?k= followed by the secret key.")?;
    let id = base
        .strip_prefix(SCRIPT_PREFIX)
        .and_then(|rest| rest.strip_suffix("/exec"))
        .ok_or("That is not a Google Apps Script web app link. It should start with https://script.google.com/macros/s/ and end in /exec.")?;
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("The web app link looks damaged. Copy it again from Deploy, Manage deployments.".to_string());
    }
    let key = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("k="))
        .unwrap_or("");
    if !valid_key(key) {
        return Err(
            "The key after ?k= should be 64 characters of 0-9 and a-f, the KEY value from Script Properties."
                .to_string(),
        );
    }
    Ok((base.to_string(), key.to_string()))
}

pub fn valid_key(key: &str) -> bool {
    key.len() == KEY_LEN && key.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// One forwarded message, as the script stores it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Row {
    pub row: i64,
    /// Unix milliseconds when the Sheet received it.
    pub received: Option<i64>,
    pub from: String,
    pub text: String,
    pub id: String,
}

#[derive(Debug, Deserialize)]
struct Page {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    last: i64,
    #[serde(default)]
    rows: Vec<Row>,
}

/// Rows after row number `after`, and the new cursor.
pub fn fetch(base: &str, key: &str, after: i64) -> Result<(i64, Vec<Row>), String> {
    if !base.starts_with(SCRIPT_PREFIX) || !valid_key(key) {
        return Err("The bank SMS link is not set up correctly. Connect it again.".to_string());
    }
    let url = format!("{base}?k={key}&after={}", after.max(1));
    let first = send_with_timeout(Request::get(&url), FETCH_TIMEOUT)
        .map_err(|e| scrub(&format!("Could not reach Google: {e}"), key))?;

    let response = if (300..400).contains(&first.status) {
        let target = first
            .redirect
            .ok_or("Google redirected without saying where.")?;
        if !trusted_redirect(&target) {
            return Err("Google redirected somewhere unexpected, so NEXUS stopped.".to_string());
        }
        send_with_timeout(Request::get(&target), FETCH_TIMEOUT)
            .map_err(|e| scrub(&format!("Could not read the Sheet: {e}"), key))?
    } else {
        first
    };
    if !response.ok() {
        return Err(format!("Google answered {} reading the Sheet.", response.status));
    }
    parse_page(&response.body)
}

fn trusted_redirect(target: &str) -> bool {
    target.starts_with(CONTENT_HOST) && !target.contains('@')
}

fn parse_page(body: &str) -> Result<(i64, Vec<Row>), String> {
    let page: Page = serde_json::from_str(body).map_err(|_| {
        "The Sheet answered with something other than bank messages. Check the script is deployed as a web app.".to_string()
    })?;
    if !page.ok {
        return Err(match page.error.as_deref() {
            Some("key") => "Google refused the key. Connect the link again with the current KEY.".to_string(),
            Some(other) => format!("The script reported: {other}"),
            None => "The script reported an error.".to_string(),
        });
    }
    Ok((page.last, page.rows))
}

/// Remove the key from anything that might be shown or logged.
fn scrub(message: &str, key: &str) -> String {
    if key.is_empty() {
        message.to_string()
    } else {
        message.replace(key, "<key>")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "27f6c8cf79f641d692ecbee9b30c063fe21473404c76440fbb21cee9dda1aaaa";

    #[test]
    fn a_pasted_link_splits_into_url_and_key() {
        let (base, key) = split_link(&format!(
            "  https://script.google.com/macros/s/AKfy_cb-1/exec?k={KEY}\n"
        ))
        .unwrap();
        assert_eq!(base, "https://script.google.com/macros/s/AKfy_cb-1/exec");
        assert_eq!(key, KEY);
    }

    #[test]
    fn bad_links_say_what_is_wrong() {
        let short = "https://script.google.com/macros/s/AKfy/exec?k=abc";
        assert!(split_link(short).unwrap_err().contains("64 characters"));
        let sheet_id = "https://script.google.com/macros/s/AKfy/exec?k=1vlaY9JkZMpxK26Rx_DtXq43EWYTbZ09xnyOElv8PqMM";
        assert!(split_link(sheet_id).unwrap_err().contains("64 characters"), "a Sheet ID is not the key");
        assert!(split_link("https://script.google.com/macros/s/AKfy/exec").unwrap_err().contains("no key"));
        let echo = format!("https://script.googleusercontent.com/macros/echo?user_content_key=x&k={KEY}");
        assert!(split_link(&echo).unwrap_err().contains("not a Google Apps Script"));
        let evil = format!("https://script.google.com.evil.com/macros/s/AKfy/exec?k={KEY}");
        assert!(split_link(&evil).is_err());
    }

    #[test]
    fn only_googles_content_host_is_followed() {
        assert!(trusted_redirect("https://script.googleusercontent.com/macros/echo?user_content_key=x&lib=y"));
        assert!(!trusted_redirect("http://script.googleusercontent.com/macros/echo"));
        assert!(!trusted_redirect("https://script.googleusercontent.com.evil.com/x"));
        assert!(!trusted_redirect("https://evil.com/?https://script.googleusercontent.com/"));
        assert!(!trusted_redirect("https://script.googleusercontent.com@evil.com/"));
    }

    #[test]
    fn pages_parse_and_a_refused_key_is_explained() {
        let body = r#"{"ok":true,"last":2,"rows":[{"row":2,"received":1761300000000,"from":"AX-ICICIT-S","text":"ICICI ...","sent":1,"id":"abc"}]}"#;
        let (last, rows) = parse_page(body).unwrap();
        assert_eq!(last, 2);
        assert_eq!(rows[0].from, "AX-ICICIT-S");
        assert_eq!(rows[0].received, Some(1_761_300_000_000));
        assert!(parse_page(r#"{"ok":false,"error":"key"}"#).unwrap_err().contains("refused the key"));
        assert!(parse_page("<html>").is_err());
    }

    /// Live, ignored by default: reads the user's own Sheet through the link
    /// saved during setup, printing counts only.
    /// `cargo test --lib bank_sms::tests::live_fetch -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_fetch() {
        let home = std::env::var("HOME").unwrap();
        let link = std::fs::read_to_string(format!("{home}/.nexus-bank-link")).unwrap();
        let (base, key) = split_link(&link).unwrap();
        let (last, rows) = fetch(&base, &key, 1).expect("fetch");
        println!("last row {last}, {} message(s)", rows.len());
        for r in &rows {
            println!("  row {} from {} -> {:?}", r.row, r.from, super::super::spend::read_sms(&r.from, &r.text, "2026-10-24"));
        }
    }

    #[test]
    fn the_key_never_survives_into_a_message() {
        let m = scrub(&format!("Could not resolve host: x?k={KEY}"), KEY);
        assert!(!m.contains(KEY));
    }
}
