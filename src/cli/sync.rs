//! League/stats sync: fetch-and-persist league settings, roster snapshot,
//! transaction history, and weekly stat caching — plus the player_id →
//! display-name lookup that DOM-facing browser flows resolve through first.
use std::collections::HashMap;

use console::style;
use indicatif::{ProgressBar, ProgressStyle};

use crate::analysis::AnalysisService;
use crate::client::SleeperClient;
use crate::config::Config;
use crate::db::StatsDb;
use crate::error::Result;

pub(super) async fn cmd_sync_league(
    client: &SleeperClient,
    db: &StatsDb,
    league_id: &str,
) -> Result<()> {
    let league = client.get_league(league_id).await?;
    db.upsert_league(&league).await?;
    println!(
        "{} stored: {} | {} | {} | Superflex: {}",
        style("✓").green().bold(),
        style(&league.name).bold(),
        league.scoring_type(),
        league.status,
        if league.is_superflex() {
            style("yes").green().bold().to_string()
        } else {
            style("no").dim().to_string()
        },
    );
    println!("Roster slots: {}", league.roster_positions.join(", "));
    Ok(())
}

pub(super) async fn cmd_league_info(db: &StatsDb, league_id: &str) -> Result<()> {
    let Some(league) = db.get_league(league_id).await? else {
        println!("League {league_id} not found locally. Run `sleeper sync-league {league_id}` first.");
        return Ok(());
    };

    println!("{}", style(format!("{} ({})", league.name, league.league_id)).bold().underlined());
    println!("Season:     {} ({})", league.season, league.status);
    println!("Format:     {} | {} teams", league.scoring_type(), league.total_rosters);
    println!(
        "Superflex:  {}",
        if league.is_superflex() {
            style("YES — QBs score at WR/RB value").green().bold().to_string()
        } else {
            style("no").dim().to_string()
        }
    );
    println!("Roster slots:");
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for pos in &league.roster_positions {
        *counts.entry(pos.clone()).or_default() += 1;
    }
    for (pos, n) in &counts {
        println!("  {pos:<12} × {n}");
    }
    if let Some(rounds) = league.settings.draft_rounds {
        println!("Draft rounds: {rounds}");
    }
    Ok(())
}

/// At-a-glance league view: one row per roster with skill vs. IDP position
/// counts side by side, so scarcity/surplus is comparable across teams
/// instead of only visible one roster at a time. Also persists the full
/// resolved snapshot to disk + `stats.db` (see `AnalysisService::league_snapshot`).
pub(super) async fn cmd_league_snapshot(svc: &AnalysisService, config: &Config, league_id: &str) -> Result<()> {
    let snapshot = svc.league_snapshot(league_id).await?;
    let snapshot_path = config.snapshot_path(league_id);
    if let Some(parent) = snapshot_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&snapshot_path, serde_json::to_vec_pretty(&snapshot)?).await?;

    println!(
        "{}",
        style(format!(
            "{} ({}) — {}",
            snapshot["league_name"].as_str().unwrap_or(league_id),
            snapshot["season"].as_str().unwrap_or("?"),
            league_id,
        ))
        .bold()
        .underlined()
    );
    println!(
        "{:<3} {:<20} {:>6}  {:>16}  {:>13}  {:>7}",
        "RID", "Owner", "W-L", "Skill QB/RB/WR/TE", "IDP DL/LB/DB", "K/DEF"
    );
    const SKILL: [&str; 4] = ["QB", "RB", "WR", "TE"];
    const IDP: [&str; 3] = ["DL", "LB", "DB"];
    const OTHER: [&str; 2] = ["K", "DEF"];
    let count_of = |counts: &serde_json::Value, pos: &str| -> i64 {
        counts.get(pos).and_then(serde_json::Value::as_i64).unwrap_or(0)
    };
    let mut rosters: Vec<&serde_json::Value> =
        snapshot["rosters"].as_array().map(|v| v.iter().collect()).unwrap_or_default();
    rosters.sort_by_key(|r| r["roster_id"].as_i64().unwrap_or(0));
    for r in rosters {
        let counts = &r["position_counts"];
        let skill: Vec<String> =
            SKILL.iter().map(|p| count_of(counts, p).to_string()).collect();
        let idp: Vec<String> = IDP.iter().map(|p| count_of(counts, p).to_string()).collect();
        let other: i64 = OTHER.iter().map(|p| count_of(counts, p)).sum();
        let idp_total: i64 = IDP.iter().map(|p| count_of(counts, p)).sum();
        let idp_col = format!("{} ({idp_total})", idp.join("/"));
        let idp_col = if idp_total >= 6 { style(idp_col).yellow().to_string() } else { idp_col };
        println!(
            "{:<3} {:<20} {:>3}-{:<3} {:>16}  {:>13}  {:>7}",
            r["roster_id"].as_i64().unwrap_or(0),
            r["owner"].as_str().unwrap_or("?"),
            r["record"]["wins"].as_i64().unwrap_or(0),
            r["record"]["losses"].as_i64().unwrap_or(0),
            skill.join("/"),
            idp_col,
            other,
        );
    }
    println!(
        "\n{} full resolved snapshot: {}",
        style("↳").dim(),
        snapshot_path.display()
    );
    Ok(())
}

