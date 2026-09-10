/// Cross-process rate limiter implemented as a goroutine actor.
///
/// Callers send a oneshot reply channel into the actor's inbox. The actor
/// serialises access via a single tokio task, checks a SQLite timestamp
/// table (cross-process safe via WAL), and replies when a permit is granted.
/// Callers never hold a mutex — they just await the reply.
use std::path::Path;
use std::sync::Arc;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;
use tokio::sync::{mpsc, oneshot};

type PermitRequest = oneshot::Sender<()>;

#[derive(Clone)]
pub struct RateLimiter {
    tx: mpsc::Sender<PermitRequest>,
}

impl RateLimiter {
    /// Spawn the rate-limiter actor. Returns immediately; the actor runs as a
    /// background goroutine for the lifetime of the process.
    pub async fn spawn(path: &Path, limit_per_minute: usize) -> crate::error::Result<Self> {
        let pool = open_pool(path).await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS rate_timestamps (
                ts REAL NOT NULL
            );",
        )
        .execute(&pool)
        .await?;

        let (tx, rx) = mpsc::channel::<PermitRequest>(256);
        let pool = Arc::new(pool);
        tokio::spawn(actor(rx, pool, limit_per_minute));
        Ok(Self { tx })
    }

    /// Acquire a rate-limit permit. Blocks (async) until a slot is available.
    pub async fn acquire(&self) {
        let (reply_tx, reply_rx) = oneshot::channel();
        // If the actor is gone (process shutting down) just continue.
        if self.tx.send(reply_tx).await.is_ok() {
            let _ = reply_rx.await;
        }
    }
}

async fn actor(
    mut rx: mpsc::Receiver<PermitRequest>,
    pool: Arc<SqlitePool>,
    limit_per_minute: usize,
) {
    while let Some(reply) = rx.recv().await {
        loop {
            match check_and_record(&pool, limit_per_minute).await {
                Ok(Some(wait_ms)) => {
                    tokio::time::sleep(tokio::time::Duration::from_millis(wait_ms)).await;
                }
                Ok(None) => {
                    let _ = reply.send(());
                    break;
                }
                Err(e) => {
                    tracing::warn!("rate_limit_db_error: {e}");
                    let _ = reply.send(());
                    break;
                }
            }
        }
    }
}

/// Returns `None` if a permit was recorded, or `Some(wait_ms)` to sleep before retrying.
async fn check_and_record(pool: &SqlitePool, limit: usize) -> crate::error::Result<Option<u64>> {
    let mut tx = pool.begin().await?;

    let now: f64 = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
    let cutoff = now - 60.0;

    // Remove expired timestamps
    sqlx::query("DELETE FROM rate_timestamps WHERE ts <= ?")
        .bind(cutoff)
        .execute(&mut *tx)
        .await?;

    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM rate_timestamps")
        .fetch_one(&mut *tx)
        .await?;

    if (count as usize) < limit {
        sqlx::query("INSERT INTO rate_timestamps (ts) VALUES (?)")
            .bind(now)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Ok(None);
    }

    // Find oldest timestamp and compute wait
    let (oldest,): (f64,) = sqlx::query_as("SELECT MIN(ts) FROM rate_timestamps")
        .fetch_one(&mut *tx)
        .await?;
    let wait_secs = (oldest + 60.0) - now + 0.05;
    let wait_ms = (wait_secs * 1000.0).max(50.0) as u64;

    tx.rollback().await.ok();
    Ok(Some(wait_ms))
}

async fn open_pool(path: &Path) -> crate::error::Result<SqlitePool> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal);
    let pool = SqlitePoolOptions::new()
        .max_connections(1) // serialise writes through a single connection
        .connect_with(options)
        .await?;
    Ok(pool)
}
