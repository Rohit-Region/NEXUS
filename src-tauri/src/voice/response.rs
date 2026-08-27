//! NEXUS-011: what NEXUS says back.
//!
//! Deterministic response templates. This module is the whole "language
//! model" of the voice feature, and it is a `match` statement.
//!
//! Three properties are load-bearing:
//!
//! 1. **Keyed by outcome, never by transcript.** [`VoiceOutcome`] has no field
//!    that can carry recognised speech. A phrase the recognizer heard cannot
//!    reach the speaker even by accident, because there is no channel for it.
//!    The only free text is a project name, and that is read from the database
//!    row that was actually opened.
//! 2. **Pure.** No I/O, no clock, no state. The same outcome always produces
//!    the same sentence, which is what makes it testable.
//! 3. **Spoken only after the fact.** The caller constructs an outcome once a
//!    command has executed, or once a match has definitively failed. Nothing
//!    here decides whether to speak; it only decides what the words are.

use serde::{Deserialize, Serialize};

/// Longest project name spoken aloud. A pathological name should not turn
/// into a minute of speech; the sentence is still correct when clipped.
const MAX_SPOKEN_NAME: usize = 60;

/// What happened, in terms NEXUS can describe.
///
/// Serialised with an internal `kind` tag so the TypeScript side constructs it
/// as a discriminated union and the compiler rejects a malformed outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum VoiceOutcome {
    /// A palette command ran. `command_id` is the registry id from
    /// src/lib/commands.ts, which stays the single source of truth.
    #[serde(rename_all = "camelCase")]
    Executed {
        command_id: String,
        /// Only for project-scoped commands. From the database, not speech.
        project_name: Option<String>,
    },
    /// A search result was opened rather than a command run.
    #[serde(rename_all = "camelCase")]
    OpenedProject { project_name: String },
    /// The matcher produced nothing. Definitive: the user is owed an answer.
    NoMatch,
    /// Execution was attempted and failed.
    Failed,
    /// The user dismissed the palette without choosing anything.
    Cancelled,
}

/// Collapse whitespace and clip, so a name reads as one spoken phrase.
fn speakable(name: &str) -> Option<String> {
    let collapsed = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    if collapsed.chars().count() <= MAX_SPOKEN_NAME {
        return Some(collapsed);
    }
    Some(collapsed.chars().take(MAX_SPOKEN_NAME).collect())
}

/// The sentence NEXUS speaks for an outcome.
///
/// Every branch is a fixed string with at most a project name interpolated.
pub fn response_for(outcome: &VoiceOutcome) -> String {
    match outcome {
        VoiceOutcome::Executed {
            command_id,
            project_name,
        } => executed_response(command_id, project_name.as_deref()),

        VoiceOutcome::OpenedProject { project_name } => match speakable(project_name) {
            Some(name) => format!("Opening {name}."),
            None => "Opening the project.".to_string(),
        },

        // Says what failed and what to do, without repeating what was heard.
        // The palette is already showing the transcript on screen; saying it
        // back adds nothing and would put recognised speech into the audio
        // path, which is exactly what this module refuses to do.
        VoiceOutcome::NoMatch => {
            "Sorry, that isn't a command I know. It's on screen if you'd like to edit it."
                .to_string()
        }

        VoiceOutcome::Failed => "Sorry, that didn't work.".to_string(),

        VoiceOutcome::Cancelled => "Cancelled.".to_string(),
    }
}

/// Templates for executed commands, keyed by registry id.
///
/// The `create-task-` arm matches on prefix because those ids are generated
/// per project (spec 009 2.3). The project id in the suffix is deliberately
/// not parsed: the name arrives alongside, already resolved by the caller.
fn executed_response(command_id: &str, project_name: Option<&str>) -> String {
    match command_id {
        "nav-overview" => "Opening Overview.".to_string(),
        "nav-projects" => "Opening Projects.".to_string(),
        "nav-registry" => "Opening Registry.".to_string(),
        "nav-settings" => "Opening Settings.".to_string(),
        "create-project" => "Opening a new project.".to_string(),
        id if id.starts_with("create-task-") => {
            match project_name.and_then(speakable) {
                Some(name) => format!("Opening a new task in {name}."),
                None => "Opening a new task.".to_string(),
            }
        }
        // A command added later without a template still gets an
        // acknowledgement rather than silence. Silence is indistinguishable
        // from a broken microphone, which is the failure this milestone exists
        // to remove.
        _ => "Done.".to_string(),
    }
}

