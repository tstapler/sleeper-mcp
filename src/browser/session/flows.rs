//! Live-verified DOM-action flows (waiver claim, lineup swap, trade-propose
//! stub) — built entirely on the generic seam primitives from `mod.rs`
//! (`click_selector`/`wait_for_selector`/`type_into_selector`/`click_in_row`),
//! never on transport types.
// kibitzer:ignore file-size -- Epic 1.4's task brief scopes this work to
// flows.rs/fake.rs/a new test file only (coordinator constraint, avoiding a
// merge conflict with a parallel worker's Epic 1.1 files); splitting this
// file is a reasonable follow-up but out of this epic's scope.
use crate::browser::{other, BrowserSession};
use crate::error::Result;

use super::debug::DEAD_SESSION_HINT;

/// Selector bundle live-verified against real sleeper.com rows (plan.md
/// Story 3.1, 2026-09-11) — never guessed. The player list is
/// `react-virtualized` (off-screen rows aren't in the DOM at all), so the
/// search box is the only reliable way to get a row mounted before touching
/// it.
mod claim_selectors {
    pub const PLAYER_SEARCH_INPUT: &str = ".player-search input";
    pub const PLAYER_LIST_ITEM: &str = ".player-list-item";
    pub const ACTION_BUTTON: &str = ".player-list-item a.player-action-button";
    pub const ON_WAIVERS_PROBE_JS: &str =
        "!!document.querySelector('.player-list-item a.player-action-button.waiver')";
    pub const MODAL_ROOT: &str = ".add-player-panel";
    pub const DROP_ROW: &str = ".add-player-panel a.team-roster-item";
    pub const BID_INPUT: &str = ".add-player-panel .bid-input-container input";
    pub const SUBMIT_BUTTON: &str = ".add-player-panel button";
    pub const SUBMIT_WAIVER_TEXT: &str = "Make Waiver Bid";
    pub const SUBMIT_ADD_TEXT: &str = "Add Player";
}

/// Selector bundle for the Team/lineup page, spot-checked 2026-09-11
/// (plan.md Epic 4 lineup mechanism spot-check) then fully verified before
/// this flow was written: clicking a starter's position link enters an
/// in-place swap-select mode (no modal) — the clicked starter and every
/// eligible bench player get marked `.selected`/`.valid` respectively, an
/// ineligible bench player gets `.invalid`. Clicking a `.valid` row's link
/// completes the swap immediately — there is no separate save/confirm step.
mod lineup_selectors {
    pub const ROSTER_ROW: &str = ".team-roster-item";
    pub const ROW_LINK: &str = "a.cell-position";
    pub const VALID_TARGET_ROW: &str = ".team-roster-item.valid";
}

/// Story 3.2 seam-side claim flow, live-verified 2026-09-11 (plan.md Epic 3,
/// Story 3.1 selector bundle). `player_name`/`drop_player_name` are the
/// resolved Sleeper display names (callers resolve player_id -> full_name
/// before crossing the seam — the DOM has no player_id to select on).
/// `bid_amount` is ignored on the free-agent (non-waiver) path, since that
/// modal has no bid field.
pub async fn claim_player_via_seam(
    session: &dyn BrowserSession,
    league_id: &str,
    player_name: &str,
    bid_amount: u32,
    drop_player_name: Option<&str>,
) -> Result<()> {
    use claim_selectors::*;

    let start = tracing::info_span!("claim.start", league = league_id, player = player_name);
    let _enter = start.enter();
    if !session.is_logged_in().await.unwrap_or(false) {
        return Err(other(DEAD_SESSION_HINT));
    }

    session.goto(&format!("https://sleeper.com/leagues/{league_id}/players")).await?;
    session.wait_for_selector(PLAYER_SEARCH_INPUT, 10_000).await?;
    session.type_into_selector(PLAYER_SEARCH_INPUT, player_name).await?;
    session.wait_for_selector(PLAYER_LIST_ITEM, 5_000).await?;

    let on_waivers = session.eval_bool(ON_WAIVERS_PROBE_JS).await?;
    session.click_selector(ACTION_BUTTON).await?;
    session.wait_for_selector(MODAL_ROOT, 5_000).await?;

    if let Some(drop_name) = drop_player_name {
        session.click_in_row(DROP_ROW, drop_name, "").await?;
    }

    let confirm = tracing::info_span!("claim.confirm", drop = ?drop_player_name, bid = bid_amount, on_waivers);
    let _enter_confirm = confirm.enter();
    if on_waivers {
        session.type_into_selector(BID_INPUT, &bid_amount.to_string()).await?;
        session.click_in_row(SUBMIT_BUTTON, SUBMIT_WAIVER_TEXT, "").await?;
    } else {
        session.click_in_row(SUBMIT_BUTTON, SUBMIT_ADD_TEXT, "").await?;
    }
    tracing::info!("claim submitted");
    Ok(())
}

