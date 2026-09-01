//! NEXUS-015 defect C, part two: phrases that carry their own arguments.
//!
//! `zero_input_actions` made actions like "list my tabs" reachable, but the
//! ones people actually ask for carry a target: open *a named project* in
//! *a named editor*, open *a website*, search for *some words*. Those need
//! an argument extracted from the phrase, and until they had one, every
//! editor and browser action was reachable only through a reasoning
//! provider.
//!
//! This is pattern matching over a fixed grammar, not language understanding.
//! Every rule below is a shape you can read in one line, and each one either
//! matches exactly or declines. Nothing is inferred, nothing is scored, and
//! an unmatched phrase falls through to the general matcher untouched.
//!
//! Editors and projects are resolved **from the registry and the database**,
//! never from a hardcoded list, so registering a new editor makes it
//! speakable with no code change.

use rusqlite::Connection;
use serde_json::json;

use crate::db::ides::list_ides;

/// A matched phrase, ready for the gate.
#[derive(Debug, Clone, PartialEq)]
pub struct Parametric {
    pub action_id: String,
    pub input: serde_json::Value,
    pub summary: String,
}

/// Words that separate a target from a destination: "open X **in** Y".
const IN_WORDS: &[&str] = &["in", "with", "using"];

/// Words that carry no meaning of their own around a target.
const BARE_REFERENCE_FILLER: &[&str] = &[
    "go", "to", "the", "a", "an", "in", "on", "up", "into", "over", "at", "please", "hey", "nexus",
    "me", "my",
];

/// Words that mean "bring this to the front".
const SWITCH_VERBS: &[&str] = &["switch", "focus", "go", "open", "show", "bring"];

/// Verbs that introduce dictation into a page.
///
/// Checked before search, because "type search engines are useful" is
/// dictation, not a request to search for "engines are useful".
const DICTATE_VERBS: &[&str] = &["type", "dictate", "write"];

/// Words that introduce sending someone a message.
const MESSAGE_WORDS: [&str; 5] = ["message", "text", "whatsapp", "msg", "send"];

/// Words that separate who from what.
///
/// Ordered longest first so "saying that" does not match on "that" and leave
/// "saying" glued to the front of the message.
const BODY_SEPARATORS: [&str; 6] = [" saying that ", " telling ", " saying ", " that says ", " that ", " to say "];

/// The words after the separator, whoever they turn out to be for.
///
/// Kept separate from the contact lookup so a message to a name NEXUS has to
/// ask about is not lost: the user should not have to say the whole sentence
/// again once they have picked who they meant.
pub fn message_body(original: &str) -> String {
    let lower = original.to_lowercase();
    BODY_SEPARATORS
        .iter()
        .filter_map(|sep| lower.find(sep).map(|at| at + sep.len()))
        .min()
        .map(|start| original[start..].trim().to_string())
        .unwrap_or_default()
}

/// Who a message request was addressed to, when nobody by that name exists.
///
/// Declining is correct -- NEXUS will not guess a number -- but the phrase
/// then fell through to the reasoning provider and the user was told
/// "ollama is not responding", which answers a question they did not ask.
/// This recovers the name so they can be told the real reason.
pub fn unknown_recipient(tokens: &[String], original: &str, conn: &Connection) -> Option<String> {
    if !tokens.iter().any(|t| MESSAGE_WORDS.contains(&t.as_str())) {
        return None;
    }
    if whatsapp_message(tokens, original, conn).is_some() {
        return None;
    }

    recipient_name(tokens, original)
}

/// The name a message was addressed to, in the user's own capitals.
///
/// The name follows "to", which is how people address a message. It stops at
/// the body separator so "to Divi saying hello" does not name the person
/// "Divi saying hello".
pub fn recipient_name(tokens: &[String], original: &str) -> Option<String> {
    let at = tokens.iter().position(|t| t == "to")?;
    let name: Vec<&str> = tokens[at + 1..]
        .iter()
        .map(|s| s.as_str())
        .take_while(|w| !["saying", "that", "telling", "say"].contains(w))
        .collect();
    if name.is_empty() {
        return None;
    }

    // Taken from what they actually said rather than the normalised tokens,
    // because it is read back to them: "Ama", not "mataji".
    let joined = name.join(" ");
    let lower = original.to_lowercase();
    Some(match lower.find(&joined) {
        Some(at) => original[at..at + joined.len()].to_string(),
        None => joined,
    })
}

