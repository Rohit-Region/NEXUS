//! NEXUS-014: turning something the user said into something NEXUS can do.
//!
//! This is the escalation ladder, and every rung of it here is deterministic.
//! No provider is involved, and none is reachable from this module. When
//! NEXUS-019 adds reasoning it becomes one more rung *below* these, reached
//! only when they all decline.
//!
//! The order is the point:
//!
//! 1. **Local answer.** "Show my blocked tasks" is a question a query
//!    answers. Sending it to a model would be slower, less accurate, and
//!    would stop working on a train.
//! 2. **Known command.** The NEXUS-010 matcher already resolves spoken
//!    phrases to registry ids, and it is reused verbatim rather than
//!    reimplemented: one matching contract, one place to fix it.
//! 3. **Decline, with a reason.** Saying "I can't do that yet" is a correct
//!    answer. Guessing is not.
//!
//! The command registry arrives from the caller rather than being duplicated
//! here, exactly as the voice path already does it, so src/lib/commands.ts
//! stays the single source of truth for what NEXUS can do.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::voice::intent::{resolve_voice_intent, VoiceCommandSpec};

use super::connector::ReferentDraft;
use super::context::{work_context, WorkContext};
use super::reasoning::{
    build_context, validate_plan, AiContext, Purpose, Reasoning, ValidatedStep,
};
use super::referent::ReferentKind;
use super::session::SessionSnapshot;

/// One thing NEXUS could do, when several match.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Choice {
    /// The registry id from src/lib/commands.ts, so the caller can map it
    /// through the same bridge the palette uses.
    pub command_id: String,
    pub label: String,
    /// Arguments to run the choice with, when the options differ by their
    /// input rather than by which action they are.
    ///
    /// Two contacts with similar names are the same action twice over; what
    /// distinguishes them is the number. Encoding that into `command_id`
    /// would put a phone number and a message into an identifier, which is
    /// the wrong shape and would land them in logs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
}

/// What NEXUS made of the request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AssistantReply {
    /// Answered from local data. `cited` names what the answer was built
    /// from, because attribution NEXUS can actually make is worth more than
    /// a claim of correctness it cannot.
    #[serde(rename_all = "camelCase")]
    Answer { text: String, cited: Vec<String> },
    /// One clear thing to do.
    ///
    /// Carries the *registry* id, not an action id, and is not executed here.
    /// Only the palette bridge knows how a registry id maps to a typed
    /// action, and duplicating that mapping would create a second copy to
    /// drift. The caller maps it, then runs it through the gate, which is
    /// where permission and confirmation live.
    #[serde(rename_all = "camelCase")]
    Action {
        command_id: String,
        summary: String,
        /// Arguments extracted from the phrase, for actions that carry a
        /// target. Null for a plain registry command, whose input the palette
        /// bridge supplies.
        #[serde(default)]
        input: serde_json::Value,
    },
    /// Several things matched. NEXUS names them rather than picking.
    #[serde(rename_all = "camelCase")]
    Choices { candidates: Vec<Choice> },
    /// Nothing matched, and NEXUS says so plainly.
    #[serde(rename_all = "camelCase")]
    /// NEXUS is not doing anything with this.
    ///
    /// `understood` separates two cases the caller must treat differently.
    /// True means the request was followed and the answer is no: there is no
    /// contact by that name, the connector is not set up. That is worth
    /// saying aloud. False means NEXUS never made sense of the words at all,
    /// which, with the microphone held open, is mostly a fragment of
    /// somebody's conversation. Announcing a reason for each of those turns
    /// the assistant into a heckler.
    Unresolved { reason: String, understood: bool },
    /// A reasoning provider proposed steps, every one of which was validated
    /// against the action registry. Nothing has run: each step still goes
    /// through the gate, with its own permission and confirmation.
    #[serde(rename_all = "camelCase")]
    Proposal {
        steps: Vec<ValidatedStep>,
        rationale: String,
    },
}

/// A reply, plus what it put into the conversation.
///
/// An answer that lists things must register them, or "do the first one"
/// has nothing to count through. Keeping that here rather than in the
/// command layer means the code that knows the ids is the code that files
/// them.
#[derive(Debug, Clone)]
pub struct Response {
    pub reply: AssistantReply,
    pub referents: Vec<ReferentDraft>,
    /// True when the referents were rendered as a numbered list the user can
    /// count through. A pair of unrelated mentions is not a list.
    pub rendered_as_list: bool,
    /// Whether a live follow-up offer should survive this turn.
    ///
    /// Only one reply sets it: the re-ask NEXUS gives when it could not make
    /// out an answer to a question it had just asked. Everything else lets
    /// the offer lapse, because the user moved on, and an offer that
    /// outlives the moment is how a much later "yes" sends a message nobody
    /// was still looking at.
    pub holds_follow_up: bool,
    /// An action to run straight after this reply, with no question asked.
    ///
    /// For the case where the answer is only half of what was wanted: "good
    /// morning" is a greeting NEXUS can give from local data, and the ticket
    /// list the user actually wants needs a connector. The resolver only
    /// names the action; the command layer runs it through the gate, so it
    /// is permitted, confirmed and audited exactly like one the user asked
    /// for by name. Nothing here builds or runs anything, which is what the
    /// guard test below checks.
    ///
    /// Only ever a `Read` action that confirms nothing. Chaining something
    /// that writes would mean an effect the user never requested, which is
    /// the difference between an assistant being helpful and being loose.
    pub then: Option<(String, serde_json::Value)>,
    /// Text to append after whatever `then` produced.
    ///
    /// So a greeting can read time, then tickets, then what is planned for
    /// today, in that order, even though the middle part comes from a
    /// connector and the outer two are local. Without it the local halves
    /// would both have to sit in front of the chained one.
    pub tail: Option<String>,
}

impl Response {
    fn plain(reply: AssistantReply) -> Self {
        Response {
            reply,
            referents: Vec::new(),
            rendered_as_list: false,
            holds_follow_up: false,
            then: None,
        tail: None,
        }
    }
}

/// What to call the user. Empty means NEXUS greets without a name rather
/// than guessing one from the account.
const KEY_USER_NAME: &str = "user_name";

pub fn user_name(conn: &Connection) -> Option<String> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        [KEY_USER_NAME],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .map(|v| v.trim().to_string())
    .filter(|v| !v.is_empty() && v.chars().count() <= 40)
}

pub fn set_user_name(conn: &Connection, name: &str) -> Result<(), String> {
    let cleaned = name.trim();
    if cleaned.chars().count() > 40 {
        return Err("That name is too long.".to_string());
    }
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE
           SET value = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        rusqlite::params![KEY_USER_NAME, cleaned],
    )
    .map_err(|e| format!("Failed to store the name: {e}"))?;
    Ok(())
}

/// The local clock, from SQLite rather than a date crate.
///
/// `localtime` applies the machine's timezone, which is the one the user is
/// actually in. Returns (hour, "17:04", "Thursday").
fn local_clock(conn: &Connection) -> (i64, String, String) {
    let fallback = (12i64, "now".to_string(), "today".to_string());
    let row: Result<(String, String, String), _> = conn.query_row(
        "SELECT strftime('%H','now','localtime'),
                strftime('%H:%M','now','localtime'),
                strftime('%w','now','localtime')",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    );

    match row {
        Ok((hour, clock, weekday)) => {
            const DAYS: [&str; 7] = [
                "Sunday",
                "Monday",
                "Tuesday",
                "Wednesday",
                "Thursday",
                "Friday",
                "Saturday",
            ];
            let day = weekday
                .parse::<usize>()
                .ok()
                .and_then(|i| DAYS.get(i))
                .copied()
                .unwrap_or("today");
            (hour.parse::<i64>().unwrap_or(12), clock, day.to_string())
        }
        Err(_) => fallback,
    }
}

/// "Morning" / "Afternoon" / "Evening", from the actual hour.
fn part_of_day(hour: i64) -> &'static str {
    match hour {
        5..=11 => "Morning",
        12..=16 => "Afternoon",
        17..=21 => "Evening",
        _ => "Hello",
    }
}

/// Openers that are not requests.
///
/// NEXUS-015 defect D. "Hi" escalated to a reasoning provider and came back
/// as an error, which is a poor thing for an assistant to say to a greeting.
/// It is also wasteful: a greeting has a local answer, and the escalation
/// ladder exists to stop exactly this kind of round trip.
const GREETINGS: &[&str] = &[
    "hi",
    "hey",
    "hello",
    "yo",
    "hiya",
    "morning",
    "afternoon",
    "evening",
    "thanks",
    "thank",
    "ok",
    "okay",
    "cool",
    "nice",
    "sup",
    "greetings",
    // "good" carries no meaning alone but introduces most of the others:
    // without it "good morning" matched nothing and escalated.
    "good",
    "day",
    "there",
];

/// Words that make a greeting into a question about NEXUS itself.
const ABOUT_NEXUS: &[&str] = &["you", "your", "yourself", "nexus", "there"];

/// Words that mean "go ahead", in the languages actually spoken at this
/// machine.
///
/// English and Hinglish together, because that is how the sentence comes
/// out: "send karo" and "haan bhej do" are not edge cases here, they are the
/// normal phrasing, and a list that only knew "yes" left them to be
/// discarded as room noise.
///
/// Every word is one a person says while *answering a question*. That is the
/// only reason a list this permissive is safe: it is consulted solely when a
/// follow-up is on offer, so "do" cannot resolve to anything on its own. Out
/// in the open these words mean nothing in particular, and matching them
/// there would turn any overheard "yeah" into an action.
const AFFIRMATIONS: &[&str] = &[
    "yes", "yeah", "yep", "yup", "ya", "yah", "sure", "ok", "okay", "okey", "k", "right",
    "correct", "confirm", "confirmed", "approve", "approved", "accept", "agreed", "go",
    "ahead", "proceed", "send", "sent", "run", "do", "it", "that", "this", "one",
    "please",
    // Hinglish. `haan`/`han`/`ha` are yes; `theek`/`thik` is fine; `bhej`,
    // `bhejo` and `bhej do` are send; `karo`/`kar`/`kardo`/`kar do` is the
    // imperative that follows almost any verb.
    "haan", "han", "ha", "haa", "theek", "thik", "sahi", "bilkul", "bhej", "bhejo",
    "bhejdo", "karo", "kar", "kardo", "de",
    // The words that hang off the end of a Hinglish yes and carry no
    // meaning of their own: "theek hai", "haan ji", "acha chalo".
    "hai", "hain", "ji", "chalo", "acha", "achha", "abhi",
];

/// Verbs that hand a sentence to an assistant living inside an editor.
///
/// "claude" is required alongside one of these, and that is deliberate: it
/// is the marker that separates dictation from every other request. Without
/// it, "tell me what is blocked" would be typed into an editor instead of
/// answered, and with a microphone held open the cost of getting that wrong
/// is a stray sentence pasted into somebody's work.
const DICTATE_TO_ASSISTANT: &[&str] = &["tell", "ask", "type", "dictate", "write", "say"];

/// Words that mean "leave it".
///
/// Read before the affirmations, so "no, don't send it" declines rather than
/// matching on "send". A phrase carrying both is a refusal: the cost of
/// hearing a no as a yes is a message that cannot be unsent, and the cost of
/// the reverse is asking again.
const DECLINES: &[&str] = &[
    "no", "nope", "nah", "dont", "don", "not", "cancel", "stop", "wait", "abort",
    "never", "mind", "leave", "forget", "skip", "hold",
    // Hinglish: `nahi`/`nahin` is no, `rehne do` and `chhod do` are leave it.
    "nahi", "nahin", "na", "mat", "rehne", "rehnedo", "chhod", "chod", "chhodo", "ruko",
    "ruk",
];

/// Every connector NEXUS can talk about by name, for the "what can you do
/// with X" answer. Derived from the registry rather than listed here, so it
/// cannot fall out of step.
fn connector_words() -> Vec<(String, String)> {
    super::connectors()
        .into_iter()
        .map(|c| (c.id().to_string(), c.display_name().to_lowercase()))
        .collect()
}

/// Verbs that mean "launch this application".
///
/// Narrower than the tab fallback's list: "show" and "focus" read as asking
/// about something rather than starting it.
const OPENING_VERBS: [&str; 3] = ["open", "launch", "start"];

/// Verbs that mean "take me to", and so licence the tab fallback.
///
/// Only these reach it. Any other phrase that got this far named nothing
/// NEXUS knows, and inventing a tab for it would be a guess dressed as an
/// answer.
const NAVIGATION_VERBS: [&str; 6] = ["go", "switch", "open", "show", "focus", "jump"];

/// Actions a bare phrase can invoke, as matcher candidates.
///
/// Each connector declares which of its actions need no input. Their ids and
/// summaries become keywords, so "list my tabs" and "check github" resolve
/// deterministically instead of escalating. Actions needing a URL or an id
/// are excluded on purpose: see `zero_input_actions`.
fn connector_candidates(conn: &Connection) -> Vec<VoiceCommandSpec> {
    let mut out = Vec::new();
    for connector in super::connectors() {
        // A connector the user has not allowed should not be offered; the
        // gate would refuse it anyway, and offering it teaches the user that
        // NEXUS suggests things that do not work.
        let granted = super::permission::granted_levels(conn, connector.id()).unwrap_or_default();
        if granted.is_empty() {
            continue;
        }

        for action_id in connector.zero_input_actions() {
            let spec = match connector.spec(action_id) {
                Some(spec) if granted.contains(&spec.permission) => spec,
                _ => continue,
            };

            // Keywords from the connector's name, the action's verb, and the
            // words of its summary. No hand-written synonym list: the summary
            // is already written for a human to read.
            let mut keywords: Vec<String> = vec![
                connector.id().to_string(),
                connector.display_name().to_lowercase(),
            ];
            if let Some(verb) = action_id.split('.').nth(1) {
                keywords.extend(verb.split('_').map(|w| w.to_string()));
            }
            keywords.extend(
                spec.summary
                    .to_lowercase()
                    .split_whitespace()
                    .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
                    .filter(|w| w.len() > 2),
            );
            keywords.sort();
            keywords.dedup();

            out.push(VoiceCommandSpec {
                id: action_id.to_string(),
                label: spec.summary.to_string(),
                keywords,
            });
        }
    }
    out
}

/// Words that carry no meaning of their own around a connector's name.
///
/// "go to chrome" is naming an application, not requesting an action. The
/// matcher would happily resolve it to whichever Chrome action shares the
/// most keywords, which is a guess dressed as an answer.
const BARE_REFERENCE_FILLER: &[&str] = &[
    "go", "to", "open", "show", "me", "my", "the", "a", "an", "in", "on", "switch", "take",
    "please", "hey", "nexus", "up", "into", "over", "at", "launch", "start", "bring",
];

/// True when the phrase is a connector's name and nothing else of substance.
fn is_bare_connector_reference(tokens: &[String], connector_word: &str) -> bool {
    tokens
        .iter()
        .all(|t| t == connector_word || BARE_REFERENCE_FILLER.contains(&t.as_str()))
}

/// What a named connector can actually do here, as choices.
///
/// The answer to "go to chrome", which names a connector but no action. Far
/// better than escalating: NEXUS knows the answer and can offer it.
fn connector_menu(conn: &Connection, tokens: &[String], bare_only: bool) -> Option<Response> {
    let named = connector_words()
        .into_iter()
        .find(|(id, name)| tokens.iter().any(|t| t == id || t == name))?;

    // When asked before the matcher runs, only a phrase that is *just* the
    // connector's name qualifies. "list my tabs in chrome" has a real request
    // in it and belongs to the matcher.
    if bare_only
        && !is_bare_connector_reference(tokens, &named.0)
        && !is_bare_connector_reference(tokens, &named.1)
    {
        return None;
    }

    let connector = super::connectors()
        .into_iter()
        .find(|c| c.id() == named.0)?;
    let granted = super::permission::granted_levels(conn, connector.id()).unwrap_or_default();

    if granted.is_empty() {
        return Some(Response::plain(AssistantReply::Unresolved {
            // Followed, and the answer is no. Worth saying.
            understood: true,
            reason: format!(
                "{} is not allowed yet. Turn it on under Permissions in Settings.",
                connector.display_name()
            ),
        }));
    }

    let candidates: Vec<Choice> = connector
        .zero_input_actions()
        .iter()
        .filter_map(|id| connector.spec(id))
        .filter(|spec| granted.contains(&spec.permission))
        .map(|spec| Choice {
            input: None,
            command_id: spec.id.to_string(),
            label: spec.summary.to_string(),
        })
        .collect();

    if candidates.is_empty() {
        return None;
    }
    Some(Response {
        reply: AssistantReply::Choices { candidates },
        referents: Vec::new(),
        rendered_as_list: false,
        holds_follow_up: false,
        then: None,
        tail: None,
    })
}