/// Story 4.1 seam-side lineup-swap flow, live-verified 2026-09-11 (plan.md
/// Epic 4 lineup mechanism spot-check). `out_player_name`/`in_player_name`
/// are resolved Sleeper display names (callers resolve player_id ->
/// full_name before crossing the seam, same as the claim flow). There is no
/// confirm step on the live UI — clicking the bench replacement's row
/// completes the swap immediately.
pub async fn lineup_set_via_seam(
    session: &dyn BrowserSession,
    league_id: &str,
    out_player_name: &str,
    in_player_name: &str,
) -> Result<()> {
    use lineup_selectors::*;

    let start = tracing::info_span!(
        "lineup.start",
        league = league_id,
        out = out_player_name,
        r#in = in_player_name
    );
    let _enter = start.enter();
    if !session.is_logged_in().await.unwrap_or(false) {
        return Err(other(DEAD_SESSION_HINT));
    }

    session.goto(&format!("https://sleeper.com/leagues/{league_id}/team")).await?;
    session.wait_for_selector(ROSTER_ROW, 10_000).await?;
    // Enter select mode: click the currently-started player's position link.
    session.click_in_row(ROSTER_ROW, out_player_name, ROW_LINK).await?;
    // Only an eligible bench row carries `.valid` — an ineligible in_player
    // simply won't match here, failing closed instead of clicking the wrong row.
    session.wait_for_selector(VALID_TARGET_ROW, 5_000).await?;

    let confirm = tracing::info_span!("lineup.confirm", out = out_player_name, r#in = in_player_name);
    let _enter_confirm = confirm.enter();
    session.click_in_row(VALID_TARGET_ROW, in_player_name, ROW_LINK).await?;
    tracing::info!("lineup saved");
    Ok(())
}

/// Story 4.x seam-side stub — STUB ONLY (Epic 4 cut per appetite rule;
/// selectors live-unverified). Through-seam signature, no automation.
pub async fn trade_propose_via_seam(
    session: &dyn BrowserSession,
    league_id: &str,
    give: &str,
    get: &str,
) -> Result<()> {
    let start = tracing::info_span!("trade.start", league = league_id, give = give, get = get);
    let _enter = start.enter();
    tracing::info!("trade gated: Epic 4 cut, no automation attempted");
    if !session.is_logged_in().await.unwrap_or(false) {
        return Err(other(DEAD_SESSION_HINT));
    }
    Err(other(
        "selectors live-unverified — trade automation disabled (Epic 4 deferred; see plan.md cut note)",
    ))
}

/// Root-cause fix (2026-11-10): the public REST `/league/{id}/transactions/{round}`
/// endpoint only ever returns `status: "complete"` entries — live-verified
/// against this exact league: a pending waiver claim and rejected/cancelled
/// trades are both invisible there, for every roster, not just the caller's
/// own. Pending claims are only visible through the same authenticated
/// GraphQL query the Sleeper web app itself fires when the Team page's
/// WAIVER/TRADE sub-tab is opened: `league_transactions_filtered`.
///
/// Auth: the app sends the token it already holds in
/// `localStorage.token` as the raw `Authorization` header value (no
/// `Bearer ` prefix — confirmed live; a `Bearer `-prefixed value is
/// rejected). That value is read and used ENTIRELY inside the page's own
/// JS via `fetch` — it is built into the request here, in Rust, only as a
/// JS snippet that reads `localStorage` itself; the token never crosses
/// into a Rust value, a log line, or this function's return value, which
/// is only the (non-secret) transactions payload.
pub async fn pending_transactions_via_seam(
    session: &dyn BrowserSession,
    league_id: &str,
    roster_id: i64,
    type_filters: &[&str],
) -> Result<String> {
    let start = tracing::info_span!(
        "pending_transactions.start",
        league = league_id,
        roster_id,
        types = ?type_filters
    );
    let _enter = start.enter();

    let types_json = serde_json::to_string(type_filters).map_err(other)?;
    let query = format!(
        r#"query league_transactions_filtered {{ league_transactions_filtered(league_id: "{league_id}", roster_id_filters: [{roster_id}], type_filters: {types_json}, leg_filters: [], status_filters: []) {{ adds drops created league_id leg metadata roster_ids settings status status_updated transaction_id type player_map waiver_budget }} }}"#
    );
    let body = serde_json::json!({
        "operationName": "league_transactions_filtered",
        "variables": {},
        "query": query,
    })
    .to_string();
    let body_json = serde_json::to_string(&body).map_err(other)?;
    let raw = graphql_post_via_seam(session, &body_json).await?;
    tracing::info!("pending_transactions fetched");
    Ok(raw)
}

/// Fix 4 (Phase 6 review): shared core for every GraphQL POST this module
/// makes through the seam — the login check, and the `localStorage`
/// token-read-entirely-in-page-JS `fetch('/graphql', ...)` template (see
/// [`pending_transactions_via_seam`]'s own doc comment for why the token
/// never crosses into a Rust value). Parameterized only by the
/// already-JSON-encoded request body (`operationName`/`variables`/`query`),
/// the one part that actually varies between callers.
async fn graphql_post_via_seam(session: &dyn BrowserSession, body_json: &str) -> Result<String> {
    if !session.is_logged_in().await.unwrap_or(false) {
        return Err(other(DEAD_SESSION_HINT));
    }
    let js = format!(
        r#"(function(){{
            let t; try {{ t = JSON.parse(localStorage.getItem('token_data')); }} catch (e) {{ t = null; }}
            const tok = (t && (t.token || t.access_token || t.jwt)) || localStorage.getItem('token');
            return fetch('/graphql', {{
                method: 'POST',
                headers: {{ 'content-type': 'application/json', 'authorization': tok }},
                body: {body_json}
            }}).then(r => r.text());
        }})()"#
    );
    session.eval_string(&js).await
}