/// "send a message to <contact> saying <words>".
///
/// The contact has to already exist: NEXUS will not guess a number, and a
/// message to the wrong person is not recoverable. An unknown name declines
/// here and the phrase carries on to the other shapes.
fn whatsapp_message(tokens: &[String], original: &str, conn: &Connection) -> Option<Parametric> {
    if !tokens.iter().any(|t| MESSAGE_WORDS.contains(&t.as_str())) {
        return None;
    }

    let lower = original.to_lowercase();
    let contacts = crate::db::contacts::list_contacts(conn).ok()?;

    // Longest name first, so "Divya Raj" is not matched as "Divya".
    let mut candidates: Vec<_> = contacts
        .into_iter()
        .filter_map(|c| {
            let needle = c.name.to_lowercase();
            lower.find(&needle).map(|at| (needle.len(), at, c))
        })
        .collect();
    candidates.sort_by(|a, b| b.0.cmp(&a.0));

    let (name_len, name_at, contact) = match candidates.into_iter().next() {
        Some(found) => found,
        None => {
            // Nobody by that name in NEXUS's own list. WhatsApp has one
            // already, so ask it rather than making the user retype
            // hundreds of people to send one message.
            //
            // Exact match only, and only when it names one person: two
            // contacts sharing a name is a question, not a coin toss, and
            // it is answered a rung up where NEXUS can offer both.
            let spoken = recipient_name(tokens, original)?;
            let found = super::wa_contacts::lookup(&spoken);
            let only = match found.as_slice() {
                [one] => one.clone(),
                _ => return None,
            };
            let body = message_body(original);
            let summary = if body.is_empty() {
                format!("Open a WhatsApp chat with {}", only.name)
            } else {
                format!("Message {} on WhatsApp: \"{body}\"", only.name)
            };
            return Some(Parametric {
                action_id: "whatsapp.compose_message".to_string(),
                input: json!({
                    "phone": only.phone,
                    "message": body,
                    "displayName": only.name,
                }),
                summary,
            });
        }
    };

    // The body is whatever follows the separator, in the user's own words.
    // Taken from the original rather than the tokens because a message is
    // dictation: its capitals and punctuation are the point.
    let after_name = name_at + name_len;
    let body = BODY_SEPARATORS
        .iter()
        .filter_map(|sep| {
            lower[after_name..]
                .find(sep)
                .map(|at| after_name + at + sep.len())
        })
        .min()
        .map(|start| original[start..].trim().to_string())
        .unwrap_or_default();

    let summary = if body.is_empty() {
        format!("Open a WhatsApp chat with {}", contact.name)
    } else {
        format!("Message {} on WhatsApp: \"{body}\"", contact.name)
    };

    Some(Parametric {
        action_id: "whatsapp.compose_message".to_string(),
        input: json!({
            "phone": contact.phone,
            "message": body,
            "displayName": contact.name,
        }),
        summary,
    })
}

/// A Jira issue key spoken or typed in a phrase.
///
/// Two forms, because dictation renders the same key both ways. Written out
/// with the hyphen ("PROJ-101") it is unmistakable and is accepted anywhere in
/// the phrase. Spoken, the hyphen is lost and it arrives as two tokens
/// ("proj", "101"), which is also what "task 3" looks like. Rather than
/// demanding the phrase also say "jira", the two cases that genuinely are
/// not issue keys are excluded: NEXUS's own entity nouns, and the name of a
/// project in the database. Requiring "jira" rejected the phrasing people
/// actually use, including "open PROJ-101 in the browser" -- which is the
/// wording NEXUS itself prints back to them.
///
/// Validation is `jira_connector::valid_key`, so what is accepted here is
/// exactly what the connector will accept later.
/// Verbs that move an issue somewhere.
const TRANSITION_VERBS: &[&str] = &["move", "set", "mark", "transition", "change", "put"];

/// "move PROJ-387 to done" as an action, if the phrase is one.
///
/// The status is taken verbatim from after the joining word and matched
/// against what the workflow actually offers at execution time. Nothing is
/// validated here: a status this shape accepted but Jira does not have comes
/// back naming the ones it does, which is a better answer than a refusal
/// written from a list NEXUS would have to keep in step.
fn jira_transition(tokens: &[String], original: &str, conn: &Connection) -> Option<Parametric> {
    let verb = tokens
        .iter()
        .position(|t| TRANSITION_VERBS.contains(&t.as_str()))?;
    let key = issue_key(&tokens[verb..], original, conn)?;

    // Everything after the last " to " is the destination. Searched from the
    // right because a status can contain the word: "move PROJ-1 to ready to
    // test" ends at the status, not in the middle of it.
    let lower = original.to_lowercase();
    let at = lower.rfind(" to ")?;
    let status = original[at + 4..].trim();
    if status.is_empty() {
        return None;
    }

    Some(Parametric {
        action_id: "jira.transition_issue".to_string(),
        input: json!({ "key": key, "status": status }),
        summary: format!("Move {key} to {status}"),
    })
}