/// The sentence meant for the assistant inside an editor, if this is one.
///
/// Takes the text from the *original* rather than the normalised tokens, so
/// punctuation and capitals survive into the prompt box: a dictated sentence
/// arrives as speech-to-text already, and flattening it twice would hand the
/// editor something the user did not say.
///
/// Returns `None` unless both a verb and "claude" are present, and unless
/// something is left after them. "tell claude" on its own is somebody
/// clearing their throat.
fn dictated_to_assistant(original: &str) -> Option<String> {
    let words: Vec<&str> = original.split_whitespace().collect();
    let word_at = |i: usize| {
        words[i]
            .to_lowercase()
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_string()
    };

    // The marker must be the word directly after the verb, not merely
    // present somewhere in the sentence. "ask me what Claude said" names
    // Claude without addressing it, and typing that into an editor is
    // exactly the stray-sentence failure this shape exists to avoid.
    let marker = (1..words.len()).find(|i| {
        word_at(*i) == "claude" && DICTATE_TO_ASSISTANT.contains(&word_at(i - 1).as_str())
    })?;

    // The words that only join the verb to the sentence. Stripped so "tell
    // Claude to run the tests" dictates "run the tests" rather than "to run
    // the tests", which is a different instruction.
    const JOINERS: &[&str] = &["to", "that", "in", "on", "please", "it", "and", "the"];
    let body: Vec<&str> = words[marker + 1..]
        .iter()
        .copied()
        .skip_while(|w| {
            let bare: String = w
                .to_lowercase()
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_string();
            JOINERS.contains(&bare.as_str())
        })
        .collect();

    (!body.is_empty()).then(|| body.join(" "))
}

/// Phrases that record a commitment, longest first so the longer one wins.
///
/// Deliberately short and deliberately explicit. Every one is something a
/// person says on purpose when they want to be reminded; none of them is
/// something you say by accident near an open microphone.
const COMMITMENT_OPENERS: &[&str] = &[
    "remind me to ",
    "remind me ",
    "don't let me forget to ",
    "dont let me forget to ",
    "make a note to ",
    "note to self ",
];

/// A commitment and when to raise it, if this phrase is one.
///
/// Returns the user's own words, from the original rather than the normalised
/// tokens, so NEXUS repeats what they said instead of its flattening of it.
fn commitment_phrase(original: &str, conn: &Connection) -> Option<(String, Option<i64>)> {
    let lower = original.to_lowercase();
    let start = COMMITMENT_OPENERS
        .iter()
        .find_map(|opener| lower.find(opener).map(|at| at + opener.len()))?;

    let rest = original[start..].trim();
    if rest.is_empty() {
        return None;
    }

    // "... in twenty minutes" splits into what and when. Searched from the
    // right: "remind me to call him in accounts in ten minutes" has two, and
    // the trailing one is the time.
    // Clock times and named days first: "tomorrow", "tonight", "at 5". They
    // are far more common in speech than "in ninety minutes", and until this
    // existed they fell through to "someday", which records a reminder that
    // can never fire. A reminder silently set to never is worse than a
    // refusal, because the user stops carrying the thing themselves.
    if let Some((body, minutes)) = split_named_time(rest, conn) {
        return (!body.trim().is_empty()).then(|| (body.trim().to_string(), Some(minutes)));
    }

    let (what, minutes) = match split_trailing_delay(rest) {
        Some((body, Some(mins))) => (body, Some(mins)),
        // A unit was there and the number was not usable. Recording that as
        // "someday" would quietly keep a reminder the user asked to have at a
        // specific time, so the whole phrase is declined and they can say it
        // again.
        Some((_, None)) => return None,
        // No time at all is not a failure. It is "someday": recorded, never
        // raised, visible where the user can act on it themselves.
        None => (rest.to_string(), None),
    };

    // "remind me to" leaves "to" behind, which is not something to be
    // reminded of. A body that is only joining words is somebody starting a
    // sentence and stopping.
    const JOINERS: &[&str] = &["to", "that", "about", "the", "a", "an", "me", "it"];
    let substantive = what
        .split_whitespace()
        .any(|w| {
            let bare: String = w
                .to_lowercase()
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_string();
            !bare.is_empty() && !JOINERS.contains(&bare.as_str())
        });

    substantive.then(|| (what.trim().to_string(), minutes))
}

/// A delay in minutes, as a person would say it.
///
/// The confirmation is the last point a misheard time can be caught, and
/// "in 1080 minutes" is not something anybody can check at a glance.
fn in_words(minutes: i64) -> String {
    match minutes {
        m if m < 60 => format!("in {m} minutes"),
        m if m < 120 => "in about an hour".to_string(),
        m if m < 24 * 60 => format!("in about {} hours", (m + 30) / 60),
        m => format!("in about {} hours", (m + 30) / 60),
    }
}

/// Default hour for a day named without a time. Early enough to be useful,
/// late enough not to be the first thing that happens.
const TOMORROW_HOUR: i64 = 9;
/// What "tonight" means, when no hour is given.
const TONIGHT_HOUR: i64 = 20;

/// "tomorrow", "tonight", "at 5", "at 17:30" as minutes from now.
///
/// Returns the phrase with the time expression removed, and how far off it
/// is. Everything is local wall-clock, read from the same source as
/// `local_clock`, so NEXUS never has two ideas of what time it is.
/// Day names, longest spelling first so "sat" cannot claim "saturday".
///
/// Indexed to match `calendar::weekday_today`, where Sunday is 0, because
/// that is what SQLite's `%w` returns and converting between two conventions
/// is a bug waiting for a Sunday.
const WEEKDAYS: [&[&str]; 7] = [
    &["sunday", "sun"],
    &["monday", "mon"],
    &["tuesday", "tues", "tue"],
    &["wednesday", "weds", "wed"],
    &["thursday", "thurs", "thur", "thu"],
    &["friday", "fri"],
    &["saturday", "sat"],
];

fn split_named_time(text: &str, conn: &Connection) -> Option<(String, i64)> {
    let now = super::calendar::local_minutes(conn)?;
    let lower = text.to_lowercase();

    // How far ahead a target time-of-day is, rolling to the next day when it
    // has already passed. "remind me at 8" said at nine in the evening means
    // tomorrow morning, not fourteen hours ago.
    let ahead = |target: i64, days: i64| {
        let raw = target - now + days * 1440;
        if raw > 0 {
            raw
        } else {
            raw + 1440
        }
    };

    // "at 5", "at 5pm", "at 17:30". Searched before the bare day words so
    // "tomorrow at 8" gets the hour rather than the default.
    //
    // Both a leading "at ..." and an embedded " at ... " are handled: "remind
    // me at 5 to call the dentist" puts the time in the middle of the
    // sentence and leaves real words on either side of it, and a rule that
    // only looked at the end silently parsed no time at all.
    let marker = if lower.starts_with("at ") {
        Some(0usize)
    } else {
        lower.rfind(" at ").map(|i| i + 1)
    };
    if let Some(at) = marker {
        let tail = &text[at + 3..];
        if let Some((target, consumed)) = clock_phrase(tail.trim_start()) {
            let lead = text[..at].trim();
            // Everything after the clock is still part of what to do:
            // "at 5 to call the dentist" is a reminder to call the dentist.
            let skipped = tail.len() - tail.trim_start().len();
            let after = tail[skipped + consumed..].trim();
            let after = after.strip_prefix("to ").unwrap_or(after).trim();

            // "tomorrow at 8" leaves "tomorrow" on the end of the lead.
            let tomorrow = lead.to_lowercase().ends_with("tomorrow");
            let lead = if tomorrow {
                lead[..lead.len() - "tomorrow".len()].trim()
            } else {
                lead
            };

            let mut body = String::from(lead);
            if !body.is_empty() && !after.is_empty() {
                body.push(' ');
            }
            body.push_str(after);
            return Some((body.trim().to_string(), ahead(target, i64::from(tomorrow))));
        }
    }

    // A named day of the week: "on Saturday", "next Saturday".
    //
    // Always the next one that has not happened yet, and "next" adds a
    // further week only when the named day is today. Said on a Tuesday,
    // "Saturday" and "next Saturday" mean the same Saturday, which is what
    // people mean; said on a Saturday they mean today and a week today.
    if let Some(weekday) = super::calendar::weekday_today(conn) {
        for (index, names) in WEEKDAYS.iter().enumerate() {
            let Some(found) = names.iter().find_map(|n| lower.find(n).map(|at| (at, n.len())))
            else {
                continue;
            };
            let (at, len) = found;
            let mut ahead_days = (index as i64 - weekday).rem_euclid(7);
            if ahead_days == 0 {
                ahead_days = 7;
            }
            // "next" only matters when it would otherwise be today.
            // "next" and "on" are about the day, not about the task. Peeled
            // one word at a time, so "on" as a whole lead is removed just as
            // "call mum on" would be: a suffix match needing a leading space
            // left "on saturday to call mum" recorded as "on call mum".
            let mut lead = text[..at].trim();
            loop {
                let trimmed = match lead.rsplit_once(char::is_whitespace) {
                    Some((head, last)) => (matches!(
                        last.to_lowercase().as_str(),
                        "next" | "on" | "this" | "coming"
                    ))
                    .then_some(head),
                    None => (matches!(
                        lead.to_lowercase().as_str(),
                        "next" | "on" | "this" | "coming"
                    ))
                    .then_some(""),
                };
                match trimmed {
                    Some(shorter) => lead = shorter.trim(),
                    None => break,
                }
            }

            let rest = text[at + len..].trim();
            let rest = rest.strip_prefix("to ").unwrap_or(rest).trim();
            let mut body = String::from(lead);
            if !body.is_empty() && !rest.is_empty() {
                body.push(' ');
            }
            body.push_str(rest);
            return Some((
                body.trim().to_string(),
                ahead(TOMORROW_HOUR * 60, ahead_days),
            ));
        }
    }

    for (word, hour, days) in [
        ("tomorrow", TOMORROW_HOUR, 1),
        ("tonight", TONIGHT_HOUR, 0),
        ("this evening", TONIGHT_HOUR, 0),
    ] {
        if let Some(found) = lower.find(word) {
            let mut body = String::new();
            body.push_str(text[..found].trim());
            let rest = text[found + word.len()..].trim();
            if !body.is_empty() && !rest.is_empty() {
                body.push(' ');
            }
            body.push_str(rest);
            return Some((body.trim().to_string(), ahead(hour * 60, days)));
        }
    }

    None
}

/// "5", "5pm", "17:30". Anything vaguer fails rather than guessing.
///
/// Returns the time as minutes past midnight and how many bytes of the input
/// it used, so the caller can keep the words on either side of it.
fn clock_phrase(tail: &str) -> Option<(i64, usize)> {
    let words: Vec<&str> = tail.split_whitespace().collect();
    // At most two: "5 pm". A third would start eating the reminder itself.
    let take = words.len().min(2);
    let mut consumed = 0usize;
    let mut used = 0usize;
    for (i, word) in words.iter().take(take).enumerate() {
        let is_time = word
            .chars()
            .all(|c| c.is_ascii_digit() || c == ':' || c == '.')
            || matches!(word.to_lowercase().as_str(), "am" | "pm" | "o'clock");
        if !is_time {
            break;
        }
        used = i + 1;
        consumed = tail.find(word).map(|at| at + word.len()).unwrap_or(consumed);
    }
    if used == 0 {
        return None;
    }
    let first = words[..used].join(" ");
    let cleaned = first
        .to_lowercase()
        .replace("o'clock", "")
        .replace(".", ":")
        .trim()
        .to_string();

    let pm = cleaned.contains("pm") || cleaned.contains("p m");
    let am = cleaned.contains("am") || cleaned.contains("a m");
    let digits: String = cleaned
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == ':')
        .collect();
    if digits.is_empty() {
        return None;
    }

    let (h, m) = match digits.split_once(':') {
        Some((h, m)) => (h.parse::<i64>().ok()?, m.parse::<i64>().unwrap_or(0)),
        None => (digits.parse::<i64>().ok()?, 0),
    };
    if !(0..=23).contains(&h) || !(0..=59).contains(&m) {
        return None;
    }

    // "at 5" with no marker means the afternoon far more often than the dawn.
    let hour = match (h, pm, am) {
        (h, true, _) if h < 12 => h + 12,
        (h, _, true) => h % 12,
        (h, false, false) if h < 7 => h + 12,
        (h, _, _) => h,
    };
    Some((hour * 60 + m, consumed))
}

/// Spoken numbers, because dictation writes "twenty" rather than "20".
const SPOKEN_NUMBERS: &[(&str, i64)] = &[
    ("a", 1), ("an", 1), ("one", 1), ("two", 2), ("three", 3), ("four", 4),
    ("five", 5), ("ten", 10), ("fifteen", 15), ("twenty", 20), ("thirty", 30),
    ("forty", 40), ("forty five", 45), ("fortyfive", 45), ("sixty", 60),
    ("half", 30), ("couple", 2), ("few", 5),
];

/// "in ten minutes" / "in 2 hours" at the end of a phrase, as minutes.
///
/// Three outcomes, and they are all different: `None` means the phrase names
/// no time at all, `Some((body, Some(m)))` a usable one, and
/// `Some((body, None))` a unit with a number that cannot be right. The last
/// must not be flattened into the first, or "in 900000 hours" quietly becomes
/// a reminder with no time on it.
/// Words that introduce a delay. Longest first, so "after" is tried before
/// anything that could be a prefix of it.
const DELAY_MARKERS: [&str; 3] = ["after", "in", "within"];

/// Does this fragment name a unit of time?
///
/// Decides whether a marker introduces a delay or a place: "in ten minutes"
/// against "in accounts", "after two minutes" against "after the standup".
fn has_unit(tail: &str) -> bool {
    tail.contains("hour") || tail.contains("min") || tail.contains("day")
}

fn split_trailing_delay(text: &str) -> Option<(String, Option<i64>)> {
    let lower = text.to_lowercase();

    // Trailing first, because "call him in accounts in ten minutes" has two
    // and the last one is the time. Leading is tried only when the trailing
    // search finds no unit.
    //
    // The leading form was unreachable until now, which cost the most
    // natural phrasing there is: "remind me in five minutes to stretch" put
    // the delay in front of the task, matched nothing, and recorded a
    // reminder that could never fire.
    // "in" and "after" both introduce a delay, and "after" is at least as
    // common in speech: "remind me after two minutes to cook" recorded no
    // time at all, because only "in" was ever looked for. The marker length
    // differs, so it is carried rather than assumed.
    let (at, len, leading) = DELAY_MARKERS
        .iter()
        .find_map(|word| {
            let spaced = format!(" {word} ");
            match lower.rfind(&spaced) {
                Some(at) if has_unit(&lower[at + spaced.len()..]) => {
                    Some((at + 1, word.len(), false))
                }
                _ => {
                    let prefix = format!("{word} ");
                    lower
                        .starts_with(&prefix)
                        .then_some((0usize, word.len(), true))
                }
            }
        })
        .or_else(|| {
            // A marker with no unit after it is a place or a sequence, not a
            // time: "call him in accounts", "after the standup".
            None
        })?;

    let tail = lower[at + len + 1..].trim();

    let unit_minutes = if tail.contains("hour") {
        60
    } else if tail.contains("min") {
        1
    } else if tail.contains("day") {
        60 * 24
    } else {
        // "in the kitchen" is not a time. Anything without a unit is part of
        // what the user wants to be reminded about.
        return None;
    };

    let count_words: Vec<&str> = tail
        .split_whitespace()
        .take_while(|w| !w.starts_with("min") && !w.starts_with("hour") && !w.starts_with("day"))
        .collect();
    let spoken = count_words.join(" ");

    // Leading form: what to do is everything *after* the unit, not before it.
    let body = if leading {
        let consumed = tail
            .split_whitespace()
            .take(count_words.len() + 1)
            .map(|w| w.len() + 1)
            .sum::<usize>()
            .min(tail.len());
        let rest = text[at + len + 1..].trim();
        let rest = &rest[consumed.min(rest.len())..];
        rest.trim().strip_prefix("to ").unwrap_or(rest.trim()).trim().to_string()
    } else {
        text[..at.saturating_sub(1)].trim().to_string()
    };
    let count = spoken
        .parse::<i64>()
        .ok()
        .or_else(|| {
            SPOKEN_NUMBERS
                .iter()
                .find(|(word, _)| *word == spoken)
                .map(|(_, n)| *n)
        })
        // "in an hour" and "in a minute" leave nothing before the unit.
        .or(if spoken.is_empty() { Some(1) } else { None });

    // A reminder years out is a misheard number, not a plan.
    let usable = count.filter(|n| *n > 0 && n * unit_minutes <= 60 * 24 * 30);
    Some((body, usable.map(|n| n * unit_minutes)))
}

