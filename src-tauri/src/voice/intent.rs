//! Deterministic voice transcript resolution (NEXUS-010, defect D-3).
//!
//! # Why this exists, separately from NEXUS-009
//!
//! NEXUS-009's `matchesQuery` requires the *entire* query to be a literal
//! substring of one field. That suits typing, where a user types a fragment
//! ("sett"), and fails for speech, where a user says a whole sentence in
//! arbitrary word order. Measured: "open settings", "open projects",
//! "create project" and all four Tanglish phrases matched zero commands.
//!
//! This module fixes that **for the voice path only**. NEXUS-009's matcher and
//! the keyboard/palette matching contract are untouched: nothing here is
//! called when a user types.
//!
//! # Why it stays deterministic
//!
//! - Fixed, explicit word lists. No model, no embeddings, no inference.
//! - Integer arithmetic only. No floats, no probabilities, no randomness.
//! - Total ordering: score descending, then command id ascending, so the same
//!   transcript always produces byte-identical output.
//! - No I/O and no network. A pure function of (transcript, registry, names).
//!
//! It never executes anything. It returns candidate ids; the palette shows
//! them and the user confirms, exactly as with the keyboard.
//!
//! # The registry is not duplicated
//!
//! The command list is passed in from `src/lib/commands.ts`, which remains the
//! single source of truth. This module holds no command vocabulary of its own.

use serde::{Deserialize, Serialize};

/// Harmless conversational and filler words, dropped before matching.
///
/// The Tamil entries are treated purely as noise words: they carry no meaning
/// here and are not translated. That is the whole point, and is why no NLP,
/// translation layer or Tamil dependency is involved.
const FILLER_WORDS: &[&str] = &[
    // English conversational filler
    "a",
    "an",
    "the",
    "to",
    "go",
    "please",
    "can",
    "could",
    "would",
    "you",
    "just",
    "now",
    "for",
    "me",
    "my",
    "hey",
    "ok",
    "okay",
    "um",
    "uh",
    "and",
    "then",
    "in",
    "on",
    "of",
    "it",
    "is",
    // Demonstratives. Pure deixis: "read this page" and "read the page" are
    // the same request, and treating "this" as a noun made the first one
    // match nothing at all.
    "this",
    "that",
    "these",
    "those",
    "here",
    "there",
    "let",
    "us",
    "lets",
    // UI nouns people append without meaning anything by them:
    // "open settings tab", "go to projects screen". Deliberately excludes
    // "list", which is a real keyword of Go to Projects.
    "tab",
    "screen",
    "page",
    "window",
    "section",
    "panel",
    "menu",
    "view",
    // Tanglish imperative/politeness endings, treated as noise only
    "pannunga",
    "pannu",
    "pannuga",
    "panniduga",
    "pannunga.",
    "seyyunga",
    "seyyu",
    "kaattu",
    "kaamunga",
    "venum",
    "irukku",
    "la",
    "da",
    "ne",
    "nga",
];

/// Words meaning "make a new one".
const CREATE_SIGNALS: &[&str] = &["new", "create", "add", "make", "start"];

/// Nouns that indicate the create target is a task rather than a project.
const TASK_WORDS: &[&str] = &["task", "todo", "ticket"];

/// A command as the frontend registry defines it. Passed in, never invented.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceCommandSpec {
    pub id: String,
    pub label: String,
    pub keywords: Vec<String>,
}

/// What the transcript resolved to. Candidates only: nothing is executed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VoiceIntent {
    /// Registry command ids, best first. Empty when nothing matched.
    pub command_ids: Vec<String>,
    /// Seeds the palette's existing search. Used to open a project by name,
    /// and as a fallback when no command matched.
    pub search_query: String,
    /// The transcript after normalisation, shown for transparency.
    pub normalized: String,
    /// The project name recognised in the transcript, if any.
    pub project_name: Option<String>,
    /// True when several candidates tied, so the user must choose.
    pub ambiguous: bool,
}

/// Minimum score to be offered at all.
const SCORE_THRESHOLD: i64 = 2;
/// An exact token match is stronger evidence than a singular/plural one.
/// Without this, "open projects" ties Go to Projects against New Project and
/// the id tiebreak picks the wrong command.
const EXACT_MATCH: i64 = 2;
const PLURAL_MATCH: i64 = 1;
/// A label hit is worth more than a keyword hit.
const LABEL_WEIGHT: i64 = 2;
const KEYWORD_WEIGHT: i64 = 1;
/// Never offer more than this many candidates.
const MAX_CANDIDATES: usize = 8;



