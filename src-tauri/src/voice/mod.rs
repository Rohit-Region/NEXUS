//! On-device speech recognition (NEXUS-010, Option 3).
//!
//! Pipeline, exactly as specified:
//!
//! ```text
//! microphone -> AVAudioEngine -> AVAudioPCMBuffer
//!            -> SFSpeechAudioBufferRecognitionRequest
//!            -> SFSpeechRecognizer -> transcript -> (frontend)
//! ```
//!
//! Hard invariants enforced here:
//!
//! - `requiresOnDeviceRecognition = true` is set unconditionally, and the
//!   session refuses to start when the recognizer reports that on-device
//!   recognition is unsupported. There is deliberately no remote fallback.
//! - Audio never touches the filesystem. `SFSpeechURLRecognitionRequest` is
//!   not used; buffers are appended live and dropped.
//! - Nothing here writes a transcript to the database, a file, or a log.
//!   Transcripts are emitted to the frontend and held nowhere else.
//! - This module never executes a command. It produces a string; matching,
//!   confirmation and execution all belong to the NEXUS-009 registry and the
//!   command palette.
//!
//! Threading: every Objective-C object below lives on the main thread and is
//! reached only through `run_on_main_thread`, because `Retained<T>` is neither
//! `Send` nor `Sync`.

pub mod intent;
/// NEXUS-011: deterministic response templates.
pub mod response;
/// NEXUS-011: on-device speech synthesis.
pub mod speech;