fn issue_key(tokens: &[String], original: &str, conn: &Connection) -> Option<String> {
    use super::jira_connector::valid_key;

    // Written form: any hyphenated word that validates.
    for word in original.split(|c: char| c.is_whitespace() || c == ',') {
        let trimmed = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '-');
        if trimmed.contains('-') {
            if let Some(key) = valid_key(trimmed) {
                return Some(key);
            }
        }
    }

    // Spoken form: an adjacent name and number. Any two words next to a
    // number look like this, so the phrase must also point at the pair: it
    // either says what kind of thing it is, or asks to open it by name.
    // Without that, "it took 1.5 hours" resolves to TOOK-1.
    let says_jira = tokens
        .iter()
        .any(|t| t == "jira" || t == "ticket" || t == "tickets" || t == "issue");
    let known: Vec<String> = projects(conn).into_iter().map(|(_, name)| name).collect();

    tokens.windows(2).enumerate().find_map(|(at, pair)| {
        let (name, number) = (&pair[0], &pair[1]);

        // "open proj 101", and "go to proj 101" with a joining word in
        // between. Two tokens of look-back covers both without reaching so
        // far back that an unrelated verb earlier in the sentence counts.
        let opened = tokens[at.saturating_sub(2)..at]
            .iter()
            .any(|w| OPEN_VERBS.contains(&w.as_str()));
        if !says_jira && !opened {
            return None;
        }

        // "open task 3" is a task, not TASK-3. These are the things NEXUS
        // already counts and numbers itself.
        if NOT_A_PROJECT_KEY.contains(&name.as_str()) {
            return None;
        }
        // "open alpha 2" names a project NEXUS knows, so it is not a Jira
        // key either. Checked against the database, not a guessed list.
        if known.iter().any(|p| p.eq_ignore_ascii_case(name)) {
            return None;
        }
        // Jira project keys are short and alphabetic. A longer name next to
        // a number is a word and a count, not a key.
        if name.len() < 2 || name.len() > 10 || !name.chars().all(|c| c.is_ascii_alphabetic()) {
            return None;
        }
        valid_key(&format!("{name}-{number}"))
    })
}

/// Words that take a number without being a Jira project.
///
/// NEXUS numbers these itself, so "task 3" and "step 2" mean its own rows
/// rather than an issue in someone's tracker.
const NOT_A_PROJECT_KEY: [&str; 24] = [
    "task", "tasks", "project", "projects", "step", "item", "version", "number", "line", "row",
    "option", "window", "point", "part",
    // Verbs, which land next to a number often enough to matter: without
    // these, "open -101" tokenises to ("open", "101") and becomes OPEN-101.
    "open", "show", "go", "read", "find", "view", "browse", "switch", "jump", "launch",
];

/// Verbs that mean "take me to this thing", licensing the spoken form.
const OPEN_VERBS: [&str; 8] = [
    "open", "show", "go", "view", "browse", "launch", "jump", "switch",
];

/// Verbs that introduce a search.
const SEARCH_VERBS: &[&str] = &["search", "google", "look"];

/// Domain endings NEXUS will treat a bare word as a website for.
///
/// A allowlist rather than "anything with a dot", because "version 2.1" and
/// "task 3.4" are not websites and opening a browser for them would be
/// startling.
const TLDS: &[&str] = &[
    "com",
    "org",
    "net",
    "io",
    "dev",
    "ai",
    "co",
    "app",
    "sh",
    "me",
    "gov",
    "edu",
    "info",
    "xyz",
    "cloud",
    "tech",
    "atlassian",
    "github",
];

/// Editors, as words that can be spoken.
///
/// Built from `ides.name`, so a two-word editor name is reachable by either
/// word and a newly registered editor becomes speakable immediately, with no
/// code change and no list here to fall out of step.
fn editor_words(conn: &Connection) -> Vec<(i64, String, Vec<String>)> {
    list_ides(conn, true)
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.executable_path.is_some())
        .map(|entry| {
            let words: Vec<String> = entry
                .name
                .to_lowercase()
                .split_whitespace()
                .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
                .filter(|w| w.len() > 2)
                .collect();
            (entry.id, entry.name, words)
        })
        .collect()
}

fn projects(conn: &Connection) -> Vec<(i64, String)> {
    let mut stmt = match conn.prepare("SELECT id, name FROM projects") {
        Ok(stmt) => stmt,
        Err(_) => return Vec::new(),
    };
    stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })
    .map(|rows| rows.filter_map(|r| r.ok()).collect())
    .unwrap_or_default()
}

/// A project whose name appears among these tokens.
///
/// Longest name first, so "UI-TEST-WORK" is preferred over a project called
/// "UI" when both could match.
fn find_project(tokens: &[String], conn: &Connection) -> Option<(i64, String)> {
    let mut all = projects(conn);
    all.sort_by_key(|(_, name)| std::cmp::Reverse(name.len()));

    all.into_iter().find(|(_, name)| {
        let words: Vec<String> = name
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(|w| w.to_string())
            .collect();
        !words.is_empty() && words.iter().all(|w| tokens.contains(w))
    })
}

/// Turn a spoken domain into a URL NEXUS will open.
///
/// Only http and https ever come out of here, which is the same rule
/// `safe_url` enforces at the connector. Checked twice on purpose: this one
/// keeps a wrong guess from ever becoming an offer.
pub fn as_url(token: &str) -> Option<String> {
    let cleaned = token.trim().trim_end_matches(|c| c == '.' || c == ',');
    if cleaned.starts_with("http://") || cleaned.starts_with("https://") {
        return Some(cleaned.to_string());
    }
    let host = cleaned.split('/').next()?;
    let last = host.rsplit('.').next()?;
    if host.contains('.') && TLDS.contains(&last) && !host.starts_with('.') {
        return Some(format!("https://{cleaned}"));
    }
    None
}

