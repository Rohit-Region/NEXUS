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
    (
        8,
        "
        -- Migration 008: the personal profile ('Me').
        --
        -- What NEXUS knows about the person it works for: goals, the role they
        -- are aiming at, skills, what they are learning, their financial
        -- targets, a free-text 'about'. Every pillar (news, money, learning,
        -- career, the briefing) reads this so it personalises from one source
        -- of truth rather than re-asking who the user is.
        --
        -- Key-value on purpose. New profile fields arrive as the pillars are
        -- built, and a key-value store adds one without a migration each time.
        -- Only the user writes here; nothing is inferred from conversation,
        -- the same rule the commitments table keeps.
        CREATE TABLE IF NOT EXISTS profile (
            key        TEXT    PRIMARY KEY,
            value      TEXT    NOT NULL,
            updated_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );
        ",
    ),
    (
        9,
        "
        -- Migration 009: portfolio holdings (Money section).
        --
        -- What the user owns, priced live by symbol. A share and a gold ETF are
        -- the same row shape: a Yahoo symbol and a quantity. Gold is held as an
        -- ETF rather than grams so it prices in INR through the same path as a
        -- stock, with no spot-plus-forex-plus-purity math to get wrong.
        --
        -- No prices are stored. A stored price goes stale the moment it is
        -- written; the value is always fetched fresh and shown, never kept.
        -- Only what the user entered lives here: the symbol and how much they
        -- hold.
        CREATE TABLE IF NOT EXISTS holdings (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            symbol     TEXT    NOT NULL,
            label      TEXT    NOT NULL DEFAULT '',
            quantity   REAL    NOT NULL,
            created_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );
        ",
    ),
    (
        10,
        "
        -- Migration 010: holding kinds.
        --
        -- A portfolio is not only listed stocks. A mutual fund is valued once a
        -- day at its NAV by AMFI scheme code, not by a live ticker; a ULIP or a
        -- cash balance has no feed at all and is a figure the user keeps
        -- current themselves. 'kind' says which, so each row is priced the
        -- right way:
        --   stock  - symbol is a market ticker, quantity is units      (live)
        --   mf     - symbol is an AMFI scheme code, quantity is units   (daily NAV)
        --   manual - symbol is empty, quantity is the rupee value       (as entered)
        -- Existing rows are stocks, which is the default.
        ALTER TABLE holdings ADD COLUMN kind TEXT NOT NULL DEFAULT 'stock';
        ",
    ),
    (
        11,
        "
        -- Migration 011: savings goals (Money section).
        --
        -- A thing the user is saving toward, with a target and a date. Progress
        -- and the monthly saving needed are computed from these fields and the
        -- current date, never stored, so they are never stale. Only what the
        -- user entered lives here.
        CREATE TABLE IF NOT EXISTS goals (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            name            TEXT    NOT NULL,
            target_amount   REAL    NOT NULL,
            current_amount  REAL    NOT NULL DEFAULT 0,
            -- YYYY-MM-DD.
            target_date     TEXT    NOT NULL,
            -- Assumed annual return while saving, as a percentage.
            expected_return REAL    NOT NULL DEFAULT 8,
            created_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );
        ",
    ),
    (
        12,
        "
        -- Migration 012: recurring money obligations (Money > Bills).
        --
        -- Bills, EMIs, insurance premiums, subscriptions. The next due date is
        -- derived from the anchor and recurrence relative to today, never
        -- stored, so a monthly bill shows its upcoming date without anyone
        -- marking it paid. India's statutory tax dates are not here; they are
        -- fixed each year and computed, not entered.
        CREATE TABLE IF NOT EXISTS bills (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            name        TEXT    NOT NULL,
            -- bill | emi | insurance | subscription
            kind        TEXT    NOT NULL DEFAULT 'bill',
            amount      REAL,
            -- once | monthly | quarterly | yearly
            recurrence  TEXT    NOT NULL DEFAULT 'monthly',
            -- YYYY-MM-DD anchor; the next occurrence is computed from it.
            anchor_date TEXT    NOT NULL,
            created_at  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );
        ",
    ),
    (
        13,
        "
        -- Migration 013: Learn. Skills, their plan, flashcards, and reviews.
        --
        -- A skill has an ordered plan of topics (model-drafted, user-editable
        -- by ticking off) and a deck of cards scheduled by spaced repetition.
        -- Days are whole UTC day numbers since the epoch, which keeps 'due
        -- today' a plain integer comparison. Reviews are kept only as the day
        -- and grade, enough for the streak and nothing more.
        CREATE TABLE IF NOT EXISTS skills (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            name       TEXT    NOT NULL,
            created_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );

        CREATE TABLE IF NOT EXISTS skill_topics (
            id       INTEGER PRIMARY KEY AUTOINCREMENT,
            skill_id INTEGER NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
            title    TEXT    NOT NULL,
            position INTEGER NOT NULL DEFAULT 0,
            done     INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS cards (
            id        INTEGER PRIMARY KEY AUTOINCREMENT,
            skill_id  INTEGER NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
            topic_id  INTEGER REFERENCES skill_topics(id) ON DELETE SET NULL,
            front     TEXT    NOT NULL,
            back      TEXT    NOT NULL,
            ease      REAL    NOT NULL DEFAULT 2.5,
            interval  INTEGER NOT NULL DEFAULT 0,
            reps      INTEGER NOT NULL DEFAULT 0,
            -- UTC day number the card is next due. New cards are due at once.
            due_day   INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS cards_due ON cards (skill_id, due_day);

        CREATE TABLE IF NOT EXISTS card_reviews (
            id       INTEGER PRIMARY KEY AUTOINCREMENT,
            card_id  INTEGER NOT NULL,
            grade    TEXT    NOT NULL,
            day      INTEGER NOT NULL
        );
        ",
    ),
    (
        14,
        "
        -- Migration 014: the job application tracker (Career).
        --
        -- Roles saved from discovery and roles applied to anywhere else
        -- (LinkedIn, Naukri, referrals), in one pipeline. NEXUS never submits an
        -- application; this records what the user did and reminds them of the
        -- next step they set.
        CREATE TABLE IF NOT EXISTS job_applications (
            id               INTEGER PRIMARY KEY AUTOINCREMENT,
            company          TEXT    NOT NULL,
            role             TEXT    NOT NULL,
            url              TEXT    NOT NULL DEFAULT '',
            -- Where it came from: Remotive, We Work Remotely, or manual.
            source           TEXT    NOT NULL DEFAULT 'manual',
            -- saved | applied | interviewing | offer | rejected
            status           TEXT    NOT NULL DEFAULT 'saved',
            next_action      TEXT    NOT NULL DEFAULT '',
            -- YYYY-MM-DD, optional follow-up date.
            next_action_date TEXT,
            notes            TEXT    NOT NULL DEFAULT '',
            created_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );
        ",
    ),
    (
        15,
        "
        -- Migration 015: resume tailoring.
        --
        -- `documents` holds long user-written text, starting with the base
        -- resume. It is deliberately not in `profile`: every profile field is
        -- prepended to model prompts (Learn, the briefing), and a full resume
        -- in every one of those would bloat them for nothing.
        --
        -- Each application keeps the job description it was tailored against
        -- (snapshotted from discovery, or pasted), and the last tailored
        -- resume and cover letter, so they are there to copy later.
        CREATE TABLE IF NOT EXISTS documents (
            key        TEXT PRIMARY KEY,
            content    TEXT NOT NULL,
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );

        ALTER TABLE job_applications ADD COLUMN description     TEXT NOT NULL DEFAULT '';
        ALTER TABLE job_applications ADD COLUMN tailored_resume TEXT NOT NULL DEFAULT '';
        ALTER TABLE job_applications ADD COLUMN cover_letter    TEXT NOT NULL DEFAULT '';
        ",
    ),
    (
        16,
        "
        -- Migration 016: mock interview practice.
        --
        -- Questions are generated per application from its job description and
        -- the resume; the user's answer and the coach's feedback are kept so a
        -- practice session can be picked up again. Nothing here is sent
        -- anywhere: the model runs on this machine.
        CREATE TABLE IF NOT EXISTS interview_questions (
            id             INTEGER PRIMARY KEY AUTOINCREMENT,
            application_id INTEGER NOT NULL,
            question       TEXT    NOT NULL,
            answer         TEXT    NOT NULL DEFAULT '',
            feedback       TEXT    NOT NULL DEFAULT '',
            created_at     TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );

        CREATE INDEX IF NOT EXISTS interview_questions_app
            ON interview_questions (application_id);
        ",
    ),
    (
        17,
        "
        -- Migration 017: the supervised task clone.
        --
        -- `clone_allowed` is the consent: a task can run against a project
        -- only once the user has switched it on for that project, because the
        -- repository is read by Claude Code and leaves the machine. Off for
        -- every existing project.
        --
        -- A run records what happened at each stage so the user can judge it:
        -- Claude's own summary, the diff, and the test result NEXUS observed
        -- (NULL when no test command was found, never assumed to pass).
        ALTER TABLE projects ADD COLUMN clone_allowed INTEGER NOT NULL DEFAULT 0;

        CREATE TABLE IF NOT EXISTS clone_runs (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            project_id  INTEGER NOT NULL,
            task        TEXT    NOT NULL,
            -- queued | preparing | editing | testing | ready | failed | applied | discarded
            status      TEXT    NOT NULL DEFAULT 'queued',
            worktree    TEXT    NOT NULL DEFAULT '',
            base_commit TEXT    NOT NULL DEFAULT '',
            summary     TEXT    NOT NULL DEFAULT '',
            diff        TEXT    NOT NULL DEFAULT '',
            diff_stat   TEXT    NOT NULL DEFAULT '',
            test_label  TEXT    NOT NULL DEFAULT '',
            test_passed INTEGER,
            test_output TEXT    NOT NULL DEFAULT '',
            error       TEXT    NOT NULL DEFAULT '',
            created_at  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            finished_at TEXT
        );
        ",
    ),
    (
        18,
        "
        -- Migration 018: spending.
        --
        -- Amounts are whole paise, never floating rupees, so a month of small
        -- payments adds up to exactly what the bank says.
        --
        -- `kind` decides what counts as spending: only 'spend' does. Income,
        -- refunds and transfers (a credit card bill paid from the bank account,
        -- an investment, money to yourself) are shown but never added to it,
        -- or a card purchase would be counted twice: once when made and again
        -- when the bill is paid.
        CREATE TABLE IF NOT EXISTS expenses (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            source       TEXT    NOT NULL,              -- sms | manual
            sms_id       TEXT    UNIQUE,
            occurred_on  TEXT    NOT NULL,              -- YYYY-MM-DD, local
            occurred_at  INTEGER,                       -- unix seconds, when known
            amount_paise INTEGER NOT NULL CHECK (amount_paise > 0),
            kind         TEXT    NOT NULL DEFAULT 'spend', -- spend | income | refund | transfer
            payee        TEXT    NOT NULL,
            payee_key    TEXT    NOT NULL,
            category     TEXT    NOT NULL DEFAULT 'Uncategorised',
            account      TEXT    NOT NULL,
            channel      TEXT    NOT NULL DEFAULT '',
            note         TEXT    NOT NULL DEFAULT '',
            raw          TEXT    NOT NULL DEFAULT '',
            created_at   TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );

        CREATE INDEX IF NOT EXISTS expenses_day ON expenses (occurred_on);

        -- What the user taught NEXUS about a payee: its category, and whether
        -- paying it is spending at all.
        CREATE TABLE IF NOT EXISTS payee_rules (
            payee_key TEXT PRIMARY KEY,
            category  TEXT NOT NULL,
            kind      TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS spend_budgets (
            category      TEXT PRIMARY KEY,
            monthly_paise INTEGER NOT NULL CHECK (monthly_paise > 0)
        );

        -- Every bank message fetched, kept so nothing is lost: read ones
        -- become expenses ('done'), unreadable ones wait for the user
        -- ('review'), and non-payments (balance alerts, dues) are 'ignored'.
        CREATE TABLE IF NOT EXISTS sms_inbox (
            sms_id      TEXT    PRIMARY KEY,
            sender      TEXT    NOT NULL,
            text        TEXT    NOT NULL,
            received_at INTEGER,
            status      TEXT    NOT NULL,
            reason      TEXT    NOT NULL DEFAULT '',
            suggestion  TEXT    NOT NULL DEFAULT ''
        );
        ",
    ),
    (
        19,
        "
        -- Migration 019: a name the user gives a payee.
        --
        -- Banks name a payee by a masked UPI handle like '*u766@pty'. A rule
        -- stays keyed on the bank's name, so it keeps matching new messages,
        -- and `display` is what the user calls it. Empty means keep the
        -- bank's name.
        ALTER TABLE payee_rules ADD COLUMN display TEXT NOT NULL DEFAULT '';
        ",
    ),
    (
        20,
        "
        -- Migration 020: the last known balance of each bank account, from a
        -- bank SMS that states it ('Avl Bal Rs. ...') or set by the user.
        -- The current balance is this, adjusted for payments booked since.
        CREATE TABLE IF NOT EXISTS account_balances (
            account       TEXT    PRIMARY KEY,
            balance_paise INTEGER NOT NULL,
            as_of         INTEGER NOT NULL,   -- unix seconds
            source        TEXT    NOT NULL    -- sms | manual
        );
        ",
    ),
    (
        21,
        "
        -- Migration 021: phone call alerts already sent, so each meeting,
        -- message or reminder rings the phone once.
        CREATE TABLE IF NOT EXISTS phone_calls_sent (
            key     TEXT    PRIMARY KEY,
            sent_at INTEGER NOT NULL
        );
        ",
    ),
    (
        22,
        "
        -- Migration 022: where a holding is held, what went into it, and
        -- prices the user enters.
        --
        -- `broker` groups holdings (ICICI Direct, Zerodha, a ULIP policy).
        -- `invested` is what was paid for it, for gain and loss; null when
        -- unknown, never guessed. `unit_price` is a NAV the user enters for a
        -- fund with no public feed (a ULIP fund), dated by `price_as_of`.
        ALTER TABLE holdings ADD COLUMN broker TEXT NOT NULL DEFAULT '';
        ALTER TABLE holdings ADD COLUMN invested REAL;
        ALTER TABLE holdings ADD COLUMN unit_price REAL;
        ALTER TABLE holdings ADD COLUMN price_as_of TEXT;

        -- Money put into a group as a whole, such as ULIP premiums, which are
        -- paid to the policy rather than to any one fund.
        CREATE TABLE IF NOT EXISTS portfolio_groups (
            name     TEXT PRIMARY KEY,
            invested REAL NOT NULL DEFAULT 0
        );
        ",
    ),
    (
        23,
        "
        -- Migration 023: the pick journal. One row per suggestion per trading
        -- day: what was suggested (score, verdict, factor scores as JSON) at
        -- what price, with the benchmark level beside it (the Nifty for a
        -- stock; null for a fund, whose benchmark is its peers that day).
        -- `retN` / `benchN` are percentage returns N sessions later, null
        -- until that window has passed.
        CREATE TABLE IF NOT EXISTS pick_log (
            day     INTEGER NOT NULL,
            kind    TEXT    NOT NULL CHECK (kind IN ('stock', 'fund')),
            symbol  TEXT    NOT NULL,
            score   REAL    NOT NULL,
            verdict TEXT    NOT NULL,
            price   REAL    NOT NULL,
            bench   REAL,
            factors TEXT    NOT NULL DEFAULT '{}',
            ret5    REAL,
            ret20   REAL,
            ret60   REAL,
            bench5  REAL,
            bench20 REAL,
            bench60 REAL,
            PRIMARY KEY (day, kind, symbol)
        );
        ",
    ),
];
