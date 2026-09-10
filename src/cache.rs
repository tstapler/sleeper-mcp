/// HTTP response cache backed by SQLite.
///
/// Stores full response bodies with their headers. Checks Cache-Control
/// max-age for freshness, and exposes ETag / Last-Modified validators
/// for conditional requests.
use std::path::Path;

use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;

use crate::error::Result;

#[derive(Debug)]
pub struct CachedResponse {
    pub status: u16,
    pub body: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

pub struct HttpCache {
    pool: SqlitePool,
    default_ttl: i64,
}

impl HttpCache {
    pub async fn open(path: &Path, default_ttl_secs: i64) -> Result<Self> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal);

        let pool = SqlitePoolOptions::new()
            .max_connections(3)
            .connect_with(options)
            .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS http_cache (
                key          TEXT PRIMARY KEY,
                status       INTEGER NOT NULL,
                body         TEXT NOT NULL,
                etag         TEXT,
                last_modified TEXT,
                cached_at    INTEGER NOT NULL,
                ttl_secs     INTEGER NOT NULL
            );",
        )
        .execute(&pool)
        .await?;

        Ok(Self { pool, default_ttl: default_ttl_secs })
    }

    fn cache_key(method: &str, url: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(method.as_bytes());
        hasher.update(b"|");
        hasher.update(url.as_bytes());
        hex::encode(hasher.finalize())
    }

    /// Returns the cached response if it exists. `is_fresh` indicates whether
    /// it is within its TTL. If stale but present, the caller should send a
    /// conditional request using `etag` / `last_modified`.
    pub async fn get(&self, method: &str, url: &str) -> Result<Option<(CachedResponse, bool)>> {
        let key = Self::cache_key(method, url);
        let now = chrono::Utc::now().timestamp();

        let row: Option<(i64, String, Option<String>, Option<String>, i64, i64)> =
            sqlx::query_as(
                "SELECT status, body, etag, last_modified, cached_at, ttl_secs
                 FROM http_cache WHERE key=?",
            )
            .bind(&key)
            .fetch_optional(&self.pool)
            .await?;

        let Some((status, body, etag, last_modified, cached_at, ttl_secs)) = row else {
            return Ok(None);
        };

        let is_fresh = (now - cached_at) < ttl_secs;
        Ok(Some((
            CachedResponse {
                status: status as u16,
                body,
                etag,
                last_modified,
            },
            is_fresh,
        )))
    }

    pub async fn set(
        &self,
        method: &str,
        url: &str,
        status: u16,
        body: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
        ttl_override: Option<i64>,
    ) -> Result<()> {
        let key = Self::cache_key(method, url);
        let now = chrono::Utc::now().timestamp();
        let ttl = ttl_override.unwrap_or(self.default_ttl);

        sqlx::query(
            "INSERT OR REPLACE INTO http_cache
             (key, status, body, etag, last_modified, cached_at, ttl_secs)
             VALUES (?,?,?,?,?,?,?)",
        )
        .bind(&key)
        .bind(status as i64)
        .bind(body)
        .bind(etag)
        .bind(last_modified)
        .bind(now)
        .bind(ttl)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Update the cached body after a successful revalidation (304-turned-200).
    pub async fn update_body(&self, method: &str, url: &str, new_body: &str) -> Result<()> {
        let key = Self::cache_key(method, url);
        let now = chrono::Utc::now().timestamp();
        sqlx::query("UPDATE http_cache SET body=?, cached_at=? WHERE key=?")
            .bind(new_body)
            .bind(now)
            .bind(&key)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Parse Cache-Control: max-age=N from response headers, returning the TTL in seconds.
    pub fn parse_max_age(cache_control: Option<&str>) -> Option<i64> {
        let cc = cache_control?;
        for part in cc.split(',') {
            let part = part.trim().to_lowercase();
            if let Some(val) = part.strip_prefix("max-age=") {
                return val.trim().parse().ok();
            }
        }
        None
    }
}