/// Epic 2.2 / Task 2.2.1a: default `type_filters` for a whole-league
/// backfill — every kind `league_transactions_filtered` can return, unlike
/// `pending_transactions_via_seam`'s caller-supplied (typically
/// `["waiver"]`) narrower list.
const LEAGUE_TRANSACTION_TYPE_FILTERS: &[&str] = &["waiver", "trade", "free_agent"];

/// Pure, no I/O (Task 2.2.1a): builds the `league_transactions_filtered`
/// GraphQL query string. Extracted out of the `eval_string` call site so
/// the query shape itself — `roster_id_filters`/`type_filters`/
/// `status_filters` — is unit-testable without a `BrowserSession` double.
fn build_league_transactions_query(
    league_id: &str,
    roster_id_filters: &[i64],
    type_filters: &[&str],
) -> Result<String> {
    let roster_ids_json = serde_json::to_string(roster_id_filters).map_err(other)?;
    let types_json = serde_json::to_string(type_filters).map_err(other)?;
    Ok(format!(
        r#"query league_transactions_filtered {{ league_transactions_filtered(league_id: "{league_id}", roster_id_filters: {roster_ids_json}, type_filters: {types_json}, leg_filters: [], status_filters: []) {{ adds drops created league_id leg metadata roster_ids settings status status_updated transaction_id type player_map waiver_budget }} }}"#
    ))
}