/// Lowercase, strip punctuation, split on whitespace, drop empties.
/// Punctuation becomes a separator so "settings," and "settings" agree.
fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .map(|s| s.to_string())
        .collect()
}

fn is_filler(token: &str) -> bool {
    FILLER_WORDS.contains(&token)
}

/// Tokens that carry meaning: everything that is not filler.
fn significant(text: &str) -> Vec<String> {
    tokenize(text)
        .into_iter()
        .filter(|t| !is_filler(t))
        .collect()
}

/// Compare two tokens, treating a trailing "s" as the same word.
///
/// Speech produces "setting" where the registry says "settings", and exact
/// equality drops the match. This is a single explicit suffix rule, not a
/// stemmer and not a language model: deterministic and inspectable.
fn tokens_match(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (short, long) = if a.len() < b.len() { (a, b) } else { (b, a) };
    long.len() == short.len() + 1 && long.ends_with('s') && long.starts_with(short)
}

fn contains_any(tokens: &[String], words: &[&str]) -> bool {
    tokens.iter().any(|t| words.contains(&t.as_str()))
}

/// Verbs that say how to act rather than what to act on.
///
/// Shared across most of the registry, so a match on one is no evidence of
/// which command was meant.
fn is_verb(word: &str) -> bool {
    COMMAND_VERBS.contains(&word)
}

const COMMAND_VERBS: [&str; 22] = [
    "list", "open", "show", "check", "go", "switch", "get", "find", "read", "close", "start",
    "new", "create", "delete", "clear", "send", "tell", "give", "make", "focus", "run", "set",
];

/// Does this command account for enough of the exchange to be offered?
///
/// Score alone is not evidence of understanding. A single shared word scores
/// LABEL_WEIGHT * EXACT_MATCH = 4, comfortably over the threshold, which is
/// how "clear my recent activity" resolved to "List **recent** Teams chats"
/// and offered to open Teams on the strength of the word "recent".
///
/// The test is that the match explains at least half of one side: half of
/// what the user said, or half of what the command is called. Requiring it
/// of the user's side alone would reject "open settings" for "Settings";
/// requiring it of the label alone would reject "tabs" for "List open tabs".
/// One word in common with a four-word label satisfies neither, which is the
/// case being excluded.
fn explains(spoken: &[String], label: &[String], keywords: &[String]) -> bool {
    if spoken.is_empty() || label.is_empty() {
        return false;
    }
    // Verbs are structural, not distinguishing: "list" is shared by half the
    // registry and "check" by most of the rest. Weighing them is what let
    // "list my tabs" tie with "List recent Teams chats", and what made
    // "check my calendar" resolve to "Check the weather". Only nouns say
    // which command was meant, so both sides are compared on nouns alone.
    let label_nouns: Vec<&String> = label.iter().filter(|w| !is_verb(w)).collect();
    let spoken_nouns: Vec<&String> = spoken.iter().filter(|w| !is_verb(w)).collect();
    if label_nouns.is_empty() {
        return false;
    }
    // A request that is all verb and filler has nothing else to go on:
    // "read this page" is just "read" once the deixis is stripped. Falling
    // back to the verb keeps those working, and they are usually ambiguous
    // enough that NEXUS asks rather than acts.
    if spoken_nouns.is_empty() {
        return spoken
            .iter()
            .any(|word| label.iter().chain(keywords).any(|k| tokens_match(word, k)));
    }

    // Every noun the user said must be accounted for. A command that does
    // not know one of the user's own nouns is not the command they meant:
    // "recent" out of "recent activity" is how Teams was offered for a
    // request that had nothing to do with Teams, and "team" out of "my
    // regards to the team" is the same mistake with a different word.
    //
    // Scoring still decides which of the qualifying commands wins; this only
    // decides which are allowed to compete.
    spoken_nouns
        .iter()
        .all(|word| label.iter().chain(keywords).any(|k| tokens_match(word, k)))
}

/// Weighted overlap: exact hits count double a singular/plural hit, so the
/// command whose wording the user actually used wins.
fn overlap(tokens: &[String], other: &[String]) -> i64 {
    other
        .iter()
        .map(|o| {
            if tokens.iter().any(|t| t == o) {
                EXACT_MATCH
            } else if tokens.iter().any(|t| tokens_match(t, o)) {
                PLURAL_MATCH
            } else {
                0
            }
        })
        .sum()
}

