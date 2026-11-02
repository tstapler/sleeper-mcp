//! Debug-surface helpers: secret redaction, the `DEBUG browser:` context
//! line, and stealth-signal health parsing. Pure string/parsing functions —
//! no transport types, so none of this needs `live.rs`'s seam.

/// Redact session-shaped secrets from debug output: `key=value` pairs for
/// cookie/token/auth-adjacent keys become `key=[redacted]`, and
/// `Bearer <token>` becomes `Bearer [redacted]`. Debug dumps must never
/// leak cookies, tokens, or credential-adjacent values.
pub fn redact_secrets(s: &str) -> String {
    const KEYS: &[&str] = &[
        "session", "token", "cookie", "auth", "password", "secret", "credential",
    ];
    let mut out = s.to_string();
    for key in KEYS {
        for sep in ["=", ": "] {
            let needle = format!("{key}{sep}");
            let mut search_from = 0;
            loop {
                let hay = out.to_lowercase();
                let Some(rel) = hay[search_from..].find(&needle) else {
                    break;
                };
                let start = search_from + rel + needle.len();
                let bytes = out.as_bytes();
                let mut end = start;
                while end < bytes.len()
                    && !bytes[end].is_ascii_whitespace()
                    && bytes[end] != b','
                    && bytes[end] != b';'
                    && bytes[end] != b'"'
                    && bytes[end] != b'}'
                {
                    end += 1;
                }
                if end > start {
                    out.replace_range(start..end, "[redacted]");
                    search_from = start + "[redacted]".len();
                } else {
                    search_from = start;
                }
            }
        }
    }
    // Bearer tokens without a key prefix.
    let mut search_from = 0;
    loop {
        let hay = out.to_lowercase();
        let Some(rel) = hay[search_from..].find("bearer ") else {
            break;
        };
        let start = search_from + rel + "bearer ".len();
        let bytes = out.as_bytes();
        let mut end = start;
        while end < bytes.len()
            && !bytes[end].is_ascii_whitespace()
            && bytes[end] != b','
            && bytes[end] != b'"'
        {
            end += 1;
        }
        if end > start {
            out.replace_range(start..end, "[redacted]");
            search_from = start + "[redacted]".len();
        } else {
            search_from = start;
        }
    }
    out
}

/// Pure `DEBUG browser:` context line (ux.md Surface G): pinned/runtime
/// chrome, flag-set hash, CDP port, profile path, cleanup/reaped PIDs.
/// Callers print it to stderr with secrets already redacted.
pub fn debug_browser_line(
    pinned: &str,
    runtime: &str,
    flag_set_hash: &str,
    cdp_port: &str,
    profile: &std::path::Path,
    reaped_pids: &[u32],
) -> String {
    let pids = if reaped_pids.is_empty() {
        "0".to_string()
    } else {
        reaped_pids.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(",")
    };
    format!(
        "DEBUG browser: chrome pinned v{pinned}, runtime v{runtime}, flag set {flag_set_hash}, cdp port {cdp_port}, profile {}, cleanup: reaped pid {pids}",
        profile.display()
    )
}

/// Stealth-signal health over a page dump (seam-level parse, beside
/// `eval_dump`/`redact_secrets`): returns a short signal summary plus whether
/// the webdriver marker came back clean (`false`/`undefined`). Callers only
/// render the result; parsing never lives in the CLI.
pub fn stealth_health(dump: &str) -> (String, bool) {
    let clean =
        dump.contains("webdriver") && (dump.contains("false") || dump.contains("undefined"));
    let summary = if clean {
        "webdriver: false".to_string()
    } else {
        "webdriver: missing/tampered".to_string()
    };
    (summary, clean)
}

/// Dead-session hint (ux.md §3 downstream surfacing): flow commands that
/// detect a dead session surface this yellow Surface C hint themselves
/// rather than dumping CDP errors.
pub const DEAD_SESSION_HINT: &str = "Not logged in — run `sleeper browser login`.";

#[cfg(test)]
mod tests {
    use super::redact_secrets;

    #[test]
    fn debug_dump_should_redact_when_session_values_present() {
        let raw = "cookies: session=abc123; auth: token=xyz789, Bearer sekrit-token headers";
        let clean = redact_secrets(raw);
        for leaked in ["abc123", "xyz789", "sekrit-token"] {
            assert!(
                !clean.contains(leaked),
                "redactor must strip session values, got {clean}"
            );
        }
        assert!(clean.contains("[redacted]"), "got {clean}");
    }
}
