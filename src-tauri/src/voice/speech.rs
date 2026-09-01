//! NEXUS-011: spoken responses, on device.
//!
//! `AVSpeechSynthesizer` renders locally installed voices with no network
//! call, which is the same local-first guarantee the recognizer carries. It
//! arrives through the `AVSpeechSynthesis` feature of `objc2-avf-audio`, a
//! crate NEXUS already depends on directly, so the dependency graph is
//! unchanged. Speaking is output, not capture, so no new usage description or
//! authorization prompt is involved.
//!
//! Two invariants live here rather than in the caller, because a caller can
//! forget:
//!
//! - **Never speak while the microphone is open.** The synthesizer would be
//!   recognised as speech and the app would talk to itself.
//! - **Interrupt and replace.** A newer response always cancels whatever is
//!   still being spoken, so a burst of commands leaves the user hearing the
//!   answer to the last one rather than a queue of stale ones.

use std::cell::RefCell;
use std::time::Duration;

use objc2::rc::Retained;
use objc2_avf_audio::{
    AVSpeechBoundary, AVSpeechSynthesisVoice, AVSpeechSynthesisVoiceGender,
    AVSpeechSynthesisVoiceQuality, AVSpeechSynthesizer, AVSpeechUtterance,
};
use objc2_foundation::NSString;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

/// The locale NEXUS prefers, matching the recognizer's `en_IN`. A response in
/// a different accent from the one that just understood you sounds like a
/// different program answering.
const PREFERRED_LANGUAGE: &str = "en-IN";

/// Shipped default: the `en-IN` voice `AVSpeechSynthesizer` actually exposes.
///
/// Not Tara. The legacy `say` command lists Tara and Aman as `en_IN` voices,
/// but they come from the Speech Synthesis Manager, a different and larger
/// voice set than `AVSpeechSynthesisVoice.speechVoices()` returns. Defaulting
/// to a voice this API cannot see would make every fresh install start in the
/// fallback path.
///
/// Stored as a name rather than an identifier because names are readable in
/// the settings table and survive an identifier change; the resolver accepts
/// either.
pub const DEFAULT_VOICE: &str = "Rishi";

/// Voices from the Speech Synthesis Manager era, exposed through
/// `AVSpeechSynthesisVoice` for compatibility.
///
/// This namespace is where the novelty voices live (Bells, Boing, Zarvox,
/// Bad News and so on). They are filtered out of the picker rather than named
/// individually: a hardcoded list of joke voices would be wrong on the next
/// machine, whereas the namespace is a property of the platform. It costs a
/// few plain voices of the same vintage, none of them `en-IN` and none
/// reported as female.
const LEGACY_VOICE_PREFIX: &str = "com.apple.speech.synthesis.voice.";

// The synthesizer is retained across calls so it can be told to stop. Held
// per-thread because `Retained<T>` is not `Send`; every entry point below
// hops to the main thread first, so in practice this is one instance.
thread_local! {
    static SYNTH: RefCell<Option<Retained<AVSpeechSynthesizer>>> =
        const { RefCell::new(None) };
}

/// A voice offered in Settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceOption {
    /// Opaque system identifier. Stored when the user picks from the list.
    pub id: String,
    pub name: String,
    pub language: String,
    /// 'default' | 'enhanced' | 'premium'. Enhanced and premium voices are
    /// downloaded by the user, so this list differs machine to machine.
    pub quality: String,
    /// 'male' | 'female' | 'unspecified', exactly as the system reports it.
    ///
    /// Never inferred from the voice's name. Apple leaves the Eloquence
    /// voices unspecified, and guessing would put a made-up label in front of
    /// the user.
    pub gender: String,
    /// True for the locale NEXUS recognises in.
    pub preferred_locale: bool,
}

/// The result of an announcement attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceSpeech {
    /// False when the request was deliberately suppressed, which is not an
    /// error: the only current reason is that the microphone is open.
    pub spoken: bool,
    /// Exactly what was sent to the synthesizer. Template output, never a
    /// transcript, and not persisted anywhere.
    pub text: String,
    /// The voice actually used, after fallback. None means the system default.
    pub voice: Option<String>,
}

