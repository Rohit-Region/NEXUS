//! Shared shapes for the two registry tables, `ides` and `ai_agents`.
//!
//! The tables are isomorphic: they differ only in the name of the type column
//! (`ide_type` against `agent_type`). Declaring the serde contract once here
//! means it cannot drift between them. The CRUD functions themselves stay
//! concrete in `ides.rs` and `agents.rs`, mirroring `projects.rs` / `tasks.rs`.

use rusqlite::Result as RusqliteResult;
use serde::{Deserialize, Serialize};

/// A registry row returned to the frontend.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryEntry {
    pub id: i64,
    pub name: String,
    /// `ides.ide_type` or `ai_agents.agent_type`, normalised to one field name.
    pub entry_type: String,
    pub executable_path: Option<String>,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Input for registering a new entry (from React).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRegistryEntryInput {
    pub name: String,
    pub entry_type: String,
    pub executable_path: Option<String>,
    /// None means the schema default, enabled.
    pub enabled: Option<bool>,
}

/// Input for updating an existing entry (from React).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRegistryEntryInput {
    pub id: i64,
    pub name: String,
    pub entry_type: String,
    pub executable_path: Option<String>,
    pub enabled: bool,
}

/// Non-emptiness only. Type labels are user-supplied metadata describing an
/// installed tool, so there is deliberately no closed vocabulary here (spec 2.6).
pub fn validate_entry(name: &str, entry_type: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("Name cannot be empty".to_string());
    }
    if entry_type.trim().is_empty() {
        return Err("Type cannot be empty".to_string());
    }
    Ok(())
}

/// Both modules SELECT their columns in this order, so one mapper serves both.
pub fn map_entry_row(row: &rusqlite::Row<'_>) -> RusqliteResult<RegistryEntry> {
    Ok(RegistryEntry {
        id:              row.get(0)?,
        name:            row.get(1)?,
        entry_type:      row.get(2)?,
        executable_path: row.get(3)?,
        enabled:         row.get(4)?,
        created_at:      row.get(5)?,
        updated_at:      row.get(6)?,
    })
}