/// Match a phrase that carries its own argument.
///
/// Returns None for anything that is not one of these exact shapes, so the
/// general matcher still sees everything this declines.
pub fn match_phrase(
    tokens: &[String],
    original: &str,
    conn: &Connection,
    is_granted: &dyn Fn(&str, super::permission::Permission) -> bool,
) -> Option<Parametric> {
    use super::permission::Permission;

    // "send a message to Divi saying I'm running late"
    //
    // Runs first: a message body is free text and could contain anything
    // the later shapes look for, including a domain or an issue key.
    if is_granted("whatsapp", Permission::Write) {
        if let Some(found) = whatsapp_message(tokens, original, conn) {
            return Some(found);
        }
    }

    // "move PROJ-387 to done", "set PROJ-387 to in progress"
    //
    // Before the plain issue-key shape below, which would otherwise swallow
    // the whole phrase and open the issue in a browser: the key is the most
    // specific thing in the sentence, so whichever shape looks first wins.
    // Without this, `jira.transition_issue` existed and nothing could say it.
    if is_granted("jira", Permission::Write) {
        if let Some(found) = jira_transition(tokens, original, conn) {
            return Some(found);
        }
    }

    // "open PROJ-101", "open the jira ticket proj 101"
    //
    // Runs before the editor and browser shapes: an issue key is specific
    // enough that nothing else it could be worth guessing at.
    if let Some(key) = issue_key(tokens, original, conn) {
        // A key on its own is a question about it, not a request to leave
        // NEXUS for a browser. After hearing "PROJ-924, ready for team
        // testing" in a morning summary, saying the number back means "tell
        // me about that one"; opening a tab is the thing you ask for out
        // loud with the word "open".
        let asked_to_open = tokens
            .first()
            .is_some_and(|first| OPEN_VERBS.contains(&first.as_str()));

        if !asked_to_open && is_granted("jira", Permission::Read) {
            return Some(Parametric {
                action_id: "jira.read_issue".to_string(),
                input: json!({ "key": key }),
                summary: format!("Read {key}"),
            });
        }
        if is_granted("jira", Permission::Interact) {
            return Some(Parametric {
                action_id: "jira.open_issue".to_string(),
                input: json!({ "key": key }),
                summary: format!("Open {key} in the browser"),
            });
        }
    }

    // "open <project> in <editor>"
    //
    // Split on the joining word so a project and an editor cannot be
    // confused for each other: everything before is the target, everything
    // after is the destination.
    if let Some(split) = tokens.iter().position(|t| IN_WORDS.contains(&t.as_str())) {
        let (before, after) = tokens.split_at(split);
        let after = &after[1..];

        if !after.is_empty() && is_granted("ide", Permission::Interact) {
            if let Some((ide_id, ide_name, _)) = editor_words(conn)
                .into_iter()
                .find(|(_, _, words)| words.iter().any(|w| after.contains(w)))
            {
                if let Some((project_id, project_name)) = find_project(before, conn) {
                    return Some(Parametric {
                        action_id: "ide.open_project".to_string(),
                        input: json!({ "ideId": ide_id, "projectId": project_id }),
                        summary: format!("Open {project_name} in {ide_name}"),
                    });
                }
            }
        }
    }

    // "switch to the <name> tab" / "go to the jira tab"
    //
    // By name rather than by index, because nobody knows which number their
    // Jira tab is, and the number changes the moment a tab opens or closes.
    if is_granted("browser", Permission::Interact) {
        // A switch verb has to appear *before* the tab word. Requiring it at
        // all keeps "list my tabs" from reading as focusing a tab called
        // "list"; requiring it to come first keeps "what tabs are open" from
        // being caught by the "open" in "are open".
        if let Some(tab_at) = tokens
            .iter()
            .position(|t| t == "tab" || t == "tabs")
            .filter(|at| {
                tokens[..*at]
                    .iter()
                    .any(|t| SWITCH_VERBS.contains(&t.as_str()))
            })
        {
            let name: Vec<&str> = tokens[..tab_at]
                .iter()
                .map(|s| s.as_str())
                .filter(|w| {
                    !SWITCH_VERBS.contains(w) && !BARE_REFERENCE_FILLER.contains(w) && *w != "my"
                })
                .collect();
            if !name.is_empty() {
                let query = name.join(" ");
                return Some(Parametric {
                    action_id: "browser.focus_tab".to_string(),
                    input: json!({ "query": query }),
                    summary: format!("Switch to the \"{query}\" tab"),
                });
            }
        }
    }

    // "type <words>" / "dictate <words>"
    //
    // Everything after the verb is the text, verbatim from the original
    // rather than the normalised tokens, so punctuation and capitals survive
    // into the box.
    if is_granted("browser", Permission::Execute) {
        let words: Vec<&str> = original.split_whitespace().collect();
        if let Some(at) = words.iter().position(|w| {
            DICTATE_VERBS.contains(
                &w.to_lowercase()
                    .trim_matches(|c: char| !c.is_alphanumeric()),
            )
        }) {
            let rest: Vec<&str> = words[at + 1..]
                .iter()
                .copied()
                .skip_while(|w| {
                    let lower = w.to_lowercase();
                    lower == "in" || lower == "into" || lower == "this" || lower == "out"
                })
                .collect();
            if !rest.is_empty() {
                let text = rest.join(" ");
                return Some(Parametric {
                    action_id: "browser.type_here".to_string(),
                    input: json!({ "text": text }),
                    summary: format!("Type \"{text}\" into the focused box"),
                });
            }
        }
    }

    // "search for <words>" / "google <words>"
    if is_granted("browser", Permission::Interact) {
        if let Some(index) = tokens
            .iter()
            .position(|t| SEARCH_VERBS.contains(&t.as_str()))
        {
            let query: Vec<&str> = tokens[index + 1..]
                .iter()
                .map(|s| s.as_str())
                .filter(|w| *w != "for" && *w != "up")
                .collect();
            if !query.is_empty() {
                let joined = query.join(" ");
                return Some(Parametric {
                    action_id: "browser.search".to_string(),
                    input: json!({ "query": joined }),
                    summary: format!("Search the web for \"{joined}\""),
                });
            }
        }

        // A spoken website: "open github.com", "go to your-team.atlassian.net"
        //
        // Uses the original text rather than the normalised tokens, because
        // normalising strips the dots that make a domain a domain.
        for word in original.split_whitespace() {
            if let Some(url) = as_url(&word.to_lowercase()) {
                return Some(Parametric {
                    action_id: "browser.open_url".to_string(),
                    input: json!({ "url": url }),
                    summary: format!("Open {url} in Chrome"),
                });
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assistant::permission::Permission;
    use crate::db::migrations::MIGRATIONS;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch("PRAGMA foreign_keys = ON;").expect("fk");
        for (_, sql) in MIGRATIONS {
            conn.execute_batch(sql).expect("migrate");
        }
        conn.execute(
            "INSERT INTO ides (name, ide_type, executable_path, enabled) VALUES
                ('IntelliJ IDEA','editor','/bin/echo',1),
                ('Visual Studio Code','editor','/bin/echo',1),
                ('Cursor','editor','/bin/echo',1)",
            [],
        )
        .expect("seed ides");
        conn.execute(
            "INSERT INTO projects (name) VALUES ('ALPHA'),('AdminService'),('UI TEST WORK')",
            [],
        )
        .expect("seed projects");
        conn
    }

    fn allow_all(_: &str, _: Permission) -> bool {
        true
    }
    fn allow_none(_: &str, _: Permission) -> bool {
        false
    }

    fn tokens(text: &str) -> Vec<String> {
        text.to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { ' ' })
            .collect::<String>()
            .split_whitespace()
            .map(String::from)
            .collect()
    }

    fn matched(text: &str, conn: &Connection) -> Option<Parametric> {
        match_phrase(&tokens(text), text, conn, &allow_all)
    }

    // -- Editors --------------------------------------------------------------

    #[test]
    fn open_a_project_in_each_registered_editor() {
        let conn = test_conn();
        for (phrase, editor) in [
            ("open ALPHA in IntelliJ", "IntelliJ IDEA"),
            ("open ALPHA in idea", "IntelliJ IDEA"),
            ("open ALPHA in code", "Visual Studio Code"),
            ("open ALPHA in Cursor", "Cursor"),
            ("open ALPHA with Cursor", "Cursor"),
        ] {
            let found = matched(phrase, &conn).unwrap_or_else(|| panic!("{phrase}"));
            assert_eq!(found.action_id, "ide.open_project", "{phrase}");
            assert!(
                found.summary.contains(editor),
                "{phrase} -> {}",
                found.summary
            );
            assert!(found.summary.contains("ALPHA"), "{phrase}");
        }
    }

    #[test]
    fn a_multi_word_project_name_still_matches() {
        let conn = test_conn();
        let found = matched("open UI TEST WORK in cursor", &conn).expect("matched");
        assert!(found.summary.contains("UI TEST WORK"), "{}", found.summary);
    }

    #[test]
    fn an_unregistered_editor_does_not_match() {
        // Editors come from the registry, so one that is not registered is
        // simply not speakable. No hardcoded list to fall out of step.
        let conn = test_conn();
        assert!(matched("open ALPHA in emacs", &conn).is_none());
    }

    #[test]
    fn a_project_that_does_not_exist_does_not_match() {
        let conn = test_conn();
        assert!(matched("open Atlantis in IntelliJ", &conn).is_none());
    }

    #[test]
    fn the_joining_word_keeps_project_and_editor_apart() {
        // Without the split, "open code in ALPHA" would resolve the same as
        // "open ALPHA in code".
        let conn = test_conn();
        assert!(matched("open code in ALPHA", &conn).is_none());
    }

    // -- WhatsApp messages -----------------------------------------------------

    fn with_contacts(conn: &Connection) {
        for (name, phone) in [("Divi", "+919876543210"), ("Divya Raj", "+919000000000")] {
            crate::db::contacts::create_contact(
                conn,
                &crate::db::contacts::ContactInput {
                    id: None,
                    name: name.to_string(),
                    phone: phone.to_string(),
                },
            )
            .expect("seed contact");
        }
    }

    #[test]
    fn a_message_to_a_known_contact_carries_the_number_and_the_words() {
        let conn = test_conn();
        with_contacts(&conn);
        let found = matched("send a message to Divi saying I am running late", &conn)
            .expect("matched");
        assert_eq!(found.action_id, "whatsapp.compose_message");
        assert_eq!(found.input["phone"], "919876543210");
        assert_eq!(found.input["message"], "I am running late");
        assert_eq!(found.input["displayName"], "Divi");
    }

    #[test]
    fn the_message_keeps_the_users_own_words() {
        // The body is dictation, not a search term: capitals, punctuation
        // and apostrophes are exactly what the recipient should see.
        let conn = test_conn();
        with_contacts(&conn);
        let found = matched("message Divi saying Can't make it, sorry!", &conn).expect("matched");
        assert_eq!(found.input["message"], "Can't make it, sorry!");
    }

    #[test]
    fn the_longest_matching_name_wins() {
        // "Divya Raj" contains no "Divi", but a shorter name that is a
        // prefix of a longer one must not steal the longer one's messages.
        let conn = test_conn();
        with_contacts(&conn);
        let found = matched("message Divya Raj saying hello", &conn).expect("matched");
        assert_eq!(found.input["displayName"], "Divya Raj");
        assert_eq!(found.input["phone"], "919000000000");
    }

    #[test]
    fn a_message_with_no_body_opens_the_chat() {
        let conn = test_conn();
        with_contacts(&conn);
        let found = matched("send a message to Divi", &conn).expect("matched");
        assert_eq!(found.action_id, "whatsapp.compose_message");
        assert_eq!(found.input["message"], "");
        assert!(found.summary.contains("Open a WhatsApp chat"), "{}", found.summary);
    }

    #[test]
    fn an_unknown_name_is_not_guessed_at() {
        // Messaging the wrong person cannot be undone, so a name NEXUS has
        // never been given resolves to nothing at all.
        let conn = test_conn();
        with_contacts(&conn);
        for phrase in [
            "send a message to Divi",
            "send a message to Rajesh saying hello",
            "message the team saying stand up in five",
        ] {
            let found = matched(phrase, &conn);
            let known = found
                .as_ref()
                .is_some_and(|f| f.action_id == "whatsapp.compose_message");
            assert_eq!(known, phrase.contains("Divi"), "{phrase} -> {found:?}");
        }
    }

    #[test]
    fn a_phrase_that_is_not_about_messaging_is_left_alone() {
        let conn = test_conn();
        with_contacts(&conn);
        for phrase in ["what is Divi working on", "open Divi", "Divi"] {
            let found = matched(phrase, &conn);
            assert!(
                found.as_ref().is_none_or(|f| f.action_id != "whatsapp.compose_message"),
                "{phrase} -> {found:?}"
            );
        }
    }

    #[test]
    fn messaging_needs_write_permission() {
        let conn = test_conn();
        with_contacts(&conn);
        let read_only = |_: &str, level: Permission| level <= Permission::Interact;
        let phrase = "send a message to Divi saying hello";
        let found = match_phrase(&tokens(phrase), phrase, &conn, &read_only);
        assert!(found.as_ref().is_none_or(|f| f.action_id != "whatsapp.compose_message"));
    }

    // -- Jira issue keys -------------------------------------------------------

    #[test]
    fn a_bare_key_is_a_question_about_it() {
        // Saying a number back after hearing a morning summary means "tell
        // me about that one". Opening a browser tab is what you ask for out
        // loud, with the word "open".
        let conn = test_conn();
        for phrase in ["PROJ-1069", "what is PROJ-1069 about", "PROJ-1069 status"] {
            let found = matched(phrase, &conn).unwrap_or_else(|| panic!("{phrase}"));
            assert_eq!(found.action_id, "jira.read_issue", "{phrase}");
            assert_eq!(found.input["key"], "PROJ-1069", "{phrase}");
        }
    }

    #[test]
    fn asking_to_open_a_key_still_opens_it() {
        let conn = test_conn();
        for phrase in ["open PROJ-101", "open jira PROJ-101 ticket", "show PROJ-101"] {
            let found = matched(phrase, &conn).unwrap_or_else(|| panic!("{phrase}"));
            assert_eq!(found.action_id, "jira.open_issue", "{phrase}");
        }
    }

    #[test]
    fn a_written_key_is_recognised_anywhere_in_the_phrase() {
        let conn = test_conn();
        // Both actions are correct answers here; what this pins is that the
        // *key* is found wherever it sits in the sentence. Which action it
        // becomes is the previous two tests' business.
        for phrase in [
            "open PROJ-101",
            "open jira PROJ-101 ticket",
            "show me proj-101",
            "what is PROJ-1069 about",
            "open PROJ-101, please",
        ] {
            let found = matched(phrase, &conn).unwrap_or_else(|| panic!("{phrase}"));
            assert!(
                found.action_id.starts_with("jira."),
                "{phrase} -> {}",
                found.action_id
            );
            assert!(found.input["key"].as_str().unwrap().starts_with("PROJ-"), "{phrase}");
        }
    }

    #[test]
    fn the_key_is_normalised_the_way_the_connector_expects() {
        // Uppercased here so the connector is handed exactly what it would
        // have produced itself. A mismatch would only show up as a 404.
        let conn = test_conn();
        let found = matched("open proj-101", &conn).expect("matched");
        assert_eq!(found.input["key"], "PROJ-101");
    }

    #[test]
    fn a_spoken_key_needs_the_phrase_to_point_at_it() {
        // This asserted the opposite when it was written: the spoken form
        // then required the phrase to say "jira". That rejected "open
        // PROJ-101 in the browser", which is the wording NEXUS prints back
        // after resolving one, so people echoed it and got a menu of Chrome
        // actions instead. The phrase now qualifies either by naming the
        // kind of thing or by opening it.
        let conn = test_conn();
        for phrase in [
            "open the jira ticket proj 101",
            "open proj 101",
            "open proj 101 in the browser",
            "show proj 1069",
            "go to proj 101",
        ] {
            let found = matched(phrase, &conn).unwrap_or_else(|| panic!("{phrase}"));
            assert_eq!(found.action_id, "jira.open_issue", "{phrase}");
            assert!(
                found.input["key"].as_str().unwrap().starts_with("PROJ-"),
                "{phrase} -> {found:?}"
            );
        }
    }

    #[test]
    fn a_name_beside_a_number_is_not_automatically_a_key() {
        // Any two words next to a number look like a spoken key. These are
        // the false positives the rule exists to exclude: NEXUS's own
        // numbered things, a project it knows, a verb that tokenised next
        // to a digit, and ordinary speech about quantities.
        let conn = test_conn();
        for phrase in [
            "open task 3",
            "open project 2",
            "open step 4",
            "show me alpha 2",
            "it took 1.5 hours",
            "we need 3 more",
            "open -101",
            "set version 2.1",
        ] {
            let found = matched(phrase, &conn);
            assert!(
                found.as_ref().is_none_or(|f| f.action_id != "jira.open_issue"),
                "{phrase} -> {found:?}"
            );
        }
    }

    #[test]
    fn a_malformed_key_is_not_a_key() {
        let conn = test_conn();
        for phrase in ["open -101", "open PROJ-", "open PROJ-abc", "open 101-KAI"] {
            let found = matched(phrase, &conn);
            assert!(
                found.as_ref().is_none_or(|f| f.action_id != "jira.open_issue"),
                "{phrase} -> {found:?}"
            );
        }
    }

    #[test]
    fn an_issue_key_is_not_mistaken_for_a_website() {
        // The browser shape runs later and would otherwise see nothing, but
        // this pins the ordering rather than leaving it to chance.
        let conn = test_conn();
        let found = matched("open PROJ-101", &conn).expect("matched");
        assert_eq!(found.action_id, "jira.open_issue");
    }

    #[test]
    fn nothing_jira_is_offered_without_permission() {
        let conn = test_conn();
        let no_jira = |id: &str, _: Permission| id != "jira";
        let found = match_phrase(&tokens("open PROJ-101"), "open PROJ-101", &conn, &no_jira);
        assert!(found.as_ref().is_none_or(|f| f.action_id != "jira.open_issue"));
    }

    // -- Browser --------------------------------------------------------------

    #[test]
    fn a_spoken_domain_becomes_an_https_url() {
        let conn = test_conn();
        for (phrase, expected) in [
            ("open github.com", "https://github.com"),
            ("go to your-team.atlassian.net", "https://your-team.atlassian.net"),
            ("open https://example.com/a", "https://example.com/a"),
        ] {
            let found = matched(phrase, &conn).unwrap_or_else(|| panic!("{phrase}"));
            assert_eq!(found.action_id, "browser.open_url", "{phrase}");
            assert_eq!(found.input["url"], expected, "{phrase}");
        }
    }

    #[test]
    fn a_number_with_a_dot_is_not_a_website() {
        // "version 2.1" and "task 3.4" must not open a browser.
        let conn = test_conn();
        for phrase in ["set version 2.1", "task 3.4 is done", "it took 1.5 hours"] {
            assert!(matched(phrase, &conn).is_none(), "{phrase}");
        }
    }

    #[test]
    fn only_http_and_https_ever_come_out() {
        assert_eq!(as_url("javascript:alert(1)"), None);
        assert_eq!(as_url("file:///etc/passwd"), None);
        assert_eq!(as_url("data:text/html,x"), None);
        assert!(as_url("github.com").expect("ok").starts_with("https://"));
    }

    #[test]
    fn dictation_carries_the_words_verbatim() {
        // Punctuation and capitals must survive into the box: this is the
        // user's sentence, not a search term.
        let conn = test_conn();
        for (phrase, expected) in [
            (
                "type Hello there, how are you?",
                "Hello there, how are you?",
            ),
            ("dictate Fix the login bug", "Fix the login bug"),
            ("type into Summarise this PR", "Summarise this PR"),
        ] {
            let found = matched(phrase, &conn).unwrap_or_else(|| panic!("{phrase}"));
            assert_eq!(found.action_id, "browser.type_here", "{phrase}");
            assert_eq!(found.input["text"], expected, "{phrase}");
        }
    }

    #[test]
    fn dictation_wins_over_search_when_both_words_appear() {
        // "type search engines are useful" is dictation, not a request to
        // search for "engines are useful".
        let conn = test_conn();
        let found = matched("type search engines are useful", &conn).expect("matched");
        assert_eq!(found.action_id, "browser.type_here");
        assert_eq!(found.input["text"], "search engines are useful");
    }

    #[test]
    fn a_bare_dictation_verb_matches_nothing() {
        let conn = test_conn();
        assert!(matched("type", &conn).is_none());
        assert!(matched("type into this", &conn).is_none());
    }

    #[test]
    fn dictation_needs_execute_permission() {
        // Typing into a page is Execute level, so Interact alone is not
        // enough to have it offered.
        let conn = test_conn();
        let interact_only = |_: &str, level: Permission| level <= Permission::Interact;
        assert!(match_phrase(
            &tokens("type hello there"),
            "type hello there",
            &conn,
            &interact_only
        )
        .is_none());
    }

    #[test]
    fn a_search_carries_its_query() {
        let conn = test_conn();
        for phrase in [
            "search for rust async",
            "google rust async",
            "search rust async",
        ] {
            let found = matched(phrase, &conn).unwrap_or_else(|| panic!("{phrase}"));
            assert_eq!(found.action_id, "browser.search", "{phrase}");
            assert_eq!(found.input["query"], "rust async", "{phrase}");
        }
    }

    #[test]
    fn a_bare_search_verb_matches_nothing() {
        let conn = test_conn();
        assert!(matched("search", &conn).is_none());
        assert!(matched("search for", &conn).is_none());
    }

    // -- Permission ------------------------------------------------------------

    #[test]
    fn nothing_is_offered_for_a_connector_that_is_not_allowed() {
        // Offering something the gate would refuse teaches the user that
        // NEXUS suggests things that do not work.
        let conn = test_conn();
        for phrase in [
            "open ALPHA in IntelliJ",
            "open github.com",
            "search for rust",
        ] {
            assert!(
                match_phrase(&tokens(phrase), phrase, &conn, &allow_none).is_none(),
                "{phrase}"
            );
        }
    }

    // -- Declining -------------------------------------------------------------

    #[test]
    fn ordinary_requests_fall_through_untouched() {
        // Anything this declines is still seen by the general matcher.
        let conn = test_conn();
        for phrase in [
            "open settings",
            "show my blocked tasks",
            "hi",
            "what am I working on",
            "list my tabs",
        ] {
            assert!(matched(phrase, &conn).is_none(), "{phrase}");
        }
    }

    #[test]
    fn switching_to_a_tab_is_by_name_not_by_number() {
        let conn = test_conn();
        for (phrase, expected) in [
            ("switch to the jira tab", "jira"),
            ("go to the claude tab", "claude"),
            ("open the github tab", "github"),
            ("focus the pull request tab", "pull request"),
        ] {
            let found = matched(phrase, &conn).unwrap_or_else(|| panic!("{phrase}"));
            assert_eq!(found.action_id, "browser.focus_tab", "{phrase}");
            assert_eq!(found.input["query"], expected, "{phrase}");
        }
    }

    #[test]
    fn listing_tabs_is_not_mistaken_for_switching_to_one() {
        // "list my tabs" would otherwise read as focusing a tab called
        // "list", which is a guess dressed as an answer.
        let conn = test_conn();
        for phrase in ["list my tabs", "what tabs are open", "how many tabs"] {
            assert!(matched(phrase, &conn).is_none(), "{phrase}");
        }
    }

    #[test]
    fn a_switch_verb_with_no_name_matches_nothing() {
        let conn = test_conn();
        assert!(matched("switch to the tab", &conn).is_none());
        assert!(matched("go to my tabs", &conn).is_none());
    }

    #[test]
    fn matching_is_pure_and_repeatable() {
        let conn = test_conn();
        let first = matched("open ALPHA in IntelliJ", &conn);
        for _ in 0..10 {
            assert_eq!(matched("open ALPHA in IntelliJ", &conn), first);
        }
    }

    #[test]
    fn no_connector_is_named_in_this_module() {
        // Editors and projects come from the database. A hardcoded editor
        // list would go stale the moment one is registered.
        let production = include_str!("parametric.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("marker");
        for hardcoded in ["IntelliJ", "Visual Studio", "Cursor", "PyCharm"] {
            assert!(!production.contains(hardcoded), "found {hardcoded}");
        }
    }
}
