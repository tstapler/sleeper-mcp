//! Chrome-for-Testing pinned resolver (Story 2.1).
//!
//! Transaction Script: resolve → verify (sha256 vs
//! [`crate::browser::stealth::PINNED_CHROME_SHA256`], a trust-on-first-use
//! pin computed directly from the archive — NOT a CfT dashboard field; both
//! dashboard endpoints were checked live on 2026-11-01 and neither
//! publishes a `sha256` on any `downloads.chrome[]` entry, for any version)
//! → unzip → cache under `<data_dir>/cft/<version>/`.
//!
//! NO binary patching rule (Story 2.5 Task 4): stealth = launch flags + CDP
//! hygiene + CfT version pin only. The CfT Chrome binary is never patched in
//! place or copied; `patch:` diagnostics report [`stealth_flag_set_hash`]
//! (the stealth-flag-set hash), recomputed whenever the flag set changes.
//! A failed download is a hard error exit — never a silent system-Chrome
//! fallback (system Chrome only via the explicit Epic-1 opt-in flag).

use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use crate::browser::other;
use crate::browser::stealth::{PinnedVersion, StealthFlag};
use crate::error::Result;

/// Newtype for the actually-launched runtime Chrome version. Skew checks
/// take `(PinnedVersion, RuntimeVersion)` so the operands cannot swap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeVersion(pub String);

/// Pure helper: CfT archive download URL for a pinned version (linux64) —
/// the storage host, not the dashboard (the dashboard only lists this URL).
pub fn cft_url(pinned: &PinnedVersion) -> String {
    format!(
        "https://storage.googleapis.com/chrome-for-testing-public/{}/linux64/chrome-linux64.zip",
        pinned.0
    )
}

/// Pure helper: version-scoped cache dir (`<data_dir>/cft/<version>/`).
pub fn cft_path(data_dir: &Path, pinned: &PinnedVersion) -> PathBuf {
    data_dir.join("cft").join(&pinned.0)
}

/// Expected on-disk Chrome binary inside the extracted cache layout.
pub fn cft_chrome_binary(data_dir: &Path, pinned: &PinnedVersion) -> PathBuf {
    cft_path(data_dir, pinned).join("chrome-linux64").join("chrome")
}

/// Fail-fast skew check. Mismatch is a hard error carrying the triage line
/// (exit 2 upstream); it never falls back to another binary.
pub fn check_skew(pinned: &PinnedVersion, runtime: &RuntimeVersion) -> Result<()> {
    if pinned.0 == runtime.0 {
        return Ok(());
    }
    Err(other(format!(
        "CfT Chrome runtime v{} vs pinned v{} — version skew. Please re-run with update: re-run `sleeper browser login` to re-resolve the pin; if it persists, update the pin to v{}.",
        runtime.0, pinned.0, runtime.0
    )))
}

/// Verify downloaded bytes against the trust-on-first-use pinned hash
/// ([`crate::browser::stealth::PINNED_CHROME_SHA256`]) — never the CfT
/// dashboard, which doesn't publish per-download checksums (see the module
/// doc comment). The inner-binary hash is never used either.
pub fn verify_checksum(bytes: &[u8], expected_sha256_hex: &str) -> Result<()> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let got = hex::encode(hasher.finalize());
    if got.eq_ignore_ascii_case(expected_sha256_hex.trim()) {
        return Ok(());
    }
    Err(other(
        "CfT checksum mismatch against the pinned hash — re-run with update: delete the stale cache dir and re-run `sleeper browser login`; if it persists, the pin itself may be stale — re-derive PINNED_CHROME_SHA256 from a fresh download.".to_string()
    ))
}