/// Epic 2.2 / Task 2.2.1a: generalizes [`pending_transactions_via_seam`] to
/// a whole-league backfill — drops the single-`roster_id` filter
/// (`roster_id_filters: &[i64]`, empty = every roster) and widens
/// `type_filters` to [`LEAGUE_TRANSACTION_TYPE_FILTERS`] and
/// `status_filters` to `[]` (every status, not just one caller-chosen
/// subset), so one round-trip covers the whole league instead of twelve
/// per-roster calls. Same `localStorage`-token-read-entirely-in-page-JS
/// discipline as `pending_transactions_via_seam` — no new auth surface.
pub async fn league_transactions_via_seam(
    session: &dyn BrowserSession,
    league_id: &str,
    roster_id_filters: &[i64],
) -> Result<String> {
    let start = tracing::info_span!(
        "league_transactions.start",
        league = league_id,
        rosters = ?roster_id_filters
    );
    let _enter = start.enter();

    let query = build_league_transactions_query(
        league_id,
        roster_id_filters,
        LEAGUE_TRANSACTION_TYPE_FILTERS,
    )?;
    let body = serde_json::json!({
        "operationName": "league_transactions_filtered",
        "variables": {},
        "query": query,
    })
    .to_string();
    let body_json = serde_json::to_string(&body).map_err(other)?;
    let raw = graphql_post_via_seam(session, &body_json).await?;
    tracing::info!("league_transactions fetched");
    Ok(raw)
}

// --- Epic 1.4 / Story 1.4.1: edit/cancel row-identity disambiguation ---
// ADR-004: resolved without a new `BrowserSession` trait primitive. See
// Step 0.5 fork #3 and the Domain Glossary entries for `claim_row_disambiguator`
// / `edit_cancel_claim_via_seam` / `EditCancelAction` / `EditCancelOutcome` in
// project_plans/waiver-intelligence/implementation/plan.md.

/// One candidate claim row, parsed from the JSON [`BrowserSession::eval_string`]
/// returns for the claims panel (Task 1.4.1a). `transaction_id` is `None`
/// when the live DOM turns out not to expose it anywhere (ADR-004
/// Consequences) — the only case `RowTarget::Fallback` exists for.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
struct CandidateRow {
    index: usize,
    text: String,
    #[serde(default)]
    transaction_id: Option<String>,
}

/// Disambiguation key for [`claim_row_disambiguator`]. A `TransactionId`
/// always wins when present; `Fallback` (bid amount / add-player name
/// substring match) is only used when no `transaction_id` is available —
/// ADR-004 explicitly forbids a "closest match" or "first match" fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RowTarget {
    TransactionId(String),
    // Only reachable once live selector discovery confirms the DOM exposes
    // no `transaction_id` at all (ADR-004 Consequences) — exercised today
    // by `claim_row_disambiguator`'s own unit tests, not yet by any caller.
    #[allow(dead_code)]
    Fallback {
        bid: Option<i64>,
        add_name: Option<String>,
    },
}

/// Task 1.4.1a: pure, no I/O. Resolves exactly one candidate row index, or
/// fails closed on 0 or >1 matches — never guesses "closest" or "first"
/// (ADR-004, pitfalls.md §3's Brissett/Brissett-Jones near-miss).
fn claim_row_disambiguator(rows: &[CandidateRow], target: &RowTarget) -> Result<usize> {
    let matches: Vec<usize> = rows
        .iter()
        .filter(|row| match target {
            RowTarget::TransactionId(tid) => row.transaction_id.as_deref() == Some(tid.as_str()),
            RowTarget::Fallback { bid, add_name } => {
                let name_ok = add_name.as_deref().is_none_or(|n| row.text.contains(n));
                let bid_ok = bid.is_none_or(|b| row.text.contains(&format!("${b}")));
                name_ok && bid_ok
            }
        })
        .map(|row| row.index)
        .collect();

    match matches.len() {
        1 => Ok(matches[0]),
        0 => Err(other(
            "no candidate claim row matched the disambiguation target — nothing clicked",
        )),
        _ => Err(other(format!(
            "ambiguous claim row match: candidate indices {matches:?} all matched — refusing to guess (ADR-004)"
        ))),
    }
}

