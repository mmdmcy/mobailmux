use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OptionalExtension, params};

pub const RESET_CREDIT_TTL_DAYS: i64 = 30;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS codex_reset_credit_state (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS codex_reset_credits (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    first_seen_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    status TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_codex_reset_credits_status_expires
ON codex_reset_credits(status, expires_at);
CREATE TABLE IF NOT EXISTS codex_reset_credit_observations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    observed_at TEXT NOT NULL,
    available_count INTEGER NOT NULL
);
"#;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResetCreditEstimate {
    pub remote_available_count: i64,
    pub tracked_available_count: i64,
    pub untracked_available_count: i64,
    pub next_expires_at: Option<String>,
}

pub fn reconcile(
    db: &Connection,
    remote_available_count: i64,
    observed_at: DateTime<Utc>,
) -> rusqlite::Result<ResetCreditEstimate> {
    let remote_available_count = remote_available_count.max(0);
    expire_old_credits(db, observed_at)?;
    let previous_count = last_observed_count(db)?;
    if let Some(previous_count) = previous_count {
        if remote_available_count > previous_count {
            insert_new_credits(db, remote_available_count - previous_count, observed_at)?;
        } else if remote_available_count < previous_count {
            mark_removed_credits(db, previous_count - remote_available_count, observed_at)?;
        }
    }
    set_last_observed_count(db, remote_available_count, observed_at)?;
    db.execute(
        "INSERT INTO codex_reset_credit_observations (observed_at, available_count) VALUES (?1, ?2)",
        params![observed_at.to_rfc3339(), remote_available_count],
    )?;
    estimate(db, remote_available_count, observed_at)
}

pub fn estimate(
    db: &Connection,
    remote_available_count: i64,
    now: DateTime<Utc>,
) -> rusqlite::Result<ResetCreditEstimate> {
    expire_old_credits(db, now)?;
    let tracked_available_count: i64 = db.query_row(
        "SELECT COUNT(*) FROM codex_reset_credits WHERE status = 'available' AND expires_at > ?1",
        params![now.to_rfc3339()],
        |row| row.get(0),
    )?;
    let next_expires_at = db
        .query_row(
            "SELECT expires_at FROM codex_reset_credits WHERE status = 'available' AND expires_at > ?1 ORDER BY expires_at ASC LIMIT 1",
            params![now.to_rfc3339()],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    Ok(ResetCreditEstimate {
        remote_available_count: remote_available_count.max(0),
        tracked_available_count,
        untracked_available_count: (remote_available_count - tracked_available_count).max(0),
        next_expires_at,
    })
}

fn last_observed_count(db: &Connection) -> rusqlite::Result<Option<i64>> {
    db.query_row(
        "SELECT value FROM codex_reset_credit_state WHERE key = 'last_available_count'",
        [],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map(|value| value.and_then(|value| value.parse::<i64>().ok()))
}

fn set_last_observed_count(
    db: &Connection,
    count: i64,
    observed_at: DateTime<Utc>,
) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO codex_reset_credit_state (key, value, updated_at)
         VALUES ('last_available_count', ?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        params![count.to_string(), observed_at.to_rfc3339()],
    )?;
    Ok(())
}

fn insert_new_credits(
    db: &Connection,
    count: i64,
    observed_at: DateTime<Utc>,
) -> rusqlite::Result<()> {
    let expires_at = observed_at + Duration::days(RESET_CREDIT_TTL_DAYS);
    for _ in 0..count {
        db.execute(
            "INSERT INTO codex_reset_credits (first_seen_at, expires_at, status, updated_at)
             VALUES (?1, ?2, 'available', ?1)",
            params![observed_at.to_rfc3339(), expires_at.to_rfc3339()],
        )?;
    }
    Ok(())
}

fn mark_removed_credits(
    db: &Connection,
    count: i64,
    observed_at: DateTime<Utc>,
) -> rusqlite::Result<()> {
    let ids = db
        .prepare(
            "SELECT id FROM codex_reset_credits
             WHERE status = 'available'
             ORDER BY expires_at ASC, id ASC
             LIMIT ?1",
        )?
        .query_map(params![count], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for id in ids {
        db.execute(
            "UPDATE codex_reset_credits SET status = 'used_or_removed', updated_at = ?1 WHERE id = ?2",
            params![observed_at.to_rfc3339(), id],
        )?;
    }
    Ok(())
}

fn expire_old_credits(db: &Connection, now: DateTime<Utc>) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE codex_reset_credits SET status = 'expired', updated_at = ?1
         WHERE status = 'available' AND expires_at <= ?1",
        params![now.to_rfc3339()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(SCHEMA).unwrap();
        db
    }

    #[test]
    fn first_observation_is_untracked_baseline() {
        let db = db();
        let now = DateTime::parse_from_rfc3339("2026-06-23T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let estimate = reconcile(&db, 2, now).unwrap();
        assert_eq!(estimate.remote_available_count, 2);
        assert_eq!(estimate.tracked_available_count, 0);
        assert_eq!(estimate.untracked_available_count, 2);
        assert!(estimate.next_expires_at.is_none());
    }

    #[test]
    fn increased_count_tracks_new_credits_for_30_days() {
        let db = db();
        let first = DateTime::parse_from_rfc3339("2026-06-23T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        reconcile(&db, 2, first).unwrap();
        let second = first + Duration::days(2);
        let estimate = reconcile(&db, 4, second).unwrap();
        assert_eq!(estimate.remote_available_count, 4);
        assert_eq!(estimate.tracked_available_count, 2);
        assert_eq!(estimate.untracked_available_count, 2);
        assert_eq!(
            estimate.next_expires_at,
            Some((second + Duration::days(30)).to_rfc3339())
        );
    }

    #[test]
    fn decreased_count_marks_tracked_credits_removed() {
        let db = db();
        let first = DateTime::parse_from_rfc3339("2026-06-23T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        reconcile(&db, 0, first).unwrap();
        reconcile(&db, 2, first + Duration::days(1)).unwrap();
        let estimate = reconcile(&db, 1, first + Duration::days(2)).unwrap();
        assert_eq!(estimate.remote_available_count, 1);
        assert_eq!(estimate.tracked_available_count, 1);
        assert_eq!(estimate.untracked_available_count, 0);
    }
}
