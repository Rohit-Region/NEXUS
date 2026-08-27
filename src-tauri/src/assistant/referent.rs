//! NEXUS-013: conversational referents.
//!
//! "Open the PR." "Do the first one." "Reply to him." None of these is
//! resolvable from the words alone, and all of them are how people actually
//! talk. This module is the difference between an assistant and a command
//! line with a microphone attached.
//!
//! Three rules keep it from becoming guesswork, and they are the reason no
//! model is involved:
//!
//! 1. **Only what NEXUS itself said becomes a referent.** The user mentioning
//!    something does not create one, because NEXUS has no handle for it and
//!    would be inventing the association.
//! 2. **Ordinals come from a list that was actually rendered.** "The first
//!    one" is a position in something the user saw, never an inference over
//!    hidden state.
//! 3. **Ambiguity asks.** Two pull requests in scope means NEXUS names both
//!    and waits. Same rule the voice matcher follows, same rule the approval
//!    flow follows: never guess when the cost of guessing wrong is an action.

use serde::{Deserialize, Serialize};

/// What kind of thing a referent points at.
///
/// `Person` is not in the original list but "reply to him" needs it, and a
/// message without a resolvable sender is a dead end for the whole flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReferentKind {
    Project,
    Task,
    PullRequest,
    JiraIssue,
    TeamsMessage,
    Person,
    Conversation,
    BrowserTab,
    IdeWorkspace,
    Suggestion,
}

impl ReferentKind {
    /// Words that name this kind in ordinary speech.
    ///
    /// A fixed vocabulary, not a synonym engine. Adding a word is a decision
    /// someone makes and a test proves, which is the point.
    fn words(self) -> &'static [&'static str] {
        match self {
            ReferentKind::Project => &["project", "repo", "repository"],
            ReferentKind::Task => &["task", "todo"],
            ReferentKind::PullRequest => &["pr", "prs", "pull", "review"],
            ReferentKind::JiraIssue => &["ticket", "issue", "jira", "story", "bug"],
            ReferentKind::TeamsMessage => &["message", "msg", "teams", "chat"],
            ReferentKind::Person => &["person", "sender", "author"],
            ReferentKind::Conversation => &["conversation", "thread"],
            ReferentKind::BrowserTab => &["tab", "page", "browser"],
            ReferentKind::IdeWorkspace => &["workspace", "ide", "editor"],
            ReferentKind::Suggestion => &["suggestion"],
        }
    }

    fn all() -> [ReferentKind; 10] {
        [
            ReferentKind::Project,
            ReferentKind::Task,
            ReferentKind::PullRequest,
            ReferentKind::JiraIssue,
            ReferentKind::TeamsMessage,
            ReferentKind::Person,
            ReferentKind::Conversation,
            ReferentKind::BrowserTab,
            ReferentKind::IdeWorkspace,
            ReferentKind::Suggestion,
        ]
    }
}

/// One thing NEXUS mentioned, and how to act on it later.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Referent {
    pub id: u64,
    pub kind: ReferentKind,
    /// What the user saw. Used verbatim when NEXUS has to ask which one.
    pub display_name: String,
    /// Which connector produced it, for the Activity view and for routing.
    pub source: String,
    /// Enough to act on it: `{"projectId": 4}`, `{"number": 8792}`. Shaped
    /// by whoever registered it and read only by whoever consumes it.
    pub metadata: serde_json::Value,
    /// The turn that introduced it. Newer turns win when a kind repeats.
    pub turn: u64,
}

/// A list NEXUS actually rendered, and therefore the only thing an ordinal
/// may index into.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderedList {
    pub id: u64,
    pub turn: u64,
    /// Referent ids, in the order the user saw them.
    pub items: Vec<u64>,
}

/// The outcome of trying to resolve a phrase.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Resolution {
    /// Exactly one candidate.
    #[serde(rename_all = "camelCase")]
    Resolved { referent: Referent },
    /// Several, and NEXUS will not pick. The candidates are returned so the
    /// caller can name them back to the user.
    #[serde(rename_all = "camelCase")]
    Ambiguous { candidates: Vec<Referent> },
    /// Nothing matched, with a reason worth showing.
    #[serde(rename_all = "camelCase")]
    Unresolved { reason: String },
}