/// Candidate CSS/DOM selector bundle for the claims panel's edit/cancel
/// controls — **UNVERIFIED** against real sleeper.com DOM (ADR-004; unlike
/// `claim_selectors`/`lineup_selectors` above, no live-run has happened for
/// this flow yet). Follows the same STUB doc-comment discipline as
/// `trade_propose_via_seam`'s selector-less stub above, with one
/// difference: `trade_propose_via_seam` never runs any JS built from a
/// selector at all (Epic 4 cut), while `edit_cancel_claim_via_seam` here IS
/// fully exercised against `FakeSession` — only the actual selector
/// strings/JS shape below remain unverified until a live-run session
/// resolves plan.md's Unresolved Questions (e.g. whether the DOM exposes
/// `transaction_id` anywhere at all, per ADR-004 Consequences).
mod edit_claim_selectors {
    /// Opens the Team page's pending-claims panel (candidate selector).
    pub const CLAIMS_TAB: &str = ".league-transactions-tab.waivers";
    /// One row per pending claim once the panel is open.
    pub const CLAIM_ROW: &str = ".pending-claim-row";
    /// Candidate attribute the DOM may expose the GraphQL `transaction_id`
    /// through. If live discovery finds no such attribute anywhere, only
    /// `claim_row_disambiguator`'s `RowTarget::Fallback` branch needs
    /// redesigning (ADR-004 Consequences) — not this module's shape.
    pub const ROW_TRANSACTION_ID_ATTR: &str = "data-transaction-id";
    pub const BID_EDIT_INPUT: &str = ".bid-input-container input";
    pub const CANCEL_BUTTON: &str = ".cancel-claim-button";
    pub const SAVE_BUTTON: &str = ".save-claim-button";
}

/// Task 1.4.1b: the two write operations `edit_cancel_claim_via_seam`
/// supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditCancelAction {
    EditBid(u32),
    Cancel,
}

/// Task 1.4.1b: never a bare `Result<()>` (Domain Glossary) — the "never
/// claim success it can't prove" discipline's concrete type.
/// `AbortedBeforeWrite` means the pre-click, same-`eval_string`-call
/// row-identity re-assertion found no single exact match, so nothing was
/// clicked and the claim is provably unchanged — safe to retry immediately.
/// `Ambiguous` means the click already fired and the post-click
/// confirmation read came back unverified — the outcome is genuinely
/// unknown, real risk in a blind retry. Neither collapses into a bare `Err`
/// that would imply nothing happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditCancelOutcome {
    Confirmed,
    AbortedBeforeWrite { reason: String },
    Ambiguous { reason: String },
}

/// Pure, no I/O: interprets the second `eval_string` call's JSON per the
/// pre-click/post-click distinction `EditCancelOutcome` draws. Unparseable
/// JSON is treated as `Ambiguous`, never as a bare error swallowed into
/// `Confirmed` or `AbortedBeforeWrite` — by the time the call attempted a
/// JSON response at all, the click may already have fired.
fn parse_edit_cancel_outcome(raw: &str) -> EditCancelOutcome {
    #[derive(serde::Deserialize, Default)]
    struct RawOutcome {
        #[serde(default)]
        reasserted: Option<bool>,
        #[serde(default)]
        confirmed: Option<bool>,
    }

    match serde_json::from_str::<RawOutcome>(raw) {
        Ok(RawOutcome {
            reasserted: Some(false),
            ..
        }) => EditCancelOutcome::AbortedBeforeWrite {
            reason: "pre-click row-identity re-assertion found no exact match — nothing clicked, claim unchanged".to_string(),
        },
        Ok(RawOutcome {
            reasserted: Some(true),
            confirmed: Some(true),
        }) => EditCancelOutcome::Confirmed,
        Ok(RawOutcome {
            reasserted: Some(true),
            ..
        }) => EditCancelOutcome::Ambiguous {
            reason: "click fired but the post-click confirmation read did not confirm success".to_string(),
        },
        _ => EditCancelOutcome::Ambiguous {
            reason: format!("post-click confirmation read was unparseable: {raw}"),
        },
    }
}

/// Builds call 1's JS (unverified selectors, ADR-004): opens the claims
/// panel and reads back every candidate row's index/text/transaction_id.
fn build_read_claim_rows_js() -> String {
    use edit_claim_selectors::*;
    format!(
        r#"(function(){{
            document.querySelector('{CLAIMS_TAB}')?.click();
            return JSON.stringify(Array.from(document.querySelectorAll('{CLAIM_ROW}')).map((el, index) => ({{
                index: index,
                text: el.textContent || '',
                transaction_id: el.getAttribute('{ROW_TRANSACTION_ID_ATTR}'),
            }})));
        }})()"#
    )
}

