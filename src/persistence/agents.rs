use crate::AgentHarness;
use crate::AgentMessageRow;
use crate::AgentSlotRow;
use crate::AgentSlotSeed;
use crate::AppState;
use crate::Connection;
use crate::DEFAULT_AGENT_SLOTS;
use crate::MAX_AGENT_SLOT_CHARS;
use crate::Path;
use crate::Utc;
use crate::agent_activity_kind;
use crate::normalize_agent_slot_name;
use crate::params;
use crate::stop_agent_job;
use rusqlite::OptionalExtension;

pub(crate) fn list_agent_slots(db: &Connection) -> rusqlite::Result<Vec<AgentSlotRow>> {
    let mut stmt =
        db.prepare("SELECT id, name, workdir, goal, harness FROM agent_slots ORDER BY id ASC")?;
    stmt.query_map([], |row| {
        let harness = row.get::<_, String>(4)?;
        Ok(AgentSlotRow {
            id: row.get(0)?,
            name: row.get(1)?,
            workdir: row.get(2)?,
            goal: row.get(3)?,
            harness: AgentHarness::parse(&harness).unwrap_or(AgentHarness::LegacyCodex),
        })
    })?
    .collect()
}

pub(crate) fn get_agent_slot(db: &Connection, id: i64) -> rusqlite::Result<Option<AgentSlotRow>> {
    db.query_row(
        "SELECT id, name, workdir, goal, harness FROM agent_slots WHERE id = ?1",
        params![id],
        |row| {
            let harness = row.get::<_, String>(4)?;
            Ok(AgentSlotRow {
                id: row.get(0)?,
                name: row.get(1)?,
                workdir: row.get(2)?,
                goal: row.get(3)?,
                harness: AgentHarness::parse(&harness).unwrap_or(AgentHarness::LegacyCodex),
            })
        },
    )
    .optional()
}

#[cfg(test)]
pub(crate) fn ensure_agent_slot(
    db: &Connection,
    name: &str,
    workdir: &Path,
) -> rusqlite::Result<i64> {
    ensure_agent_slot_with_harness(db, name, workdir, AgentHarness::Pi)
}

pub(crate) fn ensure_agent_slot_with_harness(
    db: &Connection,
    name: &str,
    workdir: &Path,
    harness: AgentHarness,
) -> rusqlite::Result<i64> {
    let existing = db
        .query_row(
            "SELECT id FROM agent_slots WHERE name = ?1",
            params![name],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    if let Some(id) = existing {
        return Ok(id);
    }
    db.execute(
        "INSERT INTO agent_slots (name, workdir, harness, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![
            name,
            workdir.to_string_lossy(),
            harness.as_str(),
            Utc::now().to_rfc3339()
        ],
    )?;
    Ok(db.last_insert_rowid())
}

/// Creates a durable, independently addressable agent lane.
///
/// The database uses case-insensitive lane names, so the final name is always
/// normalized and made unique before inserting.  This is deliberately kept in
/// persistence rather than the web handler: every caller gets the same
/// isolation guarantee.
pub(crate) fn create_agent_slot(
    db: &Connection,
    requested_name: &str,
    workdir: &Path,
    harness: AgentHarness,
) -> rusqlite::Result<AgentSlotRow> {
    let base = agent_slot_base_name(requested_name, workdir);
    let name = next_agent_slot_name(db, &base)?;
    insert_agent_slot(db, &name, workdir, "", harness)
}

/// Creates a sibling lane for work that must run in parallel with an existing
/// turn. The sibling gets its own message log, harness, and saved session.
pub(crate) fn create_parallel_agent_slot(
    db: &Connection,
    source: &AgentSlotRow,
) -> rusqlite::Result<AgentSlotRow> {
    let base = parallel_agent_slot_base(&source.name);
    let name = next_agent_slot_name(db, &base)?;
    insert_agent_slot(
        db,
        &name,
        Path::new(&source.workdir),
        &source.goal,
        source.harness,
    )
}

fn insert_agent_slot(
    db: &Connection,
    name: &str,
    workdir: &Path,
    goal: &str,
    harness: AgentHarness,
) -> rusqlite::Result<AgentSlotRow> {
    let workdir = workdir.to_string_lossy().to_string();
    db.execute(
        "INSERT INTO agent_slots (name, workdir, goal, harness, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            name,
            workdir,
            goal,
            harness.as_str(),
            Utc::now().to_rfc3339()
        ],
    )?;
    Ok(AgentSlotRow {
        id: db.last_insert_rowid(),
        name: name.to_string(),
        workdir,
        goal: goal.to_string(),
        harness,
    })
}

