//! The Mac's data copied to the phone, whole, until Phase 2's sync.
//!
//! `scripts/mac-to-phone.sh` takes a consistent snapshot of the Mac's
//! database and pushes it to the phone; `MainActivity` moves it into the
//! app's private folder, and this puts it in place of the phone's database
//! before it is opened. The Mac is the master copy until the two sync: what
//! was entered only on the phone is replaced.
//!
//! The database holds no secrets (those are in the Keychain, and come
//! separately, sealed for the phone: `MacTransfer.kt`). Settings that only
//! mean something on the Mac come along and are ignored, as their features
//! are hidden on the phone. The phone's own inbox cursor is not in the copy,
//! so the next sync reads the whole inbox again; each text that the Mac
//! already has from the Sheet carries the same id and is booked once.

use rusqlite::{Connection, OpenFlags};
use std::path::Path;

pub const FILE_NAME: &str = "nexus-db-from-mac.sqlite";

/// Put a waiting snapshot in `dir` in place of the database at `db_path`.
/// Returns whether one was taken. The snapshot is removed either way, so a
/// bad copy does not fail every start, and the phone's database is only
/// replaced by one that opens and has NEXUS's tables.
pub fn take_pending(dir: &Path, db_path: &Path) -> Result<bool, String> {
    let snapshot = dir.join(FILE_NAME);
    if !snapshot.exists() {
        return Ok(false);
    }
    if let Err(e) = check(&snapshot) {
        let _ = std::fs::remove_file(&snapshot);
        return Err(format!("The copy from the Mac was not used: {e}"));
    }
    std::fs::rename(&snapshot, db_path).map_err(|e| format!("Could not put the Mac's copy in place: {e}"))?;
    // The old database's journals must not be replayed onto the new one: the
    // script stops the app, which can leave a hot journal mid-write.
    for suffix in ["-journal", "-wal", "-shm"] {
        let mut side = db_path.as_os_str().to_owned();
        side.push(suffix);
        let _ = std::fs::remove_file(side);
    }
    Ok(true)
}

fn check(snapshot: &Path) -> Result<(), String> {
    let conn = Connection::open_with_flags(snapshot, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| e.to_string())?;
    let ok: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    if ok != "ok" {
        return Err(format!("it is damaged ({ok})"));
    }
    conn.query_row("SELECT COUNT(*) FROM _migrations", [], |r| r.get::<_, i64>(0))
        .map_err(|_| "it is not a NEXUS database".to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("nexus-from-mac-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn nexus_db(path: &Path, user: &str) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE _migrations (id INTEGER PRIMARY KEY);
             CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);",
        )
        .unwrap();
        conn.execute("INSERT INTO settings VALUES ('user_name', ?1)", [user]).unwrap();
    }

    fn user(path: &Path) -> String {
        Connection::open(path)
            .unwrap()
            .query_row("SELECT value FROM settings WHERE key = 'user_name'", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn a_snapshot_replaces_the_database_once() {
        let dir = dir("ok");
        let db = dir.join("nexus.db");
        nexus_db(&db, "phone");
        std::fs::write(dir.join("nexus.db-journal"), b"old journal").unwrap();
        std::fs::write(dir.join("nexus.db-wal"), b"old journal").unwrap();
        nexus_db(&dir.join(FILE_NAME), "mac");

        assert!(take_pending(&dir, &db).unwrap());
        assert_eq!(user(&db), "mac");
        assert!(!dir.join(FILE_NAME).exists());
        assert!(!dir.join("nexus.db-journal").exists(), "the old journals are dropped");
        assert!(!dir.join("nexus.db-wal").exists());
        assert!(!take_pending(&dir, &db).unwrap(), "nothing left to take");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_is_not_a_nexus_database_is_refused_and_removed() {
        let dir = dir("bad");
        let db = dir.join("nexus.db");
        nexus_db(&db, "phone");
        std::fs::write(dir.join(FILE_NAME), b"not a database").unwrap();
        assert!(take_pending(&dir, &db).is_err());
        assert_eq!(user(&db), "phone", "the phone's database is kept");
        assert!(!dir.join(FILE_NAME).exists());

        Connection::open(dir.join(FILE_NAME)).unwrap().execute_batch("CREATE TABLE other (x);").unwrap();
        assert!(take_pending(&dir, &db).unwrap_err().contains("not a NEXUS database"));
        assert_eq!(user(&db), "phone");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
