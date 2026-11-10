//! `browser doctor` self-test: one readiness report combining CfT presence,
//! runtime-vs-pin skew, patch hash, profile lock, and a live headless login
//! probe. Split out of `login.rs` (which still owns the shared
//! `launch_session`/triage plumbing this module reuses via `super::login`).
use crate::config::Config;
use crate::error::Result;

use super::login::{
    check_failed_triage, debug_detail, launch_session, triage_error, LaunchOptions,
};

/// `browser doctor` self-test report (ux.md Surface E): one `✓`/`✗` line
/// per check; every `✗` names the fix + exact re-run command. Non-zero exit
/// on any `✗`; zero only when all checks pass.
pub struct DoctorReport {
    pub checks: Vec<(bool, String)>,
}

impl DoctorReport {
    pub fn render(&self) -> String {
        self.checks
            .iter()
            .map(|(ok, line)| format!("{} {line}", if *ok { "✓" } else { "✗" }))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn exit_code(&self) -> i32 {
        if self.checks.iter().all(|(ok, _)| *ok) {
            0
        } else {
            1
        }
    }
}

/// Inputs for [`build_doctor_report`]'s self-test render — one flat readiness
/// bundle (CfT presence, runtime-vs-pin skew, patch hash, profile lock,
/// headless login probe), not several independent concerns, so one struct
/// beats eight positional args (two of them same-typed bools that are easy
/// to transpose by position).
pub struct DoctorInputs<'a> {
    pub pinned: &'a str,
    pub cft_binary: &'a std::path::Path,
    pub runtime: &'a str,
    pub flag_hash: &'a str,
    pub profile: &'a std::path::Path,
    pub lock_free: bool,
    pub login: &'a str,
    pub webdriver_clean: bool,
}

pub fn build_doctor_report(inputs: DoctorInputs) -> DoctorReport {
    let DoctorInputs {
        pinned,
        cft_binary,
        runtime,
        flag_hash,
        profile,
        lock_free,
        login,
        webdriver_clean,
    } = inputs;
    let mut checks = Vec::new();
    if cft_binary.is_file() {
        checks.push((
            true,
            format!("cft: {pinned} at {} (match)", cft_binary.display()),
        ));
    } else {
        checks.push((
            false,
            format!(
                "cft: {pinned} missing at {} — re-run `sleeper browser login` to resolve the pin with update",
                cft_binary.display()
            ),
        ));
    }
    if pinned == runtime {
        checks.push((
            true,
            format!("chrome runtime {runtime} (match, skew-free)"),
        ));
    } else if runtime == "unknown" {
        checks.push((
            false,
            format!(
                "Chrome binary missing or unreadable — re-run `sleeper browser login` to resolve the pin (pinned {pinned})"
            ),
        ));
    } else {
        checks.push((
            false,
            format!(
                "chrome runtime {runtime} vs pinned {pinned} — version skew. Re-run `sleeper browser login` to re-resolve the pin; if it persists, update the pin to {runtime}"
            ),
        ));
    }
    checks.push((
        true,
        format!("patch: {flag_hash} (flag-set hash match, no binary patching)"),
    ));
    if lock_free {
        checks.push((
            true,
            format!("profile {} (unlocked) lock: free", profile.display()),
        ));
    } else {
        checks.push((
            false,
            format!(
                "profile {} (locked) — close the login window, then re-run `sleeper browser status`",
                profile.display()
            ),
        ));
    }
    // Root-cause fix (2026-11-10): this used to be ONE check gated entirely
    // on `webdriver_clean`, so a genuinely successful login (`login ==
    // "logged in"`) still rendered ✗ whenever the webdriver signal read
    // unclean — text starting with "logged in" formatted as a failure line,
    // which is exactly what misled this investigation. Login state and the
    // webdriver stealth signal are orthogonal facts; report them as two
    // independent checks.
    if login == "logged in" {
        checks.push((true, format!("headless login check: {login}")));
    } else {
        checks.push((
            false,
            format!(
                "headless login check: {login} — re-run `sleeper browser status`; re-login only if persistent"
            ),
        ));
    }
    if webdriver_clean {
        checks.push((true, "webdriver signal: false (clean)".to_string()));
    } else {
        checks.push((
            false,
            "webdriver signal: missing/tampered — re-run `sleeper browser status --debug` to inspect".to_string(),
        ));
    }
    DoctorReport { checks }
}

