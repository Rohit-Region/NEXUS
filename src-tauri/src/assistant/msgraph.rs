//! Microsoft Graph sign-in, by device code.
//!
//! **Device code, not a localhost redirect.** The usual desktop OAuth flow
//! opens a temporary listener on `127.0.0.1` to catch the redirect. That
//! would make NEXUS the one thing this architecture has avoided everywhere
//! else: a process with something listening on a port. The device code flow
//! needs no listener at all. NEXUS asks Microsoft for a code, the user types
//! it into their browser, and NEXUS polls an outbound endpoint until it is
//! approved.
//!
//! **Polling happens one request at a time.** `execute_action` holds the
//! database lock while a connector runs, so an action that sat in a loop for
//! two minutes waiting for a sign-in would freeze the whole application.
//! Signing in is therefore two actions: one that starts it and returns the
//! code immediately, and one that checks once.
//!
//! **The refresh token lives in the Keychain**, written over stdin so it
//! never appears in the process table. The access token is held in memory
//! only: it expires within the hour, and writing it anywhere would be storing
//! a credential for no benefit.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::http::{
    forget_keychain_secret, keychain_secret, send, store_keychain_secret, HttpError, Request,
};

/// Keychain service holding the refresh token. The account is the signed-in
/// user, so more than one account can be stored without collision.
pub const KEYCHAIN_SERVICE: &str = "nexus-msgraph";
/// Where the refresh token is filed before an account name is known.
const PENDING_ACCOUNT: &str = "default";

/// `offline_access` is what makes a refresh token available; without it the
/// user would sign in again every hour.
pub const SCOPES: &str = "offline_access User.Read Mail.Read Mail.Send Calendars.Read";

/// Refreshed this long before the token actually expires, so a request never
/// races the expiry.
const EXPIRY_MARGIN: Duration = Duration::from_secs(120);

/// The app registration. Not secret: a public client has no secret, and both
/// values are visible in any sign-in URL.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphConfig {
    pub client_id: String,
    /// Directory id, or a domain such as `acme.com`.
    pub tenant_id: String,
    /// Set once sign-in completes, so the Keychain entry can be found again.
    #[serde(default)]
    pub account: Option<String>,
}

