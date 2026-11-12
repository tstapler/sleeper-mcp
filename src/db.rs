// kibitzer:ignore file-size -- pre-existing condition (already ~1105 lines
// before Epic 2.1's additions); this task's file scope is fixed to
// src/db.rs and src/cli/browser/transactions.rs only, so a split is
// deferred to a future task.
/// SQLite-backed permanent stats store.
///
/// Completed NFL weeks never change, so stats are stored permanently here
/// rather than going through the TTL HTTP cache. Cross-process safe via
/// SQLite WAL mode.
use std::collections::HashMap;
use std::path::Path;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;

use crate::error::Result;
use crate::models::League;

pub struct StatsDb {
    pool: SqlitePool,
}

/// Params for `StatsDb::upsert_roster_snapshot` — grouped into a struct
/// rather than positional args since the row has more fields than a call
/// site can keep straight by position.
pub struct RosterSnapshotRow<'a> {
    pub league_id: &'a str,
    pub roster_id: i32,
    pub season: &'a str,
    pub owner: Option<&'a str>,
    pub wins: i32,
    pub losses: i32,
    pub data: &'a serde_json::Value,
}

/// Params for `StatsDb::upsert_transaction`. `roster_ids`/`waiver_bid` are
/// `None`/empty for rows sourced from the public REST transactions endpoint
/// (which never carries a bid amount — live-verified 2026-11-10); rows
/// sourced from the authenticated browser GraphQL path populate both.
///
/// `status_updated` is the GraphQL-epoch timestamp of `status`; REST never
/// carries one, so it's `None` there (see `upsert_transaction`'s CAS guard).
/// `source` is plain observability metadata (`"rest"`/`"graphql"`), not
/// correctness-bearing.
pub struct TransactionRow<'a> {
    pub transaction_id: &'a str,
    pub league_id: &'a str,
    pub season: &'a str,
    pub week: i32,
    pub kind: &'a str,
    pub status: Option<&'a str>,
    pub created: i64,
    pub adds: &'a serde_json::Value,
    pub drops: &'a serde_json::Value,
    pub roster_ids: &'a serde_json::Value,
    pub waiver_bid: Option<i64>,
    pub status_updated: Option<i64>,
    pub source: &'a str,
}

/// One row for `StatsDb::record_sync_run` — a ledger entry proving a sync
/// attempt completed, so `has_full_league_graphql_coverage` can tell "never
/// synced" apart from "synced via REST only" instead of silently treating
/// both as a FAAB-remaining floor.
pub struct TransactionSyncRun<'a> {
    pub league_id: &'a str,
    pub season: &'a str,
    pub source: &'a str,
    pub roster_scope: &'a str,
    pub synced_at: chrono::DateTime<chrono::Utc>,
    pub rows_fetched: i64,
    pub rows_changed: i64,
}

/// Row shape returned by `StatsDb::list_transactions` — read from the local
/// cache only, never triggers an API/browser call.
///
/// `fetched_at`/`status_updated`/`source` (Epic 2.1, Task 2.1.1a) let a
/// staleness decision (`StalenessTolerance::is_fresh`) be made from this
/// struct alone, without a second query back into `StatsDb`.
/// Row returned by `StatsDb::list_analysis_notes`.
#[derive(Debug, Clone)]
pub struct AnalysisNote {
    pub id: i64,
    pub league_id: String,
    pub created_at: String,
    pub topic: String,
    pub summary: String,
    pub detail: Option<String>,
}

/// Row returned by `StatsDb::list_chat_transcripts` — metadata only. Deliberately
/// excludes `raw_text`: a transcript file can be large, and a list call should stay
/// cheap even when several sessions have been imported. Use `get_chat_transcript`
/// to read one back in full.
#[derive(Debug, Clone)]
pub struct ChatTranscriptMeta {
    pub session_id: String,
    pub league_id: Option<String>,
    pub imported_at: String,
    pub source_path: String,
    pub size_bytes: i64,
}

/// Row returned by `StatsDb::get_chat_transcript` — the full imported text.
#[derive(Debug, Clone)]
pub struct ChatTranscriptFull {
    pub session_id: String,
    pub league_id: Option<String>,
    pub imported_at: String,
    pub source_path: String,
    pub raw_text: String,
}

/// Params for `StatsDb::upsert_league_chat_messages` — one row from a
/// `league_chat_via_seam` page, already parsed. `author_id`/`author_name`
/// are `None` only in the (unobserved but possible) case the GraphQL row
/// itself omits them — never guessed/defaulted elsewhere.
pub struct LeagueChatMessageRow<'a> {
    pub message_id: &'a str,
    pub author_id: Option<&'a str>,
    pub author_name: Option<&'a str>,
    pub text: &'a str,
    pub created: i64,
    pub pinned: bool,
}

/// Row returned by `StatsDb::list_league_chat_messages` — read from the
/// local cache only, never triggers a browser launch.
#[derive(Debug, Clone, PartialEq)]
pub struct LeagueChatMessage {
    pub message_id: String,
    pub author_id: Option<String>,
    pub author_name: Option<String>,
    pub text: String,
    pub created: i64,
    pub pinned: bool,
}

#[derive(Debug, Clone)]
pub struct CachedTransaction {
    pub transaction_id: String,
    pub season: String,
    pub week: i32,
    pub kind: String,
    pub status: Option<String>,
    pub created: i64,
    pub adds: serde_json::Value,
    pub drops: serde_json::Value,
    pub roster_ids: serde_json::Value,
    pub waiver_bid: Option<i64>,
    pub fetched_at: chrono::DateTime<chrono::Utc>,
    pub status_updated: Option<i64>,
    pub source: Option<String>,
}