/// Whether [`do_sync`] narrates its progress. Named-mode replacement for a
/// bare `verbose: bool` so call sites read as intent (`Verbose`/`Silent`)
/// rather than an opaque `true`/`false`.
enum SyncVerbosity {
    Silent,
    Verbose,
}

impl SyncVerbosity {
    fn is_verbose(&self) -> bool {
        matches!(self, SyncVerbosity::Verbose)
    }
}

/// Inner sync: runs silently in `SyncVerbosity::Silent`, with progress output in `Verbose`.
/// Returns the number of newly stored stat weeks. Reads the client through
/// `svc.client` rather than taking one separately — `svc` already carries the
/// exact same client the caller would otherwise pass.
async fn do_sync(
    svc: &AnalysisService,
    config: &Config,
    league_id: &str,
    season_override: Option<&str>,
    verbosity: SyncVerbosity,
) -> Result<usize> {
    // Fetch league settings + rosters + users in parallel.
    let lid = league_id.to_string();
    let league_h = { let c = svc.client.clone(); let l = lid.clone(); tokio::spawn(async move { c.get_league(&l).await }) };
    let rosters_h = { let c = svc.client.clone(); let l = lid.clone(); tokio::spawn(async move { c.get_league_rosters(&l).await }) };
    let users_h = { let c = svc.client.clone(); let l = lid.clone(); tokio::spawn(async move { c.get_league_users(&l).await }) };

    let league = league_h.await??;
    let rosters = rosters_h.await??;
    let users = users_h.await??;

    svc.db.upsert_league(&league).await?;

    // Build and store roster → user mapping.
    let user_map: HashMap<_, _> = users
        .iter()
        .map(|u| (u.user_id.clone(), u.clone()))
        .collect();
    let members: Vec<_> = rosters
        .iter()
        .filter_map(|r| {
            let owner_id = r.owner_id.as_deref()?;
            let user = user_map.get(owner_id)?;
            Some((
                r.roster_id,
                owner_id.to_string(),
                user.username.clone(),
                Some(user.display_name.clone()),
            ))
        })
        .collect();
    svc.db.upsert_league_members(league_id, &members).await?;

    if verbosity.is_verbose() {
        println!(
            "  {} {} | {} | Superflex: {} | {} members",
            style("✓").green().bold(),
            style(&league.name).bold(),
            league.scoring_type(),
            if league.is_superflex() { style("yes").green().to_string() } else { "no".to_string() },
            members.len(),
        );
    }

    let nfl_state = svc.client.get_nfl_state().await?;
    let season = season_override.map(str::to_string).unwrap_or_else(|| nfl_state.season.clone());

    // League snapshot (rich per-roster view) + transaction history: independent
    // of stat-week caching below, so this runs even when every week is already
    // cached. Disk gets the latest snapshot only; `stats.db` accumulates history.
    let snapshot = svc.league_snapshot(league_id).await?;
    let snapshot_path = config.snapshot_path(league_id);
    if let Some(parent) = snapshot_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&snapshot_path, serde_json::to_vec_pretty(&snapshot)?).await?;
    let weeks: Vec<i32> = (1..=nfl_state.week.max(1)).collect();
    let txns_stored = svc.sync_transaction_history(league_id, &weeks).await?;
    if verbosity.is_verbose() {
        println!(
            "  {} league snapshot written to {} | {txns_stored} transaction(s) synced",
            style("✓").green().bold(),
            snapshot_path.display(),
        );
    }

    let already_cached = svc.db.cached_weeks(&season).await?;
    let to_fetch: Vec<i32> = (1..=18)
        .filter(|w| !already_cached.contains(w))
        .collect();

    if to_fetch.is_empty() {
        if verbosity.is_verbose() {
            println!("  {} All weeks already cached", style("✓").green().bold());
        }
        return Ok(0);
    }

    let pb = if verbosity.is_verbose() {
        let pb = ProgressBar::new(to_fetch.len() as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} weeks — {msg}")
                .unwrap()
                .progress_chars("=>-"),
        );
        Some(pb)
    } else {
        None
    };

    let mut synced = 0usize;
    for wk in &to_fetch {
        match svc.client.get_player_stats("regular", &season, *wk, "nfl").await {
            Ok(data) if !data.is_empty() => {
                svc.db.store_week(&season, *wk, &data).await?;
                synced += 1;
                if let Some(ref pb) = pb {
                    pb.set_message(format!("week {wk} ✓"));
                }
            }
            Ok(_) => {
                if let Some(ref pb) = pb {
                    pb.set_message(format!("week {wk} — no data yet"));
                }
            }
            Err(e) => {
                if let Some(ref pb) = pb {
                    pb.set_message(format!("week {wk} — {e}"));
                }
            }
        }
        if let Some(ref pb) = pb {
            pb.inc(1);
        }
    }
    if let Some(pb) = pb {
        pb.finish_with_message(format!("done — {synced} new weeks stored"));
    }
    Ok(synced)
}

