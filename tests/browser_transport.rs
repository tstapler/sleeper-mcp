//! Transport spike gate (Story 1.1 / validation R21).
//!
//! The hermetic part runs in CI: main must carry no chaser-oxide dependency
//! until a verdict task flips it (`rg chaser Cargo.toml Cargo.lock` empty).
//! The manual part (spike-branch `Cargo.toml` swap + `cargo build` + Story 1.4
//! parity gate, verdict recorded in plan.md) is operator-gated and tracked in
//! the plan, not executed here.

#[test]
fn spike_should_record_verdict_when_parity_gate_runs() {
    for (name, contents) in [
        ("Cargo.toml", include_str!("../Cargo.toml")),
        ("Cargo.lock", include_str!("../Cargo.lock")),
    ] {
        assert!(
            !contents.contains("chaser"),
            "{name} on main must not reference chaser-oxide before the spike verdict flips it (Story 1.1 AC2)"
        );
    }
    // Verdict artifact lives in plan.md Story 1.1 (stay-on-0.9.1, 2026-09-08).
    let plan = include_str!("../project_plans/undetected-chromedriver-rust/implementation/plan.md");
    assert!(
        plan.contains("stay-on-0.9.1"),
        "plan.md must record the Story 1.1 spike verdict"
    );
}
