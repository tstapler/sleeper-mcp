//! Read-only analysis-facing commands: user/league lookup, standings,
//! matchups, waivers, lineup, trade, search, transactions, roster, and
//! local-DB status/sync-stats.
use console::style;
use indicatif::{ProgressBar, ProgressStyle};

use crate::analysis::AnalysisService;
use crate::client::SleeperClient;
use crate::config::Config;
use crate::db::StatsDb;
use crate::error::Result;

pub(super) async fn cmd_user(client: &SleeperClient, identifier: &str) -> Result<()> {
    let u = client.get_user(identifier).await?;
    println!(
        "{} ({})",
        style(&u.display_name).bold(),
        u.username.as_deref().unwrap_or("—")
    );
    println!("User ID: {}", u.user_id);
    Ok(())
}

pub(super) async fn cmd_leagues(client: &SleeperClient, user_id: &str, season: &str) -> Result<()> {
    let leagues = client.get_user_leagues(user_id, season, "nfl").await?;
    let header = format!("Leagues — {user_id} ({season})");
    println!("{}", style(&header).bold().underlined());
    println!(
        "{:<22} {:<30} {:>5}  {:<12} {:<10}",
        "League ID", "Name", "Teams", "Status", "Scoring"
    );
    println!("{}", "-".repeat(84));
    for lg in &leagues {
        println!(
            "{:<22} {:<30} {:>5}  {:<12} {:<10}",
            lg.league_id,
            &lg.name,
            lg.total_rosters,
            lg.status,
            lg.scoring_type(),
        );
    }
    Ok(())
}

pub(super) async fn cmd_standings(svc: &AnalysisService, league_id: &str) -> Result<()> {
    let data = svc.standings(league_id).await?;
    println!("{}", style(format!("Standings — {league_id}")).bold().underlined());
    println!("{:>3}  {:<20} {:>7}  {:>8}  {:>8}", "#", "Team", "W-L-T", "PF", "PA");
    println!("{}", "-".repeat(55));
    for t in &data {
        let wlt = format!(
            "{}-{}-{}",
            t["wins"].as_i64().unwrap_or(0),
            t["losses"].as_i64().unwrap_or(0),
            t["ties"].as_i64().unwrap_or(0)
        );
        println!(
            "{:>3}  {:<20} {:>7}  {:>8.2}  {:>8.2}",
            t["rank"].as_i64().unwrap_or(0),
            t["display_name"].as_str().unwrap_or("?"),
            wlt,
            t["fpts"].as_f64().unwrap_or(0.0),
            t["fpts_against"].as_f64().unwrap_or(0.0),
        );
    }
    Ok(())
}

pub(super) async fn cmd_matchups(svc: &AnalysisService, league_id: &str, week: i32) -> Result<()> {
    let data = svc.matchup_overview(league_id, week).await?;
    println!("{}", style(format!("Matchups — Week {week}")).bold().underlined());
    for m in &data {
        let t1 = &m["team1"];
        let t2 = &m["team2"];
        println!(
            "  {} {:.2} vs {:.2} {}",
            t1["display_name"].as_str().unwrap_or("?"),
            t1["points"].as_f64().unwrap_or(0.0),
            t2["points"].as_f64().unwrap_or(0.0),
            t2["display_name"].as_str().unwrap_or("?"),
        );
    }
    Ok(())
}

pub(super) async fn cmd_waivers(
    svc: &AnalysisService,
    league_id: &str,
    lookback_hours: i32,
    limit: usize,
) -> Result<()> {
    let data = svc.waiver_recommendations(league_id, lookback_hours, limit).await?;
    println!("{}", style("Top Available Trending Adds").bold().underlined());
    println!("{:<25} {:>4}  {:>5}  {:>6}  {}", "Name", "Pos", "Team", "Adds", "Status");
    println!("{}", "-".repeat(60));
    for p in data["top_available_trending_adds"].as_array().unwrap_or(&vec![]) {
        let inj = p["injury_status"].as_str().unwrap_or("");
        let status = if inj.is_empty() { "Active".to_string() } else { inj.to_string() };
        println!(
            "{:<25} {:>4}  {:>5}  {:>6}  {}",
            p["name"].as_str().unwrap_or("?"),
            p["position"].as_str().unwrap_or("?"),
            p["team"].as_str().unwrap_or("?"),
            p["add_count"].as_i64().unwrap_or(0),
            status,
        );
    }
    Ok(())
}

