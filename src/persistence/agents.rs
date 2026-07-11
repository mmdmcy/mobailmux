use crate::AgentMessageRow;
use crate::AgentSlotRow;
use crate::AgentSlotSeed;
use crate::AppState;
use crate::Connection;
use crate::DEFAULT_AGENT_SLOTS;
use crate::Path;
use crate::Utc;
use crate::agent_activity_kind;
use crate::clear_agent_queue;
use crate::params;
use crate::stop_agent_job;
use rusqlite::OptionalExtension;

pub(crate) fn list_agent_slots(db: &Connection) -> rusqlite::Result<Vec<AgentSlotRow>> {
    let mut stmt = db.prepare("SELECT id, name, workdir, goal FROM agent_slots ORDER BY id ASC")?;
    stmt.query_map([], |row| {
        Ok(AgentSlotRow {
            id: row.get(0)?,
            name: row.get(1)?,
            workdir: row.get(2)?,
            goal: row.get(3)?,
        })
    })?
    .collect()
}

pub(crate) fn get_agent_slot(db: &Connection, id: i64) -> rusqlite::Result<Option<AgentSlotRow>> {
    db.query_row(
        "SELECT id, name, workdir, goal FROM agent_slots WHERE id = ?1",
        params![id],
        |row| {
            Ok(AgentSlotRow {
                id: row.get(0)?,
                name: row.get(1)?,
                workdir: row.get(2)?,
                goal: row.get(3)?,
            })
        },
    )
    .optional()
}

pub(crate) fn ensure_agent_slot(
    db: &Connection,
    name: &str,
    workdir: &Path,
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
        "INSERT INTO agent_slots (name, workdir, created_at) VALUES (?1, ?2, ?3)",
        params![name, workdir.to_string_lossy(), Utc::now().to_rfc3339()],
    )?;
    Ok(db.last_insert_rowid())
}

pub(crate) fn ensure_agent_slot_seeds(
    db: &Connection,
    seeds: &[AgentSlotSeed],
    default_workdir: &Path,
) -> rusqlite::Result<()> {
    if seeds.is_empty() {
        for name in DEFAULT_AGENT_SLOTS.split(',') {
            ensure_agent_slot(db, name, default_workdir)?;
        }
        return Ok(());
    }
    for seed in seeds {
        ensure_agent_slot(db, &seed.name, &seed.workdir)?;
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
                    "Mobailmux restarted while `{}` was running, so this local web transcript ended before Codex returned a final answer. Send a new message to continue from the saved Codex session.",
                    slot.name
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
    let _ = clear_agent_queue(state, slot_id);
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