/// Builds call 2's JS (unverified selectors, ADR-004): re-asserts the
/// resolved row's identity in the SAME `eval_string` call that performs the
/// write (TOCTOU guard, pre-mortem P1 #2) — only when that re-assertion
/// still holds does the JS click anything.
fn build_edit_cancel_write_js(
    resolved_index: usize,
    transaction_id_json: &str,
    action: EditCancelAction,
) -> String {
    use edit_claim_selectors::*;
    match action {
        EditCancelAction::EditBid(bid) => format!(
            r#"(function(){{
                const row = document.querySelectorAll('{CLAIM_ROW}')[{resolved_index}];
                if (!row || row.getAttribute('{ROW_TRANSACTION_ID_ATTR}') !== {transaction_id_json}) {{
                    return JSON.stringify({{reasserted: false}});
                }}
                const input = row.querySelector('{BID_EDIT_INPUT}');
                if (!input) return JSON.stringify({{reasserted: false}});
                input.value = '{bid}';
                input.dispatchEvent(new Event('input', {{bubbles: true}}));
                input.dispatchEvent(new Event('change', {{bubbles: true}}));
                const save = row.querySelector('{SAVE_BUTTON}');
                if (!save) return JSON.stringify({{reasserted: true}});
                save.click();
                const confirmed = (row.textContent || '').includes('${bid}');
                return JSON.stringify({{reasserted: true, clicked: true, confirmed: confirmed}});
            }})()"#
        ),
        EditCancelAction::Cancel => format!(
            r#"(function(){{
                const row = document.querySelectorAll('{CLAIM_ROW}')[{resolved_index}];
                if (!row || row.getAttribute('{ROW_TRANSACTION_ID_ATTR}') !== {transaction_id_json}) {{
                    return JSON.stringify({{reasserted: false}});
                }}
                const cancel = row.querySelector('{CANCEL_BUTTON}');
                if (!cancel) return JSON.stringify({{reasserted: false}});
                cancel.click();
                const confirmed = !document.querySelector('{CLAIM_ROW}[{ROW_TRANSACTION_ID_ATTR}="' + {transaction_id_json} + '"]');
                return JSON.stringify({{reasserted: true, clicked: true, confirmed: confirmed}});
            }})()"#
        ),
    }
}