pub(super) async fn cmd_lineup(
    svc: &AnalysisService,
    league_id: &str,
    roster_id: i32,
    week: i32,
) -> Result<()> {
    let data = svc.start_sit(league_id, roster_id, week).await?;
    println!("{}", style(format!("Start/Sit — Week {week}")).bold().underlined());
    println!("\n{}", style("Recommended Lineup").green().bold());
    for slot in data["recommended_lineup"].as_array().unwrap_or(&vec![]) {
        let slot_name = slot["slot"].as_str().unwrap_or("?");
        match slot["player_id"].as_str() {
            Some(pid) => {
                let pts = slot["projected_points"].as_f64().unwrap_or(0.0);
                println!("  {slot_name:<11} {pid} — proj {pts:.1}");
            }
            None => println!("  {slot_name:<11} (no eligible player)"),
        }
    }
    println!("\n{}", style("Bench").dim());
    for p in data["recommended_bench"].as_array().unwrap_or(&vec![]) {
        println!(
            "  {} ({}) — proj {:.1}",
            p["player_id"].as_str().unwrap_or("?"),
            p["position"].as_str().unwrap_or("?"),
            p["projected_points"].as_f64().unwrap_or(0.0),
        );
    }
    let excluded = data["excluded_unavailable"].as_array().cloned().unwrap_or_default();
    if !excluded.is_empty() {
        println!("\n{}", style("Excluded (Out/IR/PUP/inactive)").red());
        for p in &excluded {
            println!(
                "  {} ({}) — {}",
                p["player_id"].as_str().unwrap_or("?"),
                p["position"].as_str().unwrap_or("?"),
                p["injury_status"].as_str().unwrap_or(p["status"].as_str().unwrap_or("?")),
            );
        }
    }
    println!("\n(Use `sleeper search <player_id>` to resolve IDs to names)");
    Ok(())
}

pub(super) async fn cmd_trade(
    svc: &AnalysisService,
    give: &[String],
    receive: &[String],
    season: Option<&str>,
    week: Option<i32>,
) -> Result<()> {
    let data = svc.analyze_trade(give, receive, season, week).await?;
    println!("{}", style("Trade Analysis").bold().underlined());

    let print_side = |label: &str, players: &serde_json::Value| {
        println!("  {label}:");
        for p in players.as_array().unwrap_or(&vec![]) {
            let ppg = p["ppg"].as_f64().unwrap_or(0.0);
            let trend = p["trend"].as_str().unwrap_or("→");
            let gp = p["games_played"].as_i64().unwrap_or(0);
            let ppg_str = if ppg > 0.0 { format!("{ppg:.1} ppg ({gp}g) {trend}") } else { "no stats".to_string() };
            println!(
                "    {} {} ({})  {}",
                p["name"].as_str().unwrap_or(p["player_id"].as_str().unwrap_or("?")),
                p["position"].as_str().unwrap_or("?"),
                p["team"].as_str().unwrap_or("FA"),
                ppg_str,
            );
        }
    };

    print_side("You give", &data["you_give"]);
    print_side("You receive", &data["you_receive"]);

    println!(
        "\nGiving PPG:    {:.1}",
        data["giving_ppg_total"].as_f64().unwrap_or(0.0)
    );
    println!(
        "Receiving PPG: {:.1}",
        data["receiving_ppg_total"].as_f64().unwrap_or(0.0)
    );
    println!(
        "Difference:    {:+.1}",
        data["ppg_difference"].as_f64().unwrap_or(0.0)
    );
    let verdict = data["verdict"].as_str().unwrap_or("?");
    let styled = if verdict.contains("Favorable") {
        style(verdict).green().bold().to_string()
    } else if verdict.contains("Unfavorable") {
        style(verdict).red().bold().to_string()
    } else {
        style(verdict).yellow().bold().to_string()
    };
    println!("Verdict: {styled}");
    println!("Based on: {}", data["based_on"].as_str().unwrap_or("?"));
    Ok(())
}

pub(super) async fn cmd_search(
    svc: &AnalysisService,
    query: &str,
    position: Option<&str>,
    limit: usize,
) -> Result<()> {
    let results = svc.search_players(query, position, limit).await?;
    for p in &results {
        println!(
            "{} — {} {} {}{}",
            p["player_id"].as_str().unwrap_or("?"),
            p["full_name"].as_str().unwrap_or("?"),
            p["position"].as_str().unwrap_or("?"),
            p["team"].as_str().unwrap_or("?"),
            p["injury_status"]
                .as_str()
                .map(|s| format!(" [{s}]"))
                .unwrap_or_default(),
        );
    }
    Ok(())
}

pub(super) async fn cmd_transactions(
    svc: &AnalysisService,
    league_id: &str,
    week: i32,
    kind: Option<&str>,
) -> Result<()> {
    let txns = svc.recent_transactions(league_id, week, kind).await?;
    println!("{}", style(format!("Transactions — Week {week}")).bold().underlined());
    for t in &txns {
        println!(
            "[{}] {} — {}",
            t["type"].as_str().unwrap_or("?"),
            t["transaction_id"].as_str().unwrap_or("?"),
            t["status"].as_str().unwrap_or("?"),
        );
    }
    Ok(())
}

