//! Wake-word detection for always-listening mode.
//!
//! With the microphone permanently open, every stray sentence in the room
//! reaches the recogniser. This module is the filter that decides which of
//! them were addressed to NEXUS, and it is deliberately the only place that
//! decision is made.
//!
//! Nothing here touches audio, the database, or the network. It is a pure
//! function over a transcript, which is what makes the false-wake behaviour
//! testable rather than a matter of opinion.

/// What the user is called back. Editable in Settings; these are the
/// defaults chosen when always-listening was added.
pub const DEFAULT_REPLIES: [&str; 3] = ["Yes Rohi", "Yes boss", "Go ahead"];

/// How the recogniser spells the wake word.
///
/// "Nexus" is not in Apple's general vocabulary, so on-device recognition
/// returns near-misses at least as often as the real spelling. Each of these
/// was chosen because it is a plausible transcription of someone saying
/// "Nexus" and is not a word anyone says by accident at the start of a
/// sentence.
const VARIANTS: [&str; 6] = ["nexus", "nexis", "nexas", "nexsus", "nexuses", "nexux"];

/// Two-word transcriptions of the same single spoken word.
const SPLIT_VARIANTS: [[&str; 2]; 3] = [["next", "us"], ["necks", "us"], ["nex", "us"]];

/// How far into a sentence the wake word may appear.
///
/// Addressing someone puts their name at the front: "Nexus, do X" or "hey
/// Nexus, do X". Merely mentioning them puts it anywhere. Without this
/// bound, saying "I was reading about Nexus yesterday" fires a command.
const MAX_WAKE_POSITION: usize = 2;

/// Words that may sit between the wake word and the command.
const LEAD_FILLER: [&str; 8] = [
    "please", "can", "you", "could", "would", "now", "just", "um",
];

