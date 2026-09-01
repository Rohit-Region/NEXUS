//! People NEXUS can message, by name.
//!
//! WhatsApp identifies people by phone number, so without this table
//! "message Divi" resolved to nobody: there was nowhere for a name to become
//! a number.
//!
//! Typed by the user and never synced. NEXUS does not read the macOS address
//! book, which would mean a Contacts permission and read access to everyone
//! the user knows; the only people it can reach are the ones deliberately
//! entered here.

use rusqlite::{Connection, Result as RusqliteResult};
use serde::{Deserialize, Serialize};

use crate::assistant::whatsapp_connector::valid_phone;

/// Longest accepted name. Long enough for a full name, bounded because it is
/// spoken back in a confirmation prompt.
const MAX_NAME: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Contact {
    pub id: i64,
    pub name: String,
    /// International format, digits only, as `valid_phone` returns it.
    pub phone: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactInput {
    /// Absent when creating.
    pub id: Option<i64>,
    pub name: String,
    pub phone: String,
}

fn row(r: &rusqlite::Row<'_>) -> RusqliteResult<Contact> {
    Ok(Contact {
        id: r.get(0)?,
        name: r.get(1)?,
        phone: r.get(2)?,
        created_at: r.get(3)?,
        updated_at: r.get(4)?,
    })
}

pub fn list_contacts(conn: &Connection) -> Result<Vec<Contact>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, name, phone, created_at, updated_at
               FROM contacts
              ORDER BY name COLLATE NOCASE ASC",
        )
        .map_err(|e| format!("Failed to read contacts: {e}"))?;
    let rows = stmt
        .query_map([], row)
        .map_err(|e| format!("Failed to read contacts: {e}"))?;
    rows.collect::<RusqliteResult<Vec<_>>>()
        .map_err(|e| format!("Failed to read contacts: {e}"))
}

/// Validate a name and number, or say exactly what is wrong with them.
///
/// The number goes through the same check the connector applies before
/// dialling, so a contact that saves is a contact that can be messaged.
/// Accepting one here and refusing it at send time would be a trap.
fn checked(input: &ContactInput) -> Result<(String, String), String> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err("A contact needs a name.".to_string());
    }
    if name.chars().count() > MAX_NAME {
        return Err(format!("That name is longer than {MAX_NAME} characters."));
    }
    let phone = valid_phone(&input.phone).ok_or_else(|| {
        format!(
            "\"{}\" is not an international phone number. Include the country code, \
             for example +91 98765 43210.",
            input.phone.trim()
        )
    })?;
    Ok((name.to_string(), phone))
}

/// A duplicate name is a conflict, not a crash.
///
/// Names are matched case-insensitively when spoken, so two contacts
/// differing only in case would be an ambiguity with no way to resolve it by
/// voice. The unique index enforces that; this turns the violation into a
/// sentence the user can act on.
fn store_error(err: rusqlite::Error, name: &str) -> String {
    let text = err.to_string();
    if text.contains("UNIQUE") {
        format!("There is already a contact called {name}.")
    } else {
        format!("Failed to save the contact: {text}")
    }
}

pub fn create_contact(conn: &Connection, input: &ContactInput) -> Result<Contact, String> {
    let (name, phone) = checked(input)?;
    conn.execute(
        "INSERT INTO contacts (name, phone) VALUES (?1, ?2)",
        rusqlite::params![name, phone],
    )
    .map_err(|e| store_error(e, &name))?;
    let id = conn.last_insert_rowid();
    get_contact(conn, id)
}

pub fn update_contact(conn: &Connection, input: &ContactInput) -> Result<Contact, String> {
    let id = input
        .id
        .ok_or_else(|| "Which contact should be changed?".to_string())?;
    let (name, phone) = checked(input)?;
    let changed = conn
        .execute(
            "UPDATE contacts
                SET name = ?2, phone = ?3,
                    updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
              WHERE id = ?1",
            rusqlite::params![id, name, phone],
        )
        .map_err(|e| store_error(e, &name))?;
    if changed == 0 {
        return Err("That contact no longer exists.".to_string());
    }
    get_contact(conn, id)
}

