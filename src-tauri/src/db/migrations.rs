/// Each entry is (migration_id, sql).
/// IDs must be unique and increasing. Never remove or reorder existing entries.
/// Add new migrations at the end with the next sequential ID.
pub const MIGRATIONS: &[(i64, &str)] = &[
    (
        1,
        "
        -- Migration 001: initial schema

        CREATE TABLE IF NOT EXISTS ides (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            name            TEXT    NOT NULL,
            ide_type        TEXT    NOT NULL,
            executable_path TEXT,
            enabled         INTEGER NOT NULL DEFAULT 1,
            created_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );

        CREATE TABLE IF NOT EXISTS ai_agents (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            name            TEXT    NOT NULL,
            agent_type      TEXT    NOT NULL,
            enabled         INTEGER NOT NULL DEFAULT 1,
            executable_path TEXT,
            created_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );

        CREATE TABLE IF NOT EXISTS projects (
            id               INTEGER PRIMARY KEY AUTOINCREMENT,
            name             TEXT    NOT NULL,
            description      TEXT,
            repository_path  TEXT,
            repository_url   TEXT,
            default_ide_id   INTEGER REFERENCES ides(id) ON DELETE SET NULL,
            default_agent_id INTEGER REFERENCES ai_agents(id) ON DELETE SET NULL,
            created_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );

        CREATE TABLE IF NOT EXISTS tasks (
            id             INTEGER PRIMARY KEY AUTOINCREMENT,
            external_id    TEXT,
            title          TEXT    NOT NULL,
            description    TEXT,
            status         TEXT    NOT NULL DEFAULT 'open',
            project_id     INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            assigned_agent INTEGER REFERENCES ai_agents(id) ON DELETE SET NULL,
            created_at     TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at     TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_tasks_project_external
            ON tasks(project_id, external_id)
            WHERE external_id IS NOT NULL;

        CREATE TABLE IF NOT EXISTS settings (
            key        TEXT PRIMARY KEY,
            value      TEXT    NOT NULL,
            created_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );
        ",
    ),
];