impl StatsDb {
    /// `busy_timeout` exists because the transactions cache now has two
    /// independent writers (REST sync and browser/GraphQL sync) that can run
    /// concurrently; without it, the loser of a write race gets `SQLITE_BUSY`
    /// instead of retry-waiting for the lock (see `rate_limit.rs:121` for the
    /// same comment-the-concurrency-decision convention).
    pub async fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(std::time::Duration::from_secs(5));

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS player_stats (
                season    TEXT    NOT NULL,
                week      INTEGER NOT NULL,
                player_id TEXT    NOT NULL,
                stat_key  TEXT    NOT NULL,
                value     REAL    NOT NULL,
                PRIMARY KEY (season, week, player_id, stat_key)
            );
            CREATE TABLE IF NOT EXISTS weeks_fetched (
                season     TEXT    NOT NULL,
                week       INTEGER NOT NULL,
                fetched_at TEXT    NOT NULL,
                PRIMARY KEY (season, week)
            );
            CREATE TABLE IF NOT EXISTS leagues (
                league_id TEXT    PRIMARY KEY,
                data      TEXT    NOT NULL,
                synced_at TEXT    NOT NULL
            );
            CREATE TABLE IF NOT EXISTS league_members (
                league_id    TEXT    NOT NULL,
                roster_id    INTEGER NOT NULL,
                user_id      TEXT    NOT NULL,
                username     TEXT,
                display_name TEXT,
                PRIMARY KEY (league_id, roster_id)
            );
            CREATE INDEX IF NOT EXISTS league_members_username
                ON league_members (league_id, username COLLATE NOCASE);
            CREATE INDEX IF NOT EXISTS league_members_display
                ON league_members (league_id, display_name COLLATE NOCASE);
            CREATE TABLE IF NOT EXISTS roster_snapshots (
                league_id  TEXT    NOT NULL,
                roster_id  INTEGER NOT NULL,
                season     TEXT    NOT NULL,
                fetched_at TEXT    NOT NULL,
                owner      TEXT,
                wins       INTEGER,
                losses     INTEGER,
                data       TEXT    NOT NULL,
                PRIMARY KEY (league_id, roster_id, fetched_at)
            );
            CREATE INDEX IF NOT EXISTS roster_snapshots_latest
                ON roster_snapshots (league_id, roster_id, fetched_at DESC);
            CREATE TABLE IF NOT EXISTS transactions (
                transaction_id TEXT    PRIMARY KEY,
                league_id      TEXT    NOT NULL,
                season         TEXT    NOT NULL,
                week           INTEGER NOT NULL,
                type           TEXT    NOT NULL,
                status         TEXT,
                created        INTEGER,
                adds           TEXT,
                drops          TEXT,
                fetched_at     TEXT    NOT NULL
            );
            CREATE INDEX IF NOT EXISTS transactions_league_week
                ON transactions (league_id, season, week);
            CREATE TABLE IF NOT EXISTS transaction_sync_runs (
                league_id    TEXT    NOT NULL,
                season       TEXT    NOT NULL,
                source       TEXT    NOT NULL,
                roster_scope TEXT    NOT NULL,
                synced_at    TEXT    NOT NULL,
                rows_fetched INTEGER NOT NULL,
                rows_changed INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS analysis_notes (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                league_id  TEXT    NOT NULL,
                created_at TEXT    NOT NULL,
                topic      TEXT    NOT NULL,
                summary    TEXT    NOT NULL,
                detail     TEXT
            );
            CREATE INDEX IF NOT EXISTS analysis_notes_league_created
                ON analysis_notes (league_id, created_at DESC);
            CREATE TABLE IF NOT EXISTS chat_transcripts (
                session_id  TEXT PRIMARY KEY,
                league_id   TEXT,
                imported_at TEXT NOT NULL,
                source_path TEXT NOT NULL,
                raw_text    TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS chat_transcripts_league_imported
                ON chat_transcripts (league_id, imported_at DESC);
            CREATE TABLE IF NOT EXISTS league_chat_messages (
                league_id   TEXT    NOT NULL,
                message_id  TEXT    NOT NULL,
                author_id   TEXT,
                author_name TEXT,
                text        TEXT    NOT NULL,
                created     INTEGER NOT NULL,
                pinned      INTEGER NOT NULL DEFAULT 0,
                synced_at   TEXT    NOT NULL,
                PRIMARY KEY (league_id, message_id)
            );
            CREATE INDEX IF NOT EXISTS league_chat_messages_league_created
                ON league_chat_messages (league_id, created DESC);",
        )
        .execute(&pool)
        .await?;

        // `roster_ids`/`waiver_bid` columns: the authenticated-browser sync
        // path carries a real FAAB bid and every participating roster,
        // unlike the public REST endpoint. `ADD COLUMN` has no
        // `IF NOT EXISTS` in SQLite, so check `PRAGMA table_info` first
        // (idempotent re-open).
        let existing_cols: Vec<(String,)> =
            sqlx::query_as("SELECT name FROM pragma_table_info('transactions')")
                .fetch_all(&pool)
                .await?;
        let has_col = |name: &str| existing_cols.iter().any(|(c,)| c == name);
        if !has_col("roster_ids") {
            sqlx::query("ALTER TABLE transactions ADD COLUMN roster_ids TEXT").execute(&pool).await?;
        }
        if !has_col("waiver_bid") {
            sqlx::query("ALTER TABLE transactions ADD COLUMN waiver_bid INTEGER").execute(&pool).await?;
        }
        if !has_col("status_updated") {
            sqlx::query("ALTER TABLE transactions ADD COLUMN status_updated INTEGER").execute(&pool).await?;
        }
        if !has_col("source") {
            sqlx::query("ALTER TABLE transactions ADD COLUMN source TEXT").execute(&pool).await?;
        }

        Ok(Self { pool })
    }

    /// Persist a full week's stats dict (player_id → stat_key → value).
    /// Returns the number of rows inserted/replaced.
    pub async fn store_week(
        &self,
        season: &str,
        week: i32,
        raw: &HashMap<String, serde_json::Value>,
    ) -> Result<usize> {
        let mut tx = self.pool.begin().await?;
        let mut count = 0usize;

        for (player_id, stats) in raw {
            let Some(obj) = stats.as_object() else {
                continue;
            };
            for (stat_key, val) in obj {
                let Some(value) = val.as_f64() else { continue };
                sqlx::query(
                    "INSERT OR REPLACE INTO player_stats
                     (season, week, player_id, stat_key, value) VALUES (?,?,?,?,?)",
                )
                .bind(season)
                .bind(week)
                .bind(player_id.as_str())
                .bind(stat_key.as_str())
                .bind(value)
                .execute(&mut *tx)
                .await?;
                count += 1;
            }
        }

        sqlx::query(
            "INSERT OR REPLACE INTO weeks_fetched (season, week, fetched_at) VALUES (?,?,?)",
        )
        .bind(season)
        .bind(week)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(count)
    }

