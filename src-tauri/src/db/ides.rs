use rusqlite::{Connection, Result as RusqliteResult};

use super::registry::{
    map_entry_row, validate_entry, CreateRegistryEntryInput, RegistryEntry,
    UpdateRegistryEntryInput,
};

/// Selected in this order everywhere in this module so `map_entry_row` applies.
const IDE_COLUMNS: &str = "id, name, ide_type AS entry_type, executable_path,
                           enabled, created_at, updated_at";

fn get_ide_by_id(conn: &Connection, id: i64) -> Result<RegistryEntry, String> {
    conn.query_row(
        &format!("SELECT {IDE_COLUMNS} FROM ides WHERE id = ?1"),
        rusqlite::params![id],
        map_entry_row,
    )
    .map_err(|e| format!("Failed to fetch IDE {id}: {e}"))
}

// -- CRUD --------------------------------------------------------------------

/// Register a new IDE and return the full row.
pub fn insert_ide(
    conn: &Connection,
    input: &CreateRegistryEntryInput,
) -> Result<RegistryEntry, String> {
    validate_entry(&input.name, &input.entry_type)?;
    let name = input.name.trim();
    let entry_type = input.entry_type.trim();

    match input.enabled {
        Some(enabled) => conn.execute(
            "INSERT INTO ides (name, ide_type, executable_path, enabled)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![name, entry_type, input.executable_path, enabled],
        ),
        // Omit the column so SQLite applies its own DEFAULT 1.
        None => conn.execute(
            "INSERT INTO ides (name, ide_type, executable_path)
             VALUES (?1, ?2, ?3)",
            rusqlite::params![name, entry_type, input.executable_path],
        ),
    }
    .map_err(|e| format!("Failed to insert IDE: {e}"))?;

    get_ide_by_id(conn, conn.last_insert_rowid())
}

/// Return registered IDEs, newest first. `enabled_only` filters out disabled rows.
pub fn list_ides(conn: &Connection, enabled_only: bool) -> Result<Vec<RegistryEntry>, String> {
    let filter = if enabled_only {
        "WHERE enabled = 1"
    } else {
        ""
    };

    let mut stmt = conn
        .prepare(&format!(
            "SELECT {IDE_COLUMNS} FROM ides {filter} ORDER BY created_at DESC, id DESC"
        ))
        .map_err(|e| format!("Failed to prepare list_ides: {e}"))?;

    let rows = stmt
        .query_map([], map_entry_row)
        .map_err(|e| format!("Failed to query IDEs: {e}"))?;

    rows.collect::<RusqliteResult<Vec<_>>>()
        .map_err(|e| format!("Failed to collect IDEs: {e}"))
}

/// Update an IDE and return the full updated row.
pub fn update_ide(
    conn: &Connection,
    input: &UpdateRegistryEntryInput,
) -> Result<RegistryEntry, String> {
    validate_entry(&input.name, &input.entry_type)?;

    let affected = conn
        .execute(
            "UPDATE ides
                SET name            = ?1,
                    ide_type        = ?2,
                    executable_path = ?3,
                    enabled         = ?4,
                    updated_at      = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
              WHERE id = ?5",
            rusqlite::params![
                input.name.trim(),
                input.entry_type.trim(),
                input.executable_path,
                input.enabled,
                input.id,
            ],
        )
        .map_err(|e| format!("Failed to update IDE {}: {e}", input.id))?;

    if affected == 0 {
        return Err(format!("IDE {} not found", input.id));
    }

    get_ide_by_id(conn, input.id)
}

/// Delete an IDE by ID.
///
/// One statement. `projects.default_ide_id` references `ides(id)` ON DELETE
/// SET NULL, so SQLite blanks every referring project and leaves it alive.
/// The application must not emulate that (spec 4.2).
pub fn delete_ide(conn: &Connection, id: i64) -> Result<(), String> {
    let affected = conn
        .execute("DELETE FROM ides WHERE id = ?1", rusqlite::params![id])
        .map_err(|e| format!("Failed to delete IDE {id}: {e}"))?;

    if affected == 0 {
        return Err(format!("IDE {id} not found"));
    }

    Ok(())
}

