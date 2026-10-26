//! The Spending lock: Touch ID before any spending data leaves the backend.
//!
//! # Enforced here, not in the window
//!
//! Every Spending command that reads or changes data asks `check` first, and
//! while locked it returns an error instead of the data. A lock drawn only in
//! the frontend would hide numbers that had already been sent to it.
//!
//! # When it locks again
//!
//! After `AWAY` seconds with no Spending command, which is what leaving the
//! tab or NEXUS looks like, or after `AWAY` seconds with no keyboard or mouse
//! input on the Mac, which covers walking away with the tab open, the screen
//! locking and the Mac sleeping. Unlocking never persists: a restart is locked.
//!
//! # How it asks
//!
//! `LAPolicy::DeviceOwnerAuthentication`: Touch ID, with the Mac login
//! password offered by macOS itself when Touch ID is unavailable. NEXUS never
//! sees a fingerprint or the password, only macOS's yes or no.

use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;

use objc2::runtime::Bool;
use objc2_foundation::{NSError, NSString};
use objc2_local_authentication::{LABiometryType, LAContext, LAPolicy};

/// Seconds away before it locks again.
pub const AWAY: i64 = 300;

/// What a locked command returns. The window recognises it by this wording.
pub const LOCKED: &str = "Spending is locked. Unlock it with Touch ID.";

/// How long to wait for the user to answer the Touch ID prompt.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(120);

/// When Spending was last used while unlocked; `None` is locked.
static LAST_USE: Mutex<Option<i64>> = Mutex::new(None);

/// Whether an unlocked session is still open. If the Mac's idle time cannot be
/// read, the inactivity rule alone decides, rather than locking on every
/// failed read.
pub fn still_open(last_use: i64, now: i64, idle_seconds: Option<i64>) -> bool {
    now - last_use < AWAY && idle_seconds.is_none_or(|idle| idle < AWAY)
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Allow a Spending command if unlocked, and count it as use. Locks for good
/// (until the next unlock) once the session has lapsed.
pub fn check() -> Result<(), String> {
    let now = now_unix();
    let mut last = LAST_USE.lock().unwrap_or_else(|e| e.into_inner());
    match *last {
        Some(t) if still_open(t, now, super::wellbeing::idle_seconds()) => {
            *last = Some(now);
            Ok(())
        }
        _ => {
            *last = None;
            Err(LOCKED.to_string())
        }
    }
}

/// Whether it is unlocked right now, without counting as use.
pub fn is_open() -> bool {
    let last = LAST_USE.lock().unwrap_or_else(|e| e.into_inner());
    last.is_some_and(|t| still_open(t, now_unix(), super::wellbeing::idle_seconds()))
}

pub fn lock() {
    *LAST_USE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// "touchId" when this Mac can use Touch ID, else "password".
pub fn method() -> &'static str {
    let context = unsafe { LAContext::new() };
    let biometrics = unsafe { context.canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthenticationWithBiometrics) };
    if biometrics.is_ok() && unsafe { context.biometryType() } == LABiometryType::TouchID {
        "touchId"
    } else {
        "password"
    }
}

/// Ask macOS to confirm it is the owner: Touch ID, or the login password.
/// Blocks until answered (or `PROMPT_TIMEOUT`), so call it off the main thread.
pub fn unlock() -> Result<(), String> {
    let context = unsafe { LAContext::new() };
    let policy = LAPolicy::DeviceOwnerAuthentication;
    if let Err(e) = unsafe { context.canEvaluatePolicy_error(policy) } {
        return Err(format!("This Mac cannot confirm it is you right now: {}", e.localizedDescription()));
    }
    let (tx, rx) = mpsc::channel::<bool>();
    let reply = block2::RcBlock::new(move |success: Bool, _error: *mut NSError| {
        let _ = tx.send(success.as_bool());
    });
    // macOS shows: "NEXUS is trying to show your spending."
    let reason = NSString::from_str("show your spending");
    unsafe { context.evaluatePolicy_localizedReason_reply(policy, &reason, &reply) };

    match rx.recv_timeout(PROMPT_TIMEOUT) {
        Ok(true) => {
            *LAST_USE.lock().unwrap_or_else(|e| e.into_inner()) = Some(now_unix());
            Ok(())
        }
        Ok(false) => Err("Not unlocked.".to_string()),
        Err(_) => Err("The Touch ID prompt timed out.".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_stays_open_while_used_and_locks_after_five_minutes_away() {
        assert!(still_open(1_000, 1_000 + AWAY - 1, Some(10)));
        assert!(!still_open(1_000, 1_000 + AWAY, Some(10)), "five minutes without using Spending");
    }

    #[test]
    fn walking_away_from_the_mac_locks_it_even_with_the_tab_open() {
        assert!(!still_open(1_000, 1_030, Some(AWAY)), "the tab refreshed, but nobody touched the Mac");
        assert!(still_open(1_000, 1_030, None), "an unreadable idle time falls back to the inactivity rule");
    }

    #[test]
    fn it_starts_locked_and_lock_closes_it() {
        lock();
        assert!(!is_open());
        assert_eq!(check(), Err(LOCKED.to_string()));
    }
}