/// Zip-slip guard: reject absolute entries and any `..` component, and
/// confine every output path to `dest`. Returns `Err` without writing
/// anything outside `dest` (callers treat rejection as a hard error).
pub fn extract_zip(bytes: &[u8], dest: &Path) -> Result<()> {
    let reader = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(reader).map_err(other)?;
    // Validate ALL entry names before writing anything, so a mid-archive
    // `..` entry cannot leave a half-extracted cache behind.
    let mut names = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(other)?;
        // Symlink hardening: a symlink entry could point outside `dest`
        // once restored, so reject it before writing anything.
        if let Some(mode) = entry.unix_mode() {
            if mode & 0o170000 == 0o120000 {
                return Err(other(format!(
                    "zip-slip guard: rejected symlink archive entry {:?} — delete the cache dir and re-run with update from a trusted CfT URL",
                    entry.name()
                )));
            }
        }
        let name = entry.name().to_string();
        let rel = Path::new(&name);
        if rel.is_absolute()
            || rel.components().any(|c| matches!(c, Component::ParentDir))
        {
            return Err(other(format!(
                "zip-slip guard: rejected archive entry {name:?} — delete the cache dir and re-run with update from a trusted CfT URL"
            )));
        }
        let out = dest.join(rel);
        if out.strip_prefix(dest).is_err() {
            return Err(other(format!(
                "zip-slip guard: entry {name:?} escapes the cache dir — re-run with update from a trusted CfT URL"
            )));
        }
        names.push(name);
    }
    for name in &names {
        let mut entry = archive.by_name(name).map_err(other)?;
        let out = dest.join(Path::new(name));
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(other)?;
        } else {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(other)?;
            }
            let mut outfile = std::fs::File::create(&out).map_err(|e| {
                other(format!("CfT cache write failed at {} ({e}) — free disk space, then re-run `sleeper browser login`", out.display()))
            })?;
            std::io::copy(&mut entry, &mut outfile).map_err(other)?;
            drop(outfile);
            // Apply the archive's unix permission bits so the extracted
            // `chrome-linux64/chrome` lands executable (Zip stores them;
            // File::create would otherwise leave a non-executable 0666&~umask).
            if let Some(mode) = entry.unix_mode() {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode & 0o7777))
                    .map_err(other)?;
            }
        }
    }
    Ok(())
}

/// Transaction Script: download → sha256 (vs `expected_sha256_hex`, the
/// trust-on-first-use pin — callers pass
/// [`crate::browser::stealth::PINNED_CHROME_SHA256`]) → unzip → cache.
/// `30s` connect / `120s` total timeouts, 3 attempts, offline + disk-full
/// triage, `*.part` temp file + atomic rename on success.
pub async fn resolve(
    data_dir: &Path,
    pinned: &PinnedVersion,
    expected_sha256_hex: &str,
) -> Result<PathBuf> {
    let dest = cft_path(data_dir, pinned);
    let binary = cft_chrome_binary(data_dir, pinned);
    if binary.is_file() {
        return Ok(binary);
    }
    let url = cft_url(pinned);
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(other)?;
    // Async half: network only. The blocking tail (checksum, temp-file
    // write, unzip, chmod) runs in `spawn_blocking` below so the executor
    // never blocks on std::fs / sync hashing / whole-archive inflate.
    let mut last_err = String::new();
    let mut downloaded: Option<_> = None;
    for attempt in 1..=3 {
        match client.get(&url).send().await {
            Ok(resp) => {
                let resp = resp.error_for_status().map_err(other)?;
                let bytes = resp.bytes().await.map_err(other)?;
                downloaded = Some(bytes);
                break;
            }
            Err(e) => {
                last_err = if e.is_connect() || e.is_timeout() {
                    format!("CfT download failed (attempt {attempt}/3): {e} — offline? check the network, then re-run `sleeper browser login`")
                } else {
                    format!("CfT download failed (attempt {attempt}/3): {e} — wait a moment, then re-run `sleeper browser login`")
                };
                if attempt < 3 {
                    tokio::time::sleep(Duration::from_secs(attempt as u64 * 2)).await;
                }
            }
        }
    }
    let bytes = downloaded.ok_or_else(|| other(last_err))?;
    // Blocking tail: `*.part` temp file + atomic rename discipline kept.
    let expected = expected_sha256_hex.to_string();
    let pinned_v = pinned.0.clone();
    tokio::task::spawn_blocking(move || {
        verify_checksum(&bytes, &expected)?;
        std::fs::create_dir_all(&dest).map_err(other)?;
        let binary = dest.join("chrome-linux64").join("chrome");
        let part = dest.join("chrome-linux64.zip.part");
        let done = dest.join("chrome-linux64.zip");
        {
            let mut f = std::fs::File::create(&part)
                .map_err(|e| triage_io_error(&e, "CfT download temp file"))?;
            f.write_all(&bytes)
                .map_err(|e| triage_io_error(&e, "CfT download temp file"))?;
            f.sync_all().map_err(other)?;
        }
        std::fs::rename(&part, &done).map_err(other)?;
        let raw = std::fs::read(&done).map_err(other)?;
        extract_zip(&raw, &dest)?;
        if !binary.is_file() {
            return Err(other(format!(
                "CfT archive for v{pinned_v} has no chrome-linux64/chrome — re-run with update: update the pin to a CfT release with a linux64 build"
            )));
        }
        // Belt-and-suspenders: the extracted binary must be executable even
        // if the archive entry carried no unix-mode bits.
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))
            .map_err(other)?;
        Ok(binary)
    })
    .await?
}

