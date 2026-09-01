//! NEXUS-020: the suggestion engine.
//!
//! Rules over data NEXUS already holds. No provider is involved and none is
//! reachable from here: every suggestion below is a query plus a sentence.
//! That is not a limitation to be lifted later, it is the point. A suggestion
//! that needed a model would stop appearing on a train, and the useful ones
//! are joins the workspace schema was already shaped for.
//!
//! Three rules the whole engine obeys:
//!
//! 1. **Nothing executes.** A suggestion carries a *proposed* action, and the
//!    user accepting it runs that action through the gate like any other.
//!    Nothing in this file can reach a connector.
//! 2. **Every suggestion says why.** The reason is structured data, not a
//!    generated explanation, so the UI can always answer "why am I seeing
//!    this" truthfully.
//! 3. **Suggestions are derived, never stored.** Only dismissals persist. A
//!    suggestion about a deleted project simply stops being generated.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// How much a suggestion wants attention. Ordering only; nothing acts on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Priority {
    Low,
    Normal,
    High,
}

/// Why NEXUS raised this, as data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reason {
    /// The rule that fired, as a stable id.
    pub rule: String,
    /// A sentence the UI can show verbatim.
    pub explanation: String,
    /// What the rule looked at: project names, task titles, counts.
    pub evidence: Vec<String>,
}

/// Something NEXUS thinks might be worth doing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    /// Stable across regeneration, so dismissing one dismisses it for good.
    /// Derived from the rule and its subject, never from a counter.
    pub key: String,
    pub title: String,
    pub priority: Priority,
    pub reason: Reason,
    /// The action the user would run if they accept. Not executed here.
    pub action_id: String,
    pub action_input: serde_json::Value,
    /// What the button should say.
    pub accept_label: String,
}

/// A task that has not moved in this long is worth a mention.
const STALE_DAYS: i64 = 14;
/// Never raise more than this at once. A wall of suggestions is noise, and
/// noise is how a proactive assistant trains you to ignore it.
pub const MAX_SUGGESTIONS: usize = 6;

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |row| row.get::<_, i64>(0))
        .unwrap_or(0)
}

/// Blocked work, which is the thing most worth surfacing in a task tool.
fn blocked_tasks(conn: &Connection) -> Option<Suggestion> {
    let total = count(conn, "SELECT COUNT(*) FROM tasks WHERE status = 'blocked'");
    if total == 0 {
        return None;
    }

    let mut stmt = conn
        .prepare(
            "SELECT t.title, p.name
               FROM tasks t JOIN projects p ON p.id = t.project_id
              WHERE t.status = 'blocked'
              ORDER BY t.updated_at DESC LIMIT 3",
        )
        .ok()?;
    let evidence: Vec<String> = stmt
        .query_map([], |row| {
            Ok(format!(
                "{} ({})",
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?
            ))
        })
        .ok()?
        .filter_map(|r| r.ok())
        .collect();

    Some(Suggestion {
        key: "blocked-tasks".to_string(),
        title: if total == 1 {
            "1 task is blocked".to_string()
        } else {
            format!("{total} tasks are blocked")
        },
        priority: if total >= 3 {
            Priority::High
        } else {
            Priority::Normal
        },
        reason: Reason {
            rule: "blocked-tasks".to_string(),
            explanation: "Blocked work does not move on its own.".to_string(),
            evidence,
        },
        action_id: "nexus.open_projects".to_string(),
        action_input: serde_json::Value::Null,
        accept_label: "Review them".to_string(),
    })
}

/// A task nobody has touched in a fortnight.
fn stale_tasks(conn: &Connection) -> Option<Suggestion> {
    let total = count(
        conn,
        &format!(
            "SELECT COUNT(*) FROM tasks
              WHERE status = 'open'
                AND updated_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-{STALE_DAYS} days')"
        ),
    );
    if total == 0 {
        return None;
    }
    Some(Suggestion {
        key: "stale-tasks".to_string(),
        title: format!(
            "{total} open task{} untouched for {STALE_DAYS} days",
            if total == 1 { "" } else { "s" }
        ),
        priority: Priority::Low,
        reason: Reason {
            rule: "stale-tasks".to_string(),
            explanation: format!(
                "These have not changed in over {STALE_DAYS} days, so they may be done, \
                 blocked, or no longer wanted."
            ),
            evidence: vec![format!("{total} open tasks")],
        },
        action_id: "nexus.open_projects".to_string(),
        action_input: serde_json::Value::Null,
        accept_label: "Look at them".to_string(),
    })
}