use std::cell::RefCell;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use block2::{RcBlock, StackBlock};
use objc2::rc::Retained;
use objc2_avf_audio::{AVAudioEngine, AVAudioPCMBuffer, AVAudioTime};
use objc2_foundation::NSError;
use objc2_speech::{
    SFSpeechAudioBufferRecognitionRequest, SFSpeechRecognitionResult, SFSpeechRecognitionTask,
    SFSpeechRecognizer, SFSpeechRecognizerAuthorizationStatus,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

/// Listening stops after this much silence, once speech has been heard.
const SILENCE_TIMEOUT: Duration = Duration::from_millis(2500);
/// Grace period before the FIRST result. Recognition takes a moment to warm
/// up and the user needs time to start talking; applying the silence timeout
/// from session start would cancel mid-utterance.
const PRE_SPEECH_TIMEOUT: Duration = Duration::from_secs(8);
/// Hard ceiling on one listening session, independent of silence.
const MAX_SESSION: Duration = Duration::from_secs(30);
/// Tap buffer size. 4096 frames is roughly 90ms at 44.1kHz.
const TAP_BUFFER_FRAMES: u32 = 4096;

pub const EVENT_TRANSCRIPT: &str = "nexus://voice/transcript";
pub const EVENT_STATE: &str = "nexus://voice/state";
pub const EVENT_ERROR: &str = "nexus://voice/error";

/// Whether a session is currently running. Read from any thread.
static LISTENING: AtomicBool = AtomicBool::new(false);
/// Epoch millis of the last recognition activity, for the silence timeout.
static LAST_ACTIVITY_MS: AtomicU64 = AtomicU64::new(0);
/// Incremented per session so a stale watchdog cannot stop a newer session.
static SESSION_ID: AtomicU64 = AtomicU64::new(0);
/// Whether this session has produced any transcript yet, selecting which
/// timeout the watchdog applies.
static GOT_RESULT: AtomicBool = AtomicBool::new(false);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// -- Payloads ----------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceStatus {
    /// A recognizer could be constructed for the current locale.
    pub recognizer_available: bool,
    /// The recognizer reports on-device recognition support. When false the
    /// session refuses to start rather than falling back to the network.
    pub supports_on_device: bool,
    /// 'notDetermined' | 'denied' | 'restricted' | 'authorized' | 'unknown'
    pub authorization: String,
    pub listening: bool,
    /// Locale identifier the recognizer was created with.
    pub locale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceTranscript {
    pub text: String,
    pub is_final: bool,
    /// Milliseconds from session start to this result, for S-07.
    pub elapsed_ms: u64,
    /// Always true: the request is configured on-device only.
    pub on_device: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceState {
    pub listening: bool,
    /// Why listening stopped: 'user' | 'silence' | 'timeout' | 'final' | 'error'
    pub reason: String,
}

// -- Main-thread session -----------------------------------------------------

struct Session {
    engine: Retained<AVAudioEngine>,
    request: Retained<SFSpeechAudioBufferRecognitionRequest>,
    task: Retained<SFSpeechRecognitionTask>,
    #[allow(dead_code)]
    recognizer: Retained<SFSpeechRecognizer>,
}

thread_local! {
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
}

fn auth_label(status: SFSpeechRecognizerAuthorizationStatus) -> &'static str {
    match status {
        SFSpeechRecognizerAuthorizationStatus::NotDetermined => "notDetermined",
        SFSpeechRecognizerAuthorizationStatus::Denied => "denied",
        SFSpeechRecognizerAuthorizationStatus::Restricted => "restricted",
        SFSpeechRecognizerAuthorizationStatus::Authorized => "authorized",
        _ => "unknown",
    }
}

/// Build a recognizer for the user's default locale. Returns None when the
/// locale has no speech support.
fn make_recognizer() -> Option<Retained<SFSpeechRecognizer>> {
    // `new` is infallible in these bindings; a locale without speech support
    // shows up as `isAvailable() == false` rather than a nil recognizer.
    Some(unsafe { SFSpeechRecognizer::new() })
}

fn read_status() -> VoiceStatus {
    let status = unsafe { SFSpeechRecognizer::authorizationStatus() };
    let recognizer = make_recognizer();
    let (available, on_device, locale) = match &recognizer {
        Some(r) => unsafe {
            (
                r.isAvailable(),
                r.supportsOnDeviceRecognition(),
                r.locale().localeIdentifier().to_string(),
            )
        },
        None => (false, false, String::new()),
    };

    VoiceStatus {
        recognizer_available: available,
        supports_on_device: on_device,
        authorization: auth_label(status).to_string(),
        listening: LISTENING.load(Ordering::SeqCst),
        locale,
    }
}

/// Tear the session down on the main thread. Safe to call when idle.
fn stop_session_main(app: &AppHandle, reason: &str) {
    // Flip the flag BEFORE tearing down. task.cancel() makes the Speech
    // framework fire the result handler with "Recognition request was
    // canceled"; that is our own doing, not a fault, and the handler uses
    // this flag to tell the two apart.
    let was_listening = LISTENING.swap(false, Ordering::SeqCst);

    SESSION.with(|cell| {
        if let Some(session) = cell.borrow_mut().take() {
            unsafe {
                // Order matters: remove the tap before stopping the engine, or
                // the tap block can be invoked against a torn-down node.
                session.engine.inputNode().removeTapOnBus(0);
                session.engine.stop();
                session.request.endAudio();
                session.task.cancel();
            }
        }
    });

    if was_listening {
        let _ = app.emit(
            EVENT_STATE,
            VoiceState {
                listening: false,
                reason: reason.to_string(),
            },
        );
    }
}

/// Build and start a recognition session. Main thread only.
fn start_session_main(app: &AppHandle) -> Result<(), String> {
    if LISTENING.load(Ordering::SeqCst) {
        return Ok(());
    }

    let recognizer = make_recognizer()
        .ok_or_else(|| "Speech recognition is not available for this locale.".to_string())?;

    unsafe {
        if !recognizer.isAvailable() {
            return Err("The speech recognizer is temporarily unavailable.".to_string());
        }
        // C-01: no remote fallback, ever. If the device cannot recognise
        // locally we refuse rather than letting Apple route audio off-machine.
        if !recognizer.supportsOnDeviceRecognition() {
            return Err(
                "On-device speech recognition is unavailable for this language. \
                 NEXUS will not fall back to remote recognition, so voice stays off. \
                 The keyboard command palette is unaffected."
                    .to_string(),
            );
        }
    }

    let request = unsafe { SFSpeechAudioBufferRecognitionRequest::new() };
    unsafe {
        request.setRequiresOnDeviceRecognition(true);
        request.setShouldReportPartialResults(true);

        // Belt and braces: if the flag did not take, stop before any audio is
        // captured rather than trusting the setter.
        if !request.requiresOnDeviceRecognition() {
            return Err(
                "Could not force on-device recognition; refusing to start.".to_string(),
            );
        }
    }

    let engine = unsafe { AVAudioEngine::new() };
    let started = Instant::now();
    let session_id = SESSION_ID.fetch_add(1, Ordering::SeqCst) + 1;
    LAST_ACTIVITY_MS.store(now_ms(), Ordering::SeqCst);
    GOT_RESULT.store(false, Ordering::SeqCst);

    // Result handler. Emits transcripts; never matches, never executes.
    let handler_app = app.clone();
    let result_handler = RcBlock::new(
        move |result: *mut SFSpeechRecognitionResult, error: *mut NSError| {
            // Results and errors can arrive after teardown began. Anything
            // past that point is an echo of our own cancel, not news.
            if !LISTENING.load(Ordering::SeqCst) {
                return;
            }

            LAST_ACTIVITY_MS.store(now_ms(), Ordering::SeqCst);
            GOT_RESULT.store(true, Ordering::SeqCst);

            if !error.is_null() {
                let message = unsafe { (*error).localizedDescription().to_string() };
                let _ = handler_app.emit(EVENT_ERROR, message);
                let app_for_stop = handler_app.clone();
                let _ = handler_app.run_on_main_thread(move || {
                    stop_session_main(&app_for_stop, "error");
                });
                return;
            }

            if result.is_null() {
                return;
            }

            let (text, is_final) = unsafe {
                let r = &*result;
                (r.bestTranscription().formattedString().to_string(), r.isFinal())
            };

            let _ = handler_app.emit(
                EVENT_TRANSCRIPT,
                VoiceTranscript {
                    text,
                    is_final,
                    elapsed_ms: started.elapsed().as_millis() as u64,
                    on_device: true,
                },
            );

            if is_final {
                let app_for_stop = handler_app.clone();
                let _ = handler_app.run_on_main_thread(move || {
                    stop_session_main(&app_for_stop, "final");
                });
            }
        },
    );

    let task = unsafe { recognizer.recognitionTaskWithRequest_resultHandler(&request, &result_handler) };

    // Microphone tap. Buffers are appended and immediately dropped; nothing is
    // retained, copied to disk, or logged.
    let tap_request = request.clone();
    let tap = StackBlock::new(
        move |buffer: NonNull<AVAudioPCMBuffer>, _when: NonNull<AVAudioTime>| unsafe {
            tap_request.appendAudioPCMBuffer(buffer.as_ref());
        },
    )
    .copy();

    unsafe {
        let input = engine.inputNode();
        let format = input.outputFormatForBus(0);
        input.installTapOnBus_bufferSize_format_block(
            0,
            TAP_BUFFER_FRAMES,
            Some(&format),
            RcBlock::as_ptr(&tap),
        );
        engine.prepare();

        // This is where a denied or revoked microphone surfaces. We do not
        // query AVCaptureDevice (objc2-av-foundation is deliberately not a
        // dependency), so the message is honest but generic.
        if let Err(err) = engine.startAndReturnError() {
            input.removeTapOnBus(0);
            task.cancel();
            return Err(format!(
                "Could not start the microphone: {}. Check that NEXUS has microphone \
                 access in System Settings > Privacy & Security > Microphone.",
                err.localizedDescription()
            ));
        }
    }

    SESSION.with(|cell| {
        *cell.borrow_mut() = Some(Session {
            engine,
            request,
            task,
            recognizer,
        });
    });

    LISTENING.store(true, Ordering::SeqCst);
    let _ = app.emit(
        EVENT_STATE,
        VoiceState {
            listening: true,
            reason: "user".to_string(),
        },
    );

    spawn_watchdog(app.clone(), session_id, started);
    Ok(())
}

/// Bounded silence and a hard session ceiling (V-07). Runs off the main thread
/// and only ever asks the main thread to stop.
fn spawn_watchdog(app: AppHandle, session_id: u64, started: Instant) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(250));

        if !LISTENING.load(Ordering::SeqCst) || SESSION_ID.load(Ordering::SeqCst) != session_id {
            return;
        }

        let idle_ms = now_ms().saturating_sub(LAST_ACTIVITY_MS.load(Ordering::SeqCst));
        let allowed = if GOT_RESULT.load(Ordering::SeqCst) {
            SILENCE_TIMEOUT
        } else {
            PRE_SPEECH_TIMEOUT
        };
        let reason = if started.elapsed() >= MAX_SESSION {
            "timeout"
        } else if idle_ms >= allowed.as_millis() as u64 {
            "silence"
        } else {
            continue;
        };

        let app2 = app.clone();
        let r = reason.to_string();
        let _ = app.run_on_main_thread(move || stop_session_main(&app2, &r));
        return;
    });
}