/// Words that may precede the wake word.
const GREETING_PREFIX: [&str; 5] = ["hey", "hi", "ok", "okay", "hello"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wake {
    /// The wake word carried a command in the same breath.
    ///
    /// The string is the remainder in its original casing and punctuation:
    /// dictation depends on getting the user's sentence back verbatim.
    WithCommand(String),
    /// The wake word on its own. Acknowledge, then listen for the command.
    Bare,
}

/// Strip everything that is not a letter or digit and lowercase the rest.
fn normalise(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Was this utterance addressed to NEXUS, and did it carry a command?
///
/// Returns None for everything else, which is the common case in a room
/// where people are talking. None means the transcript is dropped: it is
/// never matched, never executed, and never leaves the machine.
pub fn detect(transcript: &str) -> Option<Wake> {
    let words: Vec<&str> = transcript.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }

    let normalised: Vec<String> = words.iter().map(|w| normalise(w)).collect();

    // Skip a leading greeting so "hey Nexus" and "Nexus" behave alike.
    let start = if normalised
        .first()
        .is_some_and(|w| GREETING_PREFIX.contains(&w.as_str()))
    {
        1
    } else {
        0
    };

    for (offset, word) in normalised.iter().enumerate().skip(start) {
        if offset - start > MAX_WAKE_POSITION {
            break;
        }
        // A two-word transcription consumes both words, so the command
        // starts one word further on.
        let consumed = if VARIANTS.contains(&word.as_str()) {
            1
        } else if normalised
            .get(offset + 1)
            .is_some_and(|next| SPLIT_VARIANTS.contains(&[word.as_str(), next.as_str()]))
        {
            2
        } else {
            continue;
        };

        return Some(command_after(&words, &normalised, offset + consumed));
    }

    None
}

/// Everything after the wake word, minus the politeness that isn't a command.
fn command_after(words: &[&str], normalised: &[String], from: usize) -> Wake {
    let mut at = from;
    while at < words.len() {
        let word = &normalised[at];
        // An empty normalisation is bare punctuation: the comma in
        // "Nexus, list my tabs".
        if word.is_empty() || LEAD_FILLER.contains(&word.as_str()) {
            at += 1;
            continue;
        }
        break;
    }

    let rest = words[at.min(words.len())..].join(" ");
    let rest = rest.trim_start_matches([',', '.', '!', '?', ' ']).trim();
    if rest.is_empty() {
        Wake::Bare
    } else {
        Wake::WithCommand(rest.to_string())
    }
}

/// The acknowledgement for this turn.
///
/// Rotates rather than randomises: a fixed sequence is reproducible in a
/// test, and randomness buys nothing a user would notice.
pub fn reply(replies: &[String], turn: u64) -> String {
    let usable: Vec<&String> = replies.iter().filter(|r| !r.trim().is_empty()).collect();
    if usable.is_empty() {
        return DEFAULT_REPLIES[(turn as usize) % DEFAULT_REPLIES.len()].to_string();
    }
    usable[(turn as usize) % usable.len()].trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(text: &str) -> String {
        match detect(text) {
            Some(Wake::WithCommand(c)) => c,
            other => panic!("{text:?} -> {other:?}"),
        }
    }

    // -- Being called -------------------------------------------------------

    #[test]
    fn the_bare_wake_word_asks_for_a_command() {
        for phrase in ["Nexus", "nexus", "NEXUS", "Nexus.", "Nexus?", "hey Nexus"] {
            assert_eq!(detect(phrase), Some(Wake::Bare), "{phrase}");
        }
    }

    #[test]
    fn a_command_in_the_same_breath_is_carried_through() {
        assert_eq!(command("Nexus list my tabs"), "list my tabs");
        assert_eq!(command("Nexus, list my tabs"), "list my tabs");
        assert_eq!(
            command("hey Nexus, what's on my calendar"),
            "what's on my calendar"
        );
        assert_eq!(command("okay Nexus please open GitHub"), "open GitHub");
    }

    #[test]
    fn a_misheard_wake_word_still_wakes() {
        // On-device recognition does not know the word "Nexus", so these are
        // what it actually returns. Refusing them means the assistant simply
        // does not answer some of the time.
        for phrase in [
            "Nexis list my tabs",
            "Nexas list my tabs",
            "next us list my tabs",
        ] {
            assert_eq!(command(phrase), "list my tabs", "{phrase}");
        }
        assert_eq!(detect("necks us"), Some(Wake::Bare));
    }

    // -- Not being called ---------------------------------------------------

    #[test]
    fn ordinary_speech_is_dropped() {
        // The whole point of the wake word: with the microphone always open,
        // anything not addressed to NEXUS must do nothing at all.
        for phrase in [
            "list my tabs",
            "can you pass me that",
            "I'll send it over after lunch",
            "no I think the build is broken",
            "",
            "   ",
        ] {
            assert_eq!(detect(phrase), None, "{phrase}");
        }
    }

    #[test]
    fn merely_mentioning_nexus_does_not_wake_it() {
        // Addressing someone puts their name first; talking about them does
        // not. Without the position bound this fires a command mid-meeting.
        for phrase in [
            "I was reading about Nexus yesterday",
            "so the thing I built is called Nexus",
            "tell them we should try Nexus for this",
        ] {
            assert_eq!(detect(phrase), None, "{phrase}");
        }
    }

    #[test]
    fn a_word_that_merely_starts_like_the_wake_word_is_ignored() {
        // "next" on its own is a real word people say constantly.
        for phrase in ["next please", "next one", "nexted", "next week is fine"] {
            assert_eq!(detect(phrase), None, "{phrase}");
        }
    }

    #[test]
    fn the_split_variant_needs_both_halves() {
        assert_eq!(detect("next us"), Some(Wake::Bare));
        assert_eq!(detect("next them"), None);
    }

    // -- Verbatim remainder --------------------------------------------------

    #[test]
    fn dictation_keeps_its_punctuation_and_capitals() {
        // The remainder is the user's sentence, and dictation puts it into a
        // text box unchanged. Normalising it here would silently flatten it.
        assert_eq!(
            command("Nexus type Hello there, how are you?"),
            "type Hello there, how are you?"
        );
        assert_eq!(
            command("Nexus write Fix the PROJ-1069 bug"),
            "write Fix the PROJ-1069 bug"
        );
    }

    #[test]
    fn only_leading_filler_is_stripped() {
        // "can you" before the command is politeness; inside it, it is not.
        assert_eq!(
            command("Nexus can you tell me can you hear this"),
            "tell me can you hear this"
        );
    }

    // -- Acknowledgement -----------------------------------------------------

    #[test]
    fn the_reply_rotates_through_the_set() {
        let set: Vec<String> = DEFAULT_REPLIES.iter().map(|s| s.to_string()).collect();
        assert_eq!(reply(&set, 0), "Yes Rohi");
        assert_eq!(reply(&set, 1), "Yes boss");
        assert_eq!(reply(&set, 2), "Go ahead");
        assert_eq!(reply(&set, 3), "Yes Rohi", "wraps around");
    }

    #[test]
    fn an_empty_or_blank_reply_set_falls_back_to_the_defaults() {
        // A user who clears the field in Settings still gets an answer
        // rather than silence, which would read as "it didn't hear me".
        assert_eq!(reply(&[], 0), "Yes Rohi");
        assert_eq!(reply(&["".to_string(), "   ".to_string()], 1), "Yes boss");
    }

    #[test]
    fn a_custom_reply_set_is_used_as_given() {
        let set = vec!["Sir".to_string(), "  Yep  ".to_string()];
        assert_eq!(reply(&set, 0), "Sir");
        assert_eq!(reply(&set, 1), "Yep", "trimmed");
    }

    #[test]
    fn detection_is_pure_and_repeatable() {
        let first = detect("Nexus open GitHub");
        for _ in 0..20 {
            assert_eq!(detect("Nexus open GitHub"), first);
        }
    }
}
