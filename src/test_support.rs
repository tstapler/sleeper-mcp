//! Fix 5 (Phase 6 review of the waiver-intelligence feature): shared
//! test-only fixtures that `bid_projection.rs`, `transactions.rs`,
//! `edit_cancel.rs`, and `db.rs`'s own `#[cfg(test)]` modules each
//! duplicated near-verbatim — the nanos-since-epoch + pid temp-path
//! uniqueness boilerplate, plus the `Config`/`SleeperClient`/`StatsDb`
//! construction every `AnalysisService`-level test needs.
use std::path::PathBuf;

use crate::analysis::AnalysisService;
use crate::client::SleeperClient;
use crate::config::Config;
use crate::db::StatsDb;

/// A scratch path under the OS temp dir, unique per test run — nanoseconds
/// since the epoch plus the process id, the same uniqueness scheme every
/// duplicated helper this consolidates used independently. `prefix`
/// identifies the caller/test; no extension is appended here.
pub(crate) fn unique_temp_path(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}_{}_{nanos}", std::process::id()))
}

/// `StatsDb::open` takes a file path (no in-memory mode) — each test gets
/// its own scratch file under the OS temp dir rather than sharing state.
pub(crate) async fn test_db(name: &str) -> StatsDb {
    let path = unique_temp_path(&format!("sleeper_test_{name}")).with_extension("db");
    StatsDb::open(&path).await.expect("open test db")
}

/// A scratch-dir `Config`, a `SleeperClient` built but never asked to make a
/// network call, and a fresh `StatsDb` file. Callers that only need the
/// `AnalysisService` half can destructure with `let (_, service) = ...`.
pub(crate) async fn test_analysis_service(name: &str) -> (Config, AnalysisService) {
    let data_dir = unique_temp_path(&format!("sleeper_{name}"));
    let config = Config { data_dir, ..Config::default() };
    let client =
        SleeperClient::new(&config).await.expect("build client (no network at construction)");
    let db = StatsDb::open(&config.stats_db_path()).await.expect("open test db");
    (config.clone(), AnalysisService::new(client, db))
}
