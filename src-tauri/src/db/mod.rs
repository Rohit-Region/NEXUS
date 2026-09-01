pub mod agents;
pub mod commitments;
pub mod contacts;
pub mod ides;
pub mod migrations;
pub mod projects;
pub mod registry;
pub mod search;
pub mod settings;
pub mod stats;
pub mod tasks;

use rusqlite::Connection;
use std::sync::Mutex;
use tauri::AppHandle;

/// Tauri managed state — a single connection protected by a mutex.
/// Sufficient for a local single-user desktop app; no pool required.
pub struct DbState(pub Mutex<Connection>);

/// Open (or create) the NEXUS database in the Tauri app data directory,
/// enforce foreign keys, and run any pending migrations.
pub fn init(app: &AppHandle) -> Result<Connection, String> {
    let db_path = resolve_db_path(app)?;
    log::info!("NEXUS database path: {}", db_path.display());

    let conn = Connection::open(&db_path).map_err(|e| format!("Failed to open database: {e}"))?;

    // Enforce foreign-key constraints — SQLite disables them by default.
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|e| format!("Failed to enable foreign keys: {e}"))?;

    // Ensure the migrations tracking table exists before anything else.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (
            id         INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );",
    )
    .map_err(|e| format!("Failed to create _migrations table: {e}"))?;

    run_migrations(&conn)?;

    // NEXUS-015: connectors defined in code get a row here, so adding one is
    // never a migration. New connectors arrive with no permission grants and
    // do nothing at all until the user allows them in Settings.
    crate::assistant::register_connectors(&conn)?;

    Ok(conn)
}

/// Returns the current highest applied migration ID, or 0 if none.
pub fn migration_level(conn: &Connection) -> Result<i64, String> {
    conn.query_row("SELECT COALESCE(MAX(id), 0) FROM _migrations", [], |row| {
        row.get::<_, i64>(0)
    })
    .map_err(|e| format!("Failed to read migration level: {e}"))
}

/// Resolve the path to nexus.db inside the Tauri app data directory.
fn resolve_db_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    use tauri::Manager;

    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data directory: {e}"))?;

    // Ensure the directory exists.
    std::fs::create_dir_all(&app_data)
        .map_err(|e| format!("Failed to create app data directory: {e}"))?;

    Ok(app_data.join("nexus.db"))
}

/// Apply any migrations from MIGRATIONS that have not yet been recorded
/// in the _migrations table.
fn run_migrations(conn: &Connection) -> Result<(), String> {
    let current = migration_level(conn)?;

    for &(id, sql) in migrations::MIGRATIONS {
        if id <= current {
            continue;
        }

        log::info!("Applying migration {id}");

        conn.execute_batch(sql)
            .map_err(|e| format!("Migration {id} failed: {e}"))?;

        conn.execute(
            "INSERT INTO _migrations (id) VALUES (?1)",
            rusqlite::params![id],
        )
        .map_err(|e| format!("Failed to record migration {id}: {e}"))?;

        log::info!("Migration {id} applied successfully");
    }

    Ok(())
}

// -- Tests -------------------------------------------------------------------
//
// The migration runner had no coverage until NEXUS-012, when it acquired a
// second migration and therefore, for the first time, an upgrade path over a
// database that already has data in it.

#[cfg(test)]
mod tests {
    use super::*;

    /// The bootstrap `init` performs, minus the parts that need a running app.
    fn bare_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch("PRAGMA foreign_keys = ON;").expect("fk");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS _migrations (
                id         INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
            );",
        )
        .expect("tracking table");
        conn
    }

    fn highest() -> i64 {
        migrations::MIGRATIONS
            .iter()
            .map(|(id, _)| *id)
            .max()
            .expect("at least one migration")
    }

    #[test]
    fn migration_ids_are_unique_and_ascending() {
        // The file says never remove or reorder. This is that rule, enforced.
        let ids: Vec<i64> = migrations::MIGRATIONS.iter().map(|(id, _)| *id).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            ids, sorted,
            "migrations must be unique and in ascending order"
        );
        assert_eq!(ids[0], 1, "migration ids start at 1");
    }

    #[test]
    fn a_fresh_database_reaches_the_highest_level() {
        let conn = bare_conn();
        run_migrations(&conn).expect("migrate");
        assert_eq!(migration_level(&conn).expect("level"), highest());
    }

    #[test]
    fn a_database_already_at_level_one_upgrades_without_touching_its_data() {
        // The real upgrade path: an installed NEXUS has a populated database
        // at level 1, and migration 002 has to run over it.
        let conn = bare_conn();
        let (id, sql) = migrations::MIGRATIONS[0];
        conn.execute_batch(sql).expect("apply 001");
        conn.execute("INSERT INTO _migrations (id) VALUES (?1)", [id])
            .expect("record 001");

        conn.execute("INSERT INTO projects (name) VALUES ('Atlas')", [])
            .expect("seed project");
        conn.execute(
            "INSERT INTO tasks (project_id, title) VALUES (1, 'Ship it')",
            [],
        )
        .expect("seed task");

        run_migrations(&conn).expect("upgrade");

        assert_eq!(migration_level(&conn).expect("level"), highest());
        let projects: i64 = conn
            .query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0))
            .expect("count");
        let tasks: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
            .expect("count");
        assert_eq!((projects, tasks), (1, 1), "existing rows must survive");
    }

    #[test]
    fn running_migrations_twice_changes_nothing() {
        let conn = bare_conn();
        run_migrations(&conn).expect("first");
        run_migrations(&conn).expect("second");

        assert_eq!(migration_level(&conn).expect("level"), highest());
        let applied: i64 = conn
            .query_row("SELECT COUNT(*) FROM _migrations", [], |r| r.get(0))
            .expect("count");
        assert_eq!(applied as usize, migrations::MIGRATIONS.len());

        // The seed rows in 002 use INSERT OR IGNORE, so a second pass must
        // not duplicate the connector or its grants.
        let connectors: i64 = conn
            .query_row("SELECT COUNT(*) FROM connectors", [], |r| r.get(0))
            .expect("count");
        assert_eq!(connectors, 1);
    }

    #[test]
    fn an_already_applied_migration_is_not_re_run() {
        let conn = bare_conn();
        run_migrations(&conn).expect("migrate");
        // Revoking a seeded grant then re-running must not silently restore
        // it: a migration is not a repair tool for the user's choices.
        conn.execute(
            "DELETE FROM permission_grants WHERE connector_id='nexus' AND level='destructive'",
            [],
        )
        .expect("revoke");

        run_migrations(&conn).expect("re-run");

        let restored: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM permission_grants
                  WHERE connector_id='nexus' AND level='destructive'",
                [],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(restored, 0, "a re-run must not undo a revoked grant");
    }
}
