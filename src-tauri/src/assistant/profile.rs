//! The personal profile: what NEXUS knows about the person it works for.
//!
//! # Why this exists
//!
//! Each pillar (news, money, learning, career, the briefing) needs the same
//! facts: who you are, what you are aiming at, what you are saving toward. Held
//! here once, every pillar reads the same source of truth and personalises from
//! it, rather than re-asking. This is the backbone that turns a set of cards
//! into an assistant.
//!
//! # What it is not
//!
//! Inferred. Nothing lands here except what the user typed into the profile.
//! NEXUS does not guess your goals from your calendar or your messages; a
//! profile built from guesses would be confidently wrong, and would then skew
//! every pillar that reads it. The same rule the commitments table keeps.

use std::collections::BTreeMap;

use rusqlite::Connection;

/// Longest a single field may be. A profile is a handful of lines per field,
/// not a document; this stops an accidental paste from bloating every prompt
/// that includes the profile.
const MAX_VALUE: usize = 4_000;

/// Every stored field, as a key to value map. Ordered so the profile reads the
/// same way each time it is turned into prompt context.
pub fn get_all(conn: &Connection) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut stmt = match conn.prepare("SELECT key, value FROM profile") {
        Ok(stmt) => stmt,
        Err(_) => return out,
    };
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)));
    if let Ok(rows) = rows {
        for row in rows.flatten() {
            out.insert(row.0, row.1);
        }
    }
    out
}

/// Set one field, or clear it when the value is blank. Returns the cleaned
/// value stored (empty string when cleared), so the UI reflects exactly what
/// was kept.
pub fn set_field(conn: &Connection, key: &str, value: &str) -> Result<String, String> {
    let key = key.trim();
    if key.is_empty() || key.len() > 64 {
        return Err("That is not a valid profile field.".to_string());
    }
    let value = value.trim();
    if value.len() > MAX_VALUE {
        return Err(format!("That is longer than {MAX_VALUE} characters."));
    }

    if value.is_empty() {
        conn.execute("DELETE FROM profile WHERE key = ?1", rusqlite::params![key])
            .map_err(|e| format!("Could not clear the field: {e}"))?;
        return Ok(String::new());
    }
    conn.execute(
        "INSERT INTO profile (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE
           SET value = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        rusqlite::params![key, value],
    )
    .map_err(|e| format!("Could not save the field: {e}"))?;
    Ok(value.to_string())
}

/// The profile as prompt lines, for the model to read when personalising a
/// pillar. Empty when nothing is set, so a caller can skip the section rather
/// than send an empty heading. Human-readable labels, because the model reads
/// this as context, not as a schema.
///
/// Contact details (keys starting `contact_`) are never included. They exist
/// so the user can copy them into application forms; no model needs an email
/// address or phone number to write a learning plan or a cover letter.
pub fn as_context_lines(conn: &Connection) -> Vec<String> {
    let all = get_all(conn);
    all.iter()
        .filter(|(k, v)| !v.trim().is_empty() && !k.starts_with(CONTACT_PREFIX))
        .map(|(k, v)| format!("{}: {}", label_for(k), v.trim()))
        .collect()
}

/// Profile keys holding contact details, kept out of every prompt.
pub const CONTACT_PREFIX: &str = "contact_";

/// A readable label for a known field key; the key itself (spaced) otherwise,
/// so a field added later still renders sensibly without a code change.
fn label_for(key: &str) -> String {
    match key {
        "display_name" => "Name".to_string(),
        "role_current" => "Current role".to_string(),
        "role_target" => "Target role".to_string(),
        "skills" => "Skills".to_string(),
        "learning_focus" => "Learning".to_string(),
        "goals" => "Goals".to_string(),
        "retirement_target_age" => "Target retirement age".to_string(),
        "about" => "About".to_string(),
        other => other.replace('_', " "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        for (_, sql) in crate::db::migrations::MIGRATIONS {
            conn.execute_batch(sql).expect("migrate");
        }
        conn
    }

    #[test]
    fn a_field_round_trips_and_updates_in_place() {
        let c = conn();
        assert_eq!(set_field(&c, "goals", "₹20L house by 2030").unwrap(), "₹20L house by 2030");
        assert_eq!(get_all(&c).get("goals").map(String::as_str), Some("₹20L house by 2030"));
        // A second write to the same key replaces, not duplicates.
        set_field(&c, "goals", "₹25L house by 2031").unwrap();
        assert_eq!(get_all(&c).len(), 1);
        assert_eq!(get_all(&c)["goals"], "₹25L house by 2031");
    }

    #[test]
    fn a_blank_value_clears_the_field() {
        let c = conn();
        set_field(&c, "skills", "React, Rust").unwrap();
        assert_eq!(set_field(&c, "skills", "   ").unwrap(), "");
        assert!(get_all(&c).get("skills").is_none());
    }

    #[test]
    fn context_lines_use_readable_labels_and_skip_empties() {
        let c = conn();
        set_field(&c, "role_target", "Senior Frontend Engineer").unwrap();
        set_field(&c, "display_name", "Rohit").unwrap();
        let lines = as_context_lines(&c);
        assert!(lines.contains(&"Target role: Senior Frontend Engineer".to_string()));
        assert!(lines.contains(&"Name: Rohit".to_string()));
    }

    #[test]
    fn contact_details_never_reach_a_prompt() {
        let c = conn();
        set_field(&c, "role_target", "Senior Frontend Engineer").unwrap();
        set_field(&c, "contact_email", "me@example.com").unwrap();
        set_field(&c, "contact_phone", "+919000000000").unwrap();
        let lines = as_context_lines(&c).join("\n");
        assert!(lines.contains("Senior Frontend Engineer"));
        assert!(!lines.contains("me@example.com"), "{lines}");
        assert!(!lines.contains("+919000000000"), "{lines}");
        // Still stored, for the copy buttons.
        assert_eq!(get_all(&c)["contact_email"], "me@example.com");
    }

    #[test]
    fn an_oversized_value_is_refused() {
        let c = conn();
        let huge = "x".repeat(MAX_VALUE + 1);
        assert!(set_field(&c, "about", &huge).is_err());
    }

    #[test]
    fn a_blank_key_is_refused() {
        let c = conn();
        assert!(set_field(&c, "  ", "value").is_err());
    }
}