// -- Tests -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn executed(id: &str) -> VoiceOutcome {
        VoiceOutcome::Executed {
            command_id: id.to_string(),
            project_name: None,
        }
    }

    fn executed_in(id: &str, project: &str) -> VoiceOutcome {
        VoiceOutcome::Executed {
            command_id: id.to_string(),
            project_name: Some(project.to_string()),
        }
    }

    #[test]
    fn every_navigation_command_names_its_destination() {
        for (id, expected) in [
            ("nav-overview", "Opening Overview."),
            ("nav-projects", "Opening Projects."),
            ("nav-registry", "Opening Registry."),
            ("nav-settings", "Opening Settings."),
        ] {
            assert_eq!(response_for(&executed(id)), expected, "id {id}");
        }
    }

    #[test]
    fn every_registry_command_id_has_a_specific_template() {
        // Guards against a command being added to src/lib/commands.ts without
        // a matching template: the generic fallback must stay unreachable for
        // ids that ship today.
        let shipped = [
            "nav-overview",
            "nav-projects",
            "nav-registry",
            "nav-settings",
            "create-project",
            "create-task-1",
        ];
        for id in shipped {
            assert_ne!(
                response_for(&executed(id)),
                "Done.",
                "{id} fell through to the generic template"
            );
        }
    }

    #[test]
    fn task_creation_names_the_project() {
        assert_eq!(
            response_for(&executed_in("create-task-42", "NEXUS")),
            "Opening a new task in NEXUS."
        );
    }

    #[test]
    fn task_creation_without_a_project_name_still_reads_correctly() {
        assert_eq!(
            response_for(&executed("create-task-42")),
            "Opening a new task."
        );
    }

    #[test]
    fn project_id_in_the_suffix_is_never_spoken() {
        let spoken = response_for(&executed_in("create-task-9137", "Atlas"));
        assert!(
            !spoken.contains("9137"),
            "database id leaked into speech: {spoken}"
        );
    }

    #[test]
    fn opened_project_names_the_project() {
        assert_eq!(
            response_for(&VoiceOutcome::OpenedProject {
                project_name: "Atlas".to_string(),
            }),
            "Opening Atlas."
        );
    }

    #[test]
    fn terminal_outcomes_have_fixed_wording() {
        assert_eq!(response_for(&VoiceOutcome::Cancelled), "Cancelled.");
        assert_eq!(response_for(&VoiceOutcome::Failed), "Sorry, that didn't work.");
        assert!(response_for(&VoiceOutcome::NoMatch).starts_with("Sorry,"));
    }

    #[test]
    fn no_match_does_not_repeat_what_was_heard() {
        // The type has no transcript field, so this asserts the design rather
        // than the string: a template can only speak what it is given.
        let spoken = response_for(&VoiceOutcome::NoMatch);
        assert!(!spoken.contains('"'), "quoted speech implies a transcript");
    }

    #[test]
    fn templates_are_pure() {
        let outcome = executed_in("create-task-1", "Atlas");
        let first = response_for(&outcome);
        for _ in 0..10 {
            assert_eq!(response_for(&outcome), first);
        }
    }

    #[test]
    fn whitespace_in_a_project_name_is_collapsed() {
        assert_eq!(
            response_for(&VoiceOutcome::OpenedProject {
                project_name: "  Atlas   Core \n".to_string(),
            }),
            "Opening Atlas Core."
        );
    }

    #[test]
    fn a_blank_project_name_falls_back_to_a_generic_phrase() {
        assert_eq!(
            response_for(&VoiceOutcome::OpenedProject {
                project_name: "   ".to_string(),
            }),
            "Opening the project."
        );
        assert_eq!(
            response_for(&executed_in("create-task-1", "  ")),
            "Opening a new task."
        );
    }

    #[test]
    fn a_pathological_project_name_is_clipped() {
        let long = "A".repeat(500);
        let spoken = response_for(&VoiceOutcome::OpenedProject {
            project_name: long,
        });
        assert!(
            spoken.chars().count() <= MAX_SPOKEN_NAME + 16,
            "unclipped name: {} chars",
            spoken.chars().count()
        );
    }

    #[test]
    fn a_multibyte_project_name_is_clipped_on_character_boundaries() {
        // Clipping by byte would panic here. Names are user data.
        let spoken = response_for(&VoiceOutcome::OpenedProject {
            project_name: "நெக்சஸ்".repeat(40),
        });
        assert!(spoken.starts_with("Opening "));
    }

    #[test]
    fn outcomes_round_trip_as_a_tagged_union() {
        let json = serde_json::to_string(&executed_in("create-task-1", "Atlas"))
            .expect("serialize");
        assert!(json.contains("\"kind\":\"executed\""), "{json}");
        assert!(json.contains("\"commandId\""), "{json}");
        let back: VoiceOutcome = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, executed_in("create-task-1", "Atlas"));
    }

    #[test]
    fn unit_outcomes_serialise_by_tag_alone() {
        let json = serde_json::to_string(&VoiceOutcome::NoMatch).expect("serialize");
        assert_eq!(json, "{\"kind\":\"noMatch\"}");
    }
}