fn gender_label(gender: AVSpeechSynthesisVoiceGender) -> &'static str {
    match gender {
        AVSpeechSynthesisVoiceGender::Male => "male",
        AVSpeechSynthesisVoiceGender::Female => "female",
        _ => "unspecified",
    }
}

/// Whether a voice belongs in the picker.
///
/// English because that is what the recognizer understands, and outside the
/// legacy namespace so the novelty voices stay out. Pure, so the rule is
/// tested rather than trusted.
fn selectable(identifier: &str, language: &str) -> bool {
    language.to_ascii_lowercase().starts_with("en") && !identifier.starts_with(LEGACY_VOICE_PREFIX)
}

fn quality_label(quality: AVSpeechSynthesisVoiceQuality) -> &'static str {
    match quality {
        AVSpeechSynthesisVoiceQuality::Enhanced => "enhanced",
        AVSpeechSynthesisVoiceQuality::Premium => "premium",
        _ => "default",
    }
}

/// True when `language` is the preferred locale, e.g. "en-IN".
fn is_preferred(language: &str) -> bool {
    language.eq_ignore_ascii_case(PREFERRED_LANGUAGE)
}

/// Every selectable voice on this machine, paired with the object that speaks
/// it, in display order.
///
/// Built in one pass so the picker and the resolver see exactly the same set:
/// a voice the user cannot choose must never be chosen for them, and a voice
/// offered in Settings must actually resolve.
fn collect_voices() -> Vec<(VoiceOption, Retained<AVSpeechSynthesisVoice>)> {
    let mut pairs: Vec<(VoiceOption, Retained<AVSpeechSynthesisVoice>)> =
        unsafe { AVSpeechSynthesisVoice::speechVoices() }
            .iter()
            .filter_map(|voice| {
                let id = unsafe { voice.identifier() }.to_string();
                let language = unsafe { voice.language() }.to_string();
                if !selectable(&id, &language) {
                    return None;
                }
                let option = VoiceOption {
                    name: unsafe { voice.name() }.to_string(),
                    quality: quality_label(unsafe { voice.quality() }).to_string(),
                    gender: gender_label(unsafe { voice.gender() }).to_string(),
                    preferred_locale: is_preferred(&language),
                    id,
                    language,
                };
                Some((option, voice))
            })
            .collect();

    // The recognizer's own locale first, then voices the system reports as
    // female, then alphabetical. The middle rule exists because this Mac has
    // exactly one en-IN voice and it is male: without it, anyone wanting a
    // female voice would have to hunt through the list.
    pairs.sort_by(|(a, _), (b, _)| {
        b.preferred_locale
            .cmp(&a.preferred_locale)
            .then_with(|| (b.gender == "female").cmp(&(a.gender == "female")))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.id.cmp(&b.id))
    });
    pairs
}

/// Resolve a stored preference against a list of available voices.
///
/// Pure, and separated from AVFoundation precisely so the fallback chain can
/// be tested. Returns an index into `voices`, or None to mean "let the system
/// choose".
///
/// The chain matters because the list is machine-specific: a voice present
/// when the preference was saved can be removed later, and a preference typed
/// into the database by hand may never have existed. Every step degrades to
/// something that still speaks.
///
/// 1. Exact identifier, as stored by the Settings picker.
/// 2. Voice name within the preferred locale.
/// 3. Voice name in any locale, so a deliberate en-GB choice survives.
/// 4. The configured default, when the saved preference resolves to nothing.
/// 5. Any voice in the preferred locale.
///
/// An empty preference short-circuits to None: that is the user explicitly
/// choosing the system voice, not an absent setting, and it must not be
/// "corrected" back to the default.
pub fn resolve_index(preference: &str, voices: &[VoiceOption]) -> Option<usize> {
    let wanted = preference.trim();
    if wanted.is_empty() {
        return None;
    }

    voices
        .iter()
        .position(|v| v.id == wanted)
        .or_else(|| {
            voices
                .iter()
                .position(|v| v.preferred_locale && v.name.eq_ignore_ascii_case(wanted))
        })
        .or_else(|| {
            voices
                .iter()
                .position(|v| v.name.eq_ignore_ascii_case(wanted))
        })
        .or_else(|| {
            voices
                .iter()
                .position(|v| v.preferred_locale && v.name.eq_ignore_ascii_case(DEFAULT_VOICE))
        })
        .or_else(|| voices.iter().position(|v| v.preferred_locale))
}