/// Task 1.4.1b seam-side edit/cancel flow (ADR-004). Selects the correct
/// row via [`claim_row_disambiguator`] rather than `click_in_row`'s
/// first-match-substring semantics — a mis-click here (editing or
/// cancelling the wrong pending claim) is irreversible, a materially higher
/// stakes near-miss than the existing flows'. Built only on the seam's
/// existing primitives: one `eval_string` call reads every candidate row,
/// Rust resolves the index, a second `eval_string` call re-asserts that
/// row's identity and only then performs the write — never a new
/// `BrowserSession` trait method.
pub async fn edit_cancel_claim_via_seam(
    session: &dyn BrowserSession,
    league_id: &str,
    transaction_id: &str,
    action: EditCancelAction,
) -> Result<EditCancelOutcome> {
    let start = tracing::info_span!(
        "edit_cancel.start",
        league = league_id,
        transaction_id,
        action = ?action
    );
    let _enter = start.enter();
    if !session.is_logged_in().await.unwrap_or(false) {
        return Err(other(DEAD_SESSION_HINT));
    }

    // Call 1: read every candidate row, then resolve in Rust.
    let rows_raw = session.eval_string(&build_read_claim_rows_js()).await?;
    let rows: Vec<CandidateRow> = serde_json::from_str(&rows_raw).map_err(other)?;
    let resolved_index =
        claim_row_disambiguator(&rows, &RowTarget::TransactionId(transaction_id.to_string()))?;

    let confirm = tracing::info_span!("edit_cancel.confirm", resolved_index);
    let _enter_confirm = confirm.enter();

    // Call 2: re-assert identity, write, and read back confirmation. A
    // transport-level failure on THIS call (unlike call 1, above) can land
    // after the in-page JS already fired `.click()` but before its response
    // reaches us — so it must never propagate as a bare `Err` (which would
    // misreport "nothing happened"); it's `Ambiguous`, the same as an
    // unparseable response (Blocker 3 / adversarial review).
    let transaction_id_json = serde_json::to_string(transaction_id).map_err(other)?;
    let write_js = build_edit_cancel_write_js(resolved_index, &transaction_id_json, action);
    let outcome = match session.eval_string(&write_js).await {
        Ok(outcome_raw) => parse_edit_cancel_outcome(&outcome_raw),
        Err(e) => EditCancelOutcome::Ambiguous {
            reason: format!(
                "transport error between the write click and its confirmation read: {e}"
            ),
        },
    };
    tracing::info!(outcome = ?outcome, "edit_cancel resolved");
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_brissett_rows() -> Vec<CandidateRow> {
        vec![
            CandidateRow {
                index: 0,
                text: "Jacoby Brissett — bid $78".to_string(),
                transaction_id: Some("1".to_string()),
            },
            CandidateRow {
                index: 1,
                text: "Brissett Jones — bid $12".to_string(),
                transaction_id: Some("2".to_string()),
            },
        ]
    }

    /// Happy path (Story 1.4.1 AC1): resolved by the strong key even though
    /// the rendered text is ambiguous across both rows.
    #[test]
    fn claim_row_disambiguator_should_resolve_by_strong_key_when_transaction_id_matches_one_row() {
        let rows = two_brissett_rows();
        let resolved = claim_row_disambiguator(&rows, &RowTarget::TransactionId("1".to_string()))
            .expect("strong key must resolve despite the shared ambiguous substring");
        assert_eq!(resolved, 0);
    }

    /// Error path (Story 1.4.1 AC1): a weak fallback key matching both
    /// rows' rendered text must fail closed and name both candidates —
    /// never guess the first (ADR-004).
    #[test]
    fn claim_row_disambiguator_should_fail_closed_when_fallback_key_matches_multiple_rows() {
        let rows = two_brissett_rows();
        let target = RowTarget::Fallback {
            bid: None,
            add_name: Some("Brissett".to_string()),
        };
        let err = claim_row_disambiguator(&rows, &target)
            .expect_err("ambiguous fallback match must never guess the first row");
        let msg = err.to_string();
        assert!(
            msg.contains('0') && msg.contains('1'),
            "error must name both candidate indices, got {msg}"
        );
    }

    #[test]
    fn claim_row_disambiguator_should_fail_closed_when_no_row_matches() {
        let rows = two_brissett_rows();
        let err = claim_row_disambiguator(&rows, &RowTarget::TransactionId("999".to_string()))
            .expect_err("no match must be Err, never a guessed index");
        assert!(err.to_string().contains("no candidate"));
    }

    /// Story 2.2.1 AC1, Task 2.2.1a: the whole-league query drops the
    /// single-roster hardcode and widens type/status filters, unlike
    /// `pending_transactions_via_seam`'s `roster_id_filters: [roster_id]`.
    #[test]
    fn league_transactions_query_should_request_whole_league_scope_when_roster_filter_omitted() {
        let q = build_league_transactions_query(
            "1352883872131481600",
            &[],
            LEAGUE_TRANSACTION_TYPE_FILTERS,
        )
        .expect("query must build");
        assert!(q.contains("roster_id_filters: []"), "got {q}");
        assert!(
            q.contains(r#"type_filters: ["waiver","trade","free_agent"]"#),
            "got {q}"
        );
        assert!(q.contains("status_filters: []"), "got {q}");
    }

    /// A non-empty `roster_id_filters` still renders correctly (e.g. a
    /// narrower, non-"all" scope reusing the same query builder).
    #[test]
    fn league_transactions_query_should_render_roster_scope_when_roster_filter_present() {
        let q = build_league_transactions_query("123", &[5], &["waiver"])
            .expect("query must build");
        assert!(q.contains("roster_id_filters: [5]"), "got {q}");
        assert!(q.contains(r#"type_filters: ["waiver"]"#), "got {q}");
    }
}