fn agent_slot_base_name(requested_name: &str, workdir: &Path) -> String {
    let requested = normalize_agent_slot_name(requested_name);
    if !requested.is_empty() {
        return requested.chars().take(MAX_AGENT_SLOT_CHARS).collect();
    }
    let directory_name = workdir
        .file_name()
        .and_then(|name| name.to_str())
        .map(normalize_agent_slot_name)
        .unwrap_or_default();
    if directory_name.is_empty() {
        "project".into()
    } else {
        directory_name.chars().take(MAX_AGENT_SLOT_CHARS).collect()
    }
}

fn parallel_agent_slot_base(name: &str) -> String {
    let normalized = normalize_agent_slot_name(name);
    let base = normalized
        .rsplit_once('-')
        .filter(|(_, suffix)| suffix.chars().all(|ch| ch.is_ascii_digit()))
        .map(|(base, _)| base)
        .unwrap_or(&normalized);
    if base.is_empty() {
        "agent".into()
    } else {
        base.to_string()
    }
}

fn next_agent_slot_name(db: &Connection, base: &str) -> rusqlite::Result<String> {
    let base = base.chars().take(MAX_AGENT_SLOT_CHARS).collect::<String>();
    if !agent_slot_name_exists(db, &base)? {
        return Ok(base);
    }
    for number in 2..10_000 {
        let suffix = format!("-{number}");
        let stem_len = MAX_AGENT_SLOT_CHARS.saturating_sub(suffix.len()).max(1);
        let stem = base.chars().take(stem_len).collect::<String>();
        let candidate = format!("{stem}{suffix}");
        if !agent_slot_name_exists(db, &candidate)? {
            return Ok(candidate);
        }
    }
    Err(rusqlite::Error::InvalidQuery)
}

fn agent_slot_name_exists(db: &Connection, name: &str) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT 1 FROM agent_slots WHERE name = ?1",
        params![name],
        |_| Ok(()),
    )
    .optional()
    .map(|value| value.is_some())
}

pub(crate) fn ensure_agent_slot_seeds(
    db: &Connection,
    seeds: &[AgentSlotSeed],
    default_workdir: &Path,
    default_harness: AgentHarness,
) -> rusqlite::Result<()> {
    if seeds.is_empty() {
        for name in DEFAULT_AGENT_SLOTS.split(',') {
            ensure_agent_slot_with_harness(db, name, default_workdir, default_harness)?;
        }
        return Ok(());
    }
    for seed in seeds {
        ensure_agent_slot_with_harness(db, &seed.name, &seed.workdir, default_harness)?;
    }
    Ok(())
}

pub(crate) fn list_agent_messages(
    db: &Connection,
    slot_id: i64,
) -> rusqlite::Result<Vec<AgentMessageRow>> {
    let mut stmt = db.prepare(
        "SELECT id, role, body, created_at
         FROM agent_messages
         WHERE slot_id = ?1
         ORDER BY id DESC
         LIMIT 200",
    )?;
    stmt.query_map(params![slot_id], |row| {
        Ok(AgentMessageRow {
            id: row.get(0)?,
            role: row.get(1)?,
            body: row.get(2)?,
            created_at: row.get(3)?,
        })
    })?
    .collect()
}

pub(crate) fn last_agent_message(
    db: &Connection,
    slot_id: i64,
) -> rusqlite::Result<Option<AgentMessageRow>> {
    db.query_row(
        "SELECT id, role, body, created_at
         FROM agent_messages
         WHERE slot_id = ?1
         ORDER BY id DESC
         LIMIT 1",
        params![slot_id],
        |row| {
            Ok(AgentMessageRow {
                id: row.get(0)?,
                role: row.get(1)?,
                body: row.get(2)?,
                created_at: row.get(3)?,
            })
        },
    )
    .optional()
}

pub(crate) fn agent_user_message_exists(
    db: &Connection,
    slot_id: i64,
    message_id: i64,
) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT 1
         FROM agent_messages
         WHERE id = ?1 AND slot_id = ?2 AND role = 'user'",
        params![message_id, slot_id],
        |_| Ok(()),
    )
    .optional()
    .map(|row| row.is_some())
}