// -- Phrase parsing -----------------------------------------------------------

/// Lowercase, strip punctuation, collapse whitespace.
///
/// Deliberately a local copy rather than a call into the voice matcher: that
/// module's normaliser is part of a shipped matching contract, and coupling
/// this to it would mean a change here could alter voice behaviour.
fn normalize(phrase: &str) -> Vec<String> {
    phrase
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '#' { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .map(|s| s.to_string())
        .collect()
}

/// A one-based position, if the phrase names one.
///
/// Bare digits count only when introduced by "number" or "#", so "open 8792"
/// is not read as "open the 8792nd thing".
fn ordinal(tokens: &[String]) -> Option<usize> {
    const WORDS: [(&str, usize); 20] = [
        ("first", 1), ("1st", 1),
        ("second", 2), ("2nd", 2),
        ("third", 3), ("3rd", 3),
        ("fourth", 4), ("4th", 4),
        ("fifth", 5), ("5th", 5),
        ("sixth", 6), ("6th", 6),
        ("seventh", 7), ("7th", 7),
        ("eighth", 8), ("8th", 8),
        ("ninth", 9), ("9th", 9),
        ("tenth", 10), ("10th", 10),
    ];

    for (index, token) in tokens.iter().enumerate() {
        if let Some((_, position)) = WORDS.iter().find(|(word, _)| word == token) {
            return Some(*position);
        }
        // "number three", "number 3", "#3"
        let introduced = index > 0 && tokens[index - 1] == "number";
        let hashed = token.strip_prefix('#');
        if introduced {
            if let Ok(parsed) = token.parse::<usize>() {
                if parsed > 0 {
                    return Some(parsed);
                }
            }
        }
        if let Some(rest) = hashed {
            if let Ok(parsed) = rest.parse::<usize>() {
                if parsed > 0 && parsed <= 10 {
                    return Some(parsed);
                }
            }
        }
    }
    None
}

/// The kind a phrase names, if any.
fn kind_of(tokens: &[String]) -> Option<ReferentKind> {
    // "pull request" before "request"; checked longest-first by construction
    // since each kind's word list is disjoint.
    for kind in ReferentKind::all() {
        if tokens.iter().any(|t| kind.words().contains(&t.as_str())) {
            return Some(kind);
        }
    }
    None
}

/// Whether the phrase is a bare pronoun, and whether it implies a person.
fn pronoun(tokens: &[String]) -> Option<Option<ReferentKind>> {
    const PERSON: [&str; 6] = ["him", "her", "them", "they", "he", "she"];
    const THING: [&str; 5] = ["it", "that", "this", "one", "those"];

    if tokens.iter().any(|t| PERSON.contains(&t.as_str())) {
        return Some(Some(ReferentKind::Person));
    }
    if tokens.iter().any(|t| THING.contains(&t.as_str())) {
        return Some(None);
    }
    None
}

// -- Resolution ---------------------------------------------------------------

/// Pick from candidates of one kind.
///
/// Recency decides, but only across turns. Two pull requests named in the
/// *same* answer are genuinely ambiguous: the user saw both at once and has
/// no reason to think NEXUS prefers one. Two named in different turns are
/// not: the later one is what the conversation is about now.
fn newest_unambiguous(matches: Vec<Referent>) -> Resolution {
    match matches.len() {
        0 => Resolution::Unresolved {
            reason: "nothing like that has come up yet".to_string(),
        },
        1 => Resolution::Resolved {
            referent: matches.into_iter().next().expect("length checked"),
        },
        _ => {
            let newest_turn = matches.iter().map(|r| r.turn).max().unwrap_or(0);
            let mut in_newest: Vec<Referent> = matches
                .into_iter()
                .filter(|r| r.turn == newest_turn)
                .collect();
            if in_newest.len() == 1 {
                Resolution::Resolved {
                    referent: in_newest.remove(0),
                }
            } else {
                Resolution::Ambiguous {
                    candidates: in_newest,
                }
            }
        }
    }
}

/// Resolve a phrase against what NEXUS has said.
///
/// Pure. `referents` is newest-turn-last is not assumed; ordering is derived
/// from `turn`. `lists` likewise, with the most recent used for ordinals.
pub fn resolve(phrase: &str, referents: &[Referent], lists: &[RenderedList]) -> Resolution {
    let tokens = normalize(phrase);
    if tokens.is_empty() {
        return Resolution::Unresolved {
            reason: "there was nothing to resolve".to_string(),
        };
    }

    let named_kind = kind_of(&tokens);

    // 1. An ordinal indexes the most recent rendered list, and nothing else.
    if let Some(position) = ordinal(&tokens) {
        let newest = lists.iter().max_by_key(|l| l.turn);
        let list = match newest {
            Some(list) if !list.items.is_empty() => list,
            _ => {
                return Resolution::Unresolved {
                    reason: "NEXUS has not shown you a list to count through".to_string(),
                }
            }
        };
        let item = match list.items.get(position - 1) {
            Some(id) => *id,
            None => {
                return Resolution::Unresolved {
                    reason: format!(
                        "that list only has {} item{}",
                        list.items.len(),
                        if list.items.len() == 1 { "" } else { "s" }
                    ),
                }
            }
        };
        return match referents.iter().find(|r| r.id == item) {
            Some(found) => Resolution::Resolved {
                referent: found.clone(),
            },
            None => Resolution::Unresolved {
                reason: "that item is no longer in the conversation".to_string(),
            },
        };
    }

    // 2. A named kind: "the PR", "the ticket".
    if let Some(kind) = named_kind {
        return newest_unambiguous(
            referents
                .iter()
                .filter(|r| r.kind == kind)
                .cloned()
                .collect(),
        );
    }

    // 3. A pronoun. Gendered ones mean a person; the rest mean the most
    //    recent thing of any kind.
    if let Some(implied) = pronoun(&tokens) {
        let matches: Vec<Referent> = match implied {
            Some(kind) => referents.iter().filter(|r| r.kind == kind).cloned().collect(),
            None => referents.to_vec(),
        };
        if matches.is_empty() && implied == Some(ReferentKind::Person) {
            return Resolution::Unresolved {
                reason: "NEXUS does not know who you mean".to_string(),
            };
        }
        return newest_unambiguous(matches);
    }

    Resolution::Unresolved {
        reason: "that does not refer to anything NEXUS has mentioned".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn referent(id: u64, kind: ReferentKind, name: &str, turn: u64) -> Referent {
        Referent {
            id,
            kind,
            display_name: name.to_string(),
            source: "test".to_string(),
            metadata: json!({}),
            turn,
        }
    }

    /// The scenario from the brief: NEXUS answers with three numbered things.
    fn attention_list() -> (Vec<Referent>, Vec<RenderedList>) {
        let referents = vec![
            referent(1, ReferentKind::PullRequest, "PR #8792", 1),
            referent(2, ReferentKind::JiraIssue, "KAI-515", 1),
            referent(3, ReferentKind::Project, "AdminService", 1),
            referent(4, ReferentKind::Person, "Alec", 1),
        ];
        let lists = vec![RenderedList {
            id: 1,
            turn: 1,
            items: vec![1, 2, 3],
        }];
        (referents, lists)
    }

    fn resolved_name(resolution: &Resolution) -> &str {
        match resolution {
            Resolution::Resolved { referent } => &referent.display_name,
            other => panic!("expected a single match, got {other:?}"),
        }
    }

    // -- The brief's own conversation ----------------------------------------

    #[test]
    fn open_the_pr_resolves_to_the_pull_request() {
        let (r, l) = attention_list();
        assert_eq!(resolved_name(&resolve("open the PR", &r, &l)), "PR #8792");
    }

    #[test]
    fn do_the_first_one_indexes_the_rendered_list() {
        let (r, l) = attention_list();
        assert_eq!(resolved_name(&resolve("do the first one", &r, &l)), "PR #8792");
        assert_eq!(resolved_name(&resolve("the second one", &r, &l)), "KAI-515");
        assert_eq!(resolved_name(&resolve("third", &r, &l)), "AdminService");
    }

    #[test]
    fn reply_to_him_resolves_to_the_person() {
        let (r, l) = attention_list();
        assert_eq!(resolved_name(&resolve("reply to him", &r, &l)), "Alec");
    }

    #[test]
    fn a_ticket_and_an_issue_and_jira_all_name_the_same_kind() {
        let (r, l) = attention_list();
        for phrase in ["open the ticket", "open the issue", "show me the jira"] {
            assert_eq!(resolved_name(&resolve(phrase, &r, &l)), "KAI-515", "{phrase}");
        }
    }

    // -- Ordinals ------------------------------------------------------------

    #[test]
    fn an_ordinal_needs_a_list_that_was_actually_rendered() {
        // The rule that keeps ordinals honest: no list, no counting.
        let (r, _) = attention_list();
        let resolution = resolve("the first one", &r, &[]);
        assert!(
            matches!(&resolution, Resolution::Unresolved { reason } if reason.contains("list")),
            "{resolution:?}"
        );
    }

    #[test]
    fn an_out_of_range_ordinal_says_how_many_there_were() {
        let (r, l) = attention_list();
        let resolution = resolve("the fifth one", &r, &l);
        assert!(
            matches!(&resolution, Resolution::Unresolved { reason } if reason.contains("3 items")),
            "{resolution:?}"
        );
    }

    #[test]
    fn number_three_and_hash_three_both_count() {
        let (r, l) = attention_list();
        assert_eq!(resolved_name(&resolve("number 3", &r, &l)), "AdminService");
        assert_eq!(resolved_name(&resolve("#2", &r, &l)), "KAI-515");
    }

    #[test]
    fn a_bare_number_is_not_an_ordinal() {
        // "open 8792" must not mean "open the 8792nd thing".
        let (r, l) = attention_list();
        let resolution = resolve("open 8792", &r, &l);
        assert!(matches!(resolution, Resolution::Unresolved { .. }), "{resolution:?}");
    }

    #[test]
    fn the_most_recent_list_is_the_one_counted() {
        let mut r = attention_list().0;
        r.push(referent(9, ReferentKind::Task, "Ship it", 4));
        let lists = vec![
            RenderedList { id: 1, turn: 1, items: vec![1, 2, 3] },
            RenderedList { id: 2, turn: 4, items: vec![9] },
        ];
        assert_eq!(resolved_name(&resolve("the first one", &r, &lists)), "Ship it");
    }

    // -- Ambiguity -----------------------------------------------------------

    #[test]
    fn two_prs_in_the_same_answer_are_ambiguous() {
        // The user saw both at once and has no reason to think NEXUS prefers
        // one. Guessing here is how an assistant opens the wrong thing.
        let r = vec![
            referent(1, ReferentKind::PullRequest, "PR #8792", 3),
            referent(2, ReferentKind::PullRequest, "PR #8801", 3),
        ];
        match resolve("open the PR", &r, &[]) {
            Resolution::Ambiguous { candidates } => {
                assert_eq!(candidates.len(), 2);
                let names: Vec<&str> =
                    candidates.iter().map(|c| c.display_name.as_str()).collect();
                assert!(names.contains(&"PR #8792") && names.contains(&"PR #8801"));
            }
            other => panic!("expected ambiguity, got {other:?}"),
        }
    }

    #[test]
    fn two_prs_from_different_turns_resolve_to_the_newer() {
        // Not ambiguous: the later one is what the conversation is about now.
        let r = vec![
            referent(1, ReferentKind::PullRequest, "PR #8792", 2),
            referent(2, ReferentKind::PullRequest, "PR #8801", 7),
        ];
        assert_eq!(resolved_name(&resolve("the PR", &r, &[])), "PR #8801");
    }

    #[test]
    fn ambiguous_candidates_come_back_named_so_nexus_can_ask() {
        let r = vec![
            referent(1, ReferentKind::Task, "Write the spec", 5),
            referent(2, ReferentKind::Task, "Review the spec", 5),
        ];
        match resolve("open the task", &r, &[]) {
            Resolution::Ambiguous { candidates } => {
                assert!(candidates.iter().all(|c| !c.display_name.is_empty()));
            }
            other => panic!("expected ambiguity, got {other:?}"),
        }
    }

    // -- Pronouns ------------------------------------------------------------

    #[test]
    fn it_refers_to_the_most_recent_thing() {
        let r = vec![
            referent(1, ReferentKind::PullRequest, "PR #8792", 1),
            referent(2, ReferentKind::Task, "Ship it", 6),
        ];
        assert_eq!(resolved_name(&resolve("open it", &r, &[])), "Ship it");
    }

    #[test]
    fn a_gendered_pronoun_with_no_person_says_so_rather_than_guessing() {
        let r = vec![referent(1, ReferentKind::PullRequest, "PR #8792", 1)];
        let resolution = resolve("reply to him", &r, &[]);
        assert!(
            matches!(&resolution, Resolution::Unresolved { reason } if reason.contains("who")),
            "{resolution:?}"
        );
    }

    // -- Boundaries ----------------------------------------------------------

    #[test]
    fn an_empty_conversation_resolves_nothing() {
        let resolution = resolve("open the PR", &[], &[]);
        assert!(matches!(resolution, Resolution::Unresolved { .. }));
    }

    #[test]
    fn an_empty_phrase_is_handled() {
        let (r, l) = attention_list();
        assert!(matches!(resolve("", &r, &l), Resolution::Unresolved { .. }));
        assert!(matches!(resolve("   ", &r, &l), Resolution::Unresolved { .. }));
    }

    #[test]
    fn a_phrase_naming_nothing_says_so() {
        let (r, l) = attention_list();
        let resolution = resolve("what is the weather", &r, &l);
        assert!(matches!(resolution, Resolution::Unresolved { .. }), "{resolution:?}");
    }

    #[test]
    fn resolution_is_pure() {
        let (r, l) = attention_list();
        let first = resolve("the first one", &r, &l);
        for _ in 0..20 {
            assert_eq!(resolve("the first one", &r, &l), first);
        }
    }

    #[test]
    fn case_and_trailing_punctuation_do_not_matter() {
        // What speech and typing actually produce: mixed case, a trailing
        // question or exclamation mark. Note the limit this documents:
        // "p.r." normalises to two single-letter tokens and does NOT match,
        // because treating stray letters as abbreviations would resolve far
        // more than it should.
        let (r, l) = attention_list();
        for phrase in ["Open THE PR!", "open the pr?", "  The   Pr  "] {
            assert_eq!(resolved_name(&resolve(phrase, &r, &l)), "PR #8792", "{phrase}");
        }
        assert!(
            matches!(resolve("open the p.r.", &r, &l), Resolution::Unresolved { .. }),
            "single letters must not be read as abbreviations"
        );
    }

    #[test]
    fn resolution_serialises_as_a_tagged_union() {
        let (r, l) = attention_list();
        let json = serde_json::to_string(&resolve("the PR", &r, &l)).expect("serialize");
        assert!(json.contains("\"kind\":\"resolved\""), "{json}");
        let miss = serde_json::to_string(&resolve("nothing", &r, &l)).expect("serialize");
        assert!(miss.contains("\"kind\":\"unresolved\""), "{miss}");
    }

    #[test]
    fn no_kind_shares_a_word_with_another() {
        // A shared word would make resolution order-dependent, and the order
        // is an implementation detail nobody should have to know.
        let mut seen: std::collections::HashMap<&str, ReferentKind> =
            std::collections::HashMap::new();
        for kind in ReferentKind::all() {
            for word in kind.words() {
                if let Some(other) = seen.insert(word, kind) {
                    panic!("{word:?} names both {other:?} and {kind:?}");
                }
            }
        }
    }
}