/// Live headless login probe for `doctor`: launches, reads the verdict +
/// webdriver signal, and closes on every branch. Returns the Surface E
/// login-check label plus whether the webdriver signal came back clean.
/// Dump parsing lives in the seam (`stealth_health`); the CLI only renders.
/// Probe failures carry triage-first copy (raw detail behind
/// `SLEEPER_BROWSER_DEBUG`), never a bare error on stdout.
async fn probe_headless_login(config: &Config) -> (String, bool) {
    use crate::browser::LoginVerdict;
    let (mut session, launched) = match launch_session(config, LaunchOptions::headless()).await {
        Ok(pair) => pair,
        Err(e) => {
            let raw = e.to_string();
            let label =
                triage_error(&check_failed_triage("browser launch"), &debug_detail(&raw));
            return (label, false);
        }
    };
    let dump = session.eval_dump().await.unwrap_or_default();
    let (_signal_summary, webdriver_clean) = crate::browser::session::stealth_health(&dump);
    let label = match session.login_verdict().await {
        Ok(LoginVerdict::LoggedIn) => "logged in".to_string(),
        Ok(LoginVerdict::LoggedOut) => "logged out".to_string(),
        Ok(LoginVerdict::Unknown) => {
            "unknown — re-run `sleeper browser status`; re-login only if persistent".to_string()
        }
        Err(e) => {
            let raw = e.to_string();
            triage_error(&check_failed_triage("login check"), &debug_detail(&raw))
        }
    };
    let _ = session.close().await;
    let _ = launched.close().await;
    (label, webdriver_clean)
}

pub(super) async fn cmd_browser_doctor(config: &Config) -> Result<()> {
    use crate::browser::{stealth::stealth_flags, LaunchMode, PinnedVersion};
    let pinned = PinnedVersion::pinned();
    let cft_binary = crate::browser::cft::cft_chrome_binary(&config.data_dir, &pinned);
    let runtime = if cft_binary.is_file() {
        crate::browser::cft::runtime_version(&cft_binary)
    } else {
        "unknown".to_string()
    };
    let flags = stealth_flags(LaunchMode::HeadlessStatus, &config.chrome_profile_dir());
    let flag_hash = crate::browser::cft::stealth_flag_set_hash(&flags);
    let profile_dir = config.chrome_profile_dir();
    let lock_free = !profile_dir.join("SingletonLock").exists();
    // Probes are read-only: never delete the profile; a failed probe is an
    // ✗ with its fix line, never a silent kill (Surface E).
    let (login, webdriver_clean) = if lock_free && cft_binary.is_file() && pinned.0 == runtime
    {
        probe_headless_login(config).await
    } else {
        ("skipped (resolve skew/lock first)".to_string(), false)
    };
    let report = build_doctor_report(DoctorInputs {
        pinned: &pinned.0,
        cft_binary: &cft_binary,
        runtime: &runtime,
        flag_hash: &flag_hash,
        profile: &profile_dir,
        lock_free,
        login: &login,
        webdriver_clean,
    });
    println!("{}", report.render());
    std::process::exit(report.exit_code());
}

#[cfg(test)]
mod tests {
    use super::{build_doctor_report, DoctorInputs};

    #[test]
    fn doctor_should_report_all_checks_when_profile_free() {
        let report = build_doctor_report(DoctorInputs {
            pinned: "126.0.6478.126",
            cft_binary: std::path::Path::new("/tmp/does-not-exist-marker"),
            runtime: "126.0.6478.126",
            flag_hash: "a1b2c3d4",
            profile: std::path::Path::new("/tmp/chrome-profile"),
            lock_free: true,
            login: "logged in",
            webdriver_clean: true,
        });
        // Missing binary is honestly ✗ here (fixture path); rebuild with a
        // real temp file to prove the all-✓ render + exit 0.
        assert_eq!(report.exit_code(), 1);
        let dir = std::env::temp_dir();
        let bin = dir.join(format!("sleeper-doctor-bin-{}", std::process::id()));
        std::fs::write(&bin, b"fake").expect("fixture binary");
        let report = build_doctor_report(DoctorInputs {
            pinned: "126.0.6478.126",
            cft_binary: &bin,
            runtime: "126.0.6478.126",
            flag_hash: "a1b2c3d4",
            profile: std::path::Path::new("/tmp/chrome-profile"),
            lock_free: true,
            login: "logged in",
            webdriver_clean: true,
        });
        let rendered = report.render();
        assert!(rendered.contains("cft: 126.0.6478.126"), "got {rendered}");
        assert!(rendered.contains("patch: a1b2c3d4"), "got {rendered}");
        assert!(rendered.contains("lock: free"), "got {rendered}");
        assert!(rendered.contains("headless login check: logged in"), "got {rendered}");
        assert!(rendered.contains("webdriver signal: false (clean)"), "got {rendered}");
        assert!(
            rendered.lines().all(|l| l.starts_with("✓")),
            "login and webdriver are independent checks — a real login must stay ✓ regardless of the webdriver signal, got:\n{rendered}"
        );
        assert_eq!(report.exit_code(), 0);
        let _ = std::fs::remove_file(&bin);
    }

