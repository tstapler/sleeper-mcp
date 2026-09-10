//! CfT resolver integration tests — OPERATOR-GATED, `#[ignore]` by default.
//! Require network (CfT download) and/or a live Chrome binary.
//! Do NOT run in CI: `cargo test -- --ignored <name>` only by the operator.

use sleeper::browser::cft::{cft_chrome_binary, cft_path, resolve};
use sleeper::browser::{LaunchConfig, PinnedVersion, StealthLauncher};
use std::path::PathBuf;

fn temp_data_dir(suffix: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sleeper-cft-{suffix}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp data dir");
    dir
}

/// Live download: empty `<data_dir>/cft/` → verified `<version>/chrome`.
/// Needs network + the dashboard sha256 for the pinned version; the
/// operator pastes it via `SLEEPER_CFT_SHA256` (dashboard source of truth).
#[tokio::test]
#[ignore]
async fn resolve_should_cache_verified_chrome_when_cache_empty() {
    let data_dir = temp_data_dir("live");
    let pinned = PinnedVersion::pinned();
    assert!(
        !cft_path(&data_dir, &pinned).exists(),
        "precondition: cache starts empty"
    );
    let expected = std::env::var("SLEEPER_CFT_SHA256")
        .expect("operator gates this test: set SLEEPER_CFT_SHA256 from the CfT dashboard downloads.chrome[].sha256 field");
    let binary = resolve(&data_dir, &pinned, &expected).await.expect("resolve");
    assert_eq!(binary, cft_chrome_binary(&data_dir, &pinned));
    assert!(binary.is_file(), "verified chrome must exist after resolve");
    let _ = std::fs::remove_dir_all(&data_dir);
}

/// No-silent-fallback: a failed CfT download is an error exit — system
/// Chrome is never spawned implicitly.
#[tokio::test]
#[ignore]
async fn status_should_never_use_system_chrome_when_download_fails() {
    let data_dir = temp_data_dir("nofallback");
    let bogus = PinnedVersion("0.0.0.0-bogus".to_string());
    let err = resolve(&data_dir, &bogus, &"0".repeat(64))
        .await
        .expect_err("bogus pin must fail");
    assert!(!err.to_string().is_empty());
    // And the launcher without the explicit opt-in flag refuses rather
    // than falling back to system Chrome.
    let config = LaunchConfig::headless_status(data_dir.join("chrome-profile"));
    let err = StealthLauncher::resolve_binary(&config).expect_err("no silent fallback");
    assert!(
        !err.to_string().contains("google-chrome"),
        "must not resolve system Chrome implicitly, got {err}"
    );
    let _ = std::fs::remove_dir_all(&data_dir);
}