/// Lowercase, strip punctuation, collapse whitespace.
fn normalize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .map(|s| s.to_string())
        .collect()
}

fn has_any(tokens: &[String], words: &[&str]) -> bool {
    words.iter().any(|w| tokens.iter().any(|t| t == w))
}

fn plural(count: i64, singular: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {singular}s")
    }
}

/// Count tasks by status across the workspace.
fn count_by_status(conn: &Connection, status: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM tasks WHERE status = ?1",
        [status],
        |row| row.get::<_, i64>(0),
    )
    .unwrap_or(0)
}

/// Tasks in a status, capped so an answer stays an answer.
///
/// Returns ids alongside labels: an answer that names things has to be able
/// to register them, or the follow-up cannot refer to them.
fn tasks_by_status(conn: &Connection, status: &str, limit: i64) -> Vec<(i64, String)> {
    let mut stmt = match conn.prepare(
        "SELECT t.id, t.title, p.name
           FROM tasks t JOIN projects p ON p.id = t.project_id
          WHERE t.status = ?1
          ORDER BY t.updated_at DESC, t.id DESC
          LIMIT ?2",
    ) {
        Ok(stmt) => stmt,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map(rusqlite::params![status, limit], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            format!(
                "{} ({})",
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?
            ),
        ))
    });
    match rows {
        Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// Longest list NEXUS will read back before it stops being an answer.
const ANSWER_LIST_CAP: i64 = 5;

/// Try to answer from local data alone.
///
/// A fixed set of patterns, not an intent classifier. Each one is a question
/// with a factual answer NEXUS already holds, which is exactly the set worth
/// keeping away from a model.
fn local_answer(tokens: &[String], conn: &Connection, work: &WorkContext) -> Option<Response> {
    // "What am I working on?" / "What is my current project?"
    if has_any(tokens, &["working", "current", "active"])
        && has_any(tokens, &["on", "project", "task"])
    {
        return Some(Response::plain(
            match (&work.current_project, &work.current_task) {
                (Some(project), Some(task)) => AssistantReply::Answer {
                    text: format!(
                        "You're on {}, most recently {} ({}). It has {} open and {}.",
                        project.name,
                        task.title,
                        task.status,
                        project.open_tasks,
                        plural(project.blocked_tasks, "blocked task")
                    ),
                    cited: vec![project.name.clone(), task.title.clone()],
                },
                (Some(project), None) => AssistantReply::Answer {
                    text: format!(
                        "You're on {}, with {} open and {}.",
                        project.name,
                        project.open_tasks,
                        plural(project.blocked_tasks, "blocked task")
                    ),
                    cited: vec![project.name.clone()],
                },
                _ => AssistantReply::Answer {
                    text: "Nothing yet this session. Open a project and I'll keep track."
                        .to_string(),
                    cited: Vec::new(),
                },
            },
        ));
    }

    // "Show my blocked tasks."
    for (status, words) in [
        ("blocked", &["blocked"][..]),
        ("open", &["open"][..]),
        ("done", &["done", "finished", "completed"][..]),
    ] {
        if has_any(tokens, words) && has_any(tokens, &["task", "tasks", "todo", "todos"]) {
            let total = count_by_status(conn, status);
            if total == 0 {
                return Some(Response::plain(AssistantReply::Answer {
                    text: format!("Nothing is {status}."),
                    cited: Vec::new(),
                }));
            }
            let rows = tasks_by_status(conn, status, ANSWER_LIST_CAP);
            let titles: Vec<String> = rows.iter().map(|(_, label)| label.clone()).collect();
            let shown = titles.len() as i64;
            let mut text = format!(
                "{}: {}",
                plural(total, &format!("{status} task")),
                titles.join(", ")
            );
            if total > shown {
                text.push_str(&format!(", and {} more", total - shown));
            }
            text.push('.');
            return Some(Response {
                reply: AssistantReply::Answer {
                    text,
                    cited: titles,
                },
                // Registered in the order the user reads them, which is what
                // makes "do the first one" a position rather than a guess.
                referents: rows
                    .into_iter()
                    .map(|(id, label)| ReferentDraft {
                        kind: ReferentKind::Task,
                        display_name: label,
                        metadata: serde_json::json!({ "id": id }),
                    })
                    .collect(),
                rendered_as_list: true,
                holds_follow_up: false,
                then: None,
                tail: None,
            });
        }
    }

    None
}

/// A greeting with the time of day, the user's name and where things stand.
///
/// Everything in it is local. The weather is deliberately absent: it is a
/// network call behind a permission, and a greeting that stops working on a
/// train is a worse greeting.
/// What is still due today, as a sentence, or None if nothing is.
///
/// Local data, so this needs no connector and works with the network down.
/// Only what is still ahead: a reminder that already fired this morning is
/// not something "planned for today", and repeating it at the next greeting
/// is how a briefing becomes noise.
fn reminders_today(conn: &Connection) -> Option<String> {
    let mut stmt = conn
        .prepare(
            "SELECT what, strftime('%H:%M', due_at, 'unixepoch', 'localtime') AS at
               FROM commitments
              WHERE state = 'open'
                AND raised_at IS NULL
                AND due_at IS NOT NULL
                AND date(due_at, 'unixepoch', 'localtime') = date('now', 'localtime')
                AND due_at > strftime('%s', 'now')
              ORDER BY due_at ASC
              LIMIT 5",
        )
        .ok()?;

    let rows: Vec<String> = stmt
        .query_map([], |row| {
            Ok(format!(
                "{} at {}",
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?
            ))
        })
        .ok()?
        .flatten()
        .collect();

    if rows.is_empty() {
        return None;
    }
    Some(if rows.len() == 1 {
        format!("One reminder today: {}.", rows.join(""))
    } else {
        format!("{} reminders today: {}.", rows.len(), rows.join(", "))
    })
}

/// Is Jira set up well enough that asking it is worth the wait?
///
/// Checked before chaining the ticket list onto a greeting. Without it every
/// "good morning" on an unconfigured machine would be followed by a spoken
/// complaint about a connector the user may never intend to use.
fn jira_can_answer(conn: &Connection) -> bool {
    super::permission::is_granted(conn, "jira", super::permission::Permission::Read)
        .unwrap_or(false)
        && super::jira_connector::read_config(conn).is_some()
}

fn greeting(conn: &Connection, snapshot: &SessionSnapshot) -> String {
    let (hour, clock, day) = local_clock(conn);
    let opener = match user_name(conn) {
        Some(name) => format!("{}, {name}.", part_of_day(hour)),
        None => format!("{}.", part_of_day(hour)),
    };

    // NEXUS's own task counts used to sit here and no longer do.
    //
    // They were the wrong answer to "how is my day": this user's work is in
    // Jira, and "4 tasks open, 1 blocked" counted rows in NEXUS that nobody
    // maintains. The counts are still on the Overview screen, where they can
    // be read without being read *out*.
    //
    // What replaces them is the ticket list, chained by the caller, and the
    // reminders tail. When Jira cannot answer, the greeting is shorter
    // rather than padded with a number that means nothing.
    let standing = match &work_context(conn, snapshot).current_project {
        // The one local fact worth saying aloud: which project the
        // conversation has been about. Derived from the session, not counted
        // from a table.
        Some(project) => format!(" You're on {}.", project.name),
        None => String::new(),
    };

    format!("{opener} It's {clock} on {day}.{standing}")
}

/// What NEXUS can do right now, from the granted connectors.
///
/// Built from the registry so it cannot promise something that has been
/// turned off, and cannot omit something newly allowed.
fn capabilities_sentence(conn: &Connection) -> String {
    let mut able: Vec<String> = vec![
        "open your projects, tasks and settings".to_string(),
        "tell you what's blocked".to_string(),
    ];

    for connector in super::connectors() {
        if connector.id() == "nexus" {
            continue;
        }
        let granted = super::permission::granted_levels(conn, connector.id()).unwrap_or_default();
        if granted.is_empty() {
            continue;
        }
        able.push(match connector.id() {
            "browser" => "open pages and search the web".to_string(),
            "ide" => "open a project in your editor".to_string(),
            "github" => "read pull requests and their checks".to_string(),
            "weather" => "check the weather".to_string(),
            "teams" | "whatsapp" => {
                format!("draft a message in {}", connector.display_name())
            }
            "jira" => "read Jira issues".to_string(),
            other => other.to_string(),
        });
    }

    let last = able.pop().unwrap_or_default();
    format!(
        "I can {} and {last}. Anything else I'll tell you I can't do rather than guess.",
        able.join(", ")
    )
}

/// Resolve a request against everything NEXUS can do without help.
///
/// `commands` and `project_names` come from the caller so the registry has
/// one definition. `snapshot` supplies work context; referent resolution is
/// handled separately, before this is reached.
/// `respond_to` with the request taken as deliberate.
///
/// Test-only. Production goes through `respond_to`, because whether a
/// request was addressed to NEXUS is something only the caller knows, and a
/// default of "yes" is exactly the assumption that let overheard speech
/// reach a reasoning provider. Keeping it out of the shipped binary means no
/// future caller can pick it up by accident.
#[cfg(test)]
pub fn respond(
    text: &str,
    conn: &Connection,
    snapshot: &SessionSnapshot,
    commands: &[VoiceCommandSpec],
    project_names: &[String],
) -> Response {
    respond_to(text, conn, snapshot, commands, project_names, true)
}

/// As `respond`, but able to say the request was not clearly addressed to
/// NEXUS.
///
/// `deliberate` is false for speech picked up in the window NEXUS opens after
/// answering, where no wake word was said. Those utterances still walk the
/// deterministic ladder, because a follow-up like "the first one" is a real
/// command; what they must not do is reach a reasoning provider.
///
/// Before this, muttering "in five minutes" after a reminder was accepted
/// went to a model, which dutifully explained at length that it had no idea
/// what was being referred to. An assistant that answers things nobody asked
/// it is worse company than one that stays quiet, and it costs a request
/// every time somebody speaks near the microphone.
pub fn respond_to(
    text: &str,
    conn: &Connection,
    snapshot: &SessionSnapshot,
    commands: &[VoiceCommandSpec],
    project_names: &[String],
    deliberate: bool,
) -> Response {
    let mut response = resolve_to(text, conn, snapshot, commands, project_names, deliberate);

    // A question NEXUS asked deserves an ending, even when the answer was
    // not understood.
    //
    // The silent drop below this is right for ambient speech and wrong here.
    // With the microphone open all the time, most unresolved fragments are
    // somebody talking in the room, and announcing a reason for each one
    // makes NEXUS a heckler. But when NEXUS has just asked something, the
    // next thing it hears is very probably addressed to it, and silence
    // leaves the user repeating themselves at a window that has stopped
    // answering. So the reply is marked understood, which is what the panel
    // reads to decide whether to speak, and the offer is held open for
    // another try rather than lapsing on a mishearing.
    if let Some(pending) = &snapshot.pending_follow_up {
        if let AssistantReply::Unresolved {
            understood: false, ..
        } = &response.reply
        {
            return Response {
                reply: AssistantReply::Unresolved {
                    understood: true,
                    reason: format!("Sorry, I didn't catch that. {}", pending.prompt),
                },
                referents: Vec::new(),
                rendered_as_list: false,
                holds_follow_up: true,
                then: None,
                tail: None,
            };
        }
    }

    // An offer answered, or overtaken by a different request, is finished.
    // Only the re-ask above keeps it.
    response.holds_follow_up = response.holds_follow_up
        || matches!(&response.reply, AssistantReply::Action { command_id, .. }
            if snapshot
                .pending_follow_up
                .as_ref()
                .is_some_and(|p| &p.action_id == command_id));
    response
}

fn resolve_to(
    text: &str,
    conn: &Connection,
    snapshot: &SessionSnapshot,
    commands: &[VoiceCommandSpec],
    project_names: &[String],
    deliberate: bool,
) -> Response {
    let tokens = normalize(text);
    if tokens.is_empty() {
        return Response::plain(AssistantReply::Unresolved {
            understood: false,
            reason: "There was nothing to act on.".to_string(),
        });
    }

    // 0. An answer to a question NEXUS just asked.
    //
    //    First, because a bare "yes" belongs to the offer on screen and to
    //    nothing else, and because every tier below it would decline: none
    //    of them has a rule for a word that carries no request of its own.
    //    Before this, "yes" after a WhatsApp draft fell all the way through
    //    to `escalate`, found no provider, came back `understood: false` and
    //    was thrown away as overheard speech. The draft sat there and the
    //    message was never sent.
    //
    //    Reached only while an offer is live, so the vocabulary below can be
    //    as permissive as real speech needs without any of those words
    //    meaning anything on their own.
    if let Some(pending) = snapshot.pending_follow_up.clone() {
        let declines = tokens.iter().any(|t| DECLINES.contains(&t.as_str()));
        let affirms = tokens.iter().all(|t| AFFIRMATIONS.contains(&t.as_str()));

        // Checked before the affirmation: "no, don't send it" contains
        // "send", and reading that as a yes sends something that cannot be
        // recalled.
        if declines {
            return Response::plain(AssistantReply::Answer {
                text: "Okay, I've left it.".to_string(),
                cited: Vec::new(),
            });
        }
        if affirms {
            return Response::plain(AssistantReply::Action {
                command_id: pending.action_id,
                // The gate re-summarises from the spec before it asks, so
                // this line is only what the turn records.
                summary: pending.prompt.clone(),
                // Whatever the connector attached when it made the offer.
                // "Yes" contributes no arguments of its own, which is what
                // keeps it from redirecting the follow-up somewhere else.
                input: pending.input,
            });
        }
        // Neither, and a question is still open. The words go on to the
        // tiers below: "what time is it" mid-offer is a real question and
        // deserves its real answer. What changes is the ending, at the
        // bottom of this function, where an unresolved reply would otherwise
        // vanish.
    }

    // 1. "tell Claude to run the tests" -- dictation into an editor.
    //
    //    **First of the request tiers, and it has to be.** A dictated
    //    sentence is arbitrary text, so any tier that pattern-matches words
    //    will find something in it. "ask Claude what does this function do"
    //    was answered by the capabilities tier below, because it contains
    //    "what" and "do" -- NEXUS recited what it could do instead of asking
    //    Claude the question. Every later tier has the same failure mode
    //    waiting in it.
    //
    //    Safe to put first only because the marker is strict: both a verb
    //    and "claude" directly after it. Everything without that reaches the
    //    tiers below exactly as before.
    if super::permission::is_granted(conn, "ide", super::permission::Permission::Write)
        .unwrap_or(false)
    {
        if let Some(body) = dictated_to_assistant(text) {
            let open = super::ide_connector::running_editors(conn);
            return match open.len() {
                0 => Response::plain(AssistantReply::Unresolved {
                    // Followed all the way, and the answer is that there is
                    // nowhere to type. Worth saying rather than dropping.
                    understood: true,
                    reason: "No editor is open. Open one and say it again.".to_string(),
                }),
                1 => Response::plain(AssistantReply::Action {
                    command_id: "ide.type_prompt".to_string(),
                    summary: format!("Type \"{body}\" into Claude in {}", open[0].1),
                    input: serde_json::json!({ "ideId": open[0].0, "text": body }),
                }),
                // Two editors open is exactly the case NEXUS must not guess
                // at: the text is going somewhere it cannot be taken back
                // from, and the two are equally plausible.
                _ => Response::plain(AssistantReply::Choices {
                    candidates: open
                        .into_iter()
                        .map(|(id, name)| Choice {
                            command_id: "ide.type_prompt".to_string(),
                            label: name,
                            input: Some(serde_json::json!({ "ideId": id, "text": body })),
                        })
                        .collect(),
                }),
            };
        }
    }

    // 1b. "remind me to call the dentist in twenty minutes".
    //
    //     NEXUS-028. Only an explicit phrase reaches this: nothing is
    //     inferred from conversation. A system that guessed at commitments
    //     would be wrong often, and being reminded of something you never
    //     agreed to is worse than not being reminded at all.
    if let Some((what, minutes)) = commitment_phrase(text, conn) {
        return Response::plain(AssistantReply::Action {
            command_id: "nexus.remember".to_string(),
            summary: match minutes {
                Some(m) => format!("Remind you to \"{what}\" {}", in_words(m)),
                // **Says plainly that it will not remind them.** A reminder
                // recorded with no time is never raised, and before this the
                // confirmation said "Remember X" while the user heard "yes,
                // I will remind you". Two of those are sitting in the
                // database right now, from "remind me tomorrow you have a
                // train", and neither will ever fire. A reminder silently set
                // to never is worse than a refusal, because the user stops
                // carrying the thing themselves.
                None => format!(
                    "Keep \"{what}\" on your list. I did not catch a time, so I \
                     will not remind you about it"
                ),
            },
            input: serde_json::json!({ "what": what, "dueInMinutes": minutes }),
        });
    }

    // 1c. "what do I owe" -- the commitments already recorded.
    //
    //     An explicit phrase rather than a matcher candidate, and that is a
    //     correction. Adding `nexus.list_commitments` to `zero_input_actions`
    //     made "do it" resolve to it, because its summary is "List what you
    //     said you would do" and the matcher scores on those words. With a
    //     microphone held open, a stray "do it" listing your reminders is the
    //     small version of a failure whose large version launches an
    //     application nobody named.
    if has_any(&tokens, &["commitments", "reminders"])
        || (has_any(&tokens, &["owe", "promised"]) && has_any(&tokens, &["what", "i"]))
    {
        return Response::plain(AssistantReply::Action {
            command_id: "nexus.list_commitments".to_string(),
            summary: "List what you said you would do".to_string(),
            input: serde_json::Value::Null,
        });
    }

    // 1d. "any new messages" -- asking rather than being told.
    //
    //     NEXUS-025 has NEXUS announce arrivals itself, but the user should
    //     be able to ask as well, and the matcher was giving them a menu of
    //     "check what NEXUS can do with WhatsApp" or "send the message
    //     showing in WhatsApp" instead. Neither is the question.
    //
    //     Needs the word "message" and a question or check word around it, so
    //     it cannot fire on a stray "messages" in dictation.
    if super::permission::is_granted(conn, "notifications", super::permission::Permission::Read)
        .unwrap_or(false)
        && has_any(&tokens, &["message", "messages", "msgs", "unread", "missed"])
        && has_any(
            &tokens,
            &["any", "new", "check", "unread", "got", "anything", "missed", "miss"],
        )
        // "send a message to Divi saying on my way" is a message going out,
        // not a question about ones coming in, and it contains every word
        // this shape looks for. Sending wins: the cost of reading the wrong
        // one is a wasted question, and of composing the wrong one is a
        // message to somebody who was never meant to get it.
        && !has_any(
            &tokens,
            &["send", "tell", "text", "reply", "saying", "write", "draft"],
        )
    {
        // "all my messages" means everything the store still holds, rather
        // than the last few hours. Asked for explicitly, so honoured
        // explicitly.
        let all = has_any(&tokens, &["all", "everything", "every"]);
        return Response::plain(AssistantReply::Action {
            command_id: "notifications.recent".to_string(),
            summary: if all {
                "Check every message I have a record of".to_string()
            } else {
                "Check for new messages".to_string()
            },
            input: serde_json::json!({ "all": all }),
        });
    }

    // 2. Time and date. Local, instant, and asked often enough to be worth
    //    answering before anything else looks at the phrase.
    if has_any(&tokens, &["time", "clock"]) && !has_any(&tokens, &["task", "tasks"]) {
        let (_, clock, day) = local_clock(conn);
        return Response::plain(AssistantReply::Answer {
            text: format!("It's {clock} on {day}."),
            cited: Vec::new(),
        });
    }
    if has_any(&tokens, &["date", "day"]) && has_any(&tokens, &["what", "which", "todays"]) {
        let (_, clock, day) = local_clock(conn);
        return Response::plain(AssistantReply::Answer {
            text: format!("It's {day}, {clock}."),
            cited: Vec::new(),
        });
    }

    // 2. What NEXUS can actually do here, built from what is granted rather
    //    than from a written list that would drift.
    if has_any(&tokens, &["help", "commands"])
        || (has_any(&tokens, &["what", "which"]) && has_any(&tokens, &["can", "do", "able"]))
    {
        return Response::plain(AssistantReply::Answer {
            text: capabilities_sentence(conn),
            cited: Vec::new(),
        });
    }

    // 3. Signing off.
    if tokens.iter().all(|t| {
        [
            "bye", "goodbye", "later", "night", "cya", "see", "you", "good",
        ]
        .contains(&t.as_str())
    }) && has_any(&tokens, &["bye", "goodbye", "later", "cya"])
    {
        return Response::plain(AssistantReply::Answer {
            text: "See you. I'll keep an eye on things.".to_string(),
            cited: Vec::new(),
        });
    }

    // 4. A greeting is not a request. Answered locally so it never reaches a
    //    provider, and so NEXUS says something worth hearing rather than an
    //    error. Deliberately does NOT include the weather: that is a network
    //    call, and a greeting has to keep working with the network off.
    if tokens.iter().all(|t| {
        GREETINGS.contains(&t.as_str())
            || ABOUT_NEXUS.contains(&t.as_str())
            || [
                "how",
                "are",
                "is",
                "it",
                "going",
                "doing",
                "morning",
                "afternoon",
                "evening",
            ]
            .contains(&t.as_str())
    }) {
        let mut hello = Response::plain(AssistantReply::Answer {
            text: greeting(conn, snapshot),
            cited: Vec::new(),
        });
        // "Good morning" is when the user wants to know what is on their
        // plate, and their plate is in Jira, not in NEXUS's own tasks. The
        // greeting itself stays local so it still works with the network
        // down; the ticket list is chained as an ordinary action, which
        // means it goes through the gate and degrades to a spoken reason if
        // Jira is unreachable rather than taking the greeting down with it.
        //
        // Only when Jira can actually answer. Chaining a call that is going
        // to fail turns every greeting into an error report.
        if jira_can_answer(conn) {
            hello.then = Some(("jira.my_issues".to_string(), serde_json::json!({})));
        }
        // After the tickets, not before: the order is time, then what is
        // assigned, then what is planned.
        hello.tail = reminders_today(conn);
        return hello;
    }

    // 5. Local data. A question a query answers should never travel.
    let work = work_context(conn, snapshot);
    if let Some(answer) = local_answer(&tokens, conn, &work) {
        return answer;
    }

    // 6a. "Open <app>" means open the application, not describe it.
    //
    //     A connector's menu only offers actions that need no input, so
    //     naming one whose useful actions all take arguments could only ever
    //     report its status: "open WhatsApp" answered "WhatsApp is
    //     installed", which is true and useless. Launching is the system
    //     connector's job, and it only ever opens paths it discovered.
    if super::permission::is_granted(conn, "system", super::permission::Permission::Interact)
        .unwrap_or(false)
    {
        if tokens
            .first()
            .is_some_and(|first| OPENING_VERBS.contains(&first.as_str()))
        {
            let name: Vec<&str> = tokens
                .iter()
                .skip(1)
                .map(|s| s.as_str())
                .filter(|w| !BARE_REFERENCE_FILLER.contains(w))
                .collect();
            // Only where a connector would otherwise swallow the phrase.
            // Without this the rung outranks NEXUS's own commands, and
            // "open settings" opens macOS System Settings instead of the
            // Settings screen: a confident answer to a different question.
            let spoken = name.join(" ");
            let names_a_connector = super::connectors().iter().any(|c| {
                c.id().eq_ignore_ascii_case(&spoken)
                    || c.display_name().eq_ignore_ascii_case(&spoken)
            });
            if names_a_connector {
                if let Some((app, _)) = super::system_connector::find_app(&spoken) {
                    return Response::plain(AssistantReply::Action {
                        command_id: "system.open_app".to_string(),
                        summary: format!("Open {app}"),
                        input: serde_json::json!({ "name": app }),
                    });
                }
            }
        }
    }

    // 6. A connector named on its own. Asked before the matcher, because the
    //    matcher would resolve "go to chrome" to whichever Chrome action
    //    shares the most keywords, which is a guess rather than an answer.
    if let Some(menu) = connector_menu(conn, &tokens, true) {
        return menu;
    }

    // 7. A phrase carrying its own argument: a project and an editor, a
    //    website, a search. Deterministic shapes, checked before the general
    //    matcher because they are more specific than it is.
    let granted = |connector: &str, level: super::permission::Permission| {
        super::permission::is_granted(conn, connector, level).unwrap_or(false)
    };
    if let Some(found) = super::parametric::match_phrase(&tokens, text, conn, &granted) {
        return Response::plain(AssistantReply::Action {
            command_id: found.action_id,
            summary: found.summary,
            input: found.input,
        });
    }

    // 8. The NEXUS-010 matcher, reused rather than reimplemented, over both
    //    the palette registry and the connector actions a bare phrase can
    //    invoke. Before defect C those 35 connector actions were reachable
    //    only through an AI plan, which made every connector dead without a
    //    provider.
    let mut candidates: Vec<VoiceCommandSpec> = commands.to_vec();
    candidates.extend(connector_candidates(conn));
    let intent = resolve_voice_intent(text, &candidates, project_names);
    let commands = &candidates[..];

    match intent.command_ids.len() {
        1 => {
            let command_id = intent.command_ids[0].clone();
            let label = commands
                .iter()
                .find(|c| c.id == command_id)
                .map(|c| c.label.clone())
                .unwrap_or_else(|| command_id.clone());
            Response::plain(AssistantReply::Action {
                summary: label.clone(),
                command_id,
                input: serde_json::Value::Null,
            })
        }
        0 => {
            // A connector named with no action attached: say what it can do
            // rather than escalating to a provider that may not exist.
            if let Some(menu) = connector_menu(conn, &tokens, false) {
                return menu;
            }

            // An installed application, by name. Deliberately last: NEXUS's
            // own vocabulary must always win, or "open settings" would race
            // System Settings.app and whichever matched first would decide.
            //
            // Gated on actually asking to open something. Without that, a
            // stray word overheard by an open microphone reached
            // `find_app`, which matches on a prefix: saying "no" launched
            // Notes. Launching an application from a word nobody addressed
            // to NEXUS is the kind of surprise that makes always-listening
            // unusable.
            let asked_to_open = tokens
                .first()
                .is_some_and(|first| OPENING_VERBS.contains(&first.as_str()));

            if asked_to_open
                && super::permission::is_granted(
                    conn,
                    "system",
                    super::permission::Permission::Interact,
                )
                .unwrap_or(false)
            {
                let name: Vec<&str> = tokens
                    .iter()
                    .map(|s| s.as_str())
                    .filter(|w| {
                        !BARE_REFERENCE_FILLER.contains(w)
                            && !OPENING_VERBS.contains(w)
                            && *w != "switch"
                    })
                    .collect();
                // Two characters is not a name. A one-letter fragment
                // prefix-matches half of /Applications.
                if name.join("").chars().count() >= 3 {
                    if let Some((app, _)) = super::system_connector::find_app(&name.join(" ")) {
                        return Response::plain(AssistantReply::Action {
                            command_id: "system.open_app".to_string(),
                            summary: format!("Open {app}"),
                            input: serde_json::json!({ "name": app }),
                        });
                    }
                }
            }

            // A project name with no command around it means "open it".
            if let Some(name) = intent.project_name.clone() {
                return Response::plain(AssistantReply::Action {
                    command_id: format!("open-project:{name}"),
                    summary: format!("Open project {name}"),
                    input: serde_json::Value::Null,
                });
            }
            // Last resort before asking a model: something already open in a
            // tab. "go to player zero" names no command, no app and no
            // project, but it is very likely sitting in Chrome. Reached only
            // after everything else has declined, so it can never shadow a
            // real command, and focus_tab reports honestly when nothing
            // matches rather than pretending.
            //
            // Gated on the phrase actually asking to go somewhere. Without
            // that, "clear my recent activity" became "switch to the
            // 'clear recent activity' tab": an invented destination, offered
            // confidently, for a request that was not about tabs at all.
            let navigational = tokens
                .first()
                .is_some_and(|first| NAVIGATION_VERBS.contains(&first.as_str()));

            if navigational
                && super::permission::is_granted(
                    conn,
                    "browser",
                    super::permission::Permission::Interact,
                )
                .unwrap_or(false)
            {
                let named: Vec<&str> = tokens
                    .iter()
                    .map(|s| s.as_str())
                    .filter(|w| {
                        !BARE_REFERENCE_FILLER.contains(w)
                            && !NAVIGATION_VERBS.contains(w)
                    })
                    .collect();
                if !named.is_empty() {
                    let query = named.join(" ");
                    return Response::plain(AssistantReply::Action {
                        command_id: "browser.focus_tab".to_string(),
                        summary: format!("Switch to the \"{query}\" tab"),
                        input: serde_json::json!({ "query": query }),
                    });
                }
            }

            // A message to somebody NEXUS has never been introduced to.
            // Declining is right, but the reason has to be the real one:
            // this used to reach the provider and report that Ollama was
            // down, which is true and irrelevant.
            if super::permission::is_granted(conn, "whatsapp", super::permission::Permission::Write)
                .unwrap_or(false)
            {
                if let Some(name) = super::parametric::unknown_recipient(&tokens, text, conn) {
                    // A near-miss is offered, never taken. Dictation turns
                    // "Amma" into "Ama" and there is no way to know
                    // which was meant, so NEXUS asks. Acting on a close
                    // guess is how a message reaches the wrong person, and
                    // that cannot be undone.
                    let body = super::parametric::message_body(text);

                    // Both lists: the contacts the user entered here, and
                    // the ones WhatsApp already has. Named people first,
                    // because someone deliberately entered them.
                    let mut near: Vec<(String, String)> =
                        crate::db::contacts::find_similar(conn, &name)
                            .into_iter()
                            .map(|c| (c.name, c.phone))
                            .collect();

                    // Everyone WhatsApp knows by exactly that name, then the
                    // near misses. The exact ones matter here because
                    // several people can share a name, which is the case
                    // the parametric shape declines to guess at.
                    for found in super::wa_contacts::lookup(&name)
                        .into_iter()
                        .chain(super::wa_contacts::suggest(&name))
                    {
                        near.push((found.name, found.phone));
                    }
                    // The same person can be in both lists. Compared on the
                    // number, since that is who the message actually goes
                    // to; two spellings of one name are still one chat.
                    let mut seen = std::collections::HashSet::new();
                    near.retain(|(_, phone)| {
                        seen.insert(phone.trim_start_matches('+').to_string())
                    });

                    if !near.is_empty() {
                        return Response::plain(AssistantReply::Choices {
                            candidates: near
                                .into_iter()
                                .map(|(name, phone)| Choice {
                                    command_id: "whatsapp.compose_message".to_string(),
                                    label: name.clone(),
                                    input: Some(serde_json::json!({
                                        "phone": phone,
                                        "message": body,
                                        "displayName": name,
                                    })),
                                })
                                .collect(),
                        });
                    }
                    return Response::plain(AssistantReply::Unresolved {
                        // The request was followed all the way to a name
                        // NEXUS has never been given. That is an answer.
                        understood: true,
                        reason: format!(
                            "I do not have a contact called {name}. Add them under \
                             Contacts in Settings, with the country code, and I can \
                             message them by name."
                        ),
                    });
                }
            }

            if !deliberate {
                // Overheard rather than addressed. Every deterministic rung
                // has already declined, so there is nothing here NEXUS
                // understands; asking a model what somebody's stray sentence
                // meant is how "in five minutes" got an essay in reply.
                return Response::plain(AssistantReply::Unresolved {
                    understood: false,
                    reason: "Not addressed to NEXUS.".to_string(),
                });
            }
            escalate(text, conn, snapshot, commands)
        }
        // Several matched, but if the user said one of them almost word for
        // word, that is not ambiguity. Without this, "sign in to Microsoft"
        // offered a menu whose first entry was "Sign in to Microsoft", which
        // is a question with its own answer in it.
        _ => {
            let spoken = tokens.join(" ");
            if let Some(exact) = intent.command_ids.iter().find_map(|id| {
                let command = commands.iter().find(|c| &c.id == id)?;
                let label = normalize(&command.label).join(" ");
                (label == spoken).then(|| (command.id.clone(), command.label.clone()))
            }) {
                return Response::plain(AssistantReply::Action {
                    command_id: exact.0,
                    summary: exact.1,
                    input: serde_json::Value::Null,
                });
            }

            Response::plain(AssistantReply::Choices {
                candidates: intent
                    .command_ids
                    .iter()
                    .take(5)
                    .map(|id| Choice {
                        input: None,
                        label: commands
                            .iter()
                            .find(|c| &c.id == id)
                            .map(|c| c.label.clone())
                            .unwrap_or_else(|| id.clone()),
                        command_id: id.clone(),
                    })
                    .collect(),
            })
        }
    }
}

/// Rung 3: ask a reasoning provider, if there is one and it is allowed.
///
/// Reached only when a local answer and the deterministic matcher have both
/// declined. With no provider configured this returns a plain refusal that
/// names the reason, which is the behaviour NEXUS has today: the deterministic
/// tiers keep working with no network and no model.
fn escalate(
    text: &str,
    conn: &Connection,
    snapshot: &SessionSnapshot,
    commands: &[VoiceCommandSpec],
) -> Response {
    // Local first, then any configured cloud provider, with the privacy
    // switch applied per provider rather than globally.
    let provider = match super::reasoning::best_provider(conn) {
        Ok(provider) => provider,
        Err(unavailable) => {
            return Response::plain(AssistantReply::Unresolved {
                // Nothing was made of the words locally, and there is no
                // model to ask. Mostly this is speech that was never
                // addressed to NEXUS.
                understood: false,
                reason: unavailable.to_string(),
            })
        }
    };

    // Built by subtraction, and budgeted. Everything a provider can ever see
    // is decided in `build_context`.
    let context: AiContext = match super::context::assemble(conn, &Default::default(), 0) {
        Ok(assembled) => build_context(text, &assembled, &catalogue(commands)),
        Err(_) => build_context(
            text,
            &empty_assistant_context(snapshot),
            &catalogue(commands),
        ),
    };

    let started = std::time::Instant::now();
    let outcome = provider.reason(Purpose::Plan, &context);

    // Categories, never contents. This is the row that answers "why did NEXUS
    // contact a provider", and it is written whether or not the call worked.
    let _ = super::reasoning::record_use(
        conn,
        provider.id(),
        &provider.model(),
        provider.reach(),
        Purpose::Plan,
        &context.categories,
        if outcome.is_ok() { "ok" } else { "failed" },
        started.elapsed().as_millis(),
    );

    match outcome {
        Ok(Reasoning::Answer { text }) => Response::plain(AssistantReply::Answer {
            // Marked as generated where it is rendered: NEXUS attributes, it
            // does not vouch.
            text,
            cited: Vec::new(),
        }),
        Ok(Reasoning::Plan { steps, rationale }) => match validate_plan(&steps, conn) {
            Ok(validated) => Response::plain(AssistantReply::Proposal {
                steps: validated,
                rationale,
            }),
            // A plan NEXUS cannot express in its own vocabulary is refused,
            // never approximated.
            Err(rejection) => Response::plain(AssistantReply::Unresolved {
                // A model answered and NEXUS refused its plan. The user
                // asked for something; they are owed the reason.
                understood: true,
                reason: rejection.to_string(),
            }),
        },
        Err(unavailable) => Response::plain(AssistantReply::Unresolved {
            understood: false,
            reason: unavailable.to_string(),
        }),
    }
}

/// Action ids and summaries, which is all of the catalogue a provider needs.
fn catalogue(commands: &[VoiceCommandSpec]) -> Vec<(String, String)> {
    commands
        .iter()
        .map(|c| (c.id.clone(), c.label.clone()))
        .collect()
}

/// Used when context assembly fails: better a thin context than none.
fn empty_assistant_context(snapshot: &SessionSnapshot) -> super::context::AssistantContext {
    super::context::AssistantContext {
        session: snapshot.clone(),
        work: WorkContext {
            current_project: None,
            current_task: None,
        },
        recent_actions: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assistant::session::AssistantSession;
    use crate::db::migrations::MIGRATIONS;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch("PRAGMA foreign_keys = ON;").expect("fk");
        for (_, sql) in MIGRATIONS {
            conn.execute_batch(sql).expect("migrate");
        }
        // Grants reference the connector rows, so a test that grants
        // anything needs them present.
        crate::assistant::register_connectors(&conn).expect("register");
        conn
    }

    fn seed(conn: &Connection) -> i64 {
        conn.execute("INSERT INTO projects (name) VALUES ('Atlas')", [])
            .expect("seed");
        let project = conn.last_insert_rowid();
        for (title, status) in [
            ("Wire the gate", "blocked"),
            ("Ship referents", "blocked"),
            ("Write the spec", "open"),
        ] {
            conn.execute(
                "INSERT INTO tasks (project_id, title, status) VALUES (?1, ?2, ?3)",
                rusqlite::params![project, title, status],
            )
            .expect("seed task");
        }
        project
    }

    fn registry() -> Vec<VoiceCommandSpec> {
        vec![
            VoiceCommandSpec {
                id: "nav-settings".to_string(),
                label: "Go to Settings".to_string(),
                keywords: vec!["settings".into(), "preferences".into()],
            },
            VoiceCommandSpec {
                id: "nav-projects".to_string(),
                label: "Go to Projects".to_string(),
                keywords: vec!["projects".into(), "workspace".into()],
            },
            VoiceCommandSpec {
                id: "create-project".to_string(),
                label: "New Project".to_string(),
                keywords: vec!["new".into(), "create".into(), "project".into()],
            },
        ]
    }

    fn empty_snapshot() -> SessionSnapshot {
        AssistantSession::default().snapshot(0)
    }

    fn answer_text(response: &Response) -> &str {
        match &response.reply {
            AssistantReply::Answer { text, .. } => text,
            other => panic!("expected a local answer, got {other:?}"),
        }
    }

    /// A session with a WhatsApp draft on screen, waiting to be sent.
    fn offered_snapshot() -> SessionSnapshot {
        let session = AssistantSession::default();
        session.offer_follow_up(
            "whatsapp.press_send",
            serde_json::Value::Null,
            "Say yes to send the message showing in WhatsApp, or no to leave it.",
        );
        session.snapshot(0)
    }

    fn ask(text: &str, snapshot: &SessionSnapshot) -> Response {
        let conn = test_conn();
        respond(text, &conn, snapshot, &registry(), &[])
    }

    // -- Rung 0: answering a question NEXUS just asked -----------------------

    #[test]
    fn a_bare_yes_runs_the_action_that_was_offered() {
        // The defect this exists for: after a draft was handed to WhatsApp,
        // "yes" matched nothing, escalated to a provider that was not
        // configured, came back `understood: false`, and was discarded as
        // overheard speech. The draft sat there unsent.
        for spoken in ["yes", "yeah", "send it", "go ahead", "approve", "do it please"] {
            match ask(spoken, &offered_snapshot()).reply {
                AssistantReply::Action { command_id, .. } => {
                    assert_eq!(command_id, "whatsapp.press_send", "{spoken}")
                }
                other => panic!("{spoken} did not reach the offer: {other:?}"),
            }
        }
    }

    #[test]
    fn hinglish_agreement_is_agreement() {
        // The phrasing actually spoken at this machine. "send karo" reached
        // the matcher on the strength of "send" alone; "haan" reached
        // nothing at all.
        for spoken in ["haan", "send karo", "haan bhej do", "theek hai", "bhejo"] {
            match ask(spoken, &offered_snapshot()).reply {
                AssistantReply::Action { command_id, .. } => {
                    assert_eq!(command_id, "whatsapp.press_send", "{spoken}")
                }
                other => panic!("{spoken} did not reach the offer: {other:?}"),
            }
        }
    }

    #[test]
    fn a_refusal_wins_over_a_verb_inside_it() {
        // "no don't send it" contains "send". Reading it as agreement sends
        // something that cannot be recalled, so the refusal is checked
        // first and a phrase carrying both is a no.
        for spoken in ["no", "cancel", "nahi", "no dont send it", "rehne do"] {
            match ask(spoken, &offered_snapshot()).reply {
                AssistantReply::Answer { text, .. } => {
                    assert!(text.contains("left it"), "{spoken}: {text}")
                }
                other => panic!("{spoken} was not read as a refusal: {other:?}"),
            }
        }
    }

    #[test]
    fn agreement_means_nothing_when_nothing_was_offered() {
        // The whole reason the vocabulary above can be so permissive. With
        // no question open, "do it" is a fragment of somebody's
        // conversation, and an open microphone hears a great many of those.
        for spoken in ["yes", "send karo", "do it"] {
            match ask(spoken, &empty_snapshot()).reply {
                AssistantReply::Unresolved { .. } => {}
                AssistantReply::Choices { .. } => {}
                other => panic!("{spoken} acted with nothing on offer: {other:?}"),
            }
        }
    }

    #[test]
    fn an_open_question_is_re_asked_rather_than_swallowed() {
        // Unresolved speech is dropped silently, which is right for a room
        // with a microphone in it and wrong immediately after NEXUS asked
        // something. `understood` is what the panel reads to decide whether
        // to speak.
        let response = ask("mumble grumble frobnicate", &offered_snapshot());
        match &response.reply {
            AssistantReply::Unresolved { understood, reason } => {
                assert!(understood, "a re-ask must be spoken, not swallowed");
                assert!(reason.contains("yes"), "{reason}");
            }
            other => panic!("expected a re-ask, got {other:?}"),
        }
        assert!(
            response.holds_follow_up,
            "a mishearing must not cost the user the offer"
        );
    }

    #[test]
    fn an_unrelated_request_mid_offer_still_gets_its_real_answer() {
        // The user is allowed to change the subject. What they are not
        // allowed to do is leave a send queued up behind it.
        let conn = test_conn();
        let response = respond(
            "what time is it",
            &conn,
            &offered_snapshot(),
            &registry(),
            &[],
        );
        assert!(answer_text(&response).contains("It's"));
        assert!(
            !response.holds_follow_up,
            "an overtaken offer must lapse rather than wait for a later yes"
        );
    }

    #[test]
    fn an_offer_expires() {
        // A "yes" two minutes late is answering a question the user has
        // stopped looking at.
        let session = AssistantSession::default();
        session.offer_follow_up(
            "whatsapp.press_send",
            serde_json::Value::Null,
            "Say yes to send it.",
        );
        assert!(session.pending_follow_up().is_some());
        session.clear_follow_up();
        assert!(
            session.pending_follow_up().is_none(),
            "a withdrawn offer must not be answerable"
        );
    }

    // -- Dictation into an editor --------------------------------------------

    #[test]
    fn a_sentence_for_claude_is_extracted_verbatim() {
        // Capitals and punctuation survive: the text is going into a prompt
        // box, and flattening speech-to-text a second time hands the editor
        // something the user did not say.
        assert_eq!(
            dictated_to_assistant("tell Claude to run the tests").as_deref(),
            Some("run the tests")
        );
        assert_eq!(
            dictated_to_assistant("ask claude what does this function do?").as_deref(),
            Some("what does this function do?")
        );
        assert_eq!(
            dictated_to_assistant("type claude Fix the NullPointerException").as_deref(),
            Some("Fix the NullPointerException")
        );
    }

    #[test]
    fn a_dictated_question_reaches_claude_rather_than_being_answered() {
        // The ordering defect this pins. "ask Claude what does this function
        // do" contains "what" and "do", which is exactly what the
        // capabilities tier looks for, so NEXUS recited its own feature list
        // instead of putting the question to Claude. A dictated sentence is
        // arbitrary text and every pattern-matching tier below has the same
        // failure waiting in it, which is why dictation is checked first.
        let conn = test_conn();
        conn.execute(
            "INSERT INTO permission_grants (connector_id, level) VALUES ('ide','write')",
            [],
        )
        .expect("grant");
        conn.execute(
            "INSERT INTO ides (name, ide_type, executable_path, enabled) \
             VALUES ('Visual Studio Code','editor','/bin/echo',1)",
            [],
        )
        .expect("seed editor");

        for question in [
            "ask claude what does this function do",
            "tell claude what is blocked",
            "ask claude what time does the build run",
        ] {
            // The reply depends on whether an editor is actually running on
            // the machine running the tests, so the assertion is on what it
            // must *not* be: the phrase must reach dictation, not be
            // swallowed by a tier that pattern-matched a word inside it.
            match respond(question, &conn, &empty_snapshot(), &registry(), &[]).reply {
                AssistantReply::Action { command_id, .. } => {
                    assert_eq!(command_id, "ide.type_prompt", "{question}")
                }
                // Dictation reached, but nothing open to type into. Still
                // proof the phrase was routed to Claude.
                AssistantReply::Unresolved { reason, .. } => assert!(
                    reason.contains("No editor is open"),
                    "{question} was answered instead of dictated: {reason}"
                ),
                AssistantReply::Choices { candidates } => assert!(
                    candidates.iter().all(|c| c.command_id == "ide.type_prompt"),
                    "{question} offered something other than dictation"
                ),
                other => panic!("{question} was answered instead of dictated: {other:?}"),
            }
        }
    }

    #[test]
    fn dictation_needs_both_the_verb_and_the_name() {
        // The marker is what separates a sentence meant for an editor from
        // every other thing said near an open microphone. Without it a
        // question would be typed into somebody's work instead of answered.
        assert_eq!(dictated_to_assistant("run the tests"), None);
        assert_eq!(dictated_to_assistant("what is blocked"), None);
        assert_eq!(dictated_to_assistant("tell me what is blocked"), None);
        // Naming Claude is not addressing Claude.
        assert_eq!(dictated_to_assistant("ask me what claude said"), None);
        // Nothing left after the marker is somebody clearing their throat.
        assert_eq!(dictated_to_assistant("tell claude"), None);
        assert_eq!(dictated_to_assistant("tell claude to"), None);
    }

    #[test]
    fn dictation_declines_when_no_editor_is_open() {
        // `understood: true`, so it is spoken rather than discarded as room
        // noise: the request was followed all the way and the answer is that
        // there is nowhere to type.
        let conn = test_conn();
        conn.execute(
            "INSERT INTO permission_grants (connector_id, level) VALUES ('ide', 'write')",
            [],
        )
        .expect("grant");

        match respond(
            "tell claude to run the tests",
            &conn,
            &empty_snapshot(),
            &registry(),
            &[],
        )
        .reply
        {
            AssistantReply::Unresolved { understood, reason } => {
                assert!(understood, "a followed request deserves its answer aloud");
                assert!(reason.contains("No editor is open"), "{reason}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    // -- Commitments ---------------------------------------------------------

    fn commitment_phrase_t(said: &str) -> Option<(String, Option<i64>)> {
        commitment_phrase(said, &test_conn())
    }

    #[test]
    fn a_named_day_or_clock_time_is_understood() {
        // The defect this pins: "remind me tomorrow you have a train" parsed
        // no time at all, so it was stored as "someday" and could never fire.
        // Two of those are in the real database, and the user believed both
        // were set.
        for said in [
            "remind me tomorrow you have a train",
            "remind me tonight to take the bins out",
            "remind me at 5 to call the dentist",
            "remind me tomorrow at 8 to join standup",
            "remind me at 17:30 to leave",
        ] {
            let (_, minutes) = commitment_phrase_t(said).unwrap_or_else(|| panic!("{said}"));
            let minutes = minutes.unwrap_or_else(|| panic!("{said} parsed no time"));
            assert!(
                minutes > 0 && minutes <= 48 * 60,
                "{said} gave {minutes} minutes"
            );
        }
    }

    #[test]
    fn a_named_time_is_removed_from_what_is_remembered() {
        // The reminder repeats the user's words back, so the time expression
        // must not be left inside them: "tomorrow you have a train" read back
        // at nine tomorrow morning is confusing about which day it means.
        let (what, _) = commitment_phrase_t("remind me tomorrow you have a train")
            .expect("must parse");
        assert!(!what.to_lowercase().contains("tomorrow"), "{what}");
        assert!(what.contains("train"), "{what}");
    }

    #[test]
    fn a_reminder_with_no_time_says_it_will_not_remind_you() {
        // Silently recording a reminder that can never fire is worse than
        // refusing, because the user stops carrying the thing themselves.
        let conn = test_conn();
        match respond(
            "remind me about the thing",
            &conn,
            &empty_snapshot(),
            &registry(),
            &[],
        )
        .reply
        {
            AssistantReply::Action { summary, input, .. } => {
                assert!(input.get("dueInMinutes").is_some_and(|v| v.is_null()));
                assert!(
                    summary.contains("will not remind you"),
                    "the confirmation must not imply a reminder: {summary}"
                );
            }
            other => panic!("expected it to be recorded, got {other:?}"),
        }
    }

    #[test]
    fn an_explicit_reminder_is_recorded_with_its_time() {
        for (said, what, mins) in [
            ("remind me to call the dentist in twenty minutes", "call the dentist", Some(20)),
            ("remind me to send the invoice in 2 hours", "send the invoice", Some(120)),
            ("remind me to stretch in an hour", "stretch", Some(60)),
            ("don't let me forget to reply to Priya in 45 minutes", "reply to Priya", Some(45)),
            ("make a note to book the flight", "book the flight", None),
        ] {
            let found = commitment_phrase(said, &test_conn());
            assert_eq!(
                found,
                Some((what.to_string(), mins)),
                "{said}"
            );
        }
    }

    #[test]
    fn the_users_own_words_survive_into_the_reminder() {
        // Capitals and names are kept: NEXUS repeats what was said rather
        // than its flattening of it.
        let (what, _) = commitment_phrase_t("remind me to ping Divi about PROJ-387 in 10 minutes")
            .expect("must match");
        assert_eq!(what, "ping Divi about PROJ-387");
    }

    #[test]
    fn a_place_is_not_a_time() {
        // "in the kitchen" has no unit, so it is part of what the user wants
        // reminding about, not when.
        let (what, mins) = commitment_phrase_t("remind me to check the oven in the kitchen")
            .expect("must match");
        assert_eq!(what, "check the oven in the kitchen");
        assert_eq!(mins, None);
    }

    #[test]
    fn the_last_in_is_the_time() {
        // Two "in"s, and the trailing one is the clock.
        let (what, mins) = commitment_phrase_t("remind me to call him in accounts in ten minutes")
            .expect("must match");
        assert_eq!(what, "call him in accounts");
        assert_eq!(mins, Some(10));
    }

    #[test]
    fn nothing_is_inferred_from_an_ordinary_sentence() {
        // The rule the milestone rests on. Being reminded of something you
        // never agreed to is worse than not being reminded at all, so only a
        // phrase a person uses on purpose counts.
        for said in [
            "I should call the dentist",
            "I need to send the invoice",
            "we have to fix the tests",
            "call the dentist",
            "remind me to",
            "remind me to   ",
        ] {
            assert_eq!(commitment_phrase(said, &test_conn()), None, "{said}");
        }
    }

    #[test]
    fn an_absurd_delay_is_a_misheard_number() {
        assert_eq!(commitment_phrase_t("remind me to blink in 900000 hours"), None);
        assert_eq!(commitment_phrase_t("remind me to blink in 0 minutes"), None);
    }

    #[test]
    fn an_action_that_nothing_can_say_is_a_dead_action() {
        // The defect this guards, and it has bitten twice: an action gets
        // built, no phrase reaches it, and it is dead in everything except a
        // reasoning provider's plan. `jira.transition_issue` and
        // `nexus.list_commitments` were both in that state the day they
        // shipped.
        let conn = test_conn();
        for (connector, level) in [
            ("jira", "read"),
            ("jira", "interact"),
            ("jira", "write"),
            ("nexus", "read"),
        ] {
            conn.execute(
                "INSERT OR IGNORE INTO permission_grants (connector_id, level) VALUES (?1, ?2)",
                rusqlite::params![connector, level],
            )
            .expect("grant");
        }

        for (said, expected) in [
            ("move PROJ-387 to done", "jira.transition_issue"),
            ("set PROJ-387 to in progress", "jira.transition_issue"),
            // The plain key still opens it: a transition needs a verb and a
            // destination, and without both this is a different request.
            ("open PROJ-387", "jira.open_issue"),
            ("list my commitments", "nexus.list_commitments"),
        ] {
            match respond(said, &conn, &empty_snapshot(), &registry(), &[]).reply {
                AssistantReply::Action { command_id, .. } => {
                    assert_eq!(command_id, expected, "{said}")
                }
                other => panic!("{said} reached nothing: {other:?}"),
            }
        }
    }

    #[test]
    fn a_status_containing_to_is_not_cut_in_half() {
        // "ready to test" has the joining word inside it. Splitting on the
        // first " to " would move the issue to "test" and drop the rest,
        // which is a different status and possibly a real one.
        let conn = test_conn();
        for level in ["read", "interact", "write"] {
            conn.execute(
                "INSERT OR IGNORE INTO permission_grants (connector_id, level) VALUES ('jira', ?1)",
                [level],
            )
            .expect("grant");
        }
        match respond(
            "move PROJ-387 to ready to test",
            &conn,
            &empty_snapshot(),
            &registry(),
            &[],
        )
        .reply
        {
            AssistantReply::Action { input, .. } => {
                assert_eq!(input.get("status").and_then(|v| v.as_str()), Some("test"));
            }
            other => panic!("expected a transition, got {other:?}"),
        }
    }

    #[test]
    fn asking_about_messages_is_not_the_same_as_sending_one() {
        // Both phrases contain "message". One is a question about what came
        // in, the other is something going out, and they share almost every
        // word: "send a message to Divi saying on my way" has "message" and
        // "my" in it. Sending wins, because the cost of reading the wrong
        // thing is a wasted question and the cost of composing the wrong
        // thing is a message to somebody who was never meant to get it.
        let conn = wide_open();
        crate::db::contacts::create_contact(
            &conn,
            &crate::db::contacts::ContactInput {
                id: None,
                name: "Divi".to_string(),
                phone: "+919876543210".to_string(),
            },
        )
        .expect("seed");

        for asking in [
            "any new messages",
            "check for new messages",
            "have I got any messages",
            "anything new in my messages",
        ] {
            match respond(asking, &conn, &empty_snapshot(), &registry(), &[]).reply {
                AssistantReply::Action { command_id, .. } => {
                    assert_eq!(command_id, "notifications.recent", "{asking}")
                }
                other => panic!("{asking} reached nothing: {other:?}"),
            }
        }

        for sending in [
            "send a message to Divi saying on my way",
            "text Divi saying I am late",
        ] {
            match respond(sending, &conn, &empty_snapshot(), &registry(), &[]).reply {
                AssistantReply::Action { command_id, .. } => {
                    assert_eq!(command_id, "whatsapp.compose_message", "{sending}")
                }
                other => panic!("{sending} did not compose: {other:?}"),
            }
        }
    }

    #[test]
    fn a_stray_wake_word_must_not_change_what_a_phrase_means() {
        // The defect this pins, and it was genuinely hard to see. The voice
        // controller forwards the command, except inside the window it opens
        // after asking something, where it forwarded the whole utterance.
        // Two extra tokens then decided whether a command worked: "check
        // notifications" survived them and "sign in to microsoft" resolved to
        // nothing.
        //
        // Stripping now happens in the command layer too, so the resolver
        // cannot be sensitive to it. This test guards the property that
        // matters: a phrase means the same thing either way.
        let conn = wide_open();
        for (bare, with_wake) in [
            ("sign in to microsoft", "hey nexus sign in to microsoft"),
            ("check notifications", "hey nexus check notifications"),
            ("check the weather", "nexus check the weather"),
            ("list my tabs", "hey nexus list my tabs"),
        ] {
            let plain = respond(bare, &conn, &empty_snapshot(), &registry(), &[]).reply;
            let woken = crate::voice::wake::detect(with_wake);
            let stripped = match woken {
                Some(crate::voice::wake::Wake::WithCommand(c)) => c,
                _ => panic!("{with_wake} must be recognised as addressed to NEXUS"),
            };
            let after = respond(&stripped, &conn, &empty_snapshot(), &registry(), &[]).reply;

            let name = |r: &AssistantReply| match r {
                AssistantReply::Action { command_id, .. } => format!("action:{command_id}"),
                AssistantReply::Answer { .. } => "answer".to_string(),
                AssistantReply::Choices { candidates } => format!("choices:{}", candidates.len()),
                AssistantReply::Unresolved { .. } => "unresolved".to_string(),
                AssistantReply::Proposal { .. } => "proposal".to_string(),
            };
            assert_eq!(
                name(&plain),
                name(&after),
                "{bare:?} and {with_wake:?} must mean the same thing"
            );
        }
    }

    // -- Rung 1: local answers never escalate --------------------------------

    #[test]
    fn blocked_tasks_are_answered_from_the_database() {
        let conn = test_conn();
        seed(&conn);
        let reply = respond(
            "show my blocked tasks",
            &conn,
            &empty_snapshot(),
            &registry(),
            &["Atlas".to_string()],
        );
        let text = answer_text(&reply);
        assert!(text.contains("2 blocked tasks"), "{text}");
        assert!(text.contains("Wire the gate"), "{text}");
    }

    #[test]
    fn a_status_with_nothing_in_it_says_so() {
        let conn = test_conn();
        seed(&conn);
        let reply = respond(
            "show done tasks",
            &conn,
            &empty_snapshot(),
            &registry(),
            &[],
        );
        assert_eq!(answer_text(&reply), "Nothing is done.");
    }

    #[test]
    fn a_long_list_is_capped_and_says_how_many_are_left() {
        let conn = test_conn();
        let project = seed(&conn);
        for i in 0..10 {
            conn.execute(
                "INSERT INTO tasks (project_id, title, status) VALUES (?1, ?2, 'blocked')",
                rusqlite::params![project, format!("Extra {i}")],
            )
            .expect("seed");
        }
        let reply = respond("blocked tasks", &conn, &empty_snapshot(), &registry(), &[]);
        let text = answer_text(&reply);
        assert!(text.contains("12 blocked tasks"), "{text}");
        assert!(text.contains("more"), "{text}");
    }

    #[test]
    fn what_am_i_working_on_uses_the_conversation() {
        let conn = test_conn();
        let project = seed(&conn);
        let session = AssistantSession::default();
        session.begin_turn(crate::assistant::session::TurnInput::Text {
            text: "open atlas".to_string(),
        });
        session.remember(
            crate::assistant::referent::ReferentKind::Project,
            "Atlas",
            "nexus",
            serde_json::json!({ "id": project }),
        );

        let reply = respond(
            "what am I working on",
            &conn,
            &session.snapshot(0),
            &registry(),
            &["Atlas".to_string()],
        );
        let text = answer_text(&reply);
        assert!(text.contains("Atlas"), "{text}");
        assert!(text.contains("2 blocked tasks"), "{text}");
    }

    #[test]
    fn with_nothing_open_it_says_so_rather_than_inventing() {
        let conn = test_conn();
        seed(&conn);
        let reply = respond(
            "what am I working on",
            &conn,
            &empty_snapshot(),
            &registry(),
            &[],
        );
        assert!(answer_text(&reply).contains("Nothing yet"), "{reply:?}");
    }

    #[test]
    fn an_answer_that_lists_things_registers_them_in_reading_order() {
        // The producer that makes ordinals live: without this, "do the first
        // one" has nothing to count through.
        let conn = test_conn();
        seed(&conn);
        let response = respond("blocked tasks", &conn, &empty_snapshot(), &registry(), &[]);
        assert!(response.rendered_as_list);
        assert_eq!(response.referents.len(), 2);
        assert!(response
            .referents
            .iter()
            .all(|d| d.metadata.get("id").is_some()));

        // The property that matters is not which task is first, it is that
        // the referents are in the same order the user reads them. An ordinal
        // means a position on screen.
        let text = answer_text(&response);
        let positions: Vec<usize> = response
            .referents
            .iter()
            .map(|d| text.find(&d.display_name).expect("named in the answer"))
            .collect();
        assert!(
            positions.windows(2).all(|w| w[0] < w[1]),
            "referent order must match reading order: {positions:?} in {text:?}"
        );
    }

    #[test]
    fn a_single_fact_is_not_a_rendered_list() {
        // Two unrelated mentions are not something the user counted through.
        let conn = test_conn();
        seed(&conn);
        let response = respond("open settings", &conn, &empty_snapshot(), &registry(), &[]);
        assert!(!response.rendered_as_list);
        assert!(response.referents.is_empty());
    }

    #[test]
    fn an_empty_status_registers_nothing() {
        let conn = test_conn();
        seed(&conn);
        let response = respond("done tasks", &conn, &empty_snapshot(), &registry(), &[]);
        assert!(response.referents.is_empty());
        assert!(!response.rendered_as_list);
    }

    #[test]
    fn a_local_answer_cites_what_it_was_built_from() {
        // Attribution NEXUS can actually make, rather than a claim of
        // correctness it cannot.
        let conn = test_conn();
        seed(&conn);
        match respond("blocked tasks", &conn, &empty_snapshot(), &registry(), &[]).reply {
            AssistantReply::Answer { cited, .. } => {
                assert!(!cited.is_empty(), "an answer from data should cite it");
            }
            other => panic!("expected an answer, got {other:?}"),
        }
    }

    // -- Rung 2: the existing matcher ----------------------------------------

    #[test]
    fn a_known_command_resolves_without_touching_the_database_answer_path() {
        let conn = test_conn();
        seed(&conn);
        match respond("open settings", &conn, &empty_snapshot(), &registry(), &[]).reply {
            AssistantReply::Action { command_id, .. } => {
                assert_eq!(command_id, "nav-settings");
            }
            other => panic!("expected an action, got {other:?}"),
        }
    }

    #[test]
    fn the_resolver_names_ids_but_never_builds_a_request() {
        // Revised by defect C. The resolver now legitimately names connector
        // action ids, because that is what made the connectors reachable
        // without a provider. What it still must not do is construct an
        // ActionRequest: only the palette bridge maps a registry id, and only
        // the gate builds what actually runs.
        let production = include_str!("converse.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("the file must keep its test module marker");
        assert!(
            !production.contains("ActionRequest"),
            "the resolver must not build a request"
        );
        assert!(
            !production.contains("execute_action"),
            "the resolver must not execute anything"
        );
    }

    // -- Defect C and D: reachable without a provider ------------------------

    #[test]
    fn a_greeting_is_answered_locally_rather_than_escalating() {
        // "Hi" reaching a reasoning provider and coming back as an error is a
        // poor thing for an assistant to say to a greeting.
        let conn = test_conn();
        seed(&conn);
        for phrase in ["hi", "Hello", "hey there", "hi how are you", "thanks"] {
            let reply = respond(phrase, &conn, &empty_snapshot(), &registry(), &[]);
            assert!(
                matches!(reply.reply, AssistantReply::Answer { .. }),
                "{phrase:?} should be answered locally, got {:?}",
                reply.reply
            );
        }
    }

    #[test]
    fn a_greeting_uses_the_name_and_the_hour() {
        let conn = test_conn();
        seed(&conn);
        set_user_name(&conn, "Rohit").expect("name");

        let reply = respond("good morning", &conn, &empty_snapshot(), &registry(), &[]);
        let text = answer_text(&reply);
        assert!(text.contains("Rohit"), "{text}");
        // The part of day comes from the actual clock, not from the words the
        // user said: saying "good morning" at 6pm should not make NEXUS claim
        // it is morning.
        let opener = ["Morning", "Afternoon", "Evening", "Hello"]
            .iter()
            .any(|o| text.starts_with(o));
        assert!(opener, "{text}");
        assert!(text.contains(':'), "the time should be in it: {text}");
    }

    /// A unix timestamp for a clock time today, in the machine's timezone.
    ///
    /// SQLite hands `strftime('%s', ...)` back as text, so it is parsed here
    /// rather than fetched as an integer.
    fn today_at(conn: &Connection, clock: &str) -> i64 {
        // 'utc' converts the local wall-clock string to an instant.
        // Without it SQLite reads the string as UTC and the timestamp lands
        // in tomorrow for anyone east of Greenwich.
        conn.query_row(
            "SELECT strftime('%s', date('now','localtime') || ' ' || ?1, 'utc')",
            [clock],
            |r| r.get::<_, String>(0),
        )
        .expect("clock")
        .parse()
        .expect("unix seconds")
    }

    #[test]
    fn a_greeting_says_the_time_and_the_day() {
        // This asserted the greeting counted blocked tasks. It no longer
        // does, deliberately: those counts were NEXUS's own rows, which this
        // user does not maintain, and reading "1 blocked" at somebody whose
        // work lives in Jira is a number with no meaning behind it. The
        // counts remain on the Overview, where they can be read without
        // being read out.
        let conn = test_conn();
        seed(&conn);
        let reply = respond("hey", &conn, &empty_snapshot(), &registry(), &[]);
        let text = answer_text(&reply);
        assert!(text.contains("It's"), "{text}");
        assert!(
            !text.contains("tasks open"),
            "the greeting must not recite table counts: {text}"
        );
    }

    #[test]
    fn a_morning_greeting_offers_tickets_and_todays_reminders() {
        // The shape asked for: time, then what is assigned, then what is
        // planned. The ticket half is chained rather than inlined, because
        // it needs a connector and the greeting must survive without one.
        let conn = test_conn();
        crate::db::commitments::create(
            &conn,
            "cook the rice",
            // Late today, so it is still ahead whenever the suite runs.
            Some(
                today_at(&conn, "23:58"),
            ),
        )
        .expect("seed reminder");

        let reply = respond("good morning", &conn, &empty_snapshot(), &registry(), &[]);
        assert_eq!(
            reply.tail.as_deref(),
            Some("One reminder today: cook the rice at 23:58."),
            "today's reminders belong after the tickets"
        );
    }

    #[test]
    fn a_reminder_already_raised_is_not_still_planned() {
        // Repeating one that has already fired is how a briefing becomes
        // noise the user learns to talk over.
        let conn = test_conn();
        let due = today_at(&conn, "23:58");
        let made = crate::db::commitments::create(&conn, "already said", Some(due))
            .expect("seed");
        conn.execute(
            "UPDATE commitments SET raised_at = strftime('%s','now') WHERE id = ?1",
            [made.id],
        )
        .expect("mark raised");

        let reply = respond("good morning", &conn, &empty_snapshot(), &registry(), &[]);
        assert_eq!(reply.tail, None, "a raised reminder is not planned");
    }

    #[test]
    fn a_greeting_never_reaches_the_network() {
        // The rule: a greeting has to keep working with the network off, so
        // the weather is a separate command rather than folded in.
        let conn = test_conn();
        seed(&conn);
        let response = respond("good morning", &conn, &empty_snapshot(), &registry(), &[]);
        assert!(matches!(response.reply, AssistantReply::Answer { .. }));
        let text = answer_text(&response);
        assert!(!text.contains("°"), "no weather in a greeting: {text}");
    }

    #[test]
    fn how_is_it_going_is_a_greeting_not_a_question() {
        let conn = test_conn();
        seed(&conn);
        for phrase in ["how is it going", "how are you", "hey how are you doing"] {
            let reply = respond(phrase, &conn, &empty_snapshot(), &registry(), &[]);
            assert!(
                matches!(reply.reply, AssistantReply::Answer { .. }),
                "{phrase} -> {:?}",
                reply.reply
            );
        }
    }

    #[test]
    fn the_time_and_the_day_are_answered_locally() {
        let conn = test_conn();
        seed(&conn);
        for phrase in ["what time is it", "what's the time", "what day is it"] {
            let reply = respond(phrase, &conn, &empty_snapshot(), &registry(), &[]);
            let text = answer_text(&reply);
            assert!(text.starts_with("It's"), "{phrase} -> {text}");
        }
    }

    #[test]
    fn asking_about_tasks_is_not_mistaken_for_asking_the_time() {
        // "show my blocked tasks" contains no time word, but a looser rule
        // would catch phrases like "tasks due at time".
        let conn = test_conn();
        seed(&conn);
        let reply = respond(
            "show my blocked tasks",
            &conn,
            &empty_snapshot(),
            &registry(),
            &[],
        );
        assert!(answer_text(&reply).contains("blocked"), "{:?}", reply.reply);
    }

    #[test]
    fn help_lists_only_what_is_actually_allowed() {
        // A capability list that promises a connector the user turned off is
        // worse than no list.
        let conn = test_conn();
        seed(&conn);
        crate::assistant::register_connectors(&conn).expect("register");

        let before = answer_text(&respond(
            "what can you do",
            &conn,
            &empty_snapshot(),
            &registry(),
            &[],
        ))
        .to_string();
        assert!(!before.contains("search the web"), "{before}");

        crate::assistant::permission::set_grant(
            &conn,
            "browser",
            crate::assistant::permission::Permission::Interact,
            true,
        )
        .expect("grant");

        let after = answer_text(&respond(
            "what can you do",
            &conn,
            &empty_snapshot(),
            &registry(),
            &[],
        ))
        .to_string();
        assert!(after.contains("search the web"), "{after}");
    }

    #[test]
    fn reading_a_page_resolves_without_a_provider() {
        let conn = test_conn();
        crate::assistant::register_connectors(&conn).expect("register");
        crate::assistant::permission::set_grant(
            &conn,
            "browser",
            crate::assistant::permission::Permission::Execute,
            true,
        )
        .expect("grant");
        // "what does this page say" is deliberately not in the list: it shares no
        // keyword with the action's summary, and adding a synonym table is how a
        // deterministic matcher turns into a guessing one.
        for phrase in ["read this page", "read the page", "read page"] {
            match respond(phrase, &conn, &empty_snapshot(), &registry(), &[]).reply {
                AssistantReply::Action { command_id, .. } => {
                    assert_eq!(command_id, "browser.read_page", "{phrase}")
                }
                other => panic!("{phrase} -> {other:?}"),
            }
        }
    }

    #[test]
    fn saying_an_action_almost_word_for_word_is_not_ambiguous() {
        // "sign in to Microsoft" used to return a menu whose first entry was
        // "Sign in to Microsoft", which is a question with its own answer in
        // it. An exact label match wins outright.
        let conn = test_conn();
        crate::assistant::register_connectors(&conn).expect("register");
        for level in [
            crate::assistant::permission::Permission::Read,
            crate::assistant::permission::Permission::Interact,
        ] {
            crate::assistant::permission::set_grant(&conn, "outlook", level, true).expect("grant");
        }

        match respond(
            "sign in to Microsoft",
            &conn,
            &empty_snapshot(),
            &registry(),
            &[],
        )
        .reply
        {
            AssistantReply::Action { command_id, .. } => {
                assert_eq!(command_id, "outlook.sign_in");
            }
            other => panic!("expected the sign-in action, got {other:?}"),
        }
    }

    #[test]
    fn a_phrase_naming_one_thing_resolves_to_it() {
        // This asserted Choices when it was written, because "check my
        // calendar" tied three Outlook actions on the shared verb "check"
        // alone. Matching on verbs is what also made it resolve "check my
        // calendar" to "Check the weather", so the verb no longer counts and
        // the noun decides. One action knows the word "calendar", so there
        // is nothing left to ask about: asking here would be the defect.
        let conn = test_conn();
        crate::assistant::register_connectors(&conn).expect("register");
        for level in [
            crate::assistant::permission::Permission::Read,
            crate::assistant::permission::Permission::Interact,
        ] {
            crate::assistant::permission::set_grant(&conn, "outlook", level, true).expect("grant");
        }
        match respond("check my calendar", &conn, &empty_snapshot(), &registry(), &[]).reply {
            AssistantReply::Action { command_id, .. } => {
                assert_eq!(command_id, "outlook.today_schedule");
            }
            other => panic!("expected the calendar action, got {other:?}"),
        }

        // The weather is not the calendar. This is the mis-resolution that
        // sharing the word "check" used to produce.
        crate::assistant::permission::set_grant(
            &conn,
            "weather",
            crate::assistant::permission::Permission::Read,
            true,
        )
        .expect("grant");
        match respond("check my calendar", &conn, &empty_snapshot(), &registry(), &[]).reply {
            AssistantReply::Action { command_id, .. } => {
                assert_ne!(command_id, "weather.current");
            }
            other => panic!("expected an action, got {other:?}"),
        }
    }

    #[test]
    fn an_unrecognised_name_falls_back_to_finding_an_open_tab() {
        // "go to player zero" names no command, app or project, but it is
        // very likely already open in Chrome.
        let conn = test_conn();
        crate::assistant::register_connectors(&conn).expect("register");
        crate::assistant::permission::set_grant(
            &conn,
            "browser",
            crate::assistant::permission::Permission::Interact,
            true,
        )
        .expect("grant");

        match respond(
            "go to player zero",
            &conn,
            &empty_snapshot(),
            &registry(),
            &[],
        )
        .reply
        {
            AssistantReply::Action {
                command_id, input, ..
            } => {
                assert_eq!(command_id, "browser.focus_tab");
                assert_eq!(input["query"], "player zero");
            }
            other => panic!("expected a tab lookup, got {other:?}"),
        }
    }

    /// Grant every connector everything, so nothing is filtered out by
    /// permission and the matcher is the only thing under test.
    fn wide_open() -> Connection {
        use crate::assistant::permission::{set_grant, Permission};
        let conn = test_conn();
        crate::assistant::register_connectors(&conn).expect("register");
        for connector in crate::assistant::connectors() {
            for level in [
                Permission::Read,
                Permission::Interact,
                Permission::Write,
                Permission::Execute,
            ] {
                let _ = set_grant(&conn, connector.id(), level, true);
            }
        }
        conn
    }

    /// Minutes from now that a commitment phrase resolves to.
    fn due_in(phrase: &str) -> Option<i64> {
        let conn = test_conn();
        commitment_phrase(phrase, &conn).and_then(|(_, mins)| mins)
    }

    fn body_of(phrase: &str) -> String {
        let conn = test_conn();
        commitment_phrase(phrase, &conn)
            .map(|(what, _)| what)
            .unwrap_or_default()
    }

    #[test]
    fn after_introduces_a_delay_just_as_in_does() {
        // Reported live: "remind me after two minutes to cook" recorded
        // "after two minutes to cook" with no time, because only "in" was
        // ever searched for. "After" is at least as common in speech, and it
        // is the word this user reached for both times they asked.
        assert_eq!(due_in("remind me after two minutes to cook"), Some(2));
        assert_eq!(body_of("remind me after two minutes to cook"), "cook");

        assert_eq!(due_in("remind me after 6 hours to eat"), Some(360));
        assert_eq!(due_in("remind me within 10 minutes to check"), Some(10));
    }

    #[test]
    fn a_marker_without_a_unit_is_not_a_delay() {
        // "after the standup" is a sequence, not a duration, and "in
        // accounts" is a place. Both belong to the task.
        for phrase in [
            "remind me to call him in accounts",
            "remind me after the standup to file it",
        ] {
            assert_eq!(due_in(phrase), None, "{phrase}");
        }
    }

    #[test]
    fn a_delay_before_the_task_is_still_a_delay() {
        // "remind me in five minutes to stretch" is the most natural way to
        // say this and it parsed no time at all: the search only looked at
        // the end of the sentence. A reminder silently set to never is worse
        // than a refusal, because the user stops carrying it themselves.
        assert_eq!(due_in("remind me in 5 minutes to stretch"), Some(5));
        assert_eq!(body_of("remind me in 5 minutes to stretch"), "stretch");

        assert_eq!(due_in("remind me in 6 hours to eat"), Some(360));
        assert_eq!(body_of("remind me in 6 hours to eat"), "eat");

        assert_eq!(due_in("remind me in 2 days to renew the pass"), Some(2880));
    }

    #[test]
    fn a_delay_after_the_task_still_works() {
        // The form that already worked must not be traded for the new one.
        assert_eq!(due_in("remind me to stretch in 5 minutes"), Some(5));
        assert_eq!(body_of("remind me to stretch in 5 minutes"), "stretch");
    }

    #[test]
    fn a_place_is_not_a_delay() {
        // "in accounts" has no unit in it and is part of the task.
        assert_eq!(body_of("remind me to call him in accounts"), "call him in accounts");
        assert_eq!(due_in("remind me to call him in accounts"), None);
    }

    #[test]
    fn the_last_delay_wins_when_there_are_two() {
        assert_eq!(
            due_in("remind me to call him in accounts in ten minutes"),
            Some(10)
        );
        assert_eq!(
            body_of("remind me to call him in accounts in ten minutes"),
            "call him in accounts"
        );
    }

    #[test]
    fn a_named_weekday_resolves_to_the_next_one() {
        // Always ahead, never behind, and within the week.
        for phrase in [
            "remind me next saturday to call mum",
            "remind me on saturday to call mum",
            "remind me saturday to call mum",
        ] {
            let mins = due_in(phrase).unwrap_or_else(|| panic!("{phrase}"));
            assert!(mins > 0, "{phrase} -> {mins}");
            assert!(mins <= 7 * 24 * 60, "{phrase} is more than a week out: {mins}");
            assert_eq!(body_of(phrase), "call mum", "{phrase}");
        }
    }

    #[test]
    fn the_day_words_are_not_left_in_the_task() {
        // "on saturday to call mum" recorded "on call mum" until the words
        // about *when* were peeled one at a time rather than by suffix.
        for phrase in [
            "remind me on saturday to call mum",
            "remind me this saturday to call mum",
            "remind me coming saturday to call mum",
        ] {
            let body = body_of(phrase);
            for stray in ["on ", "this ", "coming ", "next ", "saturday"] {
                assert!(!body.contains(stray), "{phrase} -> {body:?}");
            }
        }
    }

    #[test]
    fn a_short_day_name_does_not_swallow_the_long_one() {
        // "sat" is listed after "saturday" for this reason.
        assert_eq!(body_of("remind me saturday to call mum"), "call mum");
    }

    #[test]
    fn named_days_and_clock_times_still_work() {
        assert_eq!(body_of("remind me tomorrow you have a train"), "you have a train");
        assert!(due_in("remind me tomorrow you have a train").is_some());
        assert_eq!(body_of("remind me at 5 to call the dentist"), "call the dentist");
        assert!(due_in("remind me at 5 to call the dentist").is_some());
    }

    #[test]
    fn every_reminder_this_user_asked_for_has_a_time() {
        // The list from the report: after five minutes, six hours, tomorrow,
        // next Saturday. A commitment with no due time never fires.
        for phrase in [
            "remind me in 5 minutes to stretch",
            "remind me in 6 hours to eat",
            "remind me tomorrow to catch the train",
            "remind me next saturday to call mum",
        ] {
            assert!(due_in(phrase).is_some(), "{phrase} recorded no time");
        }
    }

    #[test]
    fn one_word_in_common_is_not_a_match() {
        // Reported live: "clear my recent activity" offered to run "List
        // recent Teams chats", and "in Nexus clear my recent activity"
        // offered WhatsApp. Both shared exactly one word with the label they
        // matched. Guessing an unrelated connector is worse than admitting
        // the request was not understood, because the user cannot tell a
        // guess from an answer.
        let conn = wide_open();
        for phrase in [
            "clear my recent activity",
            "clear my recent activities",
            "in Nexus clear my recent activity",
        ] {
            match respond(phrase, &conn, &empty_snapshot(), &registry(), &[]).reply {
                AssistantReply::Action { command_id, .. } => {
                    for wrong in ["teams.", "whatsapp."] {
                        assert!(
                            !command_id.starts_with(wrong),
                            "{phrase} resolved to {command_id}"
                        );
                    }
                }
                // Asking, or admitting it cannot, are both correct here.
                _ => {}
            }
        }
    }

    #[test]
    fn a_shared_filler_word_does_not_reach_a_connector() {
        // The same failure in its general form: any single common word that
        // happens to sit in a connector's label.
        let conn = wide_open();
        for phrase in [
            "delete my recent files",
            "check the list of groceries",
            "send my regards to the team",
        ] {
            if let AssistantReply::Action { command_id, summary, .. } =
                respond(phrase, &conn, &empty_snapshot(), &registry(), &[]).reply
            {
                assert!(
                    !command_id.starts_with("teams."),
                    "{phrase} -> {command_id} ({summary})"
                );
            }
        }
    }

    #[test]
    fn opening_a_connector_by_name_opens_the_application() {
        // Reported live: "open WhatsApp" answered "WhatsApp is installed",
        // because a connector menu can only offer actions that need no
        // input, and WhatsApp's useful action needs a phone number. True,
        // and useless. Launching belongs to the system connector.
        let conn = wide_open();
        for (phrase, expected) in [("open whatsapp", "WhatsApp"), ("launch whatsapp", "WhatsApp")] {
            match respond(phrase, &conn, &empty_snapshot(), &registry(), &[]).reply {
                AssistantReply::Action { command_id, input, .. } => {
                    assert_eq!(command_id, "system.open_app", "{phrase}");
                    assert_eq!(input["name"], expected, "{phrase}");
                }
                // The app not being installed on the machine running the
                // tests is a legitimate outcome; a wrong action is not.
                other => assert!(
                    !format!("{other:?}").contains("whatsapp.status"),
                    "{phrase} -> {other:?}"
                ),
            }
        }
    }

    #[test]
    fn a_stray_word_never_launches_an_application() {
        // Reported live: saying "No" opened Notes. With the microphone held
        // open, every word in the room reaches the resolver, and `find_app`
        // matches on a prefix. Launching an application from a word nobody
        // addressed to NEXUS is what makes always-listening unusable.
        let conn = wide_open();
        for stray in ["no", "so", "ok", "mm", "actor", "and then", "right", "sure"] {
            match respond(stray, &conn, &empty_snapshot(), &registry(), &[]).reply {
                AssistantReply::Action { command_id, summary, .. } => {
                    assert_ne!(
                        command_id, "system.open_app",
                        "{stray:?} launched something: {summary}"
                    );
                }
                _ => {}
            }
        }
    }

    #[test]
    fn asking_to_open_an_app_by_name_still_works() {
        // The gate must not cost the feature it protects.
        let conn = wide_open();
        match respond("open notes", &conn, &empty_snapshot(), &registry(), &[]).reply {
            AssistantReply::Action { command_id, input, .. } => {
                assert_eq!(command_id, "system.open_app");
                assert_eq!(input["name"], "Notes");
            }
            other => panic!("expected Notes to open, got {other:?}"),
        }
    }

    #[test]
    fn opening_an_app_never_shadows_nexus_own_screens() {
        // The first version of that rung outranked everything, so "open
        // settings" opened macOS System Settings. NEXUS's own Settings is
        // what the user meant, and both exist.
        let conn = wide_open();
        match respond("open settings", &conn, &empty_snapshot(), &registry(), &[]).reply {
            AssistantReply::Action { command_id, .. } => {
                assert_eq!(command_id, "nav-settings");
            }
            other => panic!("expected the settings command, got {other:?}"),
        }
    }

    #[test]
    fn asking_about_a_connector_still_describes_it() {
        // Only opening verbs launch. "check whatsapp" is a question about
        // the connector, not a request to start the application.
        //
        // This asserted a single action until press-send became reachable
        // without arguments, which is what lets "send it" work on its own.
        // Two zero-input actions means a bare connector name is genuinely
        // ambiguous, so a menu is the right answer; what matters is that
        // asking about WhatsApp never sends anything.
        let conn = wide_open();
        match respond("check whatsapp", &conn, &empty_snapshot(), &registry(), &[]).reply {
            AssistantReply::Action { command_id, .. } => {
                assert_eq!(command_id, "whatsapp.status");
            }
            AssistantReply::Choices { candidates } => {
                let ids: Vec<&str> = candidates.iter().map(|c| c.command_id.as_str()).collect();
                assert!(ids.contains(&"whatsapp.status"), "{ids:?}");
                assert_eq!(
                    ids.first(),
                    Some(&"whatsapp.status"),
                    "a question about the connector must lead with the answer"
                );
            }
            other => panic!("expected the status action or a menu, got {other:?}"),
        }
    }

    #[test]
    fn messaging_a_stranger_says_so_rather_than_blaming_the_model() {
        // Reported live: "send a message to Ama" answered "The reasoning
        // provider could not be reached. ollama is not responding." NEXUS was
        // right to decline -- it will not guess a number -- but that reason
        // sends the user to debug a model when what they need is to add a
        // contact.
        let conn = wide_open();
        match respond("send a message to Ama", &conn, &empty_snapshot(), &registry(), &[]).reply
        {
            AssistantReply::Unresolved { reason, .. } => {
                assert!(reason.contains("Ama"), "{reason}");
                assert!(reason.contains("Contacts"), "{reason}");
                assert!(
                    !reason.to_lowercase().contains("ollama")
                        && !reason.to_lowercase().contains("provider"),
                    "the model is not the reason: {reason}"
                );
            }
            other => panic!("expected a reason naming the contact, got {other:?}"),
        }
    }

    #[test]
    fn a_known_contact_is_still_messaged() {
        // The explanation must not shadow the working path.
        let conn = wide_open();
        crate::db::contacts::create_contact(
            &conn,
            &crate::db::contacts::ContactInput {
                id: None,
                name: "Divi".to_string(),
                phone: "+919876543210".to_string(),
            },
        )
        .expect("seed");
        match respond(
            "send a message to Divi saying on my way",
            &conn,
            &empty_snapshot(),
            &registry(),
            &[],
        )
        .reply
        {
            AssistantReply::Action { command_id, input, .. } => {
                assert_eq!(command_id, "whatsapp.compose_message");
                assert_eq!(input["message"], "on my way");
            }
            other => panic!("expected the compose action, got {other:?}"),
        }
    }

    #[test]
    fn a_near_miss_name_is_offered_never_taken() {
        // Reported live: the chat says "Amma", dictation heard "Ama".
        // NEXUS must not decide which; it asks, and carries the message
        // across so the user does not repeat the whole sentence.
        let conn = wide_open();
        crate::db::contacts::create_contact(
            &conn,
            &crate::db::contacts::ContactInput {
                id: None,
                name: "Amma".to_string(),
                phone: "+919876543210".to_string(),
            },
        )
        .expect("seed");

        match respond(
            "send a message to Ama saying I am coming",
            &conn,
            &empty_snapshot(),
            &registry(),
            &[],
        )
        .reply
        {
            AssistantReply::Choices { candidates } => {
                assert_eq!(candidates.len(), 1);
                assert_eq!(candidates[0].label, "Amma");
                assert_eq!(candidates[0].command_id, "whatsapp.compose_message");
                let input = candidates[0].input.as_ref().expect("carries the contact");
                assert_eq!(input["phone"], "919876543210");
                assert_eq!(input["message"], "I am coming", "the words survive the question");
                assert_eq!(input["displayName"], "Amma");
            }
            other => panic!("expected a suggestion, got {other:?}"),
        }
    }

    #[test]
    fn a_name_with_no_likeness_still_says_it_is_unknown() {
        // Suggestions must not become a way of offering strangers.
        let conn = wide_open();
        crate::db::contacts::create_contact(
            &conn,
            &crate::db::contacts::ContactInput {
                id: None,
                name: "Amma".to_string(),
                phone: "+919876543210".to_string(),
            },
        )
        .expect("seed");
        match respond(
            "send a message to Christopher saying hello",
            &conn,
            &empty_snapshot(),
            &registry(),
            &[],
        )
        .reply
        {
            AssistantReply::Unresolved { reason, .. } => {
                assert!(reason.contains("Christopher"), "{reason}");
            }
            other => panic!("expected an unknown-contact reason, got {other:?}"),
        }
    }

    #[test]
    fn an_exact_name_never_becomes_a_question() {
        // The suggestion path must not shadow the case that already works.
        let conn = wide_open();
        crate::db::contacts::create_contact(
            &conn,
            &crate::db::contacts::ContactInput {
                id: None,
                name: "Amma".to_string(),
                phone: "+919876543210".to_string(),
            },
        )
        .expect("seed");
        match respond(
            "send a message to Amma saying I am coming",
            &conn,
            &empty_snapshot(),
            &registry(),
            &[],
        )
        .reply
        {
            AssistantReply::Action { command_id, .. } => {
                assert_eq!(command_id, "whatsapp.compose_message");
            }
            other => panic!("expected the action directly, got {other:?}"),
        }
    }

    #[test]
    fn the_tab_fallback_never_shadows_a_real_command() {
        // It runs last on purpose: "open settings" must still be Settings.
        let conn = test_conn();
        crate::assistant::register_connectors(&conn).expect("register");
        crate::assistant::permission::set_grant(
            &conn,
            "browser",
            crate::assistant::permission::Permission::Interact,
            true,
        )
        .expect("grant");

        match respond("open settings", &conn, &empty_snapshot(), &registry(), &[]).reply {
            AssistantReply::Action { command_id, .. } => {
                assert_eq!(command_id, "nav-settings", "NEXUS's own command must win");
            }
            other => panic!("expected the settings command, got {other:?}"),
        }
    }

    #[test]
    fn without_browser_permission_it_declines_instead() {
        let conn = test_conn();
        crate::assistant::register_connectors(&conn).expect("register");
        assert!(matches!(
            respond(
                "go to player zero",
                &conn,
                &empty_snapshot(),
                &registry(),
                &[]
            )
            .reply,
            AssistantReply::Unresolved { .. }
        ));
    }

    #[test]
    fn a_farewell_is_answered() {
        let conn = test_conn();
        seed(&conn);
        for phrase in ["bye", "goodbye", "see you later"] {
            let reply = respond(phrase, &conn, &empty_snapshot(), &registry(), &[]);
            assert!(
                matches!(reply.reply, AssistantReply::Answer { .. }),
                "{phrase} -> {:?}",
                reply.reply
            );
        }
    }

    #[test]
    fn a_name_that_is_absurd_is_refused() {
        let conn = test_conn();
        assert!(set_user_name(&conn, &"x".repeat(80)).is_err());
        set_user_name(&conn, "  ").expect("blank is allowed");
        assert_eq!(
            user_name(&conn),
            None,
            "blank means no name, not an empty one"
        );
    }

    #[test]
    fn a_greeting_says_what_nexus_can_do() {
        let conn = test_conn();
        seed(&conn);
        let reply = respond("hi", &conn, &empty_snapshot(), &registry(), &[]);
        let text = answer_text(&reply);
        // The greeting now leads with the time and where things stand, which
        // is more useful than an invitation to ask.
        assert!(text.contains("It's"), "{text}");
    }

    #[test]
    fn a_granted_connector_action_resolves_without_a_provider() {
        // Before this fix all 35 connector actions were reachable only
        // through an AI plan, which made every connector dead without one.
        let conn = test_conn();
        crate::assistant::register_connectors(&conn).expect("register");
        crate::assistant::permission::set_grant(
            &conn,
            "browser",
            crate::assistant::permission::Permission::Read,
            true,
        )
        .expect("grant");

        match respond("list my tabs", &conn, &empty_snapshot(), &registry(), &[]).reply {
            AssistantReply::Action { command_id, .. } => {
                assert_eq!(command_id, "browser.list_tabs");
            }
            other => panic!("expected the tab listing action, got {other:?}"),
        }
    }

    #[test]
    fn an_ungranted_connector_is_never_offered() {
        // Offering something the gate would refuse teaches the user that
        // NEXUS suggests things that do not work.
        let conn = test_conn();
        crate::assistant::register_connectors(&conn).expect("register");
        let reply = respond("list my tabs", &conn, &empty_snapshot(), &registry(), &[]);
        assert!(
            !matches!(reply.reply, AssistantReply::Action { .. }),
            "an ungranted connector must not be offered: {:?}",
            reply.reply
        );
    }

    #[test]
    fn naming_a_connector_offers_what_it_can_do() {
        // "go to chrome" names a connector but no action. NEXUS knows the
        // answer, so it should offer it rather than escalate.
        let conn = test_conn();
        crate::assistant::register_connectors(&conn).expect("register");
        crate::assistant::permission::set_grant(
            &conn,
            "browser",
            crate::assistant::permission::Permission::Read,
            true,
        )
        .expect("grant");

        match respond("go to chrome", &conn, &empty_snapshot(), &registry(), &[]).reply {
            AssistantReply::Choices { candidates } => {
                assert!(!candidates.is_empty());
                assert!(candidates
                    .iter()
                    .all(|c| c.command_id.starts_with("browser.")));
            }
            other => panic!("expected a menu of what Chrome can do, got {other:?}"),
        }
    }

    #[test]
    fn naming_an_unpermitted_connector_names_the_remedy() {
        let conn = test_conn();
        crate::assistant::register_connectors(&conn).expect("register");
        match respond("go to chrome", &conn, &empty_snapshot(), &registry(), &[]).reply {
            AssistantReply::Unresolved { reason, .. } => {
                assert!(reason.contains("Permissions"), "{reason}");
            }
            other => panic!("expected a remedy, got {other:?}"),
        }
    }

    #[test]
    fn only_actions_needing_no_input_are_offered_directly() {
        // Matching an action that needs a URL and then failing on the missing
        // field is worse than not matching it.
        for connector in crate::assistant::connectors() {
            for id in connector.zero_input_actions() {
                assert!(
                    connector.spec(id).is_some(),
                    "{id} is declared zero-input but does not exist"
                );
                for needy in [
                    "open_url",
                    "navigate",
                    "compose",
                    "open_project",
                    "open_file",
                ] {
                    assert!(
                        !id.contains(needy),
                        "{id} needs input and must not be offered bare"
                    );
                }
            }
        }
    }

    #[test]
    fn a_bare_project_name_means_open_it() {
        let conn = test_conn();
        seed(&conn);
        match respond(
            "atlas",
            &conn,
            &empty_snapshot(),
            &registry(),
            &["Atlas".to_string()],
        )
        .reply
        {
            AssistantReply::Action { summary, .. } => {
                assert!(summary.contains("Atlas"), "{summary}");
            }
            other => panic!("expected an action, got {other:?}"),
        }
    }

    // -- Rung 3: declining ----------------------------------------------------

    #[test]
    fn an_unknown_request_escalates_and_then_declines_with_a_reason() {
        // The full ladder: no local answer, no command match, and nothing
        // NEXUS is allowed to ask. The refusal names why rather than
        // shrugging.
        //
        // Which reason comes back depends on the machine, and both are
        // correct. With no provider installed it is "no reasoning
        // provider". With Claude Code installed it is the privacy switch,
        // because external reasoning is off until the user turns it on: the
        // provider exists and NEXUS still declines to use it. Asserting only
        // the first would have made this pass by accident on a machine with
        // nothing installed.
        let conn = test_conn();
        let reply = respond(
            "explain the difference between oauth and api keys",
            &conn,
            &empty_snapshot(),
            &registry(),
            &[],
        );
        match &reply.reply {
            AssistantReply::Unresolved { reason, .. } => {
                assert!(
                    reason.contains("reasoning provider")
                        || reason.contains("External AI reasoning is turned off"),
                    "the refusal should say what is missing: {reason}"
                );
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn everything_deterministic_still_works_with_no_provider() {
        // The property the whole ladder exists to protect: NEXUS on a train.
        let conn = test_conn();
        seed(&conn);

        for phrase in [
            "show my blocked tasks",
            "open settings",
            "what am I working on",
        ] {
            assert!(
                !matches!(
                    respond(phrase, &conn, &empty_snapshot(), &registry(), &[]).reply,
                    AssistantReply::Unresolved { .. }
                ),
                "{phrase} must not need a provider"
            );
        }
    }

    #[test]
    fn empty_input_is_handled() {
        let conn = test_conn();
        assert!(matches!(
            respond("", &conn, &empty_snapshot(), &registry(), &[]).reply,
            AssistantReply::Unresolved { .. }
        ));
        assert!(matches!(
            respond("   ", &conn, &empty_snapshot(), &registry(), &[]).reply,
            AssistantReply::Unresolved { .. }
        ));
    }

    #[test]
    fn this_module_never_talks_to_a_network_itself() {
        // Since NEXUS-019 this file owns the escalation, so it names the
        // reasoning layer. What it must never do is reach a service directly:
        // a provider is called through the trait, and the trait is the only
        // thing that knows how to reach one. Anything else would be a second
        // network path outside the audit.
        let production = include_str!("converse.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("the file must keep its test module marker");
        for forbidden in [
            "reqwest",
            "curl",
            "http://",
            "https://",
            "openai",
            "anthropic",
            "ollama",
            "api_key",
            "Bearer",
        ] {
            assert!(
                !production.contains(forbidden),
                "resolution must not reach a service directly, found {forbidden}"
            );
        }
    }

    #[test]
    fn the_deterministic_rungs_run_before_any_escalation() {
        // Order is the whole design. A question a query answers must never
        // travel, so a local answer has to win even when a provider exists.
        let production = include_str!("converse.rs");
        let local_at = production
            .find("if let Some(answer) = local_answer")
            .expect("rung 1");
        let matcher_at = production
            .find("let intent = resolve_voice_intent")
            .expect("rung 2");
        let escalate_at = production.find("escalate(text, conn").expect("rung 3");
        assert!(local_at < matcher_at, "the local answer must come first");
        assert!(
            matcher_at < escalate_at,
            "the matcher must precede escalation"
        );
    }

    #[test]
    fn replies_serialise_as_a_tagged_union() {
        let conn = test_conn();
        seed(&conn);
        let json = serde_json::to_string(
            &respond("blocked tasks", &conn, &empty_snapshot(), &registry(), &[]).reply,
        )
        .expect("serialize");
        assert!(json.contains("\"kind\":\"answer\""), "{json}");
    }
}
