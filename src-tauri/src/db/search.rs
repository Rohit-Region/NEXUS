//! Unified cross-entity search, deferred from NEXUS-007 to this milestone.
//!
//! NEXUS-007 filtered lists client-side because each list already held its
//! complete dataset in component state. The command palette does not: there is
//! no cross-project task array anywhere, and building one per keystroke would
//! mean fetching every task in the workspace. This module is the one place
//! that queries across all four entity tables.
//!
//! Matching is a case-insensitive substring, consistent with `matchesQuery`
//! in src/lib/list-filters.ts. Deterministic: no scoring, no fuzzy matching,
//! no inference of any kind.

use rusqlite::{Connection, Result as RusqliteResult};
use serde::{Deserialize, Serialize};

/// Upper bound on returned rows. Reaching it is reported, never silent.
pub const SEARCH_RESULT_CAP: usize = 50;

/// One search hit, flattened deliberately.
///
/// A serde-tagged enum would carry a different payload per kind and force the
/// frontend to narrow before it can render a row. Every consumer needs the
/// same four things, so the shape is flat and `kind` is a plain string
/// matching the TypeScript union.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    /// 'project' | 'task' | 'ide' | 'agent'
    pub kind: String,
    pub id: i64,
    /// Primary label: project name, task title, or registry entry name.
    pub title: String,
    /// Disambiguating context: owning project for a task, type for a registry
    /// entry, repository path for a project. None when absent.
    pub subtitle: Option<String>,
    /// Navigation target. Set for projects and tasks, None for registry rows.
    pub project_id: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResults {
    pub results: Vec<SearchResult>,
    /// True when the cap was reached, so the UI can say so rather than
    /// silently pretending it showed everything.
    pub truncated: bool,
}

/// Escape the three characters that are special inside a LIKE pattern.
///
/// Without this, a user searching `snake_case` would also match `snakeXcase`,
/// because `_` matches any single character. That is a defect that produces
/// plausible output and no error, so it is handled here and guarded by
/// `escape_like_is_applied`.
///
/// The backslash must be escaped first, or the escapes added for `%` and `_`
/// would themselves be escaped a second time.
fn escape_like(query: &str) -> String {
    query
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Rows are ordered within each kind by lowercase title then id, matching the
/// total-ordering rule NEXUS-007 established for the frontend comparators.
fn map_result(kind: &'static str) -> impl Fn(&rusqlite::Row<'_>) -> RusqliteResult<SearchResult> {
    move |row| {
        Ok(SearchResult {
            kind: kind.to_string(),
            id: row.get(0)?,
            title: row.get(1)?,
            subtitle: row.get(2)?,
            project_id: row.get(3)?,
        })
    }
}

fn collect(
    conn: &Connection,
    sql: &str,
    pattern: &str,
    kind: &'static str,
) -> Result<Vec<SearchResult>, String> {
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("Failed to search workspace: {e}"))?;

    let rows = stmt
        .query_map(rusqlite::params![pattern], map_result(kind))
        .map_err(|e| format!("Failed to search workspace: {e}"))?;

    rows.collect::<RusqliteResult<Vec<_>>>()
        .map_err(|e| format!("Failed to search workspace: {e}"))
}

