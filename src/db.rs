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
}

/// Row shape returned by `StatsDb::list_transactions` — read from the local
/// cache only, never triggers an API/browser call.
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
}

impl StatsDb {
    pub async fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal);

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
                ON transactions (league_id, season, week);",
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

    /// Idempotent upsert (PK = transaction_id) of one resolved transaction —
    /// `adds`/`drops` are pre-resolved JSON (player name/position/owner, not
    /// bare IDs) so a `sqlite3 stats.db` query can filter by position/owner
    /// directly without a second lookup pass.
    pub async fn upsert_transaction(&self, row: TransactionRow<'_>) -> Result<()> {
        // `waiver_bid` is COALESCEd on conflict rather than overwritten: the
        // public-REST sync path never knows a bid amount, and a re-sync from
        // there must not erase a bid the authenticated-browser sync path
        // already recorded for the same transaction_id.
        sqlx::query(
            "INSERT INTO transactions
             (transaction_id, league_id, season, week, type, status, created, adds, drops, roster_ids, waiver_bid, fetched_at)
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?)
             ON CONFLICT(transaction_id) DO UPDATE SET
                 league_id  = excluded.league_id,
                 season     = excluded.season,
                 week       = excluded.week,
                 type       = excluded.type,
                 status     = excluded.status,
                 created    = excluded.created,
                 adds       = excluded.adds,
                 drops      = excluded.drops,
                 roster_ids = excluded.roster_ids,
                 waiver_bid = COALESCE(excluded.waiver_bid, transactions.waiver_bid),
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
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&self.pool)
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
        let rows: Vec<(String, String, i32, String, Option<String>, i64, String, String, Option<String>, Option<i64>)> =
            sqlx::query_as(
                "SELECT transaction_id, season, week, type, status, created, adds, drops, roster_ids, waiver_bid
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
            .map(|(transaction_id, season, week, kind, status, created, adds, drops, roster_ids, waiver_bid)| {
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
                })
            })
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
