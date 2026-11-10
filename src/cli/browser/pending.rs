//! Pending-waiver-claim read path (root-cause fix, 2026-11-10): the public
//! REST `/league/{id}/transactions/{round}` endpoint only ever returns
//! `status: "complete"` entries — live-verified against a real league, for
//! every roster, not just the caller's own. Pending claims are only visible
//! through the authenticated GraphQL query the Sleeper web app itself fires
//! (`pending_transactions_via_seam`, `src/browser/session/flows.rs`).
use crate::config::Config;
use crate::error::Result;

use super::login::{
    debug_detail, exit_on_action_error, launch_failure_error, launch_session, triage_error,
    LaunchOptions,
};

/// One pending (or otherwise non-`complete`) transaction, already resolved
/// to display names via the GraphQL response's own `player_map` — no
/// separate name-lookup call needed (unlike `claim`/`lineup_set`, which
/// resolve against a bare player_id the live DOM never carries).
#[derive(Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) struct PendingTransaction {
    pub(crate) transaction_id: String,
    pub(crate) kind: String,
    pub(crate) status: String,
    pub(crate) leg: i64,
    pub(crate) waiver_bid: Option<i64>,
    pub(crate) adds: Vec<String>,
    pub(crate) drops: Vec<String>,
}

impl PendingTransaction {
    pub(crate) fn render(&self) -> String {
        let adds = if self.adds.is_empty() { "—".to_string() } else { self.adds.join(", ") };
        let drops = if self.drops.is_empty() { "none".to_string() } else { self.drops.join(", ") };
        match self.waiver_bid {
            Some(bid) => format!(
                "[{}] {} leg {} — add {adds} (bid ${bid}), drop {drops}",
                self.status, self.kind, self.leg
            ),
            None => format!("[{}] {} leg {} — add {adds}, drop {drops}", self.status, self.kind, self.leg),
        }
    }
}

fn describe_player(player_map: &serde_json::Value, player_id: &str) -> String {
    let Some(p) = player_map.get(player_id) else {
        return format!("player {player_id}");
    };
    let first = p.get("first_name").and_then(|v| v.as_str()).unwrap_or("");
    let last = p.get("last_name").and_then(|v| v.as_str()).unwrap_or("");
    let pos = p.get("position").and_then(|v| v.as_str()).unwrap_or("?");
    let team = p.get("team").and_then(|v| v.as_str()).unwrap_or("FA");
    format!("{first} {last} ({pos} - {team})")
}