/// Search projects, tasks, IDEs and agents in one pass.
///
/// Kind order is fixed rather than relevance-scored. Scoring invites tuning
/// and has no obvious right answer at this scale; a stable, explainable order
/// is more useful than a clever one.
///
/// `tasks.external_id` is deliberately not searched: no milestone produces it,
/// so every row holds NULL and searching it would be dead functionality.
pub fn search_workspace(conn: &Connection, query: &str) -> Result<SearchResults, String> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        // An empty query is a valid no-op, not an error, and touches no table.
        return Ok(SearchResults {
            results: Vec::new(),
            truncated: false,
        });
    }

    let pattern = escape_like(trimmed);

    let projects = collect(
        conn,
        "SELECT id, name, repository_path, id
           FROM projects
          WHERE LOWER(name)            LIKE '%' || LOWER(?1) || '%' ESCAPE '\\'
             OR LOWER(description)     LIKE '%' || LOWER(?1) || '%' ESCAPE '\\'
             OR LOWER(repository_path) LIKE '%' || LOWER(?1) || '%' ESCAPE '\\'
             OR LOWER(repository_url)  LIKE '%' || LOWER(?1) || '%' ESCAPE '\\'
          ORDER BY LOWER(name), id",
        &pattern,
        "project",
    )?;

    let tasks = collect(
        conn,
        "SELECT t.id, t.title, p.name, t.project_id
           FROM tasks t
           INNER JOIN projects p ON p.id = t.project_id
          WHERE LOWER(t.title)       LIKE '%' || LOWER(?1) || '%' ESCAPE '\\'
             OR LOWER(t.description) LIKE '%' || LOWER(?1) || '%' ESCAPE '\\'
          ORDER BY LOWER(t.title), t.id",
        &pattern,
        "task",
    )?;

    let ides = collect(
        conn,
        "SELECT id, name, ide_type, NULL
           FROM ides
          WHERE LOWER(name)            LIKE '%' || LOWER(?1) || '%' ESCAPE '\\'
             OR LOWER(ide_type)        LIKE '%' || LOWER(?1) || '%' ESCAPE '\\'
             OR LOWER(executable_path) LIKE '%' || LOWER(?1) || '%' ESCAPE '\\'
          ORDER BY LOWER(name), id",
        &pattern,
        "ide",
    )?;

    let agents = collect(
        conn,
        "SELECT id, name, agent_type, NULL
           FROM ai_agents
          WHERE LOWER(name)            LIKE '%' || LOWER(?1) || '%' ESCAPE '\\'
             OR LOWER(agent_type)      LIKE '%' || LOWER(?1) || '%' ESCAPE '\\'
             OR LOWER(executable_path) LIKE '%' || LOWER(?1) || '%' ESCAPE '\\'
          ORDER BY LOWER(name), id",
        &pattern,
        "agent",
    )?;

    let mut results = projects;
    results.extend(tasks);
    results.extend(ides);
    results.extend(agents);

    let truncated = results.len() > SEARCH_RESULT_CAP;
    results.truncate(SEARCH_RESULT_CAP);

    Ok(SearchResults { results, truncated })
}