/// Find the project named in the transcript.
///
/// A project matches when every significant token of its name is present.
/// The longest such name wins, so "UI TEST WORK" beats a project called
/// "UI". Ties break on the name itself, keeping the result stable.
fn detect_project(tokens: &[String], project_names: &[String]) -> Option<String> {
    let mut best: Option<(usize, String)> = None;

    for name in project_names {
        let name_tokens = significant(name);
        if name_tokens.is_empty() {
            continue;
        }
        let all_present = name_tokens
            .iter()
            .all(|nt| tokens.iter().any(|t| tokens_match(t, nt)));
        if !all_present {
            continue;
        }
        let len = name_tokens.len();
        let better = match &best {
            None => true,
            Some((best_len, best_name)) => {
                len > *best_len || (len == *best_len && name < best_name)
            }
        };
        if better {
            best = Some((len, name.clone()));
        }
    }

    best.map(|(_, name)| name)
}

/// Resolve a spoken transcript to candidate commands.
///
/// Pure: same inputs always produce the same output.
pub fn resolve_voice_intent(
    transcript: &str,
    commands: &[VoiceCommandSpec],
    project_names: &[String],
) -> VoiceIntent {
    let tokens = significant(transcript);
    let normalized = tokens.join(" ");

    if tokens.is_empty() {
        return VoiceIntent {
            command_ids: Vec::new(),
            search_query: String::new(),
            normalized,
            project_name: None,
            ambiguous: false,
        };
    }

    let project = detect_project(&tokens, project_names);

    // "Create a task in X" is a create intent and must fall through to
    // scoring, where the per-project command wins. Anything else naming a
    // project means "open that project", which the existing search already
    // does, so no voice-only command vocabulary is invented here.
    let wants_new_task = contains_any(&tokens, CREATE_SIGNALS) && contains_any(&tokens, TASK_WORDS);

    if let Some(name) = &project {
        if !wants_new_task {
            return VoiceIntent {
                command_ids: Vec::new(),
                search_query: name.clone(),
                normalized,
                project_name: Some(name.clone()),
                ambiguous: false,
            };
        }
    }

    // Score every command. Label hits count double: a label is what the
    // command is called, a keyword is merely a hint.
    let spoken = significant(&normalized);

    let mut scored: Vec<(i64, &VoiceCommandSpec)> = commands
        .iter()
        .map(|c| {
            let label_tokens = significant(&c.label);
            let keyword_tokens: Vec<String> =
                c.keywords.iter().flat_map(|k| significant(k)).collect();
            let score = LABEL_WEIGHT * overlap(&tokens, &label_tokens)
                + KEYWORD_WEIGHT * overlap(&tokens, &keyword_tokens);
            let enough = explains(&spoken, &label_tokens, &keyword_tokens);
            (score, c, enough)
        })
        .filter(|(score, _, enough)| *score >= SCORE_THRESHOLD && *enough)
        .map(|(score, c, _)| (score, c))
        .collect();

    // Total order: score descending, then id ascending. No ties remain, so
    // the output is byte-identical for identical input.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));

    let top_score = scored.first().map(|(s, _)| *s).unwrap_or(0);
    let tied = scored.iter().filter(|(s, _)| *s == top_score).count();

    let command_ids: Vec<String> = scored
        .iter()
        .take(MAX_CANDIDATES)
        .map(|(_, c)| c.id.clone())
        .collect();

    // With no command match, hand the words to the existing search so the
    // user still sees something rather than a blank panel.
    let search_query = if command_ids.is_empty() {
        normalized.clone()
    } else {
        String::new()
    };

    VoiceIntent {
        command_ids,
        search_query,
        normalized,
        project_name: project,
        ambiguous: tied > 1,
    }
}

