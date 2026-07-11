use rusqlite::{Connection, params};

use super::reset_ledger;

pub const LATEST_SCHEMA_VERSION: i64 = 4;

const BASE_SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS agent_slots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL COLLATE NOCASE UNIQUE,
    workdir TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS agent_sessions (
    slot_id INTEGER PRIMARY KEY,
    thread_id TEXT NOT NULL,
    workdir TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (slot_id) REFERENCES agent_slots(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS agent_messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    slot_id INTEGER NOT NULL,
    role TEXT NOT NULL,
    body TEXT NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY (slot_id) REFERENCES agent_slots(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_agent_messages_slot_id ON agent_messages(slot_id, id);
"#;

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        PRAGMA foreign_keys = ON;
        CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        "#,
    )?;

    let current = current_version(conn)?;
    if current < 1 {
        conn.execute_batch(BASE_SCHEMA)?;
        conn.execute_batch(reset_ledger::SCHEMA)?;
        record_migration(conn, 1)?;
    }
    if current < 2 {
        migrate_single_codex_lane(conn)?;
        record_migration(conn, 2)?;
    }
    if current < 3 {
        migrate_agent_slot_goals(conn)?;
        record_migration(conn, 3)?;
    }
    if current < 4 {
        migrate_remove_attachments(conn)?;
        record_migration(conn, 4)?;
    }
    debug_assert!(current_version(conn)? >= LATEST_SCHEMA_VERSION);
    Ok(())
}

fn current_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
        row.get::<_, Option<i64>>(0)
    })
    .map(|version| version.unwrap_or(0))
}

fn record_migration(conn: &Connection, version: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
        params![version],
    )?;
    Ok(())
}

fn migrate_single_codex_lane(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE agent_slots
         SET name = 'codex'
         WHERE lower(name) = 'a'
           AND NOT EXISTS (SELECT 1 FROM agent_slots WHERE lower(name) = 'codex')",
        [],
    )?;
    conn.execute(
        "DELETE FROM agent_slots
         WHERE lower(name) IN ('a', 'b', 'c', 'd', 'e')
           AND NOT EXISTS (SELECT 1 FROM agent_messages WHERE slot_id = agent_slots.id)
           AND NOT EXISTS (SELECT 1 FROM agent_sessions WHERE slot_id = agent_slots.id)",
        [],
    )?;
    Ok(())
}

fn migrate_agent_slot_goals(conn: &Connection) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(agent_slots)")?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if columns.iter().any(|column| column == "goal") {
        return Ok(());
    }
    conn.execute(
        "ALTER TABLE agent_slots ADD COLUMN goal TEXT NOT NULL DEFAULT ''",
        [],
    )?;
    Ok(())
}

fn migrate_remove_attachments(conn: &Connection) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(agent_messages)")?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);
    if columns.iter().any(|column| column == "attachment_id") {
        conn.execute_batch(
            r#"
            PRAGMA foreign_keys = OFF;
            BEGIN;
            ALTER TABLE agent_messages RENAME TO agent_messages_with_attachments;
            CREATE TABLE agent_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                slot_id INTEGER NOT NULL,
                role TEXT NOT NULL,
                body TEXT NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY (slot_id) REFERENCES agent_slots(id) ON DELETE CASCADE
            );
            INSERT INTO agent_messages (id, slot_id, role, body, created_at)
                SELECT id, slot_id, role, body, created_at
                FROM agent_messages_with_attachments;
            DROP TABLE agent_messages_with_attachments;
            DROP TABLE IF EXISTS agent_attachments;
            CREATE INDEX idx_agent_messages_slot_id ON agent_messages(slot_id, id);
            COMMIT;
            PRAGMA foreign_keys = ON;
            "#,
        )?;
    } else {
        conn.execute_batch("DROP TABLE IF EXISTS agent_attachments;")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{BASE_SCHEMA, LATEST_SCHEMA_VERSION, current_version, migrate};
    use rusqlite::Connection;

    #[test]
    fn migrate_records_latest_schema_version() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        assert_eq!(current_version(&db).unwrap(), LATEST_SCHEMA_VERSION);
        let columns = db
            .prepare("PRAGMA table_info(agent_slots)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(columns.iter().any(|column| column == "goal"));
    }

    #[test]
    fn migration_two_renames_a_and_removes_only_empty_legacy_slots() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(BASE_SCHEMA).unwrap();
        db.execute_batch(
            r#"
            CREATE TABLE schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT INTO schema_migrations (version) VALUES (1);
            INSERT INTO agent_slots (name, workdir, created_at) VALUES
                ('a', '/tmp', 'now'),
                ('b', '/tmp', 'now'),
                ('c', '/tmp', 'now');
            INSERT INTO agent_messages (slot_id, role, body, created_at)
                SELECT id, 'user', 'keep me', 'now' FROM agent_slots WHERE name = 'c';
            "#,
        )
        .unwrap();

        migrate(&db).unwrap();

        let names = db
            .prepare("SELECT name FROM agent_slots ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(names, vec!["codex", "c"]);
        assert_eq!(current_version(&db).unwrap(), LATEST_SCHEMA_VERSION);
    }

    #[test]
    fn base_schema_is_agent_only() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        let tables = db
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert!(tables.contains(&"agent_slots".into()));
        assert!(tables.contains(&"agent_messages".into()));
        assert!(!tables.contains(&"agent_attachments".into()));
        assert!(!tables.contains(&"channels".into()));
        assert!(!tables.contains(&"download_cache".into()));
    }

    #[test]
    fn migration_four_preserves_messages_and_drops_attachments() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            r#"
            PRAGMA foreign_keys = ON;
            CREATE TABLE schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT INTO schema_migrations (version) VALUES (1), (2), (3);
            CREATE TABLE agent_slots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL COLLATE NOCASE UNIQUE,
                workdir TEXT NOT NULL,
                goal TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL
            );
            CREATE TABLE agent_sessions (
                slot_id INTEGER PRIMARY KEY,
                thread_id TEXT NOT NULL,
                workdir TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                FOREIGN KEY (slot_id) REFERENCES agent_slots(id) ON DELETE CASCADE
            );
            CREATE TABLE agent_attachments (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                slot_id INTEGER NOT NULL,
                original_name TEXT NOT NULL,
                stored_name TEXT NOT NULL,
                content_type TEXT NOT NULL,
                file_path TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE TABLE agent_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                slot_id INTEGER NOT NULL,
                role TEXT NOT NULL,
                body TEXT NOT NULL,
                attachment_id INTEGER,
                created_at TEXT NOT NULL
            );
            INSERT INTO agent_slots (name, workdir, created_at) VALUES ('codex', '/tmp', 'now');
            INSERT INTO agent_attachments
                (slot_id, original_name, stored_name, content_type, file_path, size_bytes, created_at)
                VALUES (1, 'note.txt', 'stored', 'text/plain', '/tmp/note.txt', 4, 'now');
            INSERT INTO agent_messages (slot_id, role, body, attachment_id, created_at)
                VALUES (1, 'user', 'keep this message', 1, 'now');
            "#,
        )
        .unwrap();

        migrate(&db).unwrap();

        let body: String = db
            .query_row("SELECT body FROM agent_messages", [], |row| row.get(0))
            .unwrap();
        assert_eq!(body, "keep this message");
        let attachments: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'agent_attachments'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(attachments, 0);
    }
}
