//! Name-to-number lookup against WhatsApp's own contact list.
//!
//! WhatsApp addresses people by number. Asking someone to retype hundreds of
//! contacts in order to send one message is not a product, and WhatsApp
//! already keeps that list on this machine, unencrypted, in its group
//! container.
//!
//! The boundaries this module holds to:
//!
//! - **Read-only.** Opened with `SQLITE_OPEN_READ_ONLY`, so a defect here
//!   cannot corrupt the user's WhatsApp data.
//! - **Local.** A SQLite file read. Nothing is sent anywhere.
//! - **Nothing is kept.** A number goes straight into the action input. It
//!   is never written to NEXUS's database, the audit log, or a file.
//! - **Matches only.** `suggest` has to compare the spoken name against
//!   every stored name to rank them, so it reads the column; what it hands
//!   back is capped at the closest few. There is no call that returns the
//!   address book.
//!
//! When the store is missing or its schema has moved, every function here
//! returns nothing and NEXUS falls back to its own contacts table. A
//! degraded lookup is the correct failure, not an error the user must read.

use rusqlite::{Connection, OpenFlags};

use crate::db::contacts::{close_enough, edit_distance};

/// WhatsApp's Core Data store, relative to the user's home directory.
const STORE: &str =
    "Library/Group Containers/group.net.whatsapp.WhatsApp.shared/ContactsV2.sqlite";

/// Most suggestions ever offered, however many names are close.
const MAX_SUGGESTIONS: usize = 5;

/// One person, as WhatsApp knows them.
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub name: String,
    /// International format, as stored.
    pub phone: String,
}

fn store_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    let path = std::path::Path::new(&home).join(STORE);
    path.exists().then_some(path)
}

/// Open the store read-only, or None when it is absent or unreadable.
fn open() -> Option<Connection> {
    // The unit suite must never read the real address book. It made results
    // depend on whose machine the tests ran on, and it put a live contact's
    // phone number into test output. The store is reachable under test only
    // when explicitly asked for, which is what the ignored probe below does.
    if cfg!(test) && std::env::var_os("NEXUS_WA_CONTACTS_LIVE").is_none() {
        return None;
    }
    let path = store_path()?;
    Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()
}

/// Every name and number in the store.
///
/// Private on purpose: nothing outside this module can obtain the list. The
/// two public functions filter it down to what the user just asked for.
fn all(conn: &Connection) -> Vec<Match> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT ZFULLNAME, ZPHONENUMBER
           FROM ZWAADDRESSBOOKCONTACT
          WHERE ZFULLNAME IS NOT NULL
            AND ZFULLNAME <> ''
            AND ZPHONENUMBER IS NOT NULL",
    ) else {
        return Vec::new();
    };

    let rows = stmt.query_map([], |row| {
        Ok(Match {
            name: row.get(0)?,
            phone: row.get(1)?,
        })
    });

    match rows {
        Ok(rows) => rows.flatten().collect(),
        Err(_) => Vec::new(),
    }
}

/// Everyone WhatsApp knows by exactly this name.
///
/// Exact rather than partial, deliberately: a loose match on a contact list
/// is how a message reaches the wrong person, and a sent message cannot be
/// recalled. Several people can share a name, so the caller gets all of them
/// and has to ask.
pub fn lookup(spoken: &str) -> Vec<Match> {
    let needle = spoken.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let Some(conn) = open() else {
        return Vec::new();
    };
    let mut found: Vec<Match> = all(&conn)
        .into_iter()
        .filter(|m| m.name.to_lowercase() == needle)
        .collect();
    found.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.phone.cmp(&b.phone)));
    found
}

