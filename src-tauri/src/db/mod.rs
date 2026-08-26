pub mod agents;
pub mod ides;
pub mod migrations;
pub mod projects;
pub mod registry;
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

    let conn = Connection::open(&db_path)
        .map_err(|e| format!("Failed to open database: {e}"))?;

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

    Ok(conn)
}

/// Returns the current highest applied migration ID, or 0 if none.
pub fn migration_level(conn: &Connection) -> Result<i64, String> {
    conn.query_row(
        "SELECT COALESCE(MAX(id), 0) FROM _migrations",
        [],
        |row| row.get::<_, i64>(0),
    )
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