// -- Public API used by the command layer ------------------------------------

pub fn status(app: &AppHandle) -> Result<VoiceStatus, String> {
    run_main(app, read_status)
}

pub fn request_authorization(app: &AppHandle) -> Result<(), String> {
    // The block is constructed on the main thread: RcBlock is not Send, so it
    // cannot be built here and moved across.
    run_main(app, move || {
        let handler = RcBlock::new(|_status: SFSpeechRecognizerAuthorizationStatus| {});
        unsafe { SFSpeechRecognizer::requestAuthorization(&handler) };
    })
}

pub fn start(app: &AppHandle) -> Result<(), String> {
    let app2 = app.clone();
    run_main_fallible(app, move || start_session_main(&app2))
}

pub fn stop(app: &AppHandle) -> Result<(), String> {
    let app2 = app.clone();
    run_main(app, move || stop_session_main(&app2, "user"))
}

/// Whether the microphone is currently open.
///
/// NEXUS-011 reads this to refuse to speak mid-capture: the synthesizer is
/// audible to the microphone, and NEXUS would otherwise transcribe its own
/// confirmation as the next command.
pub fn is_listening() -> bool {
    LISTENING.load(Ordering::SeqCst)
}

/// Run a closure on the main thread and wait for its value.
fn run_main<T, F>(app: &AppHandle, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(f());
    })
    .map_err(|e| format!("Failed to reach the main thread: {e}"))?;
    rx.recv()
        .map_err(|e| format!("Main thread did not respond: {e}"))
}

