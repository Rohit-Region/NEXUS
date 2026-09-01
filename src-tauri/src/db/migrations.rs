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
    (
        2,
        "
        -- Migration 002: NEXUS-012 assistant action system.
        --
        -- Three tables, deliberately mirroring the shape of the existing
        -- registry tables so the same CRUD habits apply. Nothing here stores
        -- content: `summary` holds the sentence the user already read and
        -- approved, never data NEXUS merely observed.
        CREATE TABLE IF NOT EXISTS connectors (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            connector_id TEXT    NOT NULL UNIQUE,
            display_name TEXT    NOT NULL,
            enabled      INTEGER NOT NULL DEFAULT 1,
            config_json  TEXT,
            created_at   TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at   TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );

        -- A row IS a standing grant. Absence is denial, so revoking is a
        -- DELETE and there is no tri-state to get wrong.
        CREATE TABLE IF NOT EXISTS permission_grants (
            connector_id TEXT NOT NULL
                REFERENCES connectors(connector_id) ON DELETE CASCADE,
            level        TEXT NOT NULL,
            granted_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            PRIMARY KEY (connector_id, level)
        );

        CREATE TABLE IF NOT EXISTS action_audit (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            action_id    TEXT    NOT NULL,
            connector_id TEXT    NOT NULL,
            permission   TEXT    NOT NULL,
            summary      TEXT    NOT NULL,
            outcome      TEXT    NOT NULL,
            error        TEXT,
            duration_ms  INTEGER,
            approved     INTEGER NOT NULL DEFAULT 0,
            created_at   TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );
        CREATE INDEX IF NOT EXISTS idx_action_audit_created
            ON action_audit(created_at DESC, id DESC);

        -- The local connector. It is NEXUS acting on its own workspace, so it
        -- is authorised on arrival; the grants exist to be revocable, not to
        -- be earned. `execute` is deliberately absent: nexus.* runs no
        -- commands, and a level with no actions should not be granted.
        INSERT OR IGNORE INTO connectors (connector_id, display_name)
            VALUES ('nexus', 'NEXUS Workspace');
        INSERT OR IGNORE INTO permission_grants (connector_id, level) VALUES
            ('nexus', 'read'),
            ('nexus', 'interact'),
            ('nexus', 'write'),
            ('nexus', 'destructive');
        ",
    ),
    (
        3,
        "
        -- Migration 003: NEXUS-019 reasoning audit.
        --
        -- Answers one question: why did NEXUS contact a reasoning provider?
        -- Categories, never contents. There is deliberately no column for the
        -- prompt or the response, so a future caller cannot quietly start
        -- keeping them: storing them to prove what was sent would recreate
        -- exactly the data store this architecture avoids.
        CREATE TABLE IF NOT EXISTS ai_audit (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            provider     TEXT    NOT NULL,
            model        TEXT    NOT NULL,
            reach        TEXT    NOT NULL,
            purpose      TEXT    NOT NULL,
            categories   TEXT    NOT NULL,
            outcome      TEXT    NOT NULL,
            duration_ms  INTEGER,
            created_at   TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );
        CREATE INDEX IF NOT EXISTS idx_ai_audit_created
            ON ai_audit(created_at DESC, id DESC);
        ",
    ),
    (
        4,
        "
        -- Migration 004: NEXUS-020 suggestions.
        --
        -- Only dismissals are stored, not suggestions. A suggestion is
        -- derived from current data every time it is asked for, so it cannot
        -- go stale, cannot be resurrected after the thing it referred to is
        -- gone, and needs no cleanup job. What has to persist is the user
        -- saying 'not this one', because that is the only part NEXUS cannot
        -- recompute.
        CREATE TABLE IF NOT EXISTS suggestion_dismissals (
            suggestion_key TEXT PRIMARY KEY,
            dismissed_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );
        ",
    ),
    (
        5,
        "
        -- Migration 005: NEXUS-021 surfacing history.
        --
        -- Separate from dismissals because they answer different questions.
        -- A dismissal is 'never again'; this is 'not just now'. Without it a
        -- cooldown resets on every restart, and an assistant that repeats
        -- itself every launch is one you learn to ignore.
        CREATE TABLE IF NOT EXISTS suggestion_activity (
            suggestion_key TEXT PRIMARY KEY,
            last_shown_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            shown_count    INTEGER NOT NULL DEFAULT 1
        );
        ",
    ),
    (
        6,
        "
        -- Migration 006: contacts.
        --
        -- WhatsApp identifies people by number, so 'message Divi' could not
        -- resolve to anyone: there was nowhere for a name to become a
        -- number. This is that mapping and nothing more.
        --
        -- Typed by the user, never synced. NEXUS does not read the macOS
        -- address book, so the only people it knows are the ones deliberately
        -- put here.
        --
        -- The number is stored as given and validated at use, not on write:
        -- validation lives in one place (whatsapp_connector::valid_phone) and
        -- a row that fails it should report that rather than be silently
        -- unstorable.
        CREATE TABLE IF NOT EXISTS contacts (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            name       TEXT NOT NULL,
            phone      TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );

        -- Names are matched case-insensitively when spoken, so two contacts
        -- differing only in case would be an ambiguity with no way to
        -- resolve it by voice.
        CREATE UNIQUE INDEX IF NOT EXISTS contacts_name_unique
            ON contacts (name COLLATE NOCASE);
        ",
    ),
    (
        7,
        "
        -- Migration 007: commitments. NEXUS-028.
        --
        -- The shorter-lived thing you say out loud and forget by lunchtime.
        -- Deliberately not a task: tasks belong to a project, outlive a day,
        -- and are something you go and look at. A commitment is something
        -- NEXUS brings back to you once, at a time you named, and then stops.
        --
        -- **Only what the user said explicitly lands here.** Nothing is
        -- inferred from conversation. A system that guesses at commitments
        -- will be wrong often, and being reminded of something you never
        -- agreed to is worse than not being reminded at all.
        --
        -- `raised_at` is what makes 'once' true: a commitment is brought up
        -- when due and then left alone, however long it stays open. Nagging
        -- is what trains someone to ignore an assistant.
        CREATE TABLE IF NOT EXISTS commitments (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            -- The user's own words, kept verbatim so NEXUS repeats what they
            -- said rather than its own summary of it.
            what       TEXT    NOT NULL,
            -- Unix seconds. Null means someday: recorded, never raised, and
            -- visible in the UI where the user can act on it themselves.
            due_at     INTEGER,
            -- open | done | dropped
            state      TEXT    NOT NULL DEFAULT 'open',
            raised_at  INTEGER,
            created_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );

        CREATE INDEX IF NOT EXISTS commitments_due
            ON commitments (state, due_at);
        ",
    ),
];