/// Parses the raw `league_transactions_filtered` GraphQL response. A
/// top-level `errors` array (auth failure, bad query, etc.) becomes an
/// `Err` naming the first message rather than silently returning nothing.
pub(crate) fn parse_pending_transactions(raw: &str) -> Result<Vec<PendingTransaction>> {
    let v: serde_json::Value = serde_json::from_str(raw).map_err(crate::browser::other)?;
    if let Some(msg) = v
        .get("errors")
        .and_then(|e| e.as_array())
        .and_then(|a| a.first())
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
    {
        return Err(crate::browser::other(format!("pending-transactions query failed: {msg}")));
    }
    let entries = v
        .pointer("/data/league_transactions_filtered")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::with_capacity(entries.len());
    for e in entries {
        let empty_map = serde_json::Value::Object(Default::default());
        let player_map = e.get("player_map").unwrap_or(&empty_map);
        let names = |field: &str| -> Vec<String> {
            e.get(field)
                .and_then(|m| m.as_object())
                .map(|m| m.keys().map(|id| describe_player(player_map, id)).collect())
                .unwrap_or_default()
        };
        out.push(PendingTransaction {
            transaction_id: e.get("transaction_id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            kind: e.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            status: e.get("status").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            leg: e.get("leg").and_then(|v| v.as_i64()).unwrap_or(0),
            waiver_bid: e.pointer("/settings/waiver_bid").and_then(|v| v.as_i64()),
            adds: names("adds"),
            drops: names("drops"),
        });
    }
    Ok(out)
}

fn pending_failed_triage() -> String {
    "Reading pending transactions failed — the Sleeper GraphQL schema may have changed. Re-run with SLEEPER_BROWSER_DEBUG=1 for detail.".to_string()
}

/// Core: launches through the seam headless, reads pending transactions for
/// `roster_id` filtered to `type_filters`, and closes the browser on every
/// branch — no orphan, and no printing/process exit (shared by the CLI
/// command below and the `get_pending_waivers` MCP tool).
pub(crate) async fn run_pending_transactions(
    config: &Config,
    league: &str,
    roster_id: i64,
    type_filters: &[&str],
) -> Result<Vec<PendingTransaction>> {
    use crate::browser::session::{pending_transactions_via_seam, DEAD_SESSION_HINT};
    let (mut session, launched) = match launch_session(config, LaunchOptions::headless()).await {
        Ok(pair) => pair,
        Err(e) => return Err(launch_failure_error(&e)),
    };
    let outcome = pending_transactions_via_seam(&*session, league, roster_id, type_filters).await;
    session.close().await?;
    launched.close().await?;
    let raw = match outcome {
        Ok(raw) => raw,
        Err(e) if e.to_string() == DEAD_SESSION_HINT => return Err(e),
        Err(e) => {
            return Err(crate::browser::other(triage_error(
                &pending_failed_triage(),
                &debug_detail(&e.to_string()),
            )))
        }
    };
    parse_pending_transactions(&raw)
}

/// CLI wrapper: prints one line per pending claim, or `no pending waiver
/// claims`; dead sessions get the yellow Surface C hint, everything else
/// prints red and exits 1.
pub(crate) async fn cmd_pending_waivers(config: &Config, league: &str, roster_id: i64) -> Result<()> {
    match run_pending_transactions(config, league, roster_id, &["waiver"]).await {
        Ok(txns) if txns.is_empty() => {
            println!("no pending waiver claims");
            Ok(())
        }
        Ok(txns) => {
            for t in &txns {
                println!("{}", t.render());
            }
            Ok(())
        }
        Err(e) => exit_on_action_error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_pending_transactions, PendingTransaction};

    #[test]
    fn parse_should_resolve_player_names_when_pending_claims_present() {
        let raw = r#"{"data":{"league_transactions_filtered":[
            {"adds":{"13297":5},"drops":null,"created":1,"leg":9,"settings":{"waiver_bid":5},
             "status":"pending","transaction_id":"1","type":"waiver",
             "player_map":{"13297":{"first_name":"Reggie","last_name":"Virgil","position":"WR","team":"ARI"}}}
        ]}}"#;
        let txns = parse_pending_transactions(raw).expect("must parse");
        assert_eq!(txns.len(), 1);
        assert_eq!(
            txns[0],
            PendingTransaction {
                transaction_id: "1".to_string(),
                kind: "waiver".to_string(),
                status: "pending".to_string(),
                leg: 9,
                waiver_bid: Some(5),
                adds: vec!["Reggie Virgil (WR - ARI)".to_string()],
                drops: vec![],
            }
        );
        assert!(txns[0].render().contains("Reggie Virgil"));
        assert!(txns[0].render().contains("$5"));
    }

    #[test]
    fn parse_should_surface_graphql_error_when_unauthorized() {
        let raw = r#"{"data":{"league_transactions_filtered":null},"errors":[{"message":"Unauthorized"}]}"#;
        let err = parse_pending_transactions(raw).expect_err("an errors array must become Err");
        assert!(err.to_string().contains("Unauthorized"), "got {err}");
    }

    #[test]
    fn parse_should_return_empty_when_no_pending_claims() {
        let raw = r#"{"data":{"league_transactions_filtered":[]}}"#;
        assert_eq!(parse_pending_transactions(raw).expect("must parse"), Vec::new());
    }
}