    pub async fn is_week_cached(&self, season: &str, week: i32) -> Result<bool> {
        let row: Option<(i32,)> =
            sqlx::query_as("SELECT 1 FROM weeks_fetched WHERE season=? AND week=?")
                .bind(season)
                .bind(week)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.is_some())
    }

    pub async fn cached_weeks(&self, season: &str) -> Result<Vec<i32>> {
        let rows: Vec<(i32,)> =
            sqlx::query_as("SELECT week FROM weeks_fetched WHERE season=? ORDER BY week")
                .bind(season)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|(w,)| w).collect())
    }

    /// Sum all cached weeks for a season into per-player season totals.
    pub async fn season_totals(
        &self,
        season: &str,
    ) -> Result<HashMap<String, HashMap<String, f64>>> {
        let rows: Vec<(String, String, f64)> = sqlx::query_as(
            "SELECT player_id, stat_key, SUM(value) as total
             FROM player_stats WHERE season=? GROUP BY player_id, stat_key",
        )
        .bind(season)
        .fetch_all(&self.pool)
        .await?;

        let mut result: HashMap<String, HashMap<String, f64>> = HashMap::new();
        for (player_id, stat_key, total) in rows {
            result.entry(player_id).or_default().insert(stat_key, total);
        }
        Ok(result)
    }

    /// Per-player weekly points for a season, sorted most-recent-week-first per player.
    /// Uses COALESCE to pick whichever scoring column is present (half-ppr > ppr > std).
    pub async fn weekly_pts_all_players(
        &self,
        season: &str,
    ) -> Result<HashMap<String, Vec<(i32, f64)>>> {
        let rows: Vec<(String, i32, f64)> = sqlx::query_as(
            "SELECT player_id, week,
                COALESCE(
                    MAX(CASE WHEN stat_key='pts_half_ppr' THEN value END),
                    MAX(CASE WHEN stat_key='pts_ppr'      THEN value END),
                    MAX(CASE WHEN stat_key='pts_std'      THEN value END)
                ) AS pts
             FROM player_stats
             WHERE season=?
             GROUP BY player_id, week
             HAVING pts IS NOT NULL
             ORDER BY player_id, week DESC",
        )
        .bind(season)
        .fetch_all(&self.pool)
        .await?;

        let mut result: HashMap<String, Vec<(i32, f64)>> = HashMap::new();
        for (player_id, week, pts) in rows {
            result.entry(player_id).or_default().push((week, pts));
        }
        Ok(result)
    }

    pub async fn week_stats(
        &self,
        season: &str,
        week: i32,
    ) -> Result<HashMap<String, HashMap<String, f64>>> {
        let rows: Vec<(String, String, f64)> = sqlx::query_as(
            "SELECT player_id, stat_key, value FROM player_stats WHERE season=? AND week=?",
        )
        .bind(season)
        .bind(week)
        .fetch_all(&self.pool)
        .await?;

        let mut result: HashMap<String, HashMap<String, f64>> = HashMap::new();
        for (player_id, stat_key, value) in rows {
            result.entry(player_id).or_default().insert(stat_key, value);
        }
        Ok(result)
    }

    pub async fn player_season_stats(
        &self,
        player_id: &str,
        season: &str,
    ) -> Result<HashMap<i32, HashMap<String, f64>>> {
        let rows: Vec<(i32, String, f64)> = sqlx::query_as(
            "SELECT week, stat_key, value FROM player_stats
             WHERE season=? AND player_id=? ORDER BY week",
        )
        .bind(season)
        .bind(player_id)
        .fetch_all(&self.pool)
        .await?;

        let mut result: HashMap<i32, HashMap<String, f64>> = HashMap::new();
        for (week, stat_key, value) in rows {
            result.entry(week).or_default().insert(stat_key, value);
        }
        Ok(result)
    }

    pub async fn upsert_league_members(
        &self,
        league_id: &str,
        members: &[(i32, String, Option<String>, Option<String>)],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for (roster_id, user_id, username, display_name) in members {
            sqlx::query(
                "INSERT OR REPLACE INTO league_members
                 (league_id, roster_id, user_id, username, display_name)
                 VALUES (?,?,?,?,?)",
            )
            .bind(league_id)
            .bind(roster_id)
            .bind(user_id)
            .bind(username.as_deref())
            .bind(display_name.as_deref())
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// All synced members of a league: (roster_id, user_id, username, display_name).
    pub async fn list_league_members(
        &self,
        league_id: &str,
    ) -> Result<Vec<(i32, String, Option<String>, Option<String>)>> {
        let rows: Vec<(i32, String, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT roster_id, user_id, username, display_name
             FROM league_members WHERE league_id = ?",
        )
        .bind(league_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Resolve a human-readable identifier to a roster_id.
    /// Accepts (in order): numeric roster_id, username, display_name, user_id.
    pub async fn resolve_roster_id(
        &self,
        league_id: &str,
        identifier: &str,
    ) -> Result<Option<i32>> {
        // Fast path: already a number.
        if let Ok(n) = identifier.parse::<i32>() {
            return Ok(Some(n));
        }
        let row: Option<(i32,)> = sqlx::query_as(
            "SELECT roster_id FROM league_members
             WHERE league_id = ?
               AND (
                 LOWER(username)     = LOWER(?)
                 OR LOWER(display_name) = LOWER(?)
                 OR user_id          = ?
               )
             LIMIT 1",
        )
        .bind(league_id)
        .bind(identifier)
        .bind(identifier)
        .bind(identifier)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(id,)| id))
    }

    pub async fn league_synced_at(
        &self,
        league_id: &str,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT synced_at FROM leagues WHERE league_id=?")
                .bind(league_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.and_then(|(s,)| {
            chrono::DateTime::parse_from_rfc3339(&s)
                .ok()
                .map(|dt| dt.with_timezone(&chrono::Utc))
        }))
    }

    /// Record one completed sync attempt (REST or GraphQL) in the coverage
    /// ledger. Append-only — every attempt gets a row, so
    /// `has_full_league_graphql_coverage` can tell "never backfilled" apart
    /// from "backfilled, just a while ago."
    pub async fn record_sync_run(&self, row: TransactionSyncRun<'_>) -> Result<()> {
        Self::exec_record_sync_run(&self.pool, &row).await
    }

    /// Fix 2 (Phase 6 review): transaction-scoped variant of
    /// [`Self::record_sync_run`] — see [`Self::upsert_transaction_in_tx`]'s
    /// doc comment for why `persist_league_transactions` needs this.
    pub(crate) async fn record_sync_run_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'static, sqlx::Sqlite>,
        row: TransactionSyncRun<'_>,
    ) -> Result<()> {
        Self::exec_record_sync_run(&mut **tx, &row).await
    }

    async fn exec_record_sync_run<'e, E>(executor: E, row: &TransactionSyncRun<'_>) -> Result<()>
    where
        E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
    {
        sqlx::query(
            "INSERT INTO transaction_sync_runs
             (league_id, season, source, roster_scope, synced_at, rows_fetched, rows_changed)
             VALUES (?,?,?,?,?,?,?)",
        )
        .bind(row.league_id)
        .bind(row.season)
        .bind(row.source)
        .bind(row.roster_scope)
        .bind(row.synced_at.to_rfc3339())
        .bind(row.rows_fetched)
        .bind(row.rows_changed)
        .execute(executor)
        .await?;
        Ok(())
    }

    /// Fix 2 (Phase 6 review): begins a SQL transaction against the shared
    /// pool. `'static` because `SqlitePool::begin` hands back an owned pooled
    /// connection, not a borrow of the pool — the same reason callers can
    /// hold this across `.await` points and commit it independently of
    /// `self`.
    pub(crate) async fn begin_transaction(&self) -> Result<sqlx::Transaction<'static, sqlx::Sqlite>> {
        Ok(self.pool.begin().await?)
    }

    /// Most recent `synced_at` among full-league GraphQL backfills for
    /// `(league_id, season)`, or `None` if only REST/partial rows exist (or
    /// none at all). Callers use this to label a FAAB-remaining figure a
    /// verified total rather than a silent floor.
    pub async fn has_full_league_graphql_coverage(
        &self,
        league_id: &str,
        season: &str,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT synced_at FROM transaction_sync_runs
             WHERE league_id = ? AND season = ? AND source = 'graphql' AND roster_scope = 'all'
             ORDER BY synced_at DESC LIMIT 1",
        )
        .bind(league_id)
        .bind(season)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|(s,)| {
            chrono::DateTime::parse_from_rfc3339(&s)
                .ok()
                .map(|dt| dt.with_timezone(&chrono::Utc))
        }))
    }

    /// Persist a dated analysis finding (e.g. a standings/trade-scan review) so it
    /// survives past this chat session. Returns the new row's `id`.
    pub async fn log_analysis_note(
        &self,
        league_id: &str,
        topic: &str,
        summary: &str,
        detail: Option<&str>,
    ) -> Result<i64> {
        let result = sqlx::query(
            "INSERT INTO analysis_notes (league_id, created_at, topic, summary, detail)
             VALUES (?,?,?,?,?)",
        )
        .bind(league_id)
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(topic)
        .bind(summary)
        .bind(detail)
        .execute(&self.pool)
        .await?;
        Ok(result.last_insert_rowid())
    }

    /// Most recent analysis notes, optionally filtered to one league.
    pub async fn list_analysis_notes(
        &self,
        league_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<AnalysisNote>> {
        let rows: Vec<(i64, String, String, String, String, Option<String>)> = match league_id {
            Some(lid) => {
                sqlx::query_as(
                    "SELECT id, league_id, created_at, topic, summary, detail
                     FROM analysis_notes WHERE league_id = ?
                     ORDER BY created_at DESC LIMIT ?",
                )
                .bind(lid)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query_as(
                    "SELECT id, league_id, created_at, topic, summary, detail
                     FROM analysis_notes ORDER BY created_at DESC LIMIT ?",
                )
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
            }
        };
        Ok(rows
            .into_iter()
            .map(|(id, league_id, created_at, topic, summary, detail)| AnalysisNote {
                id,
                league_id,
                created_at,
                topic,
                summary,
                detail,
            })
            .collect())
    }

    /// Import a chat session transcript verbatim, keyed by `session_id` so
    /// re-importing the same (actively-growing) session file is idempotent
    /// rather than creating duplicate rows.
    pub async fn import_chat_transcript(
        &self,
        session_id: &str,
        league_id: Option<&str>,
        source_path: &str,
        raw_text: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO chat_transcripts
             (session_id, league_id, imported_at, source_path, raw_text)
             VALUES (?,?,?,?,?)",
        )
        .bind(session_id)
        .bind(league_id)
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(source_path)
        .bind(raw_text)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Metadata for imported transcripts, optionally filtered to one league.
    /// Never loads `raw_text` — see [`ChatTranscriptMeta`]'s doc comment.
    pub async fn list_chat_transcripts(
        &self,
        league_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<ChatTranscriptMeta>> {
        let rows: Vec<(String, Option<String>, String, String, i64)> = match league_id {
            Some(lid) => {
                sqlx::query_as(
                    "SELECT session_id, league_id, imported_at, source_path, LENGTH(raw_text)
                     FROM chat_transcripts WHERE league_id = ?
                     ORDER BY imported_at DESC LIMIT ?",
                )
                .bind(lid)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query_as(
                    "SELECT session_id, league_id, imported_at, source_path, LENGTH(raw_text)
                     FROM chat_transcripts ORDER BY imported_at DESC LIMIT ?",
                )
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
            }
        };
        Ok(rows
            .into_iter()
            .map(|(session_id, league_id, imported_at, source_path, size_bytes)| ChatTranscriptMeta {
                session_id,
                league_id,
                imported_at,
                source_path,
                size_bytes,
            })
            .collect())
    }

    /// Read back one imported transcript in full, by `session_id`.
    pub async fn get_chat_transcript(&self, session_id: &str) -> Result<Option<ChatTranscriptFull>> {
        let row: Option<(String, Option<String>, String, String, String)> = sqlx::query_as(
            "SELECT session_id, league_id, imported_at, source_path, raw_text
             FROM chat_transcripts WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(session_id, league_id, imported_at, source_path, raw_text)| ChatTranscriptFull {
            session_id,
            league_id,
            imported_at,
            source_path,
            raw_text,
        }))
    }

    /// The league chat's message_ids already stored, for the sync
    /// orchestrator to tell new rows apart from re-fetched ones without a
    /// round trip per message.
    pub async fn league_chat_message_ids(
        &self,
        league_id: &str,
    ) -> Result<std::collections::HashSet<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT message_id FROM league_chat_messages WHERE league_id = ?",
        )
        .bind(league_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    /// `INSERT OR REPLACE`, keyed by `(league_id, message_id)` — re-syncing
    /// a page (e.g. the inclusive `before` boundary row, or a message that
    /// was since edited/pinned) overwrites rather than duplicates.
    pub async fn upsert_league_chat_messages(
        &self,
        league_id: &str,
        messages: &[LeagueChatMessageRow<'_>],
    ) -> Result<()> {
        let synced_at = chrono::Utc::now().to_rfc3339();
        let mut tx = self.pool.begin().await?;
        for m in messages {
            sqlx::query(
                "INSERT OR REPLACE INTO league_chat_messages
                 (league_id, message_id, author_id, author_name, text, created, pinned, synced_at)
                 VALUES (?,?,?,?,?,?,?,?)",
            )
            .bind(league_id)
            .bind(m.message_id)
            .bind(m.author_id)
            .bind(m.author_name)
            .bind(m.text)
            .bind(m.created)
            .bind(m.pinned)
            .bind(&synced_at)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Read back the league chat from the local cache only, most recent
    /// first — never triggers a browser launch (see `sleeper league-chat
    /// sync` for the write path).
    pub async fn list_league_chat_messages(
        &self,
        league_id: &str,
        limit: i64,
    ) -> Result<Vec<LeagueChatMessage>> {
        type LeagueChatRow = (String, Option<String>, Option<String>, String, i64, bool);
        let rows: Vec<LeagueChatRow> = sqlx::query_as(
            "SELECT message_id, author_id, author_name, text, created, pinned
             FROM league_chat_messages WHERE league_id = ?
             ORDER BY created DESC LIMIT ?",
        )
        .bind(league_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(message_id, author_id, author_name, text, created, pinned)| LeagueChatMessage {
                message_id,
                author_id,
                author_name,
                text,
                created,
                pinned,
            })
            .collect())
    }

    pub async fn upsert_league(&self, league: &League) -> Result<()> {
        let data = serde_json::to_string(league)?;
        sqlx::query(
            "INSERT OR REPLACE INTO leagues (league_id, data, synced_at) VALUES (?,?,?)",
        )
        .bind(&league.league_id)
        .bind(&data)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Append one roster's enriched snapshot (`data` = the resolved JSON blob
    /// analysis.rs builds — name/position/age/tier per player). Append-only
    /// (PK includes `fetched_at`) so history accumulates for trend queries
    /// (e.g. "how did roster 5's IDP count change over the season"), unlike
    /// `leagues`/`league_members` which are point-in-time upserts.
    pub async fn upsert_roster_snapshot(&self, row: RosterSnapshotRow<'_>) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO roster_snapshots
             (league_id, roster_id, season, fetched_at, owner, wins, losses, data)
             VALUES (?,?,?,?,?,?,?,?)",
        )
        .bind(row.league_id)
        .bind(row.roster_id)
        .bind(row.season)
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(row.owner)
        .bind(row.wins)
        .bind(row.losses)
        .bind(serde_json::to_string(row.data)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Best-effort staleness check purely for the warn log — the CASE
    /// clauses in `upsert_transaction`'s SQL are what actually enforce
    /// monotonicity, this only explains *why* a write had no effect. A race
    /// between this read and the upsert can't cause incorrect data, only a
    /// missed/extra log line.
    async fn warn_if_stale_status_write(&self, row: &TransactionRow<'_>) -> Result<()> {
        let Some(incoming) = row.status_updated else {
            return Ok(());
        };
        let existing: Option<(Option<i64>,)> =
            sqlx::query_as("SELECT status_updated FROM transactions WHERE transaction_id = ?")
                .bind(row.transaction_id)
                .fetch_optional(&self.pool)
                .await?;
        if let Some((Some(stored),)) = existing {
            if incoming < stored {
                tracing::warn!(
                    transaction_id = row.transaction_id,
                    incoming_status_updated = incoming,
                    stored_status_updated = stored,
                    "upsert_transaction: rejected out-of-order status write (stale status_updated)"
                );
            }
        }
        Ok(())
    }

    /// Idempotent upsert (PK = transaction_id) of one resolved transaction —
    /// `adds`/`drops` are pre-resolved JSON (player name/position/owner, not
    /// bare IDs) so a `sqlite3 stats.db` query can filter by position/owner
    /// directly without a second lookup pass.
    pub async fn upsert_transaction(&self, row: TransactionRow<'_>) -> Result<()> {
        self.warn_if_stale_status_write(&row).await?;
        Self::exec_upsert_transaction(&self.pool, &row).await
    }

    /// Fix 2 (Phase 6 review of the GraphQL backfill): transaction-scoped
    /// variant of [`Self::upsert_transaction`], so `persist_league_transactions`
    /// can run every upsert in the backfill plus the trailing
    /// `record_sync_run_in_tx` coverage row as one SQL transaction — a
    /// mid-backfill crash then can't leave a partially-updated `transactions`
    /// table with no ledger row to reveal the backfill never finished.
    pub(crate) async fn upsert_transaction_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'static, sqlx::Sqlite>,
        row: TransactionRow<'_>,
    ) -> Result<()> {
        self.warn_if_stale_status_write(&row).await?;
        Self::exec_upsert_transaction(&mut **tx, &row).await
    }

    // kibitzer:ignore long-function -- body is one atomic SQL upsert statement; the line
    // count is the query text itself, splitting it further would fragment one operation.
    async fn exec_upsert_transaction<'e, E>(executor: E, row: &TransactionRow<'_>) -> Result<()>
    where
        E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
    {
        // `waiver_bid` is COALESCEd on conflict rather than overwritten: the
        // public-REST sync path never knows a bid amount, and a re-sync from
        // there must not erase a bid the authenticated-browser sync path
        // already recorded for the same transaction_id.
        //
        // `status`/`status_updated` are CAS-guarded the same way, but on
        // epoch rather than presence: an incoming write only applies if its
        // `status_updated` is non-NULL and >= the stored value (NULL stored
        // treated as 0), so a delayed/out-of-order sync can never regress a
        // resolved status back to `pending`. A NULL incoming `status_updated`
        // (REST writes) never touches `status` at all.
        sqlx::query(
            "INSERT INTO transactions
             (transaction_id, league_id, season, week, type, status, created, adds, drops, roster_ids, waiver_bid, status_updated, source, fetched_at)
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)
             ON CONFLICT(transaction_id) DO UPDATE SET
                 league_id  = excluded.league_id,
                 season     = excluded.season,
                 week       = excluded.week,
                 type       = excluded.type,
                 status = CASE
                     WHEN excluded.status_updated IS NULL THEN transactions.status
                     WHEN excluded.status_updated >= COALESCE(transactions.status_updated, 0)
                         THEN excluded.status
                     ELSE transactions.status
                 END,
                 status_updated = CASE
                     WHEN excluded.status_updated IS NULL THEN transactions.status_updated
                     WHEN excluded.status_updated >= COALESCE(transactions.status_updated, 0)
                         THEN excluded.status_updated
                     ELSE transactions.status_updated
                 END,
                 created    = excluded.created,
                 adds       = excluded.adds,
                 drops      = excluded.drops,
                 roster_ids = excluded.roster_ids,
                 waiver_bid = COALESCE(excluded.waiver_bid, transactions.waiver_bid),
                 source     = excluded.source,
                 fetched_at = excluded.fetched_at",
        )
        .bind(row.transaction_id)
        .bind(row.league_id)
        .bind(row.season)
        .bind(row.week)
        .bind(row.kind)
        .bind(row.status)
        .bind(row.created)
        .bind(serde_json::to_string(row.adds)?)
        .bind(serde_json::to_string(row.drops)?)
        .bind(serde_json::to_string(row.roster_ids)?)
        .bind(row.waiver_bid)
        .bind(row.status_updated)
        .bind(row.source)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(executor)
        .await?;
        Ok(())
    }

    /// Cached transaction history for `league_id`, optionally narrowed to
    /// one `type` (`"waiver"`/`"trade"`/`"free_agent"`). Reads only the
    /// local cache — no API/browser call — so analysis built on this never
    /// re-fetches to run.
    pub async fn list_transactions(
        &self,
        league_id: &str,
        kind_filter: Option<&str>,
    ) -> Result<Vec<CachedTransaction>> {
        #[allow(clippy::type_complexity)]
        let rows: Vec<(
            String,
            String,
            i32,
            String,
            Option<String>,
            i64,
            String,
            String,
            Option<String>,
            Option<i64>,
            String,
            Option<i64>,
            Option<String>,
        )> = sqlx::query_as(
            "SELECT transaction_id, season, week, type, status, created, adds, drops, roster_ids, waiver_bid, fetched_at, status_updated, source
             FROM transactions
             WHERE league_id = ? AND (? IS NULL OR type = ?)
             ORDER BY created ASC",
        )
        .bind(league_id)
        .bind(kind_filter)
        .bind(kind_filter)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(
                |(
                    transaction_id,
                    season,
                    week,
                    kind,
                    status,
                    created,
                    adds,
                    drops,
                    roster_ids,
                    waiver_bid,
                    fetched_at,
                    status_updated,
                    source,
                )| {
                    Ok(CachedTransaction {
                        transaction_id,
                        season,
                        week,
                        kind,
                        status,
                        created,
                        adds: serde_json::from_str(&adds)?,
                        drops: serde_json::from_str(&drops)?,
                        roster_ids: roster_ids
                            .map(|s| serde_json::from_str(&s))
                            .transpose()?
                            .unwrap_or(serde_json::Value::Array(Vec::new())),
                        waiver_bid,
                        // Written via `to_rfc3339()` in `upsert_transaction`;
                        // `NOT NULL` since the original `CREATE TABLE`, so
                        // every row has one (unlike `status_updated`/`source`,
                        // added later via `ALTER TABLE` with no backfill).
                        fetched_at: chrono::DateTime::parse_from_rfc3339(&fetched_at)
                            .map_err(|e| crate::error::Error::Other(e.into()))?
                            .with_timezone(&chrono::Utc),
                        status_updated,
                        source,
                    })
                },
            )
            .collect()
    }

    pub async fn get_league(&self, league_id: &str) -> Result<Option<League>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT data FROM leagues WHERE league_id=?")
                .bind(league_id)
                .fetch_optional(&self.pool)
                .await?;
        row.map(|(json,)| serde_json::from_str(&json).map_err(Into::into))
            .transpose()
    }

    pub async fn list_leagues(&self) -> Result<Vec<(League, String)>> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT data, synced_at FROM leagues ORDER BY synced_at DESC")
                .fetch_all(&self.pool)
                .await?;
        rows.into_iter()
            .map(|(json, synced_at)| {
                serde_json::from_str::<League>(&json)
                    .map(|l| (l, synced_at))
                    .map_err(Into::into)
            })
            .collect()
    }

    pub async fn summary(&self) -> Result<serde_json::Value> {
        let seasons: Vec<(String, i32)> = sqlx::query_as(
            "SELECT season, COUNT(DISTINCT week) as weeks FROM weeks_fetched
             GROUP BY season ORDER BY season",
        )
        .fetch_all(&self.pool)
        .await?;

        let (total_rows,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM player_stats")
                .fetch_one(&self.pool)
                .await?;

        Ok(serde_json::json!({
            "total_stat_rows": total_rows,
            "seasons": seasons.into_iter().map(|(s, w)| serde_json::json!({
                "season": s,
                "weeks_cached": w,
            })).collect::<Vec<_>>(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_db;
    use std::sync::{Arc, Mutex};

    fn base_row<'a>(
        transaction_id: &'a str,
        empty: &'a serde_json::Value,
        roster_ids: &'a serde_json::Value,
    ) -> TransactionRow<'a> {
        TransactionRow {
            transaction_id,
            league_id: "league1",
            season: "2026",
            week: 9,
            kind: "waiver",
            status: Some("pending"),
            created: 1_781_000_000,
            adds: empty,
            drops: empty,
            roster_ids,
            waiver_bid: None,
            status_updated: None,
            source: "rest",
        }
    }

    async fn stored_status(db: &StatsDb, transaction_id: &str) -> (Option<String>, Option<i64>) {
        sqlx::query_as("SELECT status, status_updated FROM transactions WHERE transaction_id = ?")
            .bind(transaction_id)
            .fetch_one(&db.pool)
            .await
            .expect("row exists")
    }

    /// `MakeWriter` that appends formatted log output into a shared buffer —
    /// enough to assert a specific `warn!` fired without a dev-dependency.
    #[derive(Clone, Default)]
    struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for CapturedLogs {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLogs {
        type Writer = CapturedLogs;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    impl CapturedLogs {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    // AC1: an incoming write with an older status_updated than the stored
    // row never changes `status`, and the rejection is warn-logged.
    #[tokio::test]
    async fn upsert_transaction_should_reject_status_downgrade_when_status_updated_is_older() {
        let db = test_db("reject_downgrade").await;
        let empty = serde_json::json!({});
        let roster_ids = serde_json::json!([5]);

        let mut seed = base_row("998877", &empty, &roster_ids);
        seed.status = Some("complete");
        seed.status_updated = Some(1_781_900_000);
        seed.source = "graphql";
        db.upsert_transaction(seed).await.expect("seed upsert");

        let logs = CapturedLogs::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(logs.clone())
            .with_ansi(false)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        let mut stale = base_row("998877", &empty, &roster_ids);
        stale.status = Some("pending");
        stale.status_updated = Some(1_781_890_000); // 10 minutes older than stored
        stale.source = "graphql";
        db.upsert_transaction(stale).await.expect("stale upsert");

        drop(_guard);
        let log_text = logs.text();
        assert!(
            log_text.contains("rejected out-of-order"),
            "expected a warn log for the rejected downgrade, got: {log_text}"
        );

        let (status, status_updated) = stored_status(&db, "998877").await;
        assert_eq!(status, Some("complete".to_string()));
        assert_eq!(status_updated, Some(1_781_900_000));
    }

    // AC2: a REST write (status_updated: None) never touches an existing
    // status/status_updated on conflict — the CASE branch never applies.
    #[tokio::test]
    async fn upsert_transaction_should_preserve_status_when_source_is_rest_and_status_updated_is_none(
    ) {
        let db = test_db("rest_never_touches_status").await;
        let empty = serde_json::json!({});
        let roster_ids = serde_json::json!([5]);

        let mut seed = base_row("998877", &empty, &roster_ids);
        seed.status = Some("complete");
        seed.status_updated = Some(1_781_900_000);
        seed.source = "graphql";
        db.upsert_transaction(seed).await.expect("seed upsert");

        let mut rest_resync = base_row("998877", &empty, &roster_ids);
        rest_resync.status = Some("complete");
        rest_resync.status_updated = None;
        rest_resync.source = "rest";
        db.upsert_transaction(rest_resync).await.expect("rest resync upsert");

        let (status, status_updated) = stored_status(&db, "998877").await;
        assert_eq!(status, Some("complete".to_string()));
        assert_eq!(status_updated, Some(1_781_900_000));
    }

    // AC3: a first-ever status_updated value (stored row currently NULL,
    // inserted by an earlier REST sync) is always accepted, never rejected
    // as a false "stale" downgrade.
    #[tokio::test]
    async fn upsert_transaction_should_accept_status_update_when_no_prior_status_updated_exists()
    {
        let db = test_db("first_ever_status_updated").await;
        let empty = serde_json::json!({});
        let roster_ids = serde_json::json!([5]);

        let mut seed = base_row("1", &empty, &roster_ids);
        seed.status = Some("pending");
        seed.status_updated = None;
        seed.source = "rest";
        db.upsert_transaction(seed).await.expect("seed upsert");

        let mut graphql_write = base_row("1", &empty, &roster_ids);
        graphql_write.status = Some("pending");
        graphql_write.status_updated = Some(1_781_850_000);
        graphql_write.source = "graphql";
        db.upsert_transaction(graphql_write).await.expect("graphql upsert");

        let (status, status_updated) = stored_status(&db, "1").await;
        assert_eq!(status, Some("pending".to_string()));
        assert_eq!(status_updated, Some(1_781_850_000));
    }

    // Epic 1.2 / Story 1.2.1 AC1: a successful sync completion records
    // exactly one ledger row with the attempt's metadata.
    #[tokio::test]
    async fn record_sync_run_should_insert_one_ledger_row_when_sync_completes() {
        let db = test_db("record_sync_run_single_row").await;
        let synced_at = chrono::Utc::now();

        db.record_sync_run(TransactionSyncRun {
            league_id: "1352883872131481600",
            season: "2026",
            source: "graphql",
            roster_scope: "all",
            synced_at,
            rows_fetched: 187,
            rows_changed: 42,
        })
        .await
        .expect("record sync run");

        let rows: Vec<(String, String, String, String, i64, i64)> = sqlx::query_as(
            "SELECT league_id, season, source, roster_scope, rows_fetched, rows_changed
             FROM transaction_sync_runs",
        )
        .fetch_all(&db.pool)
        .await
        .expect("query ledger rows");

        assert_eq!(rows.len(), 1);
        let (league_id, season, source, roster_scope, rows_fetched, rows_changed) = &rows[0];
        assert_eq!(league_id, "1352883872131481600");
        assert_eq!(season, "2026");
        assert_eq!(source, "graphql");
        assert_eq!(roster_scope, "all");
        assert_eq!(*rows_fetched, 187);
        assert_eq!(*rows_changed, 42);
    }

    // Epic 1.2 / Story 1.2.1 AC2: only source='rest' ledger rows exist for
    // this (league_id, season) — no full-league GraphQL backfill has ever
    // completed, so coverage must report None rather than a false positive.
    #[tokio::test]
    async fn has_full_league_graphql_coverage_should_return_none_when_only_rest_rows_recorded() {
        let db = test_db("coverage_none_when_rest_only").await;

        db.record_sync_run(TransactionSyncRun {
            league_id: "1352883872131481600",
            season: "2026",
            source: "rest",
            roster_scope: "single",
            synced_at: chrono::Utc::now(),
            rows_fetched: 20,
            rows_changed: 3,
        })
        .await
        .expect("record rest sync run");

        let coverage = db
            .has_full_league_graphql_coverage("1352883872131481600", "2026")
            .await
            .expect("query coverage");

        assert_eq!(coverage, None);
    }

    // Epic 1.2 / Story 1.2.1 AC2 (happy path): a source='graphql',
    // roster_scope='all' row exists, so coverage returns its synced_at
    // rather than None.
    #[tokio::test]
    async fn has_full_league_graphql_coverage_should_return_synced_at_when_graphql_all_row_exists()
    {
        let db = test_db("coverage_some_when_graphql_all").await;
        let synced_at = chrono::Utc::now();

        db.record_sync_run(TransactionSyncRun {
            league_id: "1352883872131481600",
            season: "2026",
            source: "rest",
            roster_scope: "single",
            synced_at: synced_at - chrono::Duration::hours(1),
            rows_fetched: 20,
            rows_changed: 3,
        })
        .await
        .expect("record rest sync run");
        db.record_sync_run(TransactionSyncRun {
            league_id: "1352883872131481600",
            season: "2026",
            source: "graphql",
            roster_scope: "all",
            synced_at,
            rows_fetched: 187,
            rows_changed: 187,
        })
        .await
        .expect("record graphql sync run");

        let coverage = db
            .has_full_league_graphql_coverage("1352883872131481600", "2026")
            .await
            .expect("query coverage");

        // RFC3339 round-trips through sub-second precision losslessly, so
        // comparing the parsed DateTime (not a string) avoids a flaky
        // format mismatch.
        assert_eq!(coverage, Some(synced_at));
    }

    // Epic 1.3 / Story 1.3.1: regression guard against the busy_timeout
    // setting being dropped from the SqliteConnectOptions chain — asserts
    // the PRAGMA directly rather than only observing its effect.
    #[tokio::test]
    async fn statsdb_open_should_set_busy_timeout_via_pragma() {
        let db = test_db("busy_timeout_pragma").await;

        let (busy_timeout_ms,): (i64,) = sqlx::query_as("PRAGMA busy_timeout")
            .fetch_one(&db.pool)
            .await
            .expect("query busy_timeout pragma");

        assert_eq!(busy_timeout_ms, 5_000);
    }

    // Epic 1.3 / Story 1.3.1 AC1 (happy path): two StatsDb handles against
    // the same file, simulating a REST sync and a GraphQL sync racing each
    // other. One holds the write lock via an open transaction; the other's
    // upsert_transaction must block and succeed once the lock is released,
    // rather than returning SQLITE_BUSY. A oneshot channel (not a bare
    // sleep) gates when the second writer starts, so the test can't flake
    // by racing the lock-acquisition itself — only the busy_timeout wait is
    // timed.
    #[tokio::test]
    async fn upsert_transaction_should_succeed_under_concurrent_writers_when_busy_timeout_configured(
    ) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "sleeper_test_concurrent_writers_{}_{nanos}.db",
            std::process::id()
        ));
        let db_a = StatsDb::open(&path).await.expect("open db_a");
        let db_b = StatsDb::open(&path).await.expect("open db_b");

        let (lock_held_tx, lock_held_rx) = tokio::sync::oneshot::channel();

        let holder = tokio::spawn(async move {
            let mut tx = db_a.pool.begin().await.expect("begin lock-holder tx");
            // BEGIN is deferred in SQLite; a write is what actually takes
            // the write lock the second handle will contend on.
            sqlx::query("INSERT INTO leagues (league_id, data, synced_at) VALUES (?, ?, ?)")
                .bind("lock-holder")
                .bind("{}")
                .bind(chrono::Utc::now().to_rfc3339())
                .execute(&mut *tx)
                .await
                .expect("acquire write lock");
            lock_held_tx.send(()).expect("signal lock held");
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            tx.commit().await.expect("commit lock holder");
        });

        lock_held_rx.await.expect("wait for lock to be held");

        let empty = serde_json::json!({});
        let roster_ids = serde_json::json!([5]);
        let row = base_row("concurrent-writer", &empty, &roster_ids);
        let result = db_b.upsert_transaction(row).await;

        holder.await.expect("join lock-holder task");

        result.expect(
            "upsert_transaction should block on busy_timeout and succeed, not return SQLITE_BUSY",
        );
    }

    // Epic 2.1 / Task 2.1.1a (happy path): `list_transactions` must surface
    // `fetched_at`/`status_updated`/`source` on the returned
    // `CachedTransaction` — today it has no `fetched_at` field at all, a
    // read-side gap this task closes so a staleness decision
    // (`StalenessTolerance::is_fresh`) can be made without a second query.
    #[tokio::test]
    async fn list_transactions_should_include_fetched_at_status_updated_and_source_when_row_persisted(
    ) {
        let db = test_db("list_transactions_fetched_at").await;
        let empty = serde_json::json!({});
        let roster_ids = serde_json::json!([5]);
        let mut row = base_row("1", &empty, &roster_ids);
        row.league_id = "1352883872131481600";
        row.status_updated = Some(1_781_850_000);
        row.source = "graphql";

        let before = chrono::Utc::now();
        db.upsert_transaction(row).await.expect("upsert transaction");
        let after = chrono::Utc::now();

        let rows = db
            .list_transactions("1352883872131481600", Some("waiver"))
            .await
            .expect("list cached transactions");
        let cached = rows.iter().find(|t| t.transaction_id == "1").expect("transaction 1 present");

        assert!(
            cached.fetched_at >= before && cached.fetched_at <= after,
            "fetched_at {:?} must fall within [{before:?}, {after:?}]",
            cached.fetched_at
        );
        assert_eq!(cached.status_updated, Some(1_781_850_000));
        assert_eq!(cached.source.as_deref(), Some("graphql"));
    }

    // validation.md "Migration Test — migration_should_be_reversible",
    // scenario 1 (idempotent against an already-migrated DB): opening
    // `StatsDb` a second time against the same file — simulating an app
    // restart — must not error and must not attempt to re-add columns that
    // already exist. This proves the `PRAGMA table_info` guard in
    // `StatsDb::open` actually gates the `ALTER TABLE` calls rather than
    // relying on them failing silently.
    #[tokio::test]
    async fn migration_should_be_reversible_when_statsdb_reopened_against_already_migrated_db() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "sleeper_test_migration_reopen_{}_{nanos}.db",
            std::process::id()
        ));

        let db1 = StatsDb::open(&path).await.expect("first open");
        drop(db1);

        let db2 = StatsDb::open(&path)
            .await
            .expect("second open (simulated restart) must not error");

        let existing_cols: Vec<(String,)> =
            sqlx::query_as("SELECT name FROM pragma_table_info('transactions')")
                .fetch_all(&db2.pool)
                .await
                .expect("query table_info");
        let has_col = |name: &str| existing_cols.iter().any(|(c,)| c == name);
        assert!(has_col("roster_ids"));
        assert!(has_col("waiver_bid"));
        assert!(has_col("status_updated"));
        assert!(has_col("source"));
        // No duplicate columns from a re-run ALTER TABLE: each name appears once.
        for name in ["roster_ids", "waiver_bid", "status_updated", "source"] {
            let count = existing_cols.iter().filter(|(c,)| c == name).count();
            assert_eq!(count, 1, "column {name} must appear exactly once, not duplicated");
        }

        let (sync_runs_table_count,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='transaction_sync_runs'",
        )
        .fetch_one(&db2.pool)
        .await
        .expect("query sqlite_master");
        assert_eq!(sync_runs_table_count, 1, "transaction_sync_runs must exist exactly once");
    }

    // validation.md "Migration Test — migration_should_be_reversible",
    // scenario 2 (additive-safe against a pre-existing old-schema DB): a DB
    // file created before this project's columns/table existed — only the
    // pre-project `transactions` shape, no `status_updated`/`source`/
    // `transaction_sync_runs` — must open cleanly through `StatsDb::open`,
    // gain the new columns/table, and leave the pre-existing row's data
    // untouched. This is the "forward-only, additive, no data loss"
    // guarantee plan.md's Migration Plan section describes.
    #[tokio::test]
    async fn migration_should_be_reversible_when_statsdb_opens_pre_existing_old_schema_db() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "sleeper_test_migration_old_schema_{}_{nanos}.db",
            std::process::id()
        ));

        {
            // Hand-build the pre-project schema directly (bypassing
            // StatsDb::open) so this starts from a file that genuinely
            // predates the status_updated/source/transaction_sync_runs
            // additions, not from a file this version already migrated.
            let options = SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true);
            let pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(options)
                .await
                .expect("create old-schema db");

            sqlx::query(
                "CREATE TABLE transactions (
                    transaction_id TEXT    PRIMARY KEY,
                    league_id      TEXT    NOT NULL,
                    season         TEXT    NOT NULL,
                    week           INTEGER NOT NULL,
                    type           TEXT    NOT NULL,
                    status         TEXT,
                    created        INTEGER,
                    adds           TEXT,
                    drops          TEXT,
                    fetched_at     TEXT    NOT NULL,
                    roster_ids     TEXT,
                    waiver_bid     INTEGER
                )",
            )
            .execute(&pool)
            .await
            .expect("create pre-project transactions table");

            sqlx::query(
                "INSERT INTO transactions
                 (transaction_id, league_id, season, week, type, status, created, adds, drops, fetched_at, roster_ids, waiver_bid)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind("pre-existing-1")
            .bind("league1")
            .bind("2026")
            .bind(9_i64)
            .bind("waiver")
            .bind("complete")
            .bind(1_781_000_000_i64)
            .bind("{}")
            .bind("{}")
            .bind("2026-01-01T00:00:00Z")
            .bind("[5]")
            .bind(12_i64)
            .execute(&pool)
            .await
            .expect("insert pre-existing row");

            pool.close().await;
        }

        let db = StatsDb::open(&path)
            .await
            .expect("open against old-schema file must not error");

        let existing_cols: Vec<(String,)> =
            sqlx::query_as("SELECT name FROM pragma_table_info('transactions')")
                .fetch_all(&db.pool)
                .await
                .expect("query table_info");
        let has_col = |name: &str| existing_cols.iter().any(|(c,)| c == name);
        assert!(has_col("status_updated"), "status_updated column must be added");
        assert!(has_col("source"), "source column must be added");

        let (sync_runs_table_count,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='transaction_sync_runs'",
        )
        .fetch_one(&db.pool)
        .await
        .expect("query sqlite_master");
        assert_eq!(sync_runs_table_count, 1, "transaction_sync_runs table must be created");

        let row: (String, String, Option<String>, Option<i64>, Option<i64>, Option<String>) =
            sqlx::query_as(
                "SELECT transaction_id, league_id, status, waiver_bid, status_updated, source
                 FROM transactions WHERE transaction_id = ?",
            )
            .bind("pre-existing-1")
            .fetch_one(&db.pool)
            .await
            .expect("pre-existing row must still exist");
        let (transaction_id, league_id, status, waiver_bid, status_updated, source) = row;

        assert_eq!(transaction_id, "pre-existing-1");
        assert_eq!(league_id, "league1");
        assert_eq!(status.as_deref(), Some("complete"));
        assert_eq!(waiver_bid, Some(12), "pre-existing waiver_bid must be untouched");
        assert_eq!(status_updated, None, "new column reads back as NULL on a pre-existing row");
        assert_eq!(source, None, "new column reads back as NULL on a pre-existing row");

        // Second open against the now-migrated file: still a no-op, no
        // duplicate-column error.
        let db2 = StatsDb::open(&path)
            .await
            .expect("second open of now-migrated file must not error");
        drop(db2);
    }

    #[tokio::test]
    async fn log_analysis_note_should_round_trip_through_list_analysis_notes() {
        let db = test_db("log_note").await;
        let id = db
            .log_analysis_note("league1", "trade_scan", "summary text", Some("detail text"))
            .await
            .expect("log note");
        assert!(id > 0);

        let notes = db.list_analysis_notes(Some("league1"), 10).await.expect("list notes");
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].id, id);
        assert_eq!(notes[0].topic, "trade_scan");
        assert_eq!(notes[0].summary, "summary text");
        assert_eq!(notes[0].detail.as_deref(), Some("detail text"));
    }

    #[tokio::test]
    async fn list_analysis_notes_should_return_most_recent_first_and_respect_limit() {
        let db = test_db("list_notes_order").await;
        db.log_analysis_note("league1", "a", "first", None).await.expect("log a");
        db.log_analysis_note("league1", "b", "second", None).await.expect("log b");
        db.log_analysis_note("league1", "c", "third", None).await.expect("log c");

        let notes = db.list_analysis_notes(Some("league1"), 2).await.expect("list notes");
        assert_eq!(notes.len(), 2, "limit must be respected");
        assert_eq!(notes[0].topic, "c", "most recently logged note must come first");
        assert_eq!(notes[1].topic, "b");
    }

    #[tokio::test]
    async fn list_analysis_notes_should_filter_by_league_when_given_one() {
        let db = test_db("list_notes_league_filter").await;
        db.log_analysis_note("league1", "a", "league1 note", None).await.expect("log a");
        db.log_analysis_note("league2", "b", "league2 note", None).await.expect("log b");

        let notes = db.list_analysis_notes(Some("league1"), 10).await.expect("list notes");
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].summary, "league1 note");

        let all_notes = db.list_analysis_notes(None, 10).await.expect("list all notes");
        assert_eq!(all_notes.len(), 2, "omitting league_id must return every league's notes");
    }

    #[tokio::test]
    async fn import_chat_transcript_should_round_trip_through_get_chat_transcript() {
        let db = test_db("import_transcript").await;
        db.import_chat_transcript("session-1", Some("league1"), "/tmp/session-1.jsonl", "raw jsonl content")
            .await
            .expect("import transcript");

        let full = db
            .get_chat_transcript("session-1")
            .await
            .expect("get transcript")
            .expect("transcript must exist");
        assert_eq!(full.session_id, "session-1");
        assert_eq!(full.league_id.as_deref(), Some("league1"));
        assert_eq!(full.source_path, "/tmp/session-1.jsonl");
        assert_eq!(full.raw_text, "raw jsonl content");
    }

    #[tokio::test]
    async fn import_chat_transcript_should_replace_not_duplicate_on_re_import_of_same_session() {
        let db = test_db("import_transcript_idempotent").await;
        db.import_chat_transcript("session-1", None, "/tmp/session-1.jsonl", "v1 content")
            .await
            .expect("first import");
        db.import_chat_transcript("session-1", None, "/tmp/session-1.jsonl", "v2 content, grown since")
            .await
            .expect("re-import of same actively-growing session");

        let all = db.list_chat_transcripts(None, 10).await.expect("list transcripts");
        assert_eq!(all.len(), 1, "re-importing the same session_id must replace, not duplicate");
        assert_eq!(all[0].size_bytes, "v2 content, grown since".len() as i64);

        let full = db.get_chat_transcript("session-1").await.expect("get transcript").expect("must exist");
        assert_eq!(full.raw_text, "v2 content, grown since");
    }

    #[tokio::test]
    async fn list_chat_transcripts_should_never_load_raw_text_into_the_metadata_row() {
        let db = test_db("list_transcripts_metadata_only").await;
        db.import_chat_transcript("session-1", Some("league1"), "/tmp/a.jsonl", "some real content")
            .await
            .expect("import");

        let metas = db.list_chat_transcripts(Some("league1"), 10).await.expect("list");
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].session_id, "session-1");
        assert_eq!(metas[0].size_bytes, "some real content".len() as i64);
    }

    #[tokio::test]
    async fn get_chat_transcript_should_return_none_when_session_id_was_never_imported() {
        let db = test_db("get_transcript_missing").await;
        let result = db.get_chat_transcript("never-imported").await.expect("get transcript");
        assert!(result.is_none());
    }

    fn chat_row<'a>(message_id: &'a str, author_name: &'a str, text: &'a str, created: i64) -> LeagueChatMessageRow<'a> {
        LeagueChatMessageRow {
            message_id,
            author_id: Some("user1"),
            author_name: Some(author_name),
            text,
            created,
            pinned: false,
        }
    }

    #[tokio::test]
    async fn upsert_league_chat_messages_should_round_trip_through_list_league_chat_messages() {
        let db = test_db("league_chat_round_trip").await;
        let rows = [chat_row("m1", "tstapler", "hello league", 1000)];
        db.upsert_league_chat_messages("league1", &rows).await.expect("upsert");

        let messages = db.list_league_chat_messages("league1", 10).await.expect("list");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].message_id, "m1");
        assert_eq!(messages[0].author_name.as_deref(), Some("tstapler"));
        assert_eq!(messages[0].text, "hello league");
    }

    #[tokio::test]
    async fn list_league_chat_messages_should_return_most_recent_first() {
        let db = test_db("league_chat_order").await;
        let rows = [chat_row("m1", "a", "first", 1000), chat_row("m2", "b", "second", 2000)];
        db.upsert_league_chat_messages("league1", &rows).await.expect("upsert");

        let messages = db.list_league_chat_messages("league1", 10).await.expect("list");
        assert_eq!(messages[0].message_id, "m2", "newest created timestamp must come first");
        assert_eq!(messages[1].message_id, "m1");
    }

    #[tokio::test]
    async fn upsert_league_chat_messages_should_replace_not_duplicate_same_message_id() {
        let db = test_db("league_chat_idempotent").await;
        db.upsert_league_chat_messages("league1", &[chat_row("m1", "a", "v1", 1000)])
            .await
            .expect("first upsert");
        db.upsert_league_chat_messages("league1", &[chat_row("m1", "a", "v1 edited", 1000)])
            .await
            .expect("re-sync of same message_id (e.g. edited)");

        let messages = db.list_league_chat_messages("league1", 10).await.expect("list");
        assert_eq!(messages.len(), 1, "same message_id must replace, not duplicate");
        assert_eq!(messages[0].text, "v1 edited");
    }

    #[tokio::test]
    async fn league_chat_message_ids_should_return_only_ids_already_stored_for_that_league() {
        let db = test_db("league_chat_known_ids").await;
        db.upsert_league_chat_messages("league1", &[chat_row("m1", "a", "x", 1000)]).await.expect("upsert");
        db.upsert_league_chat_messages("league2", &[chat_row("m2", "a", "y", 1000)]).await.expect("upsert");

        let ids = db.league_chat_message_ids("league1").await.expect("ids");
        assert!(ids.contains("m1"));
        assert!(!ids.contains("m2"), "must not leak another league's message ids");
    }
}