fn run_main_fallible<F>(app: &AppHandle, f: F) -> Result<(), String>
where
    F: FnOnce() -> Result<(), String> + Send + 'static,
{
    run_main(app, f)?
}

// -- Tests -------------------------------------------------------------------
//
// Microphone hardware behaviour is deliberately not faked. What is tested is
// the pure lifecycle bookkeeping that guards the invariants.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_labels_cover_every_status() {
        assert_eq!(
            auth_label(SFSpeechRecognizerAuthorizationStatus::NotDetermined),
            "notDetermined"
        );
        assert_eq!(auth_label(SFSpeechRecognizerAuthorizationStatus::Denied), "denied");
        assert_eq!(
            auth_label(SFSpeechRecognizerAuthorizationStatus::Restricted),
            "restricted"
        );
        assert_eq!(
            auth_label(SFSpeechRecognizerAuthorizationStatus::Authorized),
            "authorized"
        );
        assert_eq!(
            auth_label(SFSpeechRecognizerAuthorizationStatus(99)),
            "unknown"
        );
    }

    #[test]
    fn listening_flag_defaults_off() {
        // Voice is off until a session is explicitly started (V-09, C-05).
        assert!(!LISTENING.load(Ordering::SeqCst));
    }

    #[test]
    fn silence_and_session_bounds_are_sane() {
        // V-07: both bounds must be finite and silence must trip first.
        assert!(SILENCE_TIMEOUT < PRE_SPEECH_TIMEOUT, "speaking needs a grace period");
        assert!(PRE_SPEECH_TIMEOUT < MAX_SESSION);
        assert!(SILENCE_TIMEOUT < MAX_SESSION);
        assert!(SILENCE_TIMEOUT.as_millis() > 0);
        assert!(MAX_SESSION.as_secs() <= 60, "sessions must not run unbounded");
    }

    #[test]
    fn transcript_payload_is_always_marked_on_device() {
        // The request is configured on-device only, so the payload cannot
        // honestly claim otherwise.
        let t = VoiceTranscript {
            text: "project create pannunga".to_string(),
            is_final: true,
            elapsed_ms: 42,
            on_device: true,
        };
        let json = serde_json::to_string(&t).expect("serialize");
        assert!(json.contains("\"onDevice\":true"));
        assert!(json.contains("\"isFinal\":true"));
        assert!(json.contains("\"elapsedMs\":42"));
    }

    #[test]
    fn status_serialises_with_camel_case_contract() {
        let s = VoiceStatus {
            recognizer_available: true,
            supports_on_device: true,
            authorization: "authorized".to_string(),
            listening: false,
            locale: "en-US".to_string(),
        };
        let json = serde_json::to_string(&s).expect("serialize");
        for key in [
            "recognizerAvailable",
            "supportsOnDevice",
            "authorization",
            "listening",
            "locale",
        ] {
            assert!(json.contains(key), "missing {key} in {json}");
        }
    }

    /// Strip comments so these guards inspect code, not documentation.
    fn code_only() -> String {
        let src = include_str!("mod.rs");
        let body = &src[..src.find("#[cfg(test)]").unwrap_or(src.len())];
        body.lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn no_url_recognition_request_is_referenced() {
        // Guards the no-persistence rule: SFSpeechURLRecognitionRequest would
        // require writing audio to disk.
        assert!(
            !code_only().contains("SFSpeechURLRecognitionRequest"),
            "URL-based recognition would mean persisting audio"
        );
    }

    #[test]
    fn no_filesystem_or_network_in_voice_path() {
        let code = code_only();
        for forbidden in ["std::fs", "File::", "reqwest", "TcpStream", "http://", "https://"] {
            assert!(
                !code.contains(forbidden),
                "voice path must not reference {forbidden}"
            );
        }
    }

    /// Regression guard for the "Recognition request was canceled" defect:
    /// teardown must clear LISTENING before cancelling, and the result
    /// handler must ignore anything arriving afterwards, or our own cancel
    /// is reported to the user as a failure.
    #[test]
    fn teardown_clears_listening_before_cancelling() {
        let code = code_only();
        let stop = &code[code.find("fn stop_session_main").expect("stop_session_main")..];
        let stop = &stop[..stop.find("\n}").unwrap_or(stop.len())];

        let flag = stop.find("LISTENING.swap(false").expect("must clear the flag");
        let cancel = stop.find("task.cancel()").expect("must cancel the task");
        assert!(
            flag < cancel,
            "LISTENING must be cleared before task.cancel(), or the cancel \
             echoes back as a user-facing error"
        );

        assert!(
            code.contains("if !LISTENING.load(Ordering::SeqCst) {"),
            "the result handler must ignore callbacks that arrive after teardown"
        );
    }

    #[test]
    fn on_device_flag_is_set_unconditionally() {
        let code = code_only();
        assert!(
            code.contains("setRequiresOnDeviceRecognition(true)"),
            "on-device recognition must be forced"
        );
        assert!(
            !code.contains("setRequiresOnDeviceRecognition(false)"),
            "there must be no path that disables on-device recognition"
        );
    }
}