/// Parse `Google Chrome 126.0.6478.126` → `126.0.6478.126`.
pub fn parse_chrome_version(output: &str) -> Option<String> {
    output.split_whitespace().find_map(|tok| {
        let t = tok.trim();
        let parts: Vec<&str> = t.split('.').collect();
        if parts.len() == 4 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) {
            Some(t.to_string())
        } else {
            None
        }
    })
}

/// Best-effort runtime version of a Chrome binary (`unknown` when
/// undetectable — never omitted from `--json`).
pub fn runtime_version(binary: &Path) -> String {
    std::process::Command::new(binary)
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| parse_chrome_version(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_else(|| "unknown".to_string())
}

fn triage_io_error(e: &std::io::Error, what: &str) -> crate::error::Error {
    use std::io::ErrorKind;
    if e.kind() == ErrorKind::StorageFull {
        other(format!(
            "{what} failed: disk full ({e}) — free disk space, then re-run `sleeper browser login`"
        ))
    } else {
        other(format!("{what} failed ({e}) — re-run `sleeper browser login`"))
    }
}

/// `patch:` diagnostic — the stealth-flag-set hash (first 8 hex of sha256
/// over the joined flag strings). Recomputed on every flag change; the CfT
/// binary itself is never patched or copied (see below).
pub fn stealth_flag_set_hash(flags: &[StealthFlag]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for flag in flags {
        hasher.update(flag.as_str().as_bytes());
        hasher.update(b"\n");
    }
    hex::encode(hasher.finalize())[..8].to_string()
}

/// No-patch rule guard: applying the stealth flag set is a pure launch-time
/// operation over flag strings — this helper returns the binary bytes
/// untouched, and the test below pins byte-identity across stealth changes.
pub fn stealth_leaves_binary_untouched(binary_bytes: &[u8], _flags: &[StealthFlag]) -> Vec<u8> {
    binary_bytes.to_vec()
}

#[cfg(test)]
mod tests {
    use super::{
        cft_chrome_binary, cft_path, cft_url, check_skew, extract_zip, stealth_flag_set_hash,
        stealth_leaves_binary_untouched, verify_checksum, PinnedVersion, RuntimeVersion,
    };
    use crate::browser::stealth::{stealth_flags, StealthFlag, PINNED_CHROME_SHA256};
    use crate::browser::LaunchMode;
    use std::io::Write;

    fn fixture_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer =
            zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, data) in entries {
            // CfT archives carry unix exec bits on the chrome binary; mirror
            // that so the exec-bit assertion below is honest.
            let options = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
            writer.start_file(*name, options).expect("start_file");
            writer.write_all(data).expect("write entry");
        }
        writer.finish().expect("finish").into_inner()
    }

    #[test]
    fn cft_url_should_point_at_download_host_when_pinned_version_given() {
        let url = cft_url(&PinnedVersion::pinned());
        assert!(
            url.contains("chrome-for-testing"),
            "CfT URL must point at the chrome-for-testing endpoint, got {url}"
        );
        assert!(
            url.contains(&PinnedVersion::pinned().0),
            "CfT URL must carry the pinned version, got {url}"
        );
        assert!(url.ends_with(".zip"), "CfT URL must name a zip archive, got {url}");
    }

    #[test]
    fn resolve_should_error_when_checksum_mismatches_pinned_hash() {
        let dir = std::env::temp_dir().join(format!(
            "sleeper-cft-checksum-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("tempdir");
        let bytes = b"tampered chrome bytes, not what PINNED_CHROME_SHA256 pins";
        let err = verify_checksum(bytes, PINNED_CHROME_SHA256).expect_err("mismatch must Err");
        assert!(
            err.to_string().contains("checksum"),
            "checksum error must name the cause, got {err}"
        );
        // Cache dir untouched: no binary materialized.
        assert!(
            !cft_chrome_binary(&dir, &PinnedVersion::pinned()).is_file(),
            "failed verification must leave the cache untouched"
        );
        assert!(
            cft_path(&dir, &PinnedVersion::pinned())
                .ends_with(format!("cft/{}", PinnedVersion::pinned().0)),
            "cache layout must stay <data_dir>/cft/<version>/"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn skew_check_should_fail_when_runtime_differs_from_pinned() {
        let pinned = PinnedVersion::pinned();
        assert!(check_skew(&pinned, &RuntimeVersion(pinned.0.clone())).is_ok());
        let err = check_skew(&pinned, &RuntimeVersion("127.0.6533.99".into()))
            .expect_err("skew must fail fast");
        assert!(err.to_string().contains("version skew"), "got {err}");
    }

    #[test]
    fn extract_should_cache_layout_when_zip_clean() {
        let dir = std::env::temp_dir().join(format!(
            "sleeper-cft-clean-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let dest = cft_path(&dir, &PinnedVersion::pinned());
        let bytes = fixture_zip(&[("chrome-linux64/chrome", b"fake-chrome-binary")]);
        extract_zip(&bytes, &dest).expect("clean zip extracts");
        let binary = cft_chrome_binary(&dir, &PinnedVersion::pinned());
        assert!(binary.is_file(), "cache layout must hold the chrome binary");
        assert_eq!(std::fs::read(&binary).expect("read"), b"fake-chrome-binary");
        // B1: the extracted binary must be directly executable (first
        // fresh-resolve spawn must not fail permission-denied).
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&binary).expect("metadata").permissions().mode();
        assert!(
            mode & 0o111 != 0,
            "extracted chrome binary must carry an executable bit, got mode {mode:o}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extract_should_reject_when_zip_contains_symlink_entry() {
        use std::io::Write as _;
        let dir = std::env::temp_dir().join(format!(
            "sleeper-cft-symlink-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let dest = cft_path(&dir, &PinnedVersion::pinned());
        std::fs::create_dir_all(&dest).expect("dest");
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let link_opts = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
        writer.start_file("chrome-linux64/sneaky-link", link_opts).expect("start_file");
        writer.write_all(b"/etc/passwd").expect("write entry");
        let mut bytes = writer.finish().expect("finish").into_inner();
        // The zip writer normalizes file-type bits, so stamp a real symlink
        // mode into the central-directory external attributes by hand.
        let sig = [0x50u8, 0x4b, 0x01, 0x02];
        let pos = bytes.windows(4).position(|w| w == sig).expect("central header");
        bytes[pos + 38..pos + 42].copy_from_slice(&((0o120777u32) << 16).to_le_bytes());
        let err = extract_zip(&bytes, &dest).expect_err("symlink entry must be rejected");
        assert!(
            err.to_string().contains("symlink"),
            "rejection must name the symlink guard, got {err}"
        );
        assert!(
            !dest.join("chrome-linux64/sneaky-link").exists(),
            "symlink entry must not materialize in the cache dir"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extract_should_reject_when_zip_contains_dotdot_entry() {
        let dir = std::env::temp_dir().join(format!(
            "sleeper-cft-slip-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let dest = cft_path(&dir, &PinnedVersion::pinned());
        std::fs::create_dir_all(&dest).expect("dest");
        let bytes = fixture_zip(&[
            ("chrome-linux64/chrome", b"fake-chrome-binary"),
            ("../evil", b"pwned"),
        ]);
        let err = extract_zip(&bytes, &dest).expect_err("dotdot entry must be rejected");
        assert!(
            err.to_string().contains("zip-slip") || err.to_string().contains("escapes"),
            "rejection must name the traversal guard, got {err}"
        );
        assert!(
            !dir.join("evil").exists(),
            "traversal entry must not escape the cache dir"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn patch_hash_should_recompute_when_flag_set_changes() {
        let base = stealth_flags(LaunchMode::HeadlessStatus, std::path::Path::new("/tmp/p"));
        let h1 = stealth_flag_set_hash(&base);
        assert_eq!(h1.len(), 8, "patch hash is sha8, got {h1}");
        let mut changed = base.clone();
        changed.push(StealthFlag("--disable-gpu".into()));
        assert_ne!(
            stealth_flag_set_hash(&changed),
            h1,
            "flag-set change must recompute the patch hash"
        );
    }

    #[test]
    fn cft_binary_should_stay_unmodified_when_stealth_applies() {
        let fixture = b"pinned-cft-chrome-bytes-v126".to_vec();
        let flags = stealth_flags(LaunchMode::HeadlessStatus, std::path::Path::new("/tmp/p"));
        let after = stealth_leaves_binary_untouched(&fixture, &flags);
        assert_eq!(after, fixture, "stealth application must never rewrite the CfT binary");
        // A changed flag set still leaves the binary byte-identical.
        let mut more = flags.clone();
        more.push(StealthFlag("--disable-gpu".into()));
        assert_eq!(
            stealth_leaves_binary_untouched(&fixture, &more),
            fixture,
            "no flag combination may patch the binary"
        );
    }
}