/// A project NEXUS cannot open in an editor, because nobody said where it is.
fn projects_without_a_path(conn: &Connection) -> Vec<Suggestion> {
    let mut stmt = match conn.prepare(
        "SELECT id, name FROM projects
          WHERE repository_path IS NULL OR trim(repository_path) = ''
          ORDER BY updated_at DESC LIMIT 2",
    ) {
        Ok(stmt) => stmt,
        Err(_) => return Vec::new(),
    };

    stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })
    .map(|rows| {
        rows.filter_map(|r| r.ok())
            .map(|(id, name)| Suggestion {
                // Keyed by the project, so dismissing one does not silence
                // the rule for every other project.
                key: format!("project-path:{id}"),
                title: format!("{name} has no folder on disk"),
                priority: Priority::Low,
                reason: Reason {
                    rule: "project-missing-path".to_string(),
                    explanation: "Without a folder, NEXUS cannot open this project in an editor."
                        .to_string(),
                    evidence: vec![name.clone()],
                },
                action_id: "nexus.open_project".to_string(),
                action_input: serde_json::json!({ "projectId": id }),
                accept_label: "Add one".to_string(),
            })
            .collect()
    })
    .unwrap_or_default()
}

/// A project pointing at a repository NEXUS could be reading pull requests
/// from, which is the join the schema was shaped for.
fn projects_with_a_repo(conn: &Connection) -> Vec<Suggestion> {
    let mut stmt = match conn.prepare(
        "SELECT id, name FROM projects
          WHERE repository_url IS NOT NULL AND trim(repository_url) != ''
          ORDER BY updated_at DESC LIMIT 2",
    ) {
        Ok(stmt) => stmt,
        Err(_) => return Vec::new(),
    };

    stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })
    .map(|rows| {
        rows.filter_map(|r| r.ok())
            .map(|(id, name)| Suggestion {
                key: format!("check-prs:{id}"),
                title: format!("Check open pull requests for {name}"),
                priority: Priority::Normal,
                reason: Reason {
                    rule: "project-has-repository".to_string(),
                    explanation: format!(
                        "{name} points at a GitHub repository, so NEXUS can read its \
                             pull requests and their checks."
                    ),
                    evidence: vec![name.clone()],
                },
                action_id: "github.list_prs".to_string(),
                action_input: serde_json::json!({ "projectId": id }),
                accept_label: "Show them".to_string(),
            })
            .collect()
    })
    .unwrap_or_default()
}

/// An editor registered at a path that is no longer there.
fn broken_editors(conn: &Connection) -> Option<Suggestion> {
    let mut stmt = conn
        .prepare("SELECT name, executable_path FROM ides WHERE enabled = 1")
        .ok()?;
    let broken: Vec<String> = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .ok()?
        .filter_map(|r| r.ok())
        .filter(|(_, path)| match path {
            Some(path) => !std::path::Path::new(path).exists(),
            None => true,
        })
        .map(|(name, _)| name)
        .collect();

    if broken.is_empty() {
        return None;
    }
    Some(Suggestion {
        key: "broken-editors".to_string(),
        title: format!(
            "{} registered editor{} cannot be found",
            broken.len(),
            if broken.len() == 1 { "" } else { "s" }
        ),
        priority: Priority::Normal,
        reason: Reason {
            rule: "editor-path-missing".to_string(),
            explanation:
                "These are registered at paths that do not exist, so opening a project in \
                 them would fail."
                    .to_string(),
            evidence: broken,
        },
        action_id: "ide.discover".to_string(),
        action_input: serde_json::Value::Null,
        accept_label: "Find installed editors".to_string(),
    })
}

/// Which suggestions the user has told NEXUS to stop raising.
fn dismissed(conn: &Connection) -> Vec<String> {
    conn.prepare("SELECT suggestion_key FROM suggestion_dismissals")
        .and_then(|mut stmt| {
            stmt.query_map([], |row| row.get::<_, String>(0))
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
        })
        .unwrap_or_default()
}

/// Everything worth raising right now, most urgent first.
///
/// Derived from current data on every call, so a suggestion about something
/// that has since been fixed simply stops appearing.
pub fn generate(conn: &Connection) -> Vec<Suggestion> {
    let mut all: Vec<Suggestion> = Vec::new();
    all.extend(blocked_tasks(conn));
    all.extend(broken_editors(conn));
    all.extend(projects_with_a_repo(conn));
    all.extend(stale_tasks(conn));
    all.extend(projects_without_a_path(conn));

    let silenced = dismissed(conn);
    all.retain(|s| !silenced.contains(&s.key));

    // Highest priority first, then stable by key so the order does not
    // shuffle between reads and move a button under the user's cursor.
    all.sort_by(|a, b| b.priority.cmp(&a.priority).then_with(|| a.key.cmp(&b.key)));
    all.truncate(MAX_SUGGESTIONS);
    all
}