pub fn delete_contact(conn: &Connection, id: i64) -> Result<(), String> {
    let changed = conn
        .execute("DELETE FROM contacts WHERE id = ?1", [id])
        .map_err(|e| format!("Failed to delete the contact: {e}"))?;
    if changed == 0 {
        return Err("That contact no longer exists.".to_string());
    }
    Ok(())
}

pub fn get_contact(conn: &Connection, id: i64) -> Result<Contact, String> {
    conn.query_row(
        "SELECT id, name, phone, created_at, updated_at FROM contacts WHERE id = ?1",
        [id],
        row,
    )
    .map_err(|e| format!("Failed to read the contact: {e}"))
}

/// How far apart two names may be and still be offered as the same person.
///
/// One edit covers the transliteration differences that dictation produces:
/// "Ama" for "Amma", "Divia" for "Divya". Two edits are allowed for
/// longer names, where a single letter is proportionally less of the word.
pub(crate) fn close_enough(a: &str, b: &str) -> bool {
    let longest = a.chars().count().max(b.chars().count());
    if longest < 4 {
        // Short names are mostly distinct people: "Ana" and "Ann" are not a
        // spelling of each other.
        return a == b;
    }
    // A name that contains the other is the same person written shorter:
    // "Divya" for "Divya Raj".
    if a.contains(b) || b.contains(a) {
        return true;
    }
    let allowed = if longest >= 8 { 2 } else { 1 };
    edit_distance(a, b) <= allowed
}

/// Levenshtein distance, iterative with a single row.
pub(crate) fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != cb);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