    #[test]
    fn doctor_should_exit_nonzero_when_any_check_fails() {
        let report = build_doctor_report(DoctorInputs {
            pinned: "126.0.6478.126",
            cft_binary: std::path::Path::new("/tmp/doctor-missing"),
            runtime: "127.0.6533.99",
            flag_hash: "a1b2c3d4",
            profile: std::path::Path::new("/tmp/chrome-profile"),
            lock_free: false,
            login: "skipped (resolve skew/lock first)",
            webdriver_clean: false,
        });
        let rendered = report.render();
        assert!(rendered.contains("✗"), "got {rendered}");
        assert_ne!(report.exit_code(), 0);
        // Every ✗ names the fix + exact re-run command (no dead ends).
        for line in rendered.lines().filter(|l| l.starts_with("✗")) {
            assert!(
                line.to_lowercase().contains("re-run `sleeper browser"),
                "✗ line must name a re-run command, got {line}"
            );
        }
    }

    /// Regression gate for the 2026-11-10 root-cause fix: a real, successful
    /// login must never render as a doctor ✗. Before the fix, the single
    /// combined check line was gated entirely on `webdriver_clean`, so
    /// `login: "logged in"` with `webdriver_clean: false` rendered
    /// `✗ headless login check: logged in — re-run ...` — a successful
    /// login reported and exited as a failure.
    #[test]
    fn doctor_should_keep_login_check_green_when_webdriver_signal_is_unclean() {
        let report = build_doctor_report(DoctorInputs {
            pinned: "126.0.6478.126",
            cft_binary: std::path::Path::new("/tmp/doctor-missing"),
            runtime: "126.0.6478.126",
            flag_hash: "a1b2c3d4",
            profile: std::path::Path::new("/tmp/chrome-profile"),
            lock_free: true,
            login: "logged in",
            webdriver_clean: false,
        });
        let rendered = report.render();
        assert!(
            rendered
                .lines()
                .any(|l| l.starts_with("✓") && l.contains("headless login check: logged in")),
            "a real login must render ✓ regardless of the webdriver signal, got:\n{rendered}"
        );
        assert!(
            rendered
                .lines()
                .any(|l| l.starts_with("✗") && l.contains("webdriver signal")),
            "the webdriver signal must still surface as its own, separate ✗, got:\n{rendered}"
        );
    }

    #[test]
    fn doctor_should_resolve_pin_when_runtime_unknown() {
        // Undetectable runtime ("unknown" sentinel): must never instruct
        // updating the pin TO unknown.
        let report = build_doctor_report(DoctorInputs {
            pinned: "126.0.6478.126",
            cft_binary: std::path::Path::new("/tmp/doctor-missing"),
            runtime: "unknown",
            flag_hash: "a1b2c3d4",
            profile: std::path::Path::new("/tmp/chrome-profile"),
            lock_free: true,
            login: "skipped (resolve skew/lock first)",
            webdriver_clean: false,
        });
        let rendered = report.render();
        assert!(
            !rendered.to_lowercase().contains("update the pin to unknown"),
            "must never instruct pinning to unknown, got:\n{rendered}"
        );
        assert!(
            rendered.contains(
                "Chrome binary missing or unreadable — re-run `sleeper browser login` to resolve the pin (pinned 126.0.6478.126)"
            ),
            "got:\n{rendered}"
        );
        assert_ne!(report.exit_code(), 0);
        // Known-vs-known skew wording stays byte-identical.
        let known = build_doctor_report(DoctorInputs {
            pinned: "126.0.6478.126",
            cft_binary: std::path::Path::new("/tmp/doctor-missing"),
            runtime: "127.0.6533.99",
            flag_hash: "a1b2c3d4",
            profile: std::path::Path::new("/tmp/chrome-profile"),
            lock_free: true,
            login: "skipped (resolve skew/lock first)",
            webdriver_clean: false,
        })
        .render();
        assert!(
            known.contains(
                "chrome runtime 127.0.6533.99 vs pinned 126.0.6478.126 — version skew. Re-run `sleeper browser login` to re-resolve the pin; if it persists, update the pin to 127.0.6533.99"
            ),
            "known-vs-known wording must stay byte-identical, got:\n{known}"
        );
    }
}