/// Read the registration from a connector's stored configuration.
pub fn config(conn: &Connection, connector_id: &str) -> Option<GraphConfig> {
    let raw: String = conn
        .query_row(
            "SELECT config_json FROM connectors WHERE connector_id = ?1",
            [connector_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()?;
    let parsed: GraphConfig = serde_json::from_str(&raw).ok()?;
    if parsed.client_id.trim().is_empty() || parsed.tenant_id.trim().is_empty() {
        return None;
    }
    Some(parsed)
}

fn tenant_base(config: &GraphConfig) -> String {
    format!(
        "https://login.microsoftonline.com/{}/oauth2/v2.0",
        config.tenant_id.trim()
    )
}

/// Percent-encode a form value.
fn form(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() * 3);
    for byte in raw.as_bytes() {
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

fn post_form(url: &str, body: String) -> Result<serde_json::Value, String> {
    let response = send(Request {
        method: "POST",
        url: url.to_string(),
        headers: vec![(
            "Content-Type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        )],
        basic_auth: None,
        body: Some(body),
    })
    .map_err(|e| match e {
        HttpError::Unreachable { .. } => {
            "Microsoft could not be reached. Check your connection.".to_string()
        }
        other => other.to_string(),
    })?;

    serde_json::from_str::<serde_json::Value>(&response.body)
        .map_err(|_| "Microsoft returned something NEXUS could not read.".to_string())
}

// -- The pending sign-in ------------------------------------------------------

#[derive(Debug, Clone)]
struct Pending {
    device_code: String,
    started: Instant,
    expires_in: u64,
}

fn pending_slot() -> &'static Mutex<Option<Pending>> {
    static SLOT: OnceLock<Mutex<Option<Pending>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// The live access token, held in memory only.
fn token_slot() -> &'static Mutex<Option<(String, Instant)>> {
    static SLOT: OnceLock<Mutex<Option<(String, Instant)>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCodePrompt {
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in_seconds: u64,
}

/// Ask Microsoft to start a sign-in. Returns immediately with the code.
pub fn begin_sign_in(config: &GraphConfig) -> Result<DeviceCodePrompt, String> {
    let value = post_form(
        &format!("{}/devicecode", tenant_base(config)),
        format!(
            "client_id={}&scope={}",
            form(config.client_id.trim()),
            form(SCOPES)
        ),
    )?;

    if let Some(error) = value.get("error").and_then(|v| v.as_str()) {
        let detail = value
            .get("error_description")
            .and_then(|v| v.as_str())
            .unwrap_or(error);
        // The two failures worth naming, because the fix differs.
        return Err(
            if detail.contains("AADSTS700016") || error == "unauthorized_client" {
                "That client id is not an application in this tenant. Check the \
             Application (client) id on the app registration."
                    .to_string()
            } else if detail.contains("AADSTS7000218") || detail.contains("public client") {
                "The app registration does not allow public client flows. Turn on \
             'Allow public client flows' under Authentication."
                    .to_string()
            } else {
                detail.to_string()
            },
        );
    }

    let device_code = value
        .get("device_code")
        .and_then(|v| v.as_str())
        .ok_or("Microsoft did not return a device code.")?
        .to_string();
    let expires_in = value
        .get("expires_in")
        .and_then(|v| v.as_u64())
        .unwrap_or(900);

    *pending_slot().lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending {
        device_code,
        started: Instant::now(),
        expires_in,
    });

    Ok(DeviceCodePrompt {
        user_code: value
            .get("user_code")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        verification_uri: value
            .get("verification_uri")
            .and_then(|v| v.as_str())
            .unwrap_or("https://microsoft.com/devicelogin")
            .to_string(),
        expires_in_seconds: expires_in,
    })
}

/// What one poll found.
#[derive(Debug, Clone, PartialEq)]
pub enum SignInProgress {
    /// The user has not finished in the browser yet.
    Waiting,
    /// Signed in, with the account name.
    Done { account: String },
    /// Over, and it will not succeed. The sign-in must be restarted.
    Failed { reason: String },
}

/// Check once whether the user has approved the sign-in.
///
/// Deliberately a single request. Looping here would hold the database lock
/// for as long as the user takes to find their phone.
pub fn poll_sign_in(conn: &Connection, config: &GraphConfig) -> SignInProgress {
    let pending = match pending_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    {
        Some(pending) => pending,
        None => {
            return SignInProgress::Failed {
                reason: "No sign-in is in progress. Start one first.".to_string(),
            }
        }
    };

    if pending.started.elapsed().as_secs() > pending.expires_in {
        *pending_slot().lock().unwrap_or_else(|e| e.into_inner()) = None;
        return SignInProgress::Failed {
            reason: "That code expired. Start signing in again.".to_string(),
        };
    }

    let value = match post_form(
        &format!("{}/token", tenant_base(config)),
        format!(
            "grant_type=urn:ietf:params:oauth:grant-type:device_code&client_id={}&device_code={}",
            form(config.client_id.trim()),
            form(&pending.device_code)
        ),
    ) {
        Ok(value) => value,
        Err(reason) => return SignInProgress::Failed { reason },
    };

    if let Some(error) = value.get("error").and_then(|v| v.as_str()) {
        return match error {
            // Both mean "keep waiting", not "something is wrong".
            "authorization_pending" | "slow_down" => SignInProgress::Waiting,
            "authorization_declined" => SignInProgress::Failed {
                reason: "Sign-in was declined in the browser.".to_string(),
            },
            "expired_token" => SignInProgress::Failed {
                reason: "That code expired. Start signing in again.".to_string(),
            },
            other => SignInProgress::Failed {
                reason: value
                    .get("error_description")
                    .and_then(|v| v.as_str())
                    .unwrap_or(other)
                    .to_string(),
            },
        };
    }

    let access = match value.get("access_token").and_then(|v| v.as_str()) {
        Some(token) => token.to_string(),
        None => {
            return SignInProgress::Failed {
                reason: "Microsoft did not return a token.".to_string(),
            }
        }
    };
    let refresh = value
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    // The account name, read out of the token's own claims rather than by
    // making another call. Only the middle segment is decoded, and only to
    // find a name to file the credential under.
    let account = account_from_token(&access).unwrap_or_else(|| PENDING_ACCOUNT.to_string());

    if !refresh.is_empty() {
        if let Err(reason) = store_keychain_secret(KEYCHAIN_SERVICE, &account, &refresh) {
            return SignInProgress::Failed { reason };
        }
    }

    cache_access_token(
        &access,
        value
            .get("expires_in")
            .and_then(|v| v.as_u64())
            .unwrap_or(3600),
    );
    *pending_slot().lock().unwrap_or_else(|e| e.into_inner()) = None;

    // Remember which account to look up next time.
    let _ = remember_account(conn, config, &account);
    SignInProgress::Done { account }
}

/// Decode the account name from an access token's claims.
///
/// Reads only `preferred_username` or `upn` from the payload, purely to
/// choose a Keychain account name. The token's signature is Microsoft's to
/// verify, not NEXUS's; nothing here trusts the contents for a decision.
fn account_from_token(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let decoded = base64_url_decode(payload)?;
    let claims: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    claims
        .get("preferred_username")
        .or_else(|| claims.get("upn"))
        .or_else(|| claims.get("unique_name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Base64url without padding, as JWTs use.
fn base64_url_decode(input: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut buffer: u32 = 0;
    let mut bits = 0u32;
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    for ch in input.bytes() {
        if ch == b'=' {
            break;
        }
        let value = TABLE.iter().position(|c| *c == ch)? as u32;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

fn remember_account(conn: &Connection, config: &GraphConfig, account: &str) -> Result<(), String> {
    let updated = GraphConfig {
        client_id: config.client_id.clone(),
        tenant_id: config.tenant_id.clone(),
        account: Some(account.to_string()),
    };
    let json = serde_json::to_string(&updated).map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE connectors SET config_json = ?1,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
          WHERE connector_id = 'outlook'",
        rusqlite::params![json],
    )
    .map_err(|e| format!("Could not store the account: {e}"))?;
    Ok(())
}

fn cache_access_token(token: &str, expires_in: u64) {
    let good_until = Instant::now() + Duration::from_secs(expires_in).saturating_sub(EXPIRY_MARGIN);
    *token_slot().lock().unwrap_or_else(|e| e.into_inner()) = Some((token.to_string(), good_until));
}

/// Whether a refresh token exists for this configuration.
pub fn is_signed_in(config: &GraphConfig) -> bool {
    let account = config
        .account
        .clone()
        .unwrap_or_else(|| PENDING_ACCOUNT.to_string());
    keychain_secret(KEYCHAIN_SERVICE, &account).is_some()
}

/// A usable access token, refreshing if the cached one has aged out.
pub fn access_token(config: &GraphConfig) -> Result<String, String> {
    if let Some((token, good_until)) = token_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    {
        if Instant::now() < good_until {
            return Ok(token);
        }
    }

    let account = config
        .account
        .clone()
        .unwrap_or_else(|| PENDING_ACCOUNT.to_string());
    let refresh = keychain_secret(KEYCHAIN_SERVICE, &account)
        .ok_or("Not signed in to Microsoft. Run the sign-in first.")?;

    let value = post_form(
        &format!("{}/token", tenant_base(config)),
        format!(
            "grant_type=refresh_token&client_id={}&refresh_token={}&scope={}",
            form(config.client_id.trim()),
            form(&refresh),
            form(SCOPES)
        ),
    )?;

    if let Some(error) = value.get("error").and_then(|v| v.as_str()) {
        // A refresh token that no longer works means consent was revoked or
        // it expired. Removing it makes the next attempt say "sign in" rather
        // than failing the same way forever.
        let _ = forget_keychain_secret(KEYCHAIN_SERVICE, &account);
        *token_slot().lock().unwrap_or_else(|e| e.into_inner()) = None;
        return Err(format!(
            "Microsoft would not renew the sign-in ({error}). Sign in again."
        ));
    }

    let access = value
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or("Microsoft did not return a token.")?
        .to_string();

    // Microsoft rotates refresh tokens: the new one replaces the old.
    if let Some(new_refresh) = value.get("refresh_token").and_then(|v| v.as_str()) {
        let _ = store_keychain_secret(KEYCHAIN_SERVICE, &account, new_refresh);
    }
    cache_access_token(
        &access,
        value
            .get("expires_in")
            .and_then(|v| v.as_u64())
            .unwrap_or(3600),
    );
    Ok(access)
}

/// Forget the sign-in entirely.
pub fn sign_out(config: &GraphConfig) -> Result<(), String> {
    let account = config
        .account
        .clone()
        .unwrap_or_else(|| PENDING_ACCOUNT.to_string());
    *token_slot().lock().unwrap_or_else(|e| e.into_inner()) = None;
    *pending_slot().lock().unwrap_or_else(|e| e.into_inner()) = None;
    forget_keychain_secret(KEYCHAIN_SERVICE, &account)
}

/// A GET against Graph, with the token attached.
pub fn graph_get(config: &GraphConfig, path: &str) -> Result<serde_json::Value, String> {
    let token = access_token(config)?;
    let response = send(Request {
        method: "GET",
        url: format!("https://graph.microsoft.com/v1.0{path}"),
        headers: vec![("Authorization".to_string(), format!("Bearer {token}"))],
        basic_auth: None,
        body: None,
    })
    .map_err(|e| e.to_string())?;

    if !response.ok() {
        return Err(match response.status {
            401 => "Microsoft rejected the sign-in. Try signing in again.".to_string(),
            403 => "Your organisation has not granted NEXUS this permission.".to_string(),
            429 => "Microsoft is rate limiting NEXUS. Try again shortly.".to_string(),
            other => format!("Microsoft returned {other}."),
        });
    }

    serde_json::from_str(&response.body)
        .map_err(|_| "Microsoft returned something NEXUS could not read.".to_string())
}

/// A POST against Graph, with the token attached.
pub fn graph_post(config: &GraphConfig, path: &str, body: serde_json::Value) -> Result<(), String> {
    let token = access_token(config)?;
    let response = send(Request {
        method: "POST",
        url: format!("https://graph.microsoft.com/v1.0{path}"),
        headers: vec![
            ("Authorization".to_string(), format!("Bearer {token}")),
            ("Content-Type".to_string(), "application/json".to_string()),
        ],
        basic_auth: None,
        body: Some(body.to_string()),
    })
    .map_err(|e| e.to_string())?;

    if response.ok() {
        Ok(())
    } else {
        Err(match response.status {
            401 => "Microsoft rejected the sign-in. Try signing in again.".to_string(),
            403 => "Your organisation has not granted NEXUS permission to do that.".to_string(),
            other => format!("Microsoft returned {other}."),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scopes_ask_for_a_refresh_token_and_nothing_broad() {
        assert!(
            SCOPES.contains("offline_access"),
            "no refresh token without it"
        );
        for narrow in ["Mail.Read", "Calendars.Read", "Mail.Send", "User.Read"] {
            assert!(SCOPES.contains(narrow), "{narrow} missing");
        }
        // The scopes an administrator would have to approve, and which this
        // deliberately does not request.
        for broad in ["Mail.ReadWrite", ".All", "Directory", "Files", "Chat."] {
            assert!(!SCOPES.contains(broad), "{broad} must not be requested");
        }
    }

    #[test]
    fn form_values_cannot_break_out_of_the_body() {
        // A tenant id or code containing an ampersand would otherwise add a
        // parameter to the request.
        let encoded = form("abc&grant_type=password&x=1");
        assert!(!encoded.contains('&') && !encoded.contains('='));
        assert_eq!(form("a b"), "a+b");
        assert_eq!(form("safe-_.~"), "safe-_.~");
    }

    #[test]
    fn a_jwt_payload_decodes_to_its_claims() {
        // {"preferred_username":"rohit.raja@acme.com"} as base64url.
        let payload = "eyJwcmVmZXJyZWRfdXNlcm5hbWUiOiJyb2hpdC5yYWphQGFjbWUuY29tIn0";
        let token = format!("header.{payload}.signature");
        assert_eq!(
            account_from_token(&token).as_deref(),
            Some("rohit.raja@acme.com")
        );
    }

    #[test]
    fn a_malformed_token_yields_no_account_rather_than_panicking() {
        for bad in ["", "notatoken", "a.b", "a.!!!.c", "a..c"] {
            assert!(account_from_token(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn base64url_handles_the_alphabet_jwts_actually_use() {
        // - and _ replace + and /, and padding is omitted.
        assert!(base64_url_decode("SGVsbG8").is_some());
        assert!(base64_url_decode("a-_b").is_some());
        assert!(base64_url_decode("!!!").is_none());
    }

    #[test]
    fn polling_with_nothing_in_progress_says_so() {
        let conn = Connection::open_in_memory().expect("open");
        let config = GraphConfig {
            client_id: "x".to_string(),
            tenant_id: "y".to_string(),
            account: None,
        };
        // No sign-in was started, so this must not reach the network.
        assert!(matches!(
            poll_sign_in(&conn, &config),
            SignInProgress::Failed { .. }
        ));
    }

    #[test]
    fn no_listener_is_ever_opened() {
        // The reason this uses device code rather than a localhost redirect.
        let production = include_str!("msgraph.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("marker");
        for forbidden in ["TcpListener", "bind(", "localhost:", "127.0.0.1:"] {
            assert!(!production.contains(forbidden), "found {forbidden}");
        }
    }

    #[test]
    fn the_access_token_is_never_written_to_disk() {
        let production = include_str!("msgraph.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("marker");
        // Only the refresh token is stored, and only in the Keychain.
        assert_eq!(
            production.matches("store_keychain_secret").count(),
            3,
            "the refresh token is stored on sign-in and on rotation, nothing else"
        );
        assert!(
            !production.contains("INSERT INTO settings"),
            "no token in the database"
        );
    }
}
