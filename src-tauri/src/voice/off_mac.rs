//! Voice, where there is no Mac: the same interface as `voice/mod.rs`, with
//! listening and speaking reported as Mac only.
//!
//! Recognition and speech are Apple's Speech and AVFoundation frameworks,
//! which exist only on Apple platforms. Working out what a transcript means
//! (`intent`, `response`, `wake`) is plain logic and is shared as it is.
//!
//! The payload types mirror the Mac's field for field, so the window reads the
//! same shape on either device.

#[path = "intent.rs"]
pub mod intent;
#[path = "response.rs"]
pub mod response;
#[path = "wake.rs"]
pub mod wake;

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

const MAC_ONLY: &str =
    "Voice uses the Mac's own speech recognition, so it works in NEXUS on the Mac.";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceStatus {
    pub recognizer_available: bool,
    pub supports_on_device: bool,
    pub authorization: String,
    pub listening: bool,
    pub locale: String,
}

pub fn status(_app: &AppHandle) -> Result<VoiceStatus, String> {
    Ok(VoiceStatus {
        recognizer_available: false,
        supports_on_device: false,
        authorization: "restricted".to_string(),
        listening: false,
        locale: String::new(),
    })
}

pub fn request_authorization(_app: &AppHandle) -> Result<(), String> {
    Err(MAC_ONLY.to_string())
}

pub fn start(_app: &AppHandle) -> Result<(), String> {
    Err(MAC_ONLY.to_string())
}

pub fn stop(_app: &AppHandle) -> Result<(), String> {
    Ok(())
}

pub fn always_listening() -> bool {
    false
}

pub fn set_always_listening(_app: &AppHandle, on: bool) -> Result<(), String> {
    if on {
        Err(MAC_ONLY.to_string())
    } else {
        Ok(())
    }
}

pub fn is_listening() -> bool {
    false
}

static WAKE_TURN: AtomicU64 = AtomicU64::new(0);

/// The next acknowledgement, advancing the rotation, as on the Mac.
pub fn next_wake_reply(replies: &[String]) -> String {
    wake::reply(replies, WAKE_TURN.fetch_add(1, Ordering::SeqCst))
}

pub mod speech {
    use serde::{Deserialize, Serialize};
    use tauri::AppHandle;

    pub const DEFAULT_VOICE: &str = "Rishi";

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct VoiceOption {
        pub id: String,
        pub name: String,
        pub language: String,
        pub quality: String,
        pub gender: String,
        pub preferred_locale: bool,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct VoiceSpeech {
        pub spoken: bool,
        pub text: String,
        pub voice: Option<String>,
    }

    /// Nothing is spoken; the reply is still shown in the window.
    pub fn speak(
        _app: &AppHandle,
        text: String,
        _preference: String,
    ) -> Result<VoiceSpeech, String> {
        Ok(VoiceSpeech {
            spoken: false,
            text,
            voice: None,
        })
    }

    pub fn stop_speaking(_app: &AppHandle) -> Result<(), String> {
        Ok(())
    }

    pub fn list_voices(_app: &AppHandle) -> Result<Vec<VoiceOption>, String> {
        Ok(Vec::new())
    }
}