/// Resolve a stored preference to an installed voice object.
fn resolve_voice(preference: &str) -> Option<Retained<AVSpeechSynthesisVoice>> {
    if preference.trim().is_empty() {
        return None;
    }

    let pairs = collect_voices();
    let options: Vec<VoiceOption> = pairs.iter().map(|(o, _)| o.clone()).collect();
    if let Some(index) = resolve_index(preference, &options) {
        return Some(pairs[index].1.clone());
    }

    // Nothing selectable matched. Ask the system for the preferred locale
    // directly, which can still succeed on a machine whose only en-IN voice
    // sits outside the selectable set. None from here means the system
    // default voice, which speaks perfectly well.
    unsafe {
        AVSpeechSynthesisVoice::voiceWithLanguage(Some(&NSString::from_str(PREFERRED_LANGUAGE)))
    }
}

/// Every selectable voice installed on this machine, for the Settings picker.
fn list_voices_main() -> Vec<VoiceOption> {
    collect_voices()
        .into_iter()
        .map(|(option, _)| option)
        .collect()
}

/// Stop whatever is being spoken. Safe when silent.
fn stop_speaking_main() {
    SYNTH.with(|cell| {
        if let Some(synth) = cell.borrow().as_ref() {
            unsafe {
                synth.stopSpeakingAtBoundary(AVSpeechBoundary::Immediate);
            }
        }
    });
}

/// Is the synthesizer still producing audio?
fn is_speaking_main() -> bool {
    SYNTH.with(|cell| {
        cell.borrow()
            .as_ref()
            .is_some_and(|synth| unsafe { synth.isSpeaking() })
    })
}

fn speak_main(app: &AppHandle, text: String, preference: String) -> VoiceSpeech {
    // The guard that keeps NEXUS from hearing itself. Checked here, at the
    // last possible moment before audio is produced, rather than in the UI.
    //
    // In always-listening the microphone is open by definition, so refusing
    // to speak would mean never speaking again. It is closed for the length
    // of the reply instead, and the supervisor reopens it afterwards.
    if super::always_listening() {
        super::mute_for_speech(app);
    } else if super::is_listening() {
        return VoiceSpeech {
            spoken: false,
            text,
            voice: None,
        };
    }

    let voice = resolve_voice(&preference);
    let voice_name = voice.as_ref().map(|v| unsafe { v.name() }.to_string());

    SYNTH.with(|cell| {
        let mut slot = cell.borrow_mut();
        let synth = slot.get_or_insert_with(|| unsafe { AVSpeechSynthesizer::new() });

        unsafe {
            // Interrupt and replace: a newer answer supersedes a stale one
            // rather than queueing behind it. Immediate rather than Word,
            // because these are one-line confirmations and a trailing
            // half-sentence is worse than a clean cut.
            synth.stopSpeakingAtBoundary(AVSpeechBoundary::Immediate);

            let utterance =
                AVSpeechUtterance::speechUtteranceWithString(&NSString::from_str(&text));
            utterance.setVoice(voice.as_deref());
            synth.speakUtterance(&utterance);
        }
    });

    VoiceSpeech {
        spoken: true,
        text,
        voice: voice_name,
    }
}

// -- Public API, all main-thread hops ----------------------------------------

pub fn speak(app: &AppHandle, text: String, preference: String) -> Result<VoiceSpeech, String> {
    let inner = app.clone();
    let spoken = super::run_main(app, move || speak_main(&inner, text, preference))?;
    if spoken.spoken && super::always_listening() {
        watch_until_silent(app.clone());
    }
    Ok(spoken)
}

/// Hold the microphone closed until the reply has actually finished.
///
/// AVSpeechSynthesizer gives no completion without a delegate, and adding
/// one means a new Objective-C class. Polling `isSpeaking` costs a main
/// thread hop every 150ms for the length of a one-line answer.
fn watch_until_silent(app: AppHandle) {
    std::thread::spawn(move || {
        // Bounded so a wedged synthesizer cannot leave the microphone shut
        // forever. Answers are one or two sentences; this is far above that.
        for _ in 0..200 {
            std::thread::sleep(Duration::from_millis(150));
            match super::run_main(&app, is_speaking_main) {
                Ok(true) => continue,
                _ => break,
            }
        }
        super::unmute_after_speech();
    });
}