pub(super) async fn cmd_roster(svc: &AnalysisService, league_id: &str, roster_id: i32) -> Result<()> {
    let data = svc.roster_with_names(league_id, roster_id).await?;
    println!(
        "{} — {}",
        style(data["owner"].as_str().unwrap_or("?")).bold(),
        data["record"]
    );
    println!("\n{}", style("Players").underlined());
    for p in data["players"].as_array().unwrap_or(&vec![]) {
        let inj = p["injury_status"]
            .as_str()
            .map(|s| format!(" [{s}]"))
            .unwrap_or_default();
        println!(
            "  {} {} {} {}{}",
            p["position"].as_str().unwrap_or("?"),
            p["name"].as_str().unwrap_or("?"),
            p["team"].as_str().unwrap_or("?"),
            p["status"].as_str().unwrap_or("?"),
            inj,
        );
    }
    Ok(())
}

pub(super) async fn cmd_analyze(svc: &AnalysisService, league_id: &str, roster_id: i32) -> Result<()> {
    let data = svc.analyze_roster_needs(league_id, roster_id).await?;
    println!("{}", style(format!("Roster Analysis — {}", data["owner"].as_str().unwrap_or("?"))).bold().underlined());

    if let Some(strengths) = data["strengths"].as_array() {
        if !strengths.is_empty() {
            println!("\n{}", style("Strengths").green().bold());
            for s in strengths {
                println!("  ✓ {}", s.as_str().unwrap_or("?"));
            }
        }
    }
    if let Some(weaknesses) = data["weaknesses"].as_array() {
        if !weaknesses.is_empty() {
            println!("\n{}", style("Weaknesses").red().bold());
            for w in weaknesses {
                println!("  ✗ {}", w.as_str().unwrap_or("?"));
            }
        }
    }
    if let Some(priorities) = data["suggested_priorities"].as_array() {
        if !priorities.is_empty() {
            println!("\n{}", style("Suggested Priorities").yellow().bold());
            for p in priorities {
                println!("  → {}", p.as_str().unwrap_or("?"));
            }
        }
    }
    Ok(())
}

pub(super) async fn cmd_available(
    svc: &AnalysisService,
    league_id: &str,
    position: Option<&str>,
    season: Option<&str>,
    limit: usize,
) -> Result<()> {
    let players = svc.available_players(league_id, position, season, limit).await?;
    println!("{}", style("Available Players").bold().underlined());
    println!("{:<25} {:>4}  {:>5}  {:>8}  {}", "Name", "Pos", "Team", "Pts", "Status");
    println!("{}", "-".repeat(60));
    for p in &players {
        let inj = p["injury_status"].as_str().unwrap_or("");
        let status = if inj.is_empty() { "Active" } else { inj };
        println!(
            "{:<25} {:>4}  {:>5}  {:>8.2}  {}",
            p["name"].as_str().unwrap_or("?"),
            p["position"].as_str().unwrap_or("?"),
            p["team"].as_str().unwrap_or("?"),
            p["pts"].as_f64().unwrap_or(0.0),
            status,
        );
    }
    Ok(())
}

pub(super) async fn cmd_sync_stats(
    client: &SleeperClient,
    config: &Config,
    season: &str,
    weeks: Option<Vec<i32>>,
) -> Result<()> {
    let db = StatsDb::open(&config.stats_db_path()).await?;

    let to_fetch: Vec<i32> = if let Some(w) = weeks {
        w
    } else {
        (1..=17)
            .filter(|_wk| true)
            .collect()
    };

    let pb = ProgressBar::new(to_fetch.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} weeks — {msg}")
            .unwrap()
            .progress_chars("=>-"),
    );
    pb.set_message(format!("Syncing {season}"));

    for wk in &to_fetch {
        match client
            .get_player_stats("regular", season, *wk, "nfl")
            .await
        {
            Ok(data) if !data.is_empty() => {
                let rows = db.store_week(season, *wk, &data).await?;
                pb.set_message(format!("Week {wk} — {rows} rows"));
            }
            Ok(_) => {
                pb.set_message(format!("Week {wk} — empty, skipping"));
            }
            Err(e) => {
                pb.set_message(format!("Week {wk} — error: {e}"));
            }
        }
        pb.inc(1);
    }
    pb.finish_with_message("Done");
    Ok(())
}

pub(super) async fn cmd_db_status(config: &Config) -> Result<()> {
    let db = StatsDb::open(&config.stats_db_path()).await?;
    let summary = db.summary().await?;
    println!("{}", style("Local Stats DB").bold().underlined());
    println!("Rows: {}", summary["total_stat_rows"]);
    for s in summary["seasons"].as_array().unwrap_or(&vec![]) {
        println!(
            "  {} — {} weeks cached",
            s["season"].as_str().unwrap_or("?"),
            s["weeks_cached"].as_i64().unwrap_or(0)
        );
    }
    Ok(())
}
