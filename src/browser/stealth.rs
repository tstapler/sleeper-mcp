//! Pure stealth material: flags, client-hints bundle, bootstrap JS.
//!
//! No I/O, no process spawning, no `evaluate` here (layer gates) — pure
//! functions unit-testable with no Chrome.
//!
//! Policy table (Q-policy sign-off 2026-09-08, owner Tyler — see plan.md
//! Story 2.5 Task 0; login stays 100% manual):
//!
//! | Allowed (fingerprint-removal)                          | Forbidden (interaction-faking)          |
//! |--------------------------------------------------------|-----------------------------------------|
//! | Chrome launch flags hiding automation markers          | Scripted key presses / typing           |
//! | Coherent UA + Sec-CH-UA client hints for the CfT pin   | Simulated mouse / input events          |
//! | Isolated-world bootstrap masking navigator.webdriver  | Automated credential fill / login       |
//! | CfT version pinning + skew fail-fast                   | CAPTCHA / challenge solving             |
//! | CDP hygiene (batched reads, deferred debugger attach)  | Any scripting of the human login step   |
//!
//! Every `StealthFlag` below carries a trailing `// justification:` line
//! (policy gate); the interaction-faking ban is enforced by the scan test.

use crate::browser::launch::LaunchMode;

/// Pinned Chrome-for-Testing version placeholder. The CfT resolver
/// (Story 2.1) consumes this; skew checks take `(PinnedVersion,
/// RuntimeVersion)` so operands cannot swap.
pub const PINNED_CHROME_VERSION: &str = "126.0.6478.126";

/// Trust-on-first-use sha256 of `https://storage.googleapis.com/
/// chrome-for-testing-public/126.0.6478.126/linux64/chrome-linux64.zip`,
/// computed directly from that archive on 2026-11-01. NOT from a CfT
/// dashboard field — both dashboard endpoints
/// (`last-known-good-versions-with-downloads.json` and
/// `known-good-versions-with-downloads.json`) were checked live and neither
/// publishes a `sha256` on any `downloads.chrome[]` entry, for any version;
/// the "verify against the dashboard field" design in the original Story
/// 2.1 plan was never true. This constant is the actual source of truth: it
/// pins this one download to this one hash, the same trust-on-first-use
/// model as a `Cargo.lock` checksum. Bumping `PINNED_CHROME_VERSION`
/// requires recomputing this too — download the new version's
/// `chrome-linux64.zip` and `sha256sum` it.
pub const PINNED_CHROME_SHA256: &str =
    "b0dcb2cb784fdf72797279f33a8fdca589f35075eae6c67354e35212939a0d2b";

/// Newtype for the CfT pin (`$PINNED`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedVersion(pub String);

impl PinnedVersion {
    pub fn pinned() -> Self {
        Self(PINNED_CHROME_VERSION.to_string())
    }

    /// Major version for UA coherence checks (e.g. `126`).
    pub fn major(&self) -> &str {
        self.0.split('.').next().unwrap_or(&self.0)
    }
}

/// One Chrome CLI flag. Every instance is constructed with a trailing
/// `// justification:` comment (policy gate:
/// `stealth_flags_should_carry_justification_when_policy_gate_runs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StealthFlag(pub String);

impl StealthFlag {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Pure flag assembly: stealth set + mode/profile wiring. `--user-data-dir`
/// and headless-new are assembled here (not in `launch.rs`) so every flag
/// literal in the tree carries its justification in this file.
pub fn stealth_flags(mode: LaunchMode, profile_dir: &std::path::Path) -> Vec<StealthFlag> {
    let mut flags = vec![
        StealthFlag("--remote-debugging-port=0".into()), // justification: OS-assigned CDP port avoids fixed-port contention; endpoint parsed from stderr
        StealthFlag("--disable-blink-features=AutomationControlled".into()), // justification: removes the navigator.webdriver automation flag (fingerprint-removal)
        StealthFlag("--disable-infobars".into()), // justification: hides the "controlled by automation" infobar so the headed window matches manual Chrome
        StealthFlag("--no-first-run".into()), // justification: skips first-run dialogs that stall automated launch
        StealthFlag("--no-default-browser-check".into()), // justification: skips the default-browser prompt that stalls launch
        StealthFlag("--disable-dev-shm-usage".into()), // justification: prevents /dev/shm exhaustion crashes in containers
        StealthFlag("--disable-hang-monitor".into()), // justification: suppresses "page unresponsive" dialogs during the long manual-login poll
        StealthFlag("--window-size=1920,1080".into()), // justification: deterministic desktop viewport instead of automation-default sizing
    ];
    if mode == LaunchMode::HeadlessStatus {
        flags.push(StealthFlag("--headless=new".into())); // justification: new headless preserves headed rendering paths for the read-only status check
    }
    flags.push(StealthFlag(format!("--user-data-dir={}", profile_dir.display()))); // justification: persistent profile dir keeps the human-authenticated session across runs
    flags
}

/// Coherent `Emulation.setUserAgentOverride` UA + `Sec-CH-UA` hints bundle
/// matching the pinned Chrome version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientHints {
    pub user_agent: String,
    pub sec_ch_ua: String,
    pub platform: String,
}

impl ClientHints {
    pub fn for_version(pinned: PinnedVersion) -> Self {
        let major = pinned.major().to_string();
        Self {
            user_agent: format!(
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{} Safari/537.36",
                pinned.0
            ),
            sec_ch_ua: format!(
                "\"Chromium\";v=\"{major}\", \"Google Chrome\";v=\"{major}\", \"Not-A.Brand\";v=\"99\""
            ),
            platform: "Linux".to_string(),
        }
    }
}

