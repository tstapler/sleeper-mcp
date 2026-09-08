# ADR-002: Chrome-for-Testing Pinned Resolver Cached Under Config::data_dir

**Status:** Accepted
**Date:** 2026-09-08
**Context:** System Chrome auto-updates, invalidating stealth patches/flags and causing version skew (already observed as chromiumoxide WS spam vs Chrome 146). Stock `Browser::launch` resolves whatever Chrome is on PATH with no pinning or diagnostics.
**Decision:** Add a minimal pinned resolver: download Chrome-for-Testing (CfT) via existing `reqwest` + `sha2` plus new `zip` crate, verify checksum, cache under `Config::data_dir` (e.g. `<data_dir>/cft/<version>/chrome`), fail fast on version mismatch. `Config::chrome_profile_dir()` remains the sole profile owner; CfT binary cache is separate from the profile dir. `zip` is the only new dependency; no webdriver/fantoccini/thirtyfour/stealth crates.
**Consequences:** Reproducible Chrome/driver pair per release; per-release re-validation required (pitfalls killer). Extra disk + download logic isolated in `src/browser/cft.rs` (pure URL/filename fns unit-testable without I/O). `browser doctor` surfaces pinned vs runtime version.
**Alternatives rejected:** system-Chrome tracking (auto-update drift); vendoring Chrome binary in git (size/licensing); SaaS/proxy browser (out of scope).