/// Contacts whose name is close to what was said, closest first.
///
/// For the case this exists for: the chat says "Amma", dictation heard
/// "Ama", and neither NEXUS nor the recogniser can know which was meant.
/// These are offered to the user, never acted on.
pub fn suggest(spoken: &str) -> Vec<Match> {
    let needle = spoken.trim().to_lowercase();
    if needle.chars().count() < 2 {
        return Vec::new();
    }
    let Some(conn) = open() else {
        return Vec::new();
    };

    let mut scored: Vec<(usize, Match)> = all(&conn)
        .into_iter()
        .filter_map(|m| {
            let name = m.name.to_lowercase();
            // An exact match is not a suggestion; `lookup` already has it.
            if name == needle {
                return None;
            }
            close_enough(&name, &needle).then(|| (edit_distance(&name, &needle), m))
        })
        .collect();

    // Closest first, then by name and number, so the same question always
    // produces the same options in the same order.
    scored.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.name.cmp(&b.1.name))
            .then_with(|| a.1.phone.cmp(&b.1.phone))
    });
    scored
        .into_iter()
        .map(|(_, m)| m)
        .take(MAX_SUGGESTIONS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Production source, so these read the shipped code rather than the
    /// machine's data. They hold on any machine, with or without WhatsApp.
    fn production() -> &'static str {
        include_str!("wa_contacts.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("marker")
    }

    #[test]
    fn an_empty_name_reads_nothing() {
        // Guards the obvious mistake: a blank name matching a row with a
        // blank name, and messaging whoever that turns out to be.
        assert!(lookup("").is_empty());
        assert!(lookup("   ").is_empty());
        assert!(suggest("").is_empty());
        assert!(suggest("x").is_empty());
    }

    #[test]
    fn a_missing_store_is_not_an_error() {
        // WhatsApp not being installed is an ordinary state, and NEXUS's own
        // contacts table still works.
        assert!(lookup("nobody is called this at all").is_empty());
        assert!(suggest("nobody is called this at all").is_empty());
    }

    #[test]
    fn the_store_is_never_opened_for_writing() {
        assert!(production().contains("SQLITE_OPEN_READ_ONLY"));
        for forbidden in ["SQLITE_OPEN_READ_WRITE", "SQLITE_OPEN_CREATE", "DELETE", "UPDATE"] {
            assert!(!production().contains(forbidden), "found {forbidden}");
        }
    }

    #[test]
    fn the_address_book_cannot_be_obtained_through_this_module() {
        // `all` is the only thing that reads every row, and it is private.
        // If it ever becomes public, the two filtered functions stop being
        // the only way out.
        assert!(
            production().contains("fn all(conn: &Connection)"),
            "the full read must stay private"
        );
        assert!(
            !production().contains("pub fn all"),
            "the full list must not be reachable from outside"
        );
    }

    #[test]
    fn suggestions_are_bounded() {
        // Without the cap, a common name would return hundreds of options
        // and the question would be unanswerable.
        assert!(production().contains("take(MAX_SUGGESTIONS)"));
        assert!(MAX_SUGGESTIONS <= 5);
    }

    #[test]
    fn the_unit_suite_never_reads_the_real_address_book() {
        // Without this the suite depends on whose machine it runs on, and a
        // failing assertion prints a real person's phone number.
        assert!(production().contains("NEXUS_WA_CONTACTS_LIVE"));
        assert!(lookup("anything at all").is_empty());
        assert!(suggest("anything at all").is_empty());
    }

    /// Exercises the real store, on request only.
    ///
    /// `NEXUS_WA_CONTACTS_LIVE=1 cargo test -- --ignored live_store`
    ///
    /// Prints counts, never names or numbers: the point is that the schema
    /// still matches, not what is in it.
    #[test]
    #[ignore]
    fn live_store_still_has_the_shape_this_expects() {
        if std::env::var_os("NEXUS_WA_CONTACTS_LIVE").is_none() {
            eprintln!("set NEXUS_WA_CONTACTS_LIVE=1 to run this");
            return;
        }
        let Some(conn) = open() else {
            eprintln!("no WhatsApp store on this machine");
            return;
        };
        let rows = all(&conn);
        eprintln!("readable, {} contacts with a name and a number", rows.len());
        assert!(!rows.is_empty(), "schema moved: no rows came back");
        assert!(
            rows.iter().all(|m| !m.name.is_empty() && !m.phone.is_empty()),
            "a row came back without a name or number"
        );
    }

    #[test]
    fn lookup_is_exact_and_says_so_in_the_query() {
        // A LIKE here would let "Am" reach "Amma" and send to the wrong
        // person. The comparison is equality on the whole name.
        assert!(!production().contains("LIKE"));
        assert!(production().contains("m.name.to_lowercase() == needle"));
    }
}
