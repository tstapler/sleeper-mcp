//! Story 1.1 (R21) — chaser-oxide spike gate.
//!
//! The spike verdict lives in
//! `project_plans/undetected-chromedriver-rust/implementation/plan.md`
//! ("Spike verdict log"). Until a verdict task flips it to `adopt`, main
//! must not depend on chaser-oxide (neither `Cargo.toml` nor `Cargo.lock`).

/// Hermetic gate (Story 1.1 AC2): main stays on chromiumoxide 0.9.1 until
/// the spike verdict flips to `adopt`. Runs with no Chrome.
#[test]
fn spike_stays_off_main_until_verdict_flips() {
    let manifest = include_str!("../Cargo.toml");
    assert!(
        !manifest.contains("chaser"),
        "Cargo.toml references chaser-oxide with no adopt verdict recorded in plan.md"
    );
}

/// Live spike gate (validation.md R21): on the spike branch only, swap
/// chromiumoxide for chaser-oxide, `cargo build`, then run the Story 1.4
/// parity gate (≥3 runs per profile state) and record adopt/stay with
/// commit hash + license check in plan.md.
///
/// Requires live Chrome + a human operator for manual logins, so it is
/// `#[ignore]`d by default:
/// `cargo test -- --ignored spike_should_record_verdict_when_parity_gate_runs`
#[test]
#[ignore = "manual: spike branch + live Chrome + human operator required (Story 1.1 Tasks 2-4)"]
fn spike_should_record_verdict_when_parity_gate_runs() {
    // Gated steps (execute on the spike branch, never on main):
    // 1. Swap `chromiumoxide` for `chaser-oxide` in Cargo.toml; `cargo build -q`.
    // 2. Run `sleeper browser status` once; record WS-error delta vs main.
    // 3. Run the Story 1.4 parity gate (≥3 runs per profile state).
    // 4. Record adopt/stay + commit hash + license check in plan.md.
    // Per AC3: WS/protocol parity failures on 0.9.1 flip the verdict to adopt.
    panic!("live spike gate: run on the spike branch with --ignored after Q-policy sign-off");
}