// -- Tests -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrations::MIGRATIONS;
    use crate::db::projects::{insert_project, CreateProjectInput};

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .expect("enable foreign keys");
        for &(_, sql) in MIGRATIONS {
            conn.execute_batch(sql).expect("apply migration");
        }
        conn
    }

    fn seed_project(conn: &Connection, name: &str) -> i64 {
        insert_project(
            conn,
            &CreateProjectInput {
                name: name.to_string(),
                description: None,
                repository_path: None,
                repository_url: None,
                default_ide_id: None,
                default_agent_id: None,
            },
        )
        .expect("insert project")
        .id
    }

    fn seed(conn: &Connection, name: &str) -> RegistryEntry {
        insert_ide(
            conn,
            &CreateRegistryEntryInput {
                name: name.to_string(),
                entry_type: "editor".to_string(),
                executable_path: Some("/usr/local/bin/thing".to_string()),
                enabled: None,
            },
        )
        .expect("insert entry")
    }

    #[test]
    fn insert_defaults_to_enabled() {
        let conn = test_conn();
        let entry = insert_ide(
            &conn,
            &CreateRegistryEntryInput {
                name: "  Padded Name  ".to_string(),
                entry_type: "  padded_type  ".to_string(),
                executable_path: None,
                enabled: None,
            },
        )
        .expect("insert entry");

        assert!(entry.enabled, "omitted enabled must use the schema default");
        assert_eq!(entry.name, "Padded Name", "name must be trimmed");
        assert_eq!(entry.entry_type, "padded_type", "type must be trimmed");
        assert_eq!(entry.executable_path, None);
        assert_eq!(entry.created_at, entry.updated_at);
    }

    #[test]
    fn insert_rejects_empty_name() {
        let conn = test_conn();
        let err = insert_ide(
            &conn,
            &CreateRegistryEntryInput {
                name: "   ".to_string(),
                entry_type: "editor".to_string(),
                executable_path: None,
                enabled: None,
            },
        )
        .expect_err("empty name must be rejected");

        assert!(
            err.contains("Name cannot be empty"),
            "unexpected error: {err}"
        );
        assert!(list_ides(&conn, false).expect("list").is_empty());
    }

    #[test]
    fn insert_rejects_empty_type() {
        let conn = test_conn();
        let err = insert_ide(
            &conn,
            &CreateRegistryEntryInput {
                name: "Named".to_string(),
                entry_type: "  ".to_string(),
                executable_path: None,
                enabled: None,
            },
        )
        .expect_err("empty type must be rejected");

        assert!(
            err.contains("Type cannot be empty"),
            "unexpected error: {err}"
        );
        assert!(list_ides(&conn, false).expect("list").is_empty());
    }

    #[test]
    fn list_returns_all_or_enabled_only() {
        let conn = test_conn();
        let on = seed(&conn, "Enabled One");
        let off = seed(&conn, "Disabled One");

        update_ide(
            &conn,
            &UpdateRegistryEntryInput {
                id: off.id,
                name: off.name.clone(),
                entry_type: off.entry_type.clone(),
                executable_path: off.executable_path.clone(),
                enabled: false,
            },
        )
        .expect("disable entry");

        let all = list_ides(&conn, false).expect("list all");
        let enabled = list_ides(&conn, true).expect("list enabled");

        assert_eq!(all.len(), 2, "enabled_only = false must return both");
        assert_eq!(enabled.len(), 1, "enabled_only = true must filter");
        assert_eq!(enabled[0].id, on.id);
        assert!(enabled.iter().all(|e| e.enabled));
    }

    #[test]
    fn update_changes_fields_and_updated_at() {
        let conn = test_conn();
        let created = seed(&conn, "Before");

        std::thread::sleep(std::time::Duration::from_millis(5));

        let updated = update_ide(
            &conn,
            &UpdateRegistryEntryInput {
                id: created.id,
                name: "After".to_string(),
                entry_type: "changed_type".to_string(),
                executable_path: None,
                enabled: true,
            },
        )
        .expect("update entry");

        assert_eq!(updated.name, "After");
        assert_eq!(updated.entry_type, "changed_type");
        assert_eq!(updated.executable_path, None, "path must be clearable");
        assert_eq!(
            updated.created_at, created.created_at,
            "created_at must not change on update"
        );
        assert_ne!(
            updated.updated_at, created.updated_at,
            "updated_at must advance on update"
        );
    }

    #[test]
    fn update_toggles_enabled() {
        let conn = test_conn();
        let entry = seed(&conn, "Toggle Me");
        assert!(entry.enabled);

        let mk = |enabled: bool| UpdateRegistryEntryInput {
            id: entry.id,
            name: entry.name.clone(),
            entry_type: entry.entry_type.clone(),
            executable_path: entry.executable_path.clone(),
            enabled,
        };

        assert!(!update_ide(&conn, &mk(false)).expect("disable").enabled);
        assert!(update_ide(&conn, &mk(true)).expect("re-enable").enabled);
    }

    #[test]
    fn update_rejects_unknown_id() {
        let conn = test_conn();
        let err = update_ide(
            &conn,
            &UpdateRegistryEntryInput {
                id: 4242,
                name: "Ghost".to_string(),
                entry_type: "editor".to_string(),
                executable_path: None,
                enabled: true,
            },
        )
        .expect_err("unknown id must be rejected");

        assert!(err.contains("not found"), "unexpected error: {err}");
    }

    #[test]
    fn delete_removes_only_that_entry() {
        let conn = test_conn();
        let keep = seed(&conn, "Keep");
        let doomed = seed(&conn, "Doomed");

        delete_ide(&conn, doomed.id).expect("delete entry");

        let remaining = list_ides(&conn, false).expect("list");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, keep.id);

        let err = delete_ide(&conn, doomed.id).expect_err("second delete must fail");
        assert!(err.contains("not found"), "unexpected error: {err}");
    }

    /// Load-bearing (spec 4.2): projects.default_ide_id is ON DELETE SET NULL.
    /// Deleting an IDE must blank the reference and keep the project alive.
    #[test]
    fn deleting_ide_nulls_project_default_and_keeps_project() {
        let conn = test_conn();
        let doomed = seed(&conn, "Doomed IDE");
        let other = seed(&conn, "Other IDE");
        let a = seed_project(&conn, "Uses Doomed");
        let b = seed_project(&conn, "Uses Other");

        conn.execute(
            "UPDATE projects SET default_ide_id = ?1 WHERE id = ?2",
            rusqlite::params![doomed.id, a],
        )
        .expect("assign doomed");
        conn.execute(
            "UPDATE projects SET default_ide_id = ?1 WHERE id = ?2",
            rusqlite::params![other.id, b],
        )
        .expect("assign other");

        let projects_before: i64 = conn
            .query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0))
            .expect("count");

        delete_ide(&conn, doomed.id).expect("delete IDE");

        let projects_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0))
            .expect("count");
        assert_eq!(
            projects_before, projects_after,
            "SET NULL must not delete any project"
        );

        let (name, default_ide): (String, Option<i64>) = conn
            .query_row(
                "SELECT name, default_ide_id FROM projects WHERE id = ?1",
                rusqlite::params![a],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("read project");
        assert_eq!(name, "Uses Doomed", "project must survive intact");
        assert_eq!(default_ide, None, "reference must be blanked");

        let untouched: Option<i64> = conn
            .query_row(
                "SELECT default_ide_id FROM projects WHERE id = ?1",
                rusqlite::params![b],
                |r| r.get(0),
            )
            .expect("read other project");
        assert_eq!(
            untouched,
            Some(other.id),
            "other defaults must be untouched"
        );
    }
}