/// Contacts whose name is close to what was said, best first.
///
/// Used only when nothing matched exactly. NEXUS never picks one of these
/// itself: they are offered, and the user says which. A near-match acted on
/// silently is how a message reaches the wrong person, and that cannot be
/// undone.
pub fn find_similar(conn: &Connection, spoken: &str) -> Vec<Contact> {
    let needle = spoken.trim().to_lowercase();
    if needle.len() < 2 {
        return Vec::new();
    }
    let mut scored: Vec<(usize, Contact)> = list_contacts(conn)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|c| {
            let name = c.name.to_lowercase();
            close_enough(&name, &needle).then(|| (edit_distance(&name, &needle), c))
        })
        .collect();
    // Closest first, then by name so the order never depends on row order.
    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name)));
    scored.into_iter().map(|(_, c)| c).take(5).collect()
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

    fn input(name: &str, phone: &str) -> ContactInput {
        ContactInput {
            id: None,
            name: name.to_string(),
            phone: phone.to_string(),
        }
    }

    #[test]
    fn a_contact_round_trips() {
        let conn = test_conn();
        let made = create_contact(&conn, &input("Divi", "+91 98765 43210")).expect("create");
        assert_eq!(made.name, "Divi");
        // Stored in the form the connector will use, not as typed.
        assert_eq!(made.phone, "919876543210");
        assert_eq!(list_contacts(&conn).expect("list").len(), 1);
    }

    #[test]
    fn a_number_that_could_not_be_messaged_is_refused_on_the_way_in() {
        // Saving a contact that fails at send time would be a trap: the user
        // would think it worked until the moment it mattered.
        let conn = test_conn();
        for bad in ["", "   ", "not a number", "12", "+91-98765-43210x"] {
            let err = create_contact(&conn, &input("Someone", bad)).expect_err(bad);
            assert!(err.contains("international phone number"), "{bad} -> {err}");
        }
    }

    #[test]
    fn a_contact_needs_a_name() {
        let conn = test_conn();
        for blank in ["", "   "] {
            assert!(create_contact(&conn, &input(blank, "+919876543210")).is_err());
        }
        let long = "x".repeat(MAX_NAME + 1);
        assert!(create_contact(&conn, &input(&long, "+919876543210")).is_err());
    }

    #[test]
    fn a_duplicate_name_is_refused_with_a_sentence_not_a_crash() {
        let conn = test_conn();
        create_contact(&conn, &input("Divi", "+919876543210")).expect("first");
        let err = create_contact(&conn, &input("divi", "+919999999999")).expect_err("second");
        assert!(err.contains("already a contact"), "{err}");
    }

    // -- Near-miss names -----------------------------------------------------

    #[test]
    fn a_transliteration_difference_still_finds_the_person() {
        // The case this exists for: the chat says "Amma", the user says
        // "Ama", and dictation has no way to know which.
        let conn = test_conn();
        create_contact(&conn, &input("Amma", "+919876543210")).expect("create");
        for spoken in ["Ama", "amma", "Ammi"] {
            let found = find_similar(&conn, spoken);
            assert_eq!(found.len(), 1, "{spoken} -> {found:?}");
            assert_eq!(found[0].name, "Amma", "{spoken}");
        }
    }

    #[test]
    fn a_shorter_form_of_a_longer_name_matches() {
        let conn = test_conn();
        create_contact(&conn, &input("Divya Raj", "+919876543210")).expect("create");
        assert_eq!(find_similar(&conn, "Divya").len(), 1);
    }

    #[test]
    fn a_different_person_is_not_offered() {
        // The whole point of asking rather than guessing is undone if the
        // suggestions are strangers.
        let conn = test_conn();
        create_contact(&conn, &input("Amma", "+919876543210")).expect("create");
        create_contact(&conn, &input("Rajesh", "+919000000000")).expect("create");
        let found = find_similar(&conn, "Ama");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Amma");
    }

    #[test]
    fn short_names_are_matched_exactly_only() {
        // "Ana" and "Ann" are one edit apart and are not the same person.
        let conn = test_conn();
        create_contact(&conn, &input("Ana", "+919876543210")).expect("create");
        assert!(find_similar(&conn, "Ann").is_empty());
        assert_eq!(find_similar(&conn, "Ana").len(), 1);
    }

    #[test]
    fn several_near_names_all_come_back_closest_first() {
        // The user picks; NEXUS must not decide for them. Order is by
        // distance so the likeliest is offered first.
        let conn = test_conn();
        for name in ["Amma", "Aman", "Ammaji", "Rajesh"] {
            create_contact(&conn, &input(name, "+919876543210")).expect("create");
        }
        let found = find_similar(&conn, "Ama");
        let names: Vec<&str> = found.iter().map(|c| c.name.as_str()).collect();
        // Both offered names are one edit away and tie, so the order falls
        // back to the name. "Ammaji" is three edits and is deliberately not
        // offered: past one edit on a name this short the suggestions stop
        // being the same person.
        assert_eq!(names, vec!["Aman", "Amma"]);
    }

    #[test]
    fn suggestions_are_bounded_and_repeatable() {
        let conn = test_conn();
        for i in 0..12 {
            create_contact(&conn, &input(&format!("Amma{i}"), "+919876543210")).expect("create");
        }
        let first = find_similar(&conn, "Amma");
        assert!(first.len() <= 5, "{} suggestions", first.len());
        for _ in 0..5 {
            assert_eq!(find_similar(&conn, "Amma"), first);
        }
    }

    #[test]
    fn nothing_is_suggested_for_a_name_with_no_likeness() {
        let conn = test_conn();
        create_contact(&conn, &input("Amma", "+919876543210")).expect("create");
        for spoken in ["", "x", "Christopher"] {
            assert!(find_similar(&conn, spoken).is_empty(), "{spoken}");
        }
    }

    #[test]
    fn updating_keeps_the_row_and_changes_the_number() {
        let conn = test_conn();
        let made = create_contact(&conn, &input("Divi", "+919876543210")).expect("create");
        let changed = update_contact(
            &conn,
            &ContactInput {
                id: Some(made.id),
                name: "Divi".to_string(),
                phone: "+91 90000 00000".to_string(),
            },
        )
        .expect("update");
        assert_eq!(changed.id, made.id);
        assert_eq!(changed.phone, "919000000000");
        assert_eq!(list_contacts(&conn).expect("list").len(), 1);
    }

    #[test]
    fn deleting_something_that_is_gone_says_so() {
        let conn = test_conn();
        let made = create_contact(&conn, &input("Divi", "+919876543210")).expect("create");
        delete_contact(&conn, made.id).expect("delete");
        assert!(delete_contact(&conn, made.id).is_err());
        assert!(list_contacts(&conn).expect("list").is_empty());
    }

    #[test]
    fn contacts_come_back_in_a_stable_order() {
        let conn = test_conn();
        for name in ["zara", "Divi", "alex"] {
            create_contact(&conn, &input(name, "+919876543210")).expect("create");
        }
        let names: Vec<String> = list_contacts(&conn)
            .expect("list")
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, vec!["alex", "Divi", "zara"]);
    }
}