// -- Tests -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Mirrors what src/lib/commands.ts produces, so the tests exercise the
    /// real vocabulary rather than an invented one.
    fn registry() -> Vec<VoiceCommandSpec> {
        let mut v = vec![
            VoiceCommandSpec {
                id: "nav-overview".into(),
                label: "Go to Overview".into(),
                keywords: ["overview", "home", "dashboard", "summary", "stats"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            },
            VoiceCommandSpec {
                id: "nav-projects".into(),
                label: "Go to Projects".into(),
                keywords: ["projects", "list", "workspace"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            },
            VoiceCommandSpec {
                id: "nav-registry".into(),
                label: "Go to Registry".into(),
                keywords: ["registry", "ide", "ides", "agent", "agents", "tools"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            },
            VoiceCommandSpec {
                id: "nav-settings".into(),
                label: "Go to Settings".into(),
                keywords: ["settings", "preferences", "options", "config"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            },
            VoiceCommandSpec {
                id: "create-project".into(),
                label: "New Project".into(),
                keywords: ["new", "create", "add", "project"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            },
        ];
        for (id, name) in [(3, "ALPHA"), (4, "UI-TEST-WORK"), (6, "UI-TEST-ZULU")] {
            v.push(VoiceCommandSpec {
                id: format!("create-task-{id}"),
                label: format!("New Task in {name}"),
                keywords: ["new", "create", "add", "task", name]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            });
        }
        v
    }

    fn projects() -> Vec<String> {
        ["ALPHA", "UI-TEST-WORK", "UI-TEST-EMPTY", "UI-TEST-ZULU"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    fn resolve(t: &str) -> VoiceIntent {
        resolve_voice_intent(t, &registry(), &projects())
    }

    /// Regression: a real transcript from the microphone. "setting" is
    /// singular where the registry says "settings", and "tab" is a UI noun
    /// the user appended. Both used to defeat the match entirely.
    #[test]
    fn spoken_open_setting_tab_resolves() {
        let r = resolve("Open setting tab");
        assert_eq!(
            r.command_ids.first().map(String::as_str),
            Some("nav-settings"),
            "got {:?} for normalized {:?}",
            r.command_ids,
            r.normalized
        );
        assert_eq!(r.normalized, "open setting");
    }

    #[test]
    fn singular_and_plural_both_resolve() {
        // A singular spoken form must still reach the plural command.
        assert_eq!(
            resolve("open setting").command_ids.first(),
            resolve("open settings").command_ids.first(),
            "setting/settings must resolve alike"
        );

        // Where singular and plural are DIFFERENT commands, the exact wording
        // wins and the other is still offered as a second candidate, so the
        // user can pick. Confirmation is required either way.
        let singular = resolve("go to project");
        let plural = resolve("go to projects");
        assert_eq!(
            singular.command_ids.first().map(String::as_str),
            Some("create-project")
        );
        assert_eq!(
            plural.command_ids.first().map(String::as_str),
            Some("nav-projects")
        );
        assert!(
            singular.command_ids.contains(&"nav-projects".to_string()),
            "the plural command must still be offered, got {:?}",
            singular.command_ids
        );

        assert!(tokens_match("setting", "settings"));
        assert!(tokens_match("tasks", "task"));
        assert!(!tokens_match("task", "tasty"));
        assert!(!tokens_match("project", "projected"));
    }

    #[test]
    fn ui_nouns_are_treated_as_noise() {
        for phrase in [
            "open settings tab",
            "go to projects screen",
            "settings page",
            "show me the registry panel",
        ] {
            assert!(
                !resolve(phrase).command_ids.is_empty(),
                "{phrase:?} should still resolve"
            );
        }
    }

    // -- The four Tanglish phrases from S-08 ---------------------------------

    #[test]
    fn tanglish_project_create_pannunga() {
        let r = resolve("project create pannunga");
        assert_eq!(
            r.command_ids.first().map(String::as_str),
            Some("create-project"),
            "expected New Project, got {:?}",
            r.command_ids
        );
        assert_eq!(r.normalized, "project create");
    }

    #[test]
    fn tanglish_alpha_project_open_pannu() {
        let r = resolve("ALPHA project open pannu");
        assert_eq!(r.project_name.as_deref(), Some("ALPHA"));
        assert_eq!(
            r.search_query, "ALPHA",
            "should open the ALPHA project through the existing search"
        );
        assert!(
            r.command_ids.is_empty(),
            "opening a project is not a command; it is an existing search result"
        );
    }

    #[test]
    fn tanglish_new_task_create_pannunga() {
        let r = resolve("new task create pannunga");
        // Every New Task command must outrank New Project. Lower-ranked
        // alternatives are still offered, which is safe: the user confirms.
        let first_task = r
            .command_ids
            .iter()
            .position(|id| id.starts_with("create-task-"));
        let first_project = r.command_ids.iter().position(|id| id == "create-project");
        assert_eq!(
            first_task,
            Some(0),
            "a New Task command must rank first, got {:?}",
            r.command_ids
        );
        if let Some(p) = first_project {
            let last_task = r
                .command_ids
                .iter()
                .rposition(|id| id.starts_with("create-task-"))
                .unwrap();
            assert!(
                p > last_task,
                "New Project must rank below every New Task, got {:?}",
                r.command_ids
            );
        }
        assert!(
            r.ambiguous,
            "several projects tie at the top, so the user must choose"
        );
    }

    #[test]
    fn tanglish_settings_open_pannunga() {
        let r = resolve("settings open pannunga");
        assert_eq!(
            r.command_ids.first().map(String::as_str),
            Some("nav-settings")
        );
        assert_eq!(r.normalized, "settings open");
    }

    // -- The English phrasings that NEXUS-009 could not match ----------------

    #[test]
    fn open_settings_resolves() {
        let r = resolve("open settings");
        assert_eq!(
            r.command_ids.first().map(String::as_str),
            Some("nav-settings")
        );
    }

    #[test]
    fn open_projects_resolves() {
        let r = resolve("open projects");
        assert_eq!(
            r.command_ids.first().map(String::as_str),
            Some("nav-projects")
        );
    }

    #[test]
    fn create_project_resolves() {
        let r = resolve("create project");
        assert_eq!(
            r.command_ids.first().map(String::as_str),
            Some("create-project")
        );
    }

    // -- Project-specific command --------------------------------------------

    #[test]
    fn new_task_in_named_project_targets_that_project() {
        let r = resolve("new task in ALPHA");
        assert_eq!(
            r.command_ids.first().map(String::as_str),
            Some("create-task-3"),
            "got {:?}",
            r.command_ids
        );
        assert!(!r.ambiguous, "naming the project removes the ambiguity");
    }

    #[test]
    fn longest_project_name_wins() {
        let r = resolve("open UI TEST ZULU");
        assert_eq!(r.project_name.as_deref(), Some("UI-TEST-ZULU"));
        assert_eq!(r.search_query, "UI-TEST-ZULU");
    }

    // -- Ambiguity and the unknown case --------------------------------------

    #[test]
    fn ambiguous_transcript_offers_every_tied_candidate() {
        let r = resolve("new task");
        assert!(
            r.ambiguous,
            "one command per project ties, got {:?}",
            r.command_ids
        );
        assert!(
            r.command_ids.len() > 1,
            "an ambiguous transcript must offer choices, got {:?}",
            r.command_ids
        );
        // All three per-project New Task commands must be offered, ranked
        // above any weaker alternative.
        let tasks: Vec<&String> = r
            .command_ids
            .iter()
            .filter(|id| id.starts_with("create-task-"))
            .collect();
        assert_eq!(
            tasks.len(),
            3,
            "every project should be offered, got {:?}",
            r.command_ids
        );
        assert!(r.command_ids[0].starts_with("create-task-"));
    }

    #[test]
    fn unknown_transcript_matches_nothing_and_falls_back_to_search() {
        let r = resolve("xylophone banana quantum");
        assert!(r.command_ids.is_empty());
        assert_eq!(r.search_query, "xylophone banana quantum");
        assert!(!r.ambiguous);
    }

    #[test]
    fn empty_and_filler_only_transcripts_resolve_to_nothing() {
        for t in ["", "   ", "please can you", "pannunga pannu"] {
            let r = resolve(t);
            assert!(r.command_ids.is_empty(), "{t:?} should match nothing");
            assert_eq!(r.normalized, "", "{t:?} should normalise away entirely");
        }
    }

    // -- Normalisation --------------------------------------------------------

    #[test]
    fn filler_words_are_stripped() {
        assert_eq!(
            significant("please can you go to the settings now"),
            vec!["settings"]
        );
        assert_eq!(
            significant("settings open pannunga"),
            vec!["settings", "open"]
        );
    }

    #[test]
    fn punctuation_and_case_are_normalised() {
        let a = resolve("Open, Settings!");
        let b = resolve("open settings");
        assert_eq!(a.command_ids, b.command_ids);
        assert_eq!(a.normalized, b.normalized);
    }

    #[test]
    fn resolution_is_deterministic() {
        let first = resolve("new task create pannunga");
        for _ in 0..25 {
            assert_eq!(resolve("new task create pannunga"), first);
        }
    }

    #[test]
    fn candidate_count_is_capped() {
        let mut cmds = registry();
        for i in 0..50 {
            cmds.push(VoiceCommandSpec {
                id: format!("create-task-{}", 100 + i),
                label: format!("New Task in P{i}"),
                keywords: ["new", "create", "add", "task"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            });
        }
        let r = resolve_voice_intent("new task", &cmds, &projects());
        assert!(r.command_ids.len() <= MAX_CANDIDATES);
    }

    #[test]
    fn signal_lists_do_not_overlap_with_filler() {
        for w in CREATE_SIGNALS.iter().chain(TASK_WORDS) {
            assert!(
                !FILLER_WORDS.contains(w),
                "{w:?} is both a signal and filler, which would make matching unstable"
            );
        }
    }
}