pub fn stop_speaking(app: &AppHandle) -> Result<(), String> {
    let result = super::run_main(app, stop_speaking_main);
    // Whoever cut the reply short wants to talk, so reopen at once rather
    // than waiting for the poll to notice.
    super::unmute_after_speech();
    result
}

pub fn list_voices(app: &AppHandle) -> Result<Vec<VoiceOption>, String> {
    super::run_main(app, list_voices_main)
}

// -- Tests -------------------------------------------------------------------
//
// Audio output is not faked. What is tested is the pure logic around it: the
// parts that decide which voice and whether to speak at all.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_locale_matching_ignores_case() {
        assert!(is_preferred("en-IN"));
        assert!(is_preferred("en-in"));
        assert!(!is_preferred("en-US"));
        assert!(!is_preferred("en"));
        assert!(!is_preferred(""));
    }

    #[test]
    fn quality_labels_cover_every_tier() {
        assert_eq!(
            quality_label(AVSpeechSynthesisVoiceQuality::Default),
            "default"
        );
        assert_eq!(
            quality_label(AVSpeechSynthesisVoiceQuality::Enhanced),
            "enhanced"
        );
        assert_eq!(
            quality_label(AVSpeechSynthesisVoiceQuality::Premium),
            "premium"
        );
        // An unknown tier from a future macOS must not panic.
        assert_eq!(quality_label(AVSpeechSynthesisVoiceQuality(99)), "default");
    }

    fn voice(name: &str, language: &str, gender: &str) -> VoiceOption {
        VoiceOption {
            id: format!("com.apple.voice.compact.{language}.{name}"),
            name: name.to_string(),
            language: language.to_string(),
            quality: "default".to_string(),
            gender: gender.to_string(),
            preferred_locale: is_preferred(language),
        }
    }

    /// Mirrors what AVSpeechSynthesizer exposes on the development machine:
    /// one en-IN voice, male, plus female voices in other English locales.
    fn installed() -> Vec<VoiceOption> {
        vec![
            voice("Rishi", "en-IN", "male"),
            voice("Karen", "en-AU", "female"),
            voice("Moira", "en-IE", "female"),
            voice("Samantha", "en-US", "female"),
            voice("Tessa", "en-ZA", "female"),
            voice("Daniel", "en-GB", "male"),
            voice("Flo", "en-GB", "unspecified"),
        ]
    }

    #[test]
    fn the_shipped_default_is_rishi() {
        // Not Tara: speechVoices() does not expose her, so defaulting to Tara
        // would put every fresh install straight into the fallback path.
        assert_eq!(DEFAULT_VOICE, "Rishi");
        assert!(!DEFAULT_VOICE.contains('.'), "looks like an identifier");
    }

    #[test]
    fn the_shipped_default_resolves_against_a_realistic_voice_list() {
        let voices = installed();
        let index = resolve_index(DEFAULT_VOICE, &voices).expect("default must resolve");
        assert_eq!(voices[index].name, "Rishi");
        assert!(voices[index].preferred_locale);
    }

    #[test]
    fn an_identifier_preference_resolves_exactly() {
        let voices = installed();
        let wanted = voices[3].id.clone();
        let index = resolve_index(&wanted, &voices).expect("resolve");
        assert_eq!(voices[index].name, "Samantha");
    }

    #[test]
    fn a_name_preference_resolves_case_insensitively() {
        let voices = installed();
        let index = resolve_index("samantha", &voices).expect("resolve");
        assert_eq!(voices[index].name, "Samantha");
    }

    #[test]
    fn a_female_voice_outside_the_preferred_locale_is_selectable() {
        // This Mac has no female en-IN voice, so choosing one necessarily
        // means leaving the locale. That choice must be honoured rather than
        // pulled back to the en-IN default.
        let voices = installed();
        for name in ["Karen", "Moira", "Samantha", "Tessa"] {
            let index = resolve_index(name, &voices).expect("resolve");
            assert_eq!(voices[index].name, name);
            assert_eq!(voices[index].gender, "female");
            assert!(!voices[index].preferred_locale);
        }
    }

    #[test]
    fn an_unavailable_preference_falls_back_to_the_configured_default() {
        // The Tara case, and every case like it: a preference saved on
        // another Mac, or a voice the user has since removed.
        let voices = installed();
        let index = resolve_index("Tara", &voices).expect("must not give up");
        assert_eq!(voices[index].name, DEFAULT_VOICE);
    }

    #[test]
    fn an_unavailable_preference_falls_back_to_the_locale_when_the_default_is_gone() {
        let voices: Vec<VoiceOption> = vec![
            voice("Aman", "en-IN", "male"),
            voice("Samantha", "en-US", "female"),
        ];
        let index = resolve_index("Tara", &voices).expect("resolve");
        assert_eq!(
            voices[index].name, "Aman",
            "with no Rishi, any en-IN voice beats leaving the locale"
        );
    }

    #[test]
    fn an_unresolvable_preference_with_no_locale_voice_defers_to_the_system() {
        let voices = vec![voice("Samantha", "en-US", "female")];
        assert_eq!(
            resolve_index("Tara", &voices),
            None,
            "better the system voice than an arbitrary pick"
        );
    }

    #[test]
    fn an_empty_preference_means_the_system_voice_and_is_never_corrected() {
        let voices = installed();
        assert_eq!(resolve_index("", &voices), None);
        assert_eq!(resolve_index("   ", &voices), None);
    }

    #[test]
    fn resolution_against_an_empty_machine_never_panics() {
        assert_eq!(resolve_index(DEFAULT_VOICE, &[]), None);
        assert_eq!(resolve_index("", &[]), None);
    }

    #[test]
    fn the_picker_excludes_the_legacy_novelty_namespace() {
        // Named by namespace rather than by voice: a hardcoded list of joke
        // voices would be wrong on the next machine.
        assert!(!selectable(
            "com.apple.speech.synthesis.voice.Zarvox",
            "en-US"
        ));
        assert!(!selectable(
            "com.apple.speech.synthesis.voice.Bells",
            "en-US"
        ));
        assert!(selectable("com.apple.voice.compact.en-IN.Rishi", "en-IN"));
        assert!(selectable(
            "com.apple.voice.compact.en-US.Samantha",
            "en-US"
        ));
        assert!(selectable("com.apple.eloquence.en-GB.Flo", "en-GB"));
    }

    #[test]
    fn the_picker_excludes_languages_the_recognizer_does_not_understand() {
        assert!(!selectable("com.apple.voice.compact.fr-FR.Thomas", "fr-FR"));
        assert!(!selectable("com.apple.voice.compact.ta-IN.Vani", "ta-IN"));
        assert!(selectable("com.apple.voice.compact.en-AU.Karen", "en-AU"));
    }

    #[test]
    fn gender_labels_are_reported_never_guessed() {
        assert_eq!(gender_label(AVSpeechSynthesisVoiceGender::Female), "female");
        assert_eq!(gender_label(AVSpeechSynthesisVoiceGender::Male), "male");
        assert_eq!(
            gender_label(AVSpeechSynthesisVoiceGender::Unspecified),
            "unspecified"
        );
        // A future macOS tier must not panic or be mislabelled.
        assert_eq!(
            gender_label(AVSpeechSynthesisVoiceGender(99)),
            "unspecified"
        );
    }

    #[test]
    fn a_speech_result_reports_suppression_without_erroring() {
        // Suppression is a normal outcome, not a failure: the caller must be
        // able to tell "did not speak" from "could not speak".
        let suppressed = VoiceSpeech {
            spoken: false,
            text: "Opening Settings.".to_string(),
            voice: None,
        };
        assert!(!suppressed.spoken);
        let json = serde_json::to_string(&suppressed).expect("serialize");
        assert!(json.contains("\"spoken\":false"), "{json}");
    }

    #[test]
    fn voice_options_serialise_as_camel_case() {
        let option = VoiceOption {
            id: "com.apple.voice.compact.en-IN.Rishi".to_string(),
            name: "Rishi".to_string(),
            language: "en-IN".to_string(),
            quality: "default".to_string(),
            gender: "male".to_string(),
            preferred_locale: true,
        };
        let json = serde_json::to_string(&option).expect("serialize");
        assert!(json.contains("\"preferredLocale\":true"), "{json}");
    }
}