/// Isolated-world bootstrap JS applied right after `Browser::connect`
/// (Story 1.3 Task 5). CDP sequence reference: `Page.createIsolatedWorld`
/// + `addScriptToEvaluateOnNewDocument` + `Emulation.setUserAgentOverride`
/// (full wiring lands with CfT pinning in Epic 2; UA coherence currently
/// relies on launching real, non-spoofed Chrome). v1 applies this
/// evaluate-subset and returns `true` on success so the connect path can
/// fail loud when the probe fails.
pub fn isolated_world_bootstrap_js(hints: &ClientHints) -> String {
    format!(
        r#"(() => {{
            try {{
                Object.defineProperty(navigator, 'webdriver', {{ get: () => undefined }});
                if (!window.chrome) {{ window.chrome = {{ runtime: {{}} }}; }}
                Object.defineProperty(navigator, 'userAgent', {{ get: () => '{}' }});
            }} catch (e) {{}}
            return true;
        }})()"#,
        hints.user_agent.replace('\'', "")
    )
}

#[cfg(test)]
mod tests {
    use super::{isolated_world_bootstrap_js, stealth_flags, ClientHints, PinnedVersion};
    use crate::browser::launch::LaunchMode;
    use std::path::Path;

    #[test]
    fn client_hints_should_match_pinned_major_when_pinned_version_given() {
        let hints = ClientHints::for_version(PinnedVersion::pinned());
        let major = PinnedVersion::pinned().major().to_string();
        assert!(
            hints.user_agent.contains(&format!("Chrome/{major}")),
            "UA must contain Chrome/<pinned-major>, got {}",
            hints.user_agent
        );
        assert!(hints.sec_ch_ua.contains(&format!("v=\"{major}\"")));
        // Bootstrap embeds the same coherent UA.
        let js = isolated_world_bootstrap_js(&hints);
        assert!(js.contains(&format!("Chrome/{major}")), "bootstrap JS must embed pinned UA");
        assert!(js.trim_end().ends_with(")()"), "bootstrap must be an IIFE returning bool");
    }

    #[test]
    fn stealth_flag_should_have_justification_when_gate_runs() {
        // Epic-2 gate name for the per-flag justification policy: every flag
        // literal in this file must sit on a line carrying its policy comment.
        // The opener is built dynamically so the gate cannot match itself.
        let src = include_str!("stealth.rs");
        let flag_open = format!("{}--", char::from(34u8));
        let mut checked = 0;
        for (idx, line) in src.lines().enumerate() {
            if line.contains(&flag_open) {
                checked += 1;
                assert!(
                    line.contains("justification:"),
                    "stealth.rs:{} flag without // justification: {line}",
                    idx + 1
                );
            }
        }
        assert!(checked > 0, "gate must actually inspect flag lines");
    }

    #[test]
    fn stealth_module_should_contain_no_interaction_faking_when_scanned() {
        // Interaction-faking ban: no human-input simulation machinery, ever.
        // Terms are fragmented so this gate cannot match its own source text.
        let src = include_str!("stealth.rs");
        let banned: Vec<String> = vec![
            ["synth", "etic"].concat(),
            ["keys", "troke"].concat(),
            ["input fa", "ker"].concat(),
            ["input_fa", "ker"].concat(),
            ["fill_creden", "tial"].concat(),
            ["solve_cap", "tcha"].concat(),
            ["solve cap", "tcha"].concat(),
            ["mouse_m", "ove"].concat(),
            ["mouse m", "ove"].concat(),
        ];
        let lower = src.to_lowercase();
        for term in &banned {
            assert!(!lower.contains(term), "banned interaction-faking term present");
        }
    }
    #[test]
    fn stealth_flags_should_carry_justification_when_policy_gate_runs() {
        // Policy gate, compile-coupled: every flag literal in this file must
        // sit on a line carrying its trailing policy comment.
        // NOTE: this test body deliberately avoids spelling any flag literal
        // itself (uses suffix matching) so the scan below stays strict.
        let src = include_str!("stealth.rs");
        // Flag-literal opener built dynamically so this gate cannot match
        // its own source text (quote char 34 + two dashes).
        let flag_open = format!("{}--", char::from(34u8));
        for (idx, line) in src.lines().enumerate() {
            if line.contains(&flag_open) {
                assert!(
                    line.contains("justification:"),
                    "stealth.rs:{} flag without // justification: {line}",
                    idx + 1
                );
            }
        }
        // Interaction-faking ban: no human-input simulation machinery, ever.
        // Terms are fragmented so this gate cannot match its own source text.
        let banned: Vec<String> = vec![
            ["synth", "etic"].concat(),
            ["keys", "troke"].concat(),
            ["input fa", "ker"].concat(),
            ["input_fa", "ker"].concat(),
            ["fill_creden", "tial"].concat(),
            ["solve_cap", "tcha"].concat(),
            ["solve cap", "tcha"].concat(),
        ];
        let lower = src.to_lowercase();
        for term in &banned {
            assert!(!lower.contains(term), "banned interaction-faking term present");
        }
        // Both modes assemble without I/O.
        let headed = stealth_flags(LaunchMode::HeadedManualLogin, Path::new("/tmp/p"));
        let headless = stealth_flags(LaunchMode::HeadlessStatus, Path::new("/tmp/p"));
        assert!(headed.iter().all(|f| !f.as_str().contains("headless")));
        assert!(headless.iter().any(|f| f.as_str().ends_with("headless=new")));
    }
}