// -- Tests --------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrations::MIGRATIONS;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .expect("enable foreign keys");
        for &(_, sql) in MIGRATIONS {
            conn.execute_batch(sql).expect("apply migration");
        }
        conn
    }

    fn project(
        conn: &Connection,
        name: &str,
        desc: Option<&str>,
        path: Option<&str>,
        url: Option<&str>,
    ) -> i64 {
        conn.execute(
            "INSERT INTO projects (name, description, repository_path, repository_url)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![name, desc, path, url],
        )
        .expect("insert project");
        conn.last_insert_rowid()
    }

    fn task(conn: &Connection, project_id: i64, title: &str, desc: Option<&str>) -> i64 {
        conn.execute(
            "INSERT INTO tasks (project_id, title, description) VALUES (?1, ?2, ?3)",
            rusqlite::params![project_id, title, desc],
        )
        .expect("insert task");
        conn.last_insert_rowid()
    }

    fn ide(conn: &Connection, name: &str, ty: &str, path: Option<&str>) {
        conn.execute(
            "INSERT INTO ides (name, ide_type, executable_path) VALUES (?1, ?2, ?3)",
            rusqlite::params![name, ty, path],
        )
        .expect("insert ide");
    }

    fn agent(conn: &Connection, name: &str, ty: &str, path: Option<&str>) {
        conn.execute(
            "INSERT INTO ai_agents (name, agent_type, executable_path) VALUES (?1, ?2, ?3)",
            rusqlite::params![name, ty, path],
        )
        .expect("insert agent");
    }

    fn titles(r: &SearchResults) -> Vec<&str> {
        r.results.iter().map(|x| x.title.as_str()).collect()
    }

    #[test]
    fn search_on_empty_database() {
        let conn = test_conn();
        let r = search_workspace(&conn, "anything").expect("search");
        assert!(r.results.is_empty());
        assert!(!r.truncated);
    }

    #[test]
    fn search_empty_query_returns_nothing() {
        let conn = test_conn();
        project(&conn, "Visible", None, None, None);

        for q in ["", "   ", "\t\n"] {
            let r = search_workspace(&conn, q).expect("search");
            assert!(r.results.is_empty(), "query {q:?} must return nothing");
            assert!(!r.truncated);
        }
    }

    #[test]
    fn search_is_case_insensitive() {
        let conn = test_conn();
        project(&conn, "ALPHA Project", None, None, None);

        for q in ["alpha", "ALPHA", "AlPhA"] {
            let r = search_workspace(&conn, q).expect("search");
            assert_eq!(r.results.len(), 1, "query {q:?} must match");
        }
    }

    #[test]
    fn search_matches_substring() {
        let conn = test_conn();
        project(&conn, "Groundwork", None, None, None);

        assert_eq!(
            search_workspace(&conn, "undwo")
                .expect("search")
                .results
                .len(),
            1,
            "a mid-word substring must match"
        );
    }

    #[test]
    fn search_matches_every_project_field() {
        let conn = test_conn();
        project(&conn, "NameHit", None, None, None);
        project(&conn, "P2", Some("DescHit here"), None, None);
        project(&conn, "P3", None, Some("/tmp/PathHit"), None);
        project(&conn, "P4", None, None, Some("https://example.com/UrlHit"));

        for marker in ["NameHit", "DescHit", "PathHit", "UrlHit"] {
            let r = search_workspace(&conn, marker).expect("search");
            assert_eq!(
                r.results.len(),
                1,
                "{marker} must match exactly one project"
            );
            assert_eq!(r.results[0].kind, "project");
        }
    }

    #[test]
    fn search_matches_task_title_and_description() {
        let conn = test_conn();
        let p = project(&conn, "Host", None, None, None);
        task(&conn, p, "TitleHit task", None);
        task(&conn, p, "Other", Some("DescHit inside"));

        for marker in ["TitleHit", "DescHit"] {
            let r = search_workspace(&conn, marker).expect("search");
            assert_eq!(r.results.len(), 1, "{marker} must match one task");
            assert_eq!(r.results[0].kind, "task");
        }
    }

    #[test]
    fn search_matches_registry_name_type_and_path() {
        let conn = test_conn();
        ide(&conn, "IdeNameHit", "editor", None);
        ide(&conn, "I2", "IdeTypeHit", None);
        ide(&conn, "I3", "editor", Some("/bin/IdePathHit"));
        agent(&conn, "AgentNameHit", "assistant", None);
        agent(&conn, "A2", "AgentTypeHit", None);
        agent(&conn, "A3", "assistant", Some("/bin/AgentPathHit"));

        for (marker, kind) in [
            ("IdeNameHit", "ide"),
            ("IdeTypeHit", "ide"),
            ("IdePathHit", "ide"),
            ("AgentNameHit", "agent"),
            ("AgentTypeHit", "agent"),
            ("AgentPathHit", "agent"),
        ] {
            let r = search_workspace(&conn, marker).expect("search");
            assert_eq!(r.results.len(), 1, "{marker} must match one row");
            assert_eq!(r.results[0].kind, kind);
        }
    }

    #[test]
    fn search_task_carries_project_name_and_id() {
        let conn = test_conn();
        let p = project(&conn, "Owning Project", None, None, None);
        task(&conn, p, "Findable", None);

        let r = search_workspace(&conn, "Findable").expect("search");
        assert_eq!(r.results[0].subtitle.as_deref(), Some("Owning Project"));
        assert_eq!(r.results[0].project_id, Some(p));
    }

    #[test]
    fn search_does_not_match_external_id() {
        let conn = test_conn();
        let p = project(&conn, "Host", None, None, None);
        conn.execute(
            "INSERT INTO tasks (project_id, title, external_id)
             VALUES (?1, 'Ordinary title', 'JIRA-SECRET')",
            rusqlite::params![p],
        )
        .expect("insert imported task");

        let r = search_workspace(&conn, "JIRA-SECRET").expect("search");
        assert!(
            r.results.is_empty(),
            "external_id has no producer and must not be searchable"
        );
    }

    /// The escaping guard. Without ESCAPE handling, `_` matches any character
    /// and `snake_case` would wrongly match `snakeXcase`.
    #[test]
    fn escape_like_is_applied() {
        let conn = test_conn();
        project(&conn, "snake_case", None, None, None);
        project(&conn, "snakeXcase", None, None, None);
        project(&conn, "100% done", None, None, None);
        project(&conn, "100Zdone", None, None, None);

        let underscore = search_workspace(&conn, "snake_case").expect("search");
        assert_eq!(
            titles(&underscore),
            vec!["snake_case"],
            "underscore must match literally, not as a wildcard"
        );

        let percent = search_workspace(&conn, "100%").expect("search");
        assert_eq!(
            titles(&percent),
            vec!["100% done"],
            "percent must match literally, not as a wildcard"
        );
    }

    #[test]
    fn search_orders_by_kind_then_name() {
        let conn = test_conn();
        let p = project(&conn, "zeta HIT", None, None, None);
        project(&conn, "Alpha HIT", None, None, None);
        task(&conn, p, "task HIT", None);
        ide(&conn, "ide HIT", "editor", None);
        agent(&conn, "agent HIT", "assistant", None);

        let r = search_workspace(&conn, "HIT").expect("search");
        let kinds: Vec<&str> = r.results.iter().map(|x| x.kind.as_str()).collect();

        assert_eq!(
            kinds,
            vec!["project", "project", "task", "ide", "agent"],
            "kind order must be project, task, ide, agent"
        );
        assert_eq!(
            r.results[0].title, "Alpha HIT",
            "within a kind, ordering is case-insensitive ascending"
        );
        assert_eq!(r.results[1].title, "zeta HIT");
    }

    #[test]
    fn search_caps_results_and_reports_truncation() {
        let conn = test_conn();
        for i in 0..60 {
            project(&conn, &format!("CAPPED {i:03}"), None, None, None);
        }

        let r = search_workspace(&conn, "CAPPED").expect("search");
        assert_eq!(r.results.len(), SEARCH_RESULT_CAP);
        assert!(
            r.truncated,
            "reaching the cap must be reported, never silent"
        );
    }

    #[test]
    fn search_below_cap_reports_not_truncated() {
        let conn = test_conn();
        for i in 0..5 {
            project(&conn, &format!("SMALL {i}"), None, None, None);
        }

        let r = search_workspace(&conn, "SMALL").expect("search");
        assert_eq!(r.results.len(), 5);
        assert!(!r.truncated);
    }

    #[test]
    fn search_ignores_null_fields() {
        let conn = test_conn();
        project(&conn, "NoExtras", None, None, None);
        ide(&conn, "BareIde", "editor", None);

        // A query matching nothing must not error despite the null columns.
        let r = search_workspace(&conn, "zzz-no-match").expect("search");
        assert!(r.results.is_empty());

        // And the rows are still findable by their populated columns.
        assert_eq!(
            search_workspace(&conn, "NoExtras")
                .expect("s")
                .results
                .len(),
            1
        );
        assert_eq!(
            search_workspace(&conn, "BareIde").expect("s").results.len(),
            1
        );
    }
}