/// Stop raising one. Permanent until the user un-dismisses it.
pub fn dismiss(conn: &Connection, key: &str) -> Result<(), String> {
    conn.execute(
        "INSERT OR IGNORE INTO suggestion_dismissals (suggestion_key) VALUES (?1)",
        [key],
    )
    .map_err(|e| format!("Failed to dismiss: {e}"))?;
    Ok(())
}

/// Start raising it again.
pub fn restore(conn: &Connection, key: &str) -> Result<(), String> {
    conn.execute(
        "DELETE FROM suggestion_dismissals WHERE suggestion_key = ?1",
        [key],
    )
    .map_err(|e| format!("Failed to restore: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrations::MIGRATIONS;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch("PRAGMA foreign_keys = ON;").expect("fk");
        for (_, sql) in MIGRATIONS {
            conn.execute_batch(sql).expect("migrate");
        }
        conn
    }

    fn project(conn: &Connection, name: &str, path: Option<&str>, url: Option<&str>) -> i64 {
        conn.execute(
            "INSERT INTO projects (name, repository_path, repository_url) VALUES (?1, ?2, ?3)",
            rusqlite::params![name, path, url],
        )
        .expect("seed");
        conn.last_insert_rowid()
    }

    fn task(conn: &Connection, project: i64, title: &str, status: &str) {
        conn.execute(
            "INSERT INTO tasks (project_id, title, status) VALUES (?1, ?2, ?3)",
            rusqlite::params![project, title, status],
        )
        .expect("seed");
    }

    #[test]
    fn an_empty_workspace_suggests_nothing() {
        // Silence is the correct output when there is nothing to say.
        assert!(generate(&test_conn()).is_empty());
    }

    #[test]
    fn blocked_work_is_raised_with_the_titles_as_evidence() {
        let conn = test_conn();
        let p = project(&conn, "Atlas", Some("/tmp"), None);
        task(&conn, p, "Wire the gate", "blocked");
        task(&conn, p, "Ship referents", "blocked");

        let found = generate(&conn);
        let blocked = found
            .iter()
            .find(|s| s.key == "blocked-tasks")
            .expect("raised");
        assert!(blocked.title.contains('2'));
        assert!(blocked
            .reason
            .evidence
            .iter()
            .any(|e| e.contains("Wire the gate")));
        assert!(!blocked.reason.explanation.is_empty());
    }

    #[test]
    fn three_or_more_blocked_tasks_is_urgent() {
        let conn = test_conn();
        let p = project(&conn, "Atlas", Some("/tmp"), None);
        for i in 0..3 {
            task(&conn, p, &format!("Task {i}"), "blocked");
        }
        let blocked = generate(&conn)
            .into_iter()
            .find(|s| s.key == "blocked-tasks")
            .expect("raised");
        assert_eq!(blocked.priority, Priority::High);
    }

    #[test]
    fn a_project_with_a_repository_offers_its_pull_requests() {
        // The join the schema was shaped for.
        let conn = test_conn();
        let id = project(&conn, "Atlas", Some("/tmp"), Some("https://github.com/a/b"));
        let found = generate(&conn);
        let prs = found
            .iter()
            .find(|s| s.key == format!("check-prs:{id}"))
            .expect("raised");
        assert_eq!(prs.action_id, "github.list_prs");
        assert_eq!(prs.action_input["projectId"], id);
    }

    #[test]
    fn a_broken_editor_registration_is_raised() {
        let conn = test_conn();
        conn.execute(
            "INSERT INTO ides (name, ide_type, executable_path, enabled)
             VALUES ('Ghost','editor','/Applications/NotReal.app',1)",
            [],
        )
        .expect("seed");
        let found = generate(&conn);
        let broken = found
            .iter()
            .find(|s| s.key == "broken-editors")
            .expect("raised");
        assert_eq!(broken.action_id, "ide.discover");
        assert!(broken.reason.evidence.contains(&"Ghost".to_string()));
    }

    #[test]
    fn a_working_editor_is_not_raised() {
        let conn = test_conn();
        conn.execute(
            "INSERT INTO ides (name, ide_type, executable_path, enabled)
             VALUES ('Echo','editor','/bin/echo',1)",
            [],
        )
        .expect("seed");
        assert!(generate(&conn).iter().all(|s| s.key != "broken-editors"));
    }

    #[test]
    fn dismissing_is_permanent_and_scoped_to_one_suggestion() {
        let conn = test_conn();
        let p = project(&conn, "Atlas", Some("/tmp"), Some("https://github.com/a/b"));
        task(&conn, p, "Wire the gate", "blocked");

        assert!(generate(&conn).iter().any(|s| s.key == "blocked-tasks"));
        dismiss(&conn, "blocked-tasks").expect("dismiss");

        let after = generate(&conn);
        assert!(after.iter().all(|s| s.key != "blocked-tasks"));
        assert!(
            after.iter().any(|s| s.key.starts_with("check-prs:")),
            "dismissing one must not silence the others"
        );

        restore(&conn, "blocked-tasks").expect("restore");
        assert!(generate(&conn).iter().any(|s| s.key == "blocked-tasks"));
    }

    #[test]
    fn a_suggestion_about_something_deleted_stops_appearing() {
        // Why suggestions are derived rather than stored: there is no row to
        // go stale and no cleanup to forget.
        let conn = test_conn();
        let p = project(&conn, "Atlas", Some("/tmp"), None);
        task(&conn, p, "Wire the gate", "blocked");
        assert!(generate(&conn).iter().any(|s| s.key == "blocked-tasks"));

        conn.execute("DELETE FROM projects WHERE id = ?1", [p])
            .expect("delete");
        assert!(generate(&conn).iter().all(|s| s.key != "blocked-tasks"));
    }

    #[test]
    fn keys_are_stable_across_regeneration() {
        // A dismissal is worthless if the key changes next time.
        let conn = test_conn();
        let p = project(&conn, "Atlas", None, Some("https://github.com/a/b"));
        task(&conn, p, "Wire the gate", "blocked");
        let first: Vec<String> = generate(&conn).into_iter().map(|s| s.key).collect();
        let second: Vec<String> = generate(&conn).into_iter().map(|s| s.key).collect();
        assert_eq!(first, second);
        assert!(first.iter().any(|k| k.contains(&p.to_string())));
    }

    #[test]
    fn the_list_is_capped_and_ordered_by_priority() {
        let conn = test_conn();
        for i in 0..10 {
            let p = project(
                &conn,
                &format!("P{i}"),
                None,
                Some("https://github.com/a/b"),
            );
            task(&conn, p, "blocked one", "blocked");
        }
        let found = generate(&conn);
        assert!(found.len() <= MAX_SUGGESTIONS);
        for pair in found.windows(2) {
            assert!(pair[0].priority >= pair[1].priority, "must be ordered");
        }
    }

    #[test]
    fn every_suggestion_carries_a_structured_reason() {
        let conn = test_conn();
        let p = project(&conn, "Atlas", None, Some("https://github.com/a/b"));
        task(&conn, p, "Wire the gate", "blocked");
        for suggestion in generate(&conn) {
            assert!(!suggestion.reason.rule.is_empty(), "{}", suggestion.key);
            assert!(
                !suggestion.reason.explanation.is_empty(),
                "{}",
                suggestion.key
            );
            assert!(!suggestion.accept_label.is_empty(), "{}", suggestion.key);
            assert!(!suggestion.action_id.is_empty(), "{}", suggestion.key);
        }
    }

    #[test]
    fn every_proposed_action_actually_exists() {
        // A suggestion offering an action NEXUS does not have would fail at
        // the moment the user trusted it.
        let conn = test_conn();
        let p = project(&conn, "Atlas", None, Some("https://github.com/a/b"));
        task(&conn, p, "Wire the gate", "blocked");
        conn.execute(
            "INSERT INTO ides (name, ide_type, executable_path, enabled)
             VALUES ('Ghost','editor','/nope',1)",
            [],
        )
        .expect("seed");

        for suggestion in generate(&conn) {
            let known = crate::assistant::connectors()
                .into_iter()
                .any(|c| c.spec(&suggestion.action_id).is_some());
            assert!(
                known,
                "{} proposes {}",
                suggestion.key, suggestion.action_id
            );
        }
    }

    #[test]
    fn nothing_here_executes_or_reaches_a_provider() {
        let production = include_str!("suggestions.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("marker");
        for forbidden in ["execute_action", "dispatch", "reasoning", "providers"] {
            assert!(
                !production.contains(forbidden),
                "suggestions must only propose, found {forbidden}"
            );
        }
    }
}