pub(crate) fn append_agent_message(
    db: &Connection,
    slot_id: i64,
    role: &str,
    body: &str,
) -> rusqlite::Result<i64> {
    db.execute(
        "INSERT INTO agent_messages (slot_id, role, body, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![slot_id, role, body, Utc::now().to_rfc3339()],
    )?;
    Ok(db.last_insert_rowid())
}

pub(crate) fn update_agent_user_message(
    db: &Connection,
    slot_id: i64,
    message_id: i64,
    body: &str,
) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE agent_messages
         SET body = ?1, created_at = ?2
         WHERE id = ?3 AND slot_id = ?4 AND role = 'user'",
        params![body, Utc::now().to_rfc3339(), message_id, slot_id],
    )?;
    Ok(())
}

pub(crate) fn delete_agent_messages_after(
    db: &Connection,
    slot_id: i64,
    message_id: i64,
) -> rusqlite::Result<()> {
    db.execute(
        "DELETE FROM agent_messages WHERE slot_id = ?1 AND id > ?2",
        params![slot_id, message_id],
    )?;
    Ok(())
}

pub(crate) fn append_agent_assistant(state: &AppState, slot_id: i64, body: &str) {
    let db = state.db.lock().unwrap();
    let _ = append_agent_message(&db, slot_id, "assistant", body);
}

pub(crate) fn mark_interrupted_agent_runs(state: &AppState) {
    let db = state.db.lock().unwrap();
    let slots = list_agent_slots(&db).unwrap_or_default();
    for slot in slots {
        let Ok(Some(message)) = last_agent_message(&db, slot.id) else {
            continue;
        };
        if agent_activity_kind(&message).is_some() {
            let _ = append_agent_message(
                &db,
                slot.id,
                "assistant",
                &format!(
                    "Mobailmux restarted while `{}` was running, so this local web transcript ended before {} returned a final answer. Send a new message to continue from the saved harness session.",
                    slot.name,
                    slot.harness.display_name()
                ),
            );
        }
    }
}

pub(crate) fn set_agent_goal(db: &Connection, slot_id: i64, goal: &str) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE agent_slots SET goal = ?1 WHERE id = ?2",
        params![goal, slot_id],
    )?;
    Ok(())
}

pub(crate) fn agent_session(
    db: &Connection,
    slot_id: i64,
) -> rusqlite::Result<Option<(String, String)>> {
    db.query_row(
        "SELECT thread_id, workdir FROM agent_sessions WHERE slot_id = ?1",
        params![slot_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
}

pub(crate) fn set_agent_session(
    db: &Connection,
    slot_id: i64,
    thread_id: &str,
    workdir: &str,
) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO agent_sessions (slot_id, thread_id, workdir, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(slot_id) DO UPDATE SET
           thread_id = excluded.thread_id,
           workdir = excluded.workdir,
           updated_at = excluded.updated_at",
        params![slot_id, thread_id, workdir, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

pub(crate) fn delete_agent_session(db: &Connection, slot_id: i64) -> rusqlite::Result<()> {
    db.execute(
        "DELETE FROM agent_sessions WHERE slot_id = ?1",
        params![slot_id],
    )?;
    Ok(())
}

pub(crate) fn set_agent_workdir(
    db: &Connection,
    slot_id: i64,
    workdir: &str,
) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE agent_slots SET workdir = ?1 WHERE id = ?2",
        params![workdir, slot_id],
    )?;
    Ok(())
}

pub(crate) fn reset_agent_slot_chat(state: &AppState, slot_id: i64, workdir: &Path) -> bool {
    let stopped = stop_agent_job(state, slot_id);
    let workdir = workdir.to_string_lossy().to_string();
    let db = state.db.lock().unwrap();
    let _ = db.execute(
        "UPDATE agent_slots SET workdir = ?1 WHERE id = ?2",
        params![workdir, slot_id],
    );
    let _ = db.execute(
        "DELETE FROM agent_sessions WHERE slot_id = ?1",
        params![slot_id],
    );
    let _ = db.execute(
        "DELETE FROM agent_messages WHERE slot_id = ?1",
        params![slot_id],
    );
    stopped
}