pub(super) async fn cmd_sync(
    svc: &AnalysisService,
    config: &Config,
    league_id: &str,
    season_override: Option<&str>,
) -> Result<()> {
    println!("{}", style("Syncing league settings…").dim());
    let synced = do_sync(svc, config, league_id, season_override, SyncVerbosity::Verbose).await?;
    if synced > 0 {
        println!("  {} {synced} stat week(s) stored", style("✓").green().bold());
    }
    Ok(())
}

/// Auto-sync if the league data is older than `max_age_secs`. Silent unless new data is found.
pub(super) async fn maybe_sync(
    svc: &AnalysisService,
    config: &Config,
    league_id: &str,
    max_age_secs: i64,
) -> Result<()> {
    let stale = match svc.db.league_synced_at(league_id).await? {
        None => true,
        Some(synced_at) => {
            let age = chrono::Utc::now() - synced_at;
            age.num_seconds() > max_age_secs
        }
    };
    if stale {
        let synced = do_sync(svc, config, league_id, None, SyncVerbosity::Silent).await?;
        if synced > 0 {
            println!("{} synced {} new stat week(s)", style("↻").cyan(), synced);
        }
    }
    Ok(())
}

/// Resolve a numeric Sleeper player_id to its `full_name` — the live DOM has
/// no player_id to select on (plan.md Story 3.1), so every DOM-facing flow
/// must cross this lookup first.
pub(super) async fn resolve_player_name(client: &SleeperClient, player_id: &str) -> Result<String> {
    let players = client.get_all_players("nfl").await?;
    players
        .get(player_id)
        .and_then(|p| p.get("full_name"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| crate::error::Error::NotFound(format!(
            "No player found for id '{player_id}'. Run `sleeper search <name>` to find the right id."
        )))
}
