use clap::{Parser, Subcommand};
use console::style;
use indicatif::{ProgressBar, ProgressStyle};

use crate::analysis::AnalysisService;
use crate::config::Config;
use crate::db::StatsDb;
use crate::client::SleeperClient;
use crate::error::Result;

#[derive(Parser)]
#[command(name = "sleeper", about = "Sleeper fantasy sports CLI")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Look up a Sleeper user profile
    User { identifier: String },

    /// List all leagues for a user
    Leagues {
        user_id: String,
        #[arg(short, long)]
        season: String,
    },

    /// Show current league standings
    Standings { league_id: String },

    /// Show matchups for a week
    Matchups {
        league_id: String,
        #[arg(short, long)]
        week: i32,
    },

    /// Waiver wire recommendations
    Waivers {
        league_id: String,
        #[arg(long, default_value = "48")]
        lookback_hours: i32,
        #[arg(short, long, default_value = "15")]
        limit: usize,
    },

    /// Start/sit lineup recommendations
    Lineup {
        league_id: String,
        /// Roster ID, username, or display name
        #[arg(short, long)]
        roster_id: String,
        #[arg(short, long)]
        week: i32,
    },

    /// Analyze a potential trade
    Trade {
        #[arg(long, value_delimiter = ',')]
        give: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        receive: Vec<String>,
        #[arg(long)]
        season: Option<String>,
        #[arg(long)]
        week: Option<i32>,
    },

    /// Search for NFL players by name
    Search {
        query: String,
        #[arg(short, long)]
        position: Option<String>,
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },

    /// Show transactions for a week
    Transactions {
        league_id: String,
        #[arg(short, long)]
        week: i32,
        #[arg(short, long)]
        kind: Option<String>,
    },

    /// Show roster details for a team
    Roster {
        league_id: String,
        /// Roster ID, username, or display name
        #[arg(short, long)]
        roster_id: String,
    },

    /// Analyze roster needs and strengths
    Analyze {
        league_id: String,
        /// Roster ID, username, or display name
        #[arg(short, long)]
        roster_id: String,
    },

    /// Show unrostered players ranked by fantasy points
    Available {
        league_id: String,
        #[arg(short, long)]
        position: Option<String>,
        #[arg(short, long)]
        season: Option<String>,
        #[arg(short, long, default_value = "25")]
        limit: usize,
    },

    /// Sync historical stats to local SQLite database
    SyncStats {
        season: String,
        #[arg(long, value_delimiter = ',')]
        weeks: Option<Vec<i32>>,
    },

    /// Show local database status
    DbStatus,

    /// Dynasty roster report: tier rankings, trade bait, and cut list
    Dynasty {
        league_id: String,
        /// Roster ID, username, or display name
        #[arg(short, long)]
        roster_id: String,
    },

    /// Dynasty waiver scouting: young unrostered players ranked by relevance (search_rank), not just age
    DynastyScout {
        league_id: String,
        #[arg(short, long)]
        position: Option<String>,
        #[arg(long, default_value = "26")]
        max_age: u32,
        #[arg(short, long, default_value = "30")]
        limit: usize,
    },

    /// Find win-win trade scenarios with every other team in the league
    FindTrades {
        league_id: String,
        /// Roster ID, username, or display name
        #[arg(short, long)]
        roster_id: String,
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },

    /// Fetch league settings from Sleeper and store locally
    SyncLeague { league_id: String },

    /// Show locally stored league settings (roster positions, scoring, superflex)
    LeagueInfo { league_id: String },

    /// Sync everything: league settings + player stats for the current season (auto-detected)
    Sync {
        /// League ID to sync settings for
        league_id: String,
        /// Override the auto-detected season (e.g. 2025)
        #[arg(long)]
        season: Option<String>,
    },
}

/// Resolve a username/display_name/user_id/numeric string to a roster_id.
async fn resolve_rid(db: &crate::db::StatsDb, league_id: &str, identifier: &str) -> crate::error::Result<i32> {
    db.resolve_roster_id(league_id, identifier)
        .await?
        .ok_or_else(|| crate::error::Error::NotFound(format!(
            "No roster found for '{}' in league {}. Run `sleeper sync {}` first if you haven't.",
            identifier, league_id, league_id
        )))
}

impl Commands {
    /// Returns the league_id for commands that are scoped to a specific league, or None.
    fn league_id(&self) -> Option<&str> {
        match self {
            Commands::Standings { league_id }
            | Commands::Matchups { league_id, .. }
            | Commands::Waivers { league_id, .. }
            | Commands::Lineup { league_id, .. }
            | Commands::Transactions { league_id, .. }
            | Commands::Roster { league_id, .. }
            | Commands::Analyze { league_id, .. }
            | Commands::Available { league_id, .. }
            | Commands::Dynasty { league_id, .. }
            | Commands::DynastyScout { league_id, .. }
            | Commands::FindTrades { league_id, .. }
            | Commands::SyncLeague { league_id }
            | Commands::LeagueInfo { league_id }
            | Commands::Sync { league_id, .. } => Some(league_id),
            _ => None,
        }
    }
}

pub async fn run(cli: Cli) -> Result<()> {
    let config = Config::default();
    let client = SleeperClient::new(&config).await?;
    let db = StatsDb::open(&config.stats_db_path()).await?;
    let svc = AnalysisService::new(client.clone(), db);

    // For every command that operates on a specific league, auto-refresh stale data (>1h old).
    // Commands without a league_id (user, search, nfl-state…) rely on the HTTP cache TTL instead.
    const STALE_SECS: i64 = 3600;
    if let Some(league_id) = cli.command.league_id() {
        maybe_sync(&client, &svc, league_id, STALE_SECS).await?;
    }

    match cli.command {
        Commands::User { identifier } => cmd_user(&client, &identifier).await?,

        Commands::Leagues { user_id, season } => {
            cmd_leagues(&client, &user_id, &season).await?
        }

        Commands::Standings { league_id } => cmd_standings(&svc, &league_id).await?,

        Commands::Matchups { league_id, week } => {
            cmd_matchups(&svc, &league_id, week).await?
        }

        Commands::Waivers { league_id, lookback_hours, limit } => {
            cmd_waivers(&svc, &league_id, lookback_hours, limit).await?
        }

        Commands::Lineup { league_id, roster_id, week } => {
            let rid = resolve_rid(&svc.db, &league_id, &roster_id).await?;
            cmd_lineup(&svc, &league_id, rid, week).await?
        }

        Commands::Trade { give, receive, season, week } => {
            cmd_trade(&svc, &give, &receive, season.as_deref(), week).await?
        }

        Commands::Search { query, position, limit } => {
            cmd_search(&svc, &query, position.as_deref(), limit).await?
        }

        Commands::Transactions { league_id, week, kind } => {
            cmd_transactions(&svc, &league_id, week, kind.as_deref()).await?
        }

        Commands::Roster { league_id, roster_id } => {
            let rid = resolve_rid(&svc.db, &league_id, &roster_id).await?;
            cmd_roster(&svc, &league_id, rid).await?
        }

        Commands::Analyze { league_id, roster_id } => {
            let rid = resolve_rid(&svc.db, &league_id, &roster_id).await?;
            cmd_analyze(&svc, &league_id, rid).await?
        }

        Commands::Available { league_id, position, season, limit } => {
            cmd_available(&svc, &league_id, position.as_deref(), season.as_deref(), limit)
                .await?
        }

        Commands::SyncStats { season, weeks } => {
            cmd_sync_stats(&client, &config, &season, weeks).await?
        }

        Commands::DbStatus => cmd_db_status(&config).await?,

        Commands::Dynasty { league_id, roster_id } => {
            let rid = resolve_rid(&svc.db, &league_id, &roster_id).await?;
            cmd_dynasty(&svc, &league_id, rid).await?
        }

        Commands::DynastyScout { league_id, position, max_age, limit } => {
            cmd_dynasty_scout(&svc, &league_id, position.as_deref(), max_age, limit).await?
        }

        Commands::FindTrades { league_id, roster_id, limit } => {
            let rid = resolve_rid(&svc.db, &league_id, &roster_id).await?;
            cmd_find_trades(&svc, &league_id, rid, limit).await?
        }

        Commands::SyncLeague { league_id } => {
            cmd_sync_league(&client, &*svc.db, &league_id).await?
        }

        Commands::LeagueInfo { league_id } => {
            cmd_league_info(&*svc.db, &league_id).await?
        }

        Commands::Sync { league_id, season } => {
            cmd_sync(&client, &svc, &league_id, season.as_deref()).await?
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Command implementations
// ---------------------------------------------------------------------------

async fn cmd_user(client: &SleeperClient, identifier: &str) -> Result<()> {
    let u = client.get_user(identifier).await?;
    println!(
        "{} ({})",
        style(&u.display_name).bold(),
        u.username.as_deref().unwrap_or("—")
    );
    println!("User ID: {}", u.user_id);
    Ok(())
}

async fn cmd_leagues(client: &SleeperClient, user_id: &str, season: &str) -> Result<()> {
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

async fn cmd_standings(svc: &AnalysisService, league_id: &str) -> Result<()> {
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

async fn cmd_matchups(svc: &AnalysisService, league_id: &str, week: i32) -> Result<()> {
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

async fn cmd_waivers(
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

async fn cmd_lineup(
    svc: &AnalysisService,
    league_id: &str,
    roster_id: i32,
    week: i32,
) -> Result<()> {
    let data = svc.start_sit(league_id, roster_id, week).await?;
    println!("{}", style(format!("Start/Sit — Week {week}")).bold().underlined());
    println!("\n{}", style("Recommended Starters").green().bold());
    for pid in data["recommended_starters"].as_array().unwrap_or(&vec![]) {
        // Look up the ranked entry for projected points
        let pts = data["all_players_ranked"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .find(|p| p["player_id"] == *pid)
            .and_then(|p| p["projected_points"].as_f64())
            .unwrap_or(0.0);
        println!("  {} — proj {:.1}", pid.as_str().unwrap_or("?"), pts);
    }
    println!("\n{}", style("Bench").dim());
    for pid in data["recommended_bench"].as_array().unwrap_or(&vec![]) {
        let pts = data["all_players_ranked"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .find(|p| p["player_id"] == *pid)
            .and_then(|p| p["projected_points"].as_f64())
            .unwrap_or(0.0);
        println!("  {} — proj {:.1}", pid.as_str().unwrap_or("?"), pts);
    }
    println!("\n(Use `sleeper search <player_id>` to resolve IDs to names)");
    Ok(())
}

async fn cmd_trade(
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

async fn cmd_search(
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

async fn cmd_transactions(
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

async fn cmd_roster(svc: &AnalysisService, league_id: &str, roster_id: i32) -> Result<()> {
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

async fn cmd_analyze(svc: &AnalysisService, league_id: &str, roster_id: i32) -> Result<()> {
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

async fn cmd_available(
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

async fn cmd_sync_stats(
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

async fn cmd_db_status(config: &Config) -> Result<()> {
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

async fn cmd_dynasty(svc: &AnalysisService, league_id: &str, roster_id: i32) -> Result<()> {
    let data = svc.dynasty_report(league_id, roster_id).await?;

    println!(
        "{}\n",
        style(format!(
            "Dynasty Report — {} (roster {})",
            data["owner"].as_str().unwrap_or("?"),
            roster_id
        ))
        .bold()
        .underlined()
    );

    let summary = &data["summary"];
    println!(
        "Roster: {} players  |  {} cornerstones  {}  keepers  {}  trade  {}  cut\n",
        summary["total_roster"].as_i64().unwrap_or(0),
        style(summary["cornerstones"].as_i64().unwrap_or(0)).green().bold(),
        style(summary["keepers"].as_i64().unwrap_or(0)).cyan().bold(),
        style(summary["trade_bait"].as_i64().unwrap_or(0)).yellow().bold(),
        style(summary["cut_candidates"].as_i64().unwrap_or(0)).red().bold(),
    );

    // Helper: print one dynasty tier table with ppg + trend columns.
    fn print_dynasty_tier(
        header: &str,
        hdr_style: impl Fn() -> console::StyledObject<String>,
        players: &[serde_json::Value],
        show_status: bool,
    ) {
        if players.is_empty() { return; }
        println!("{}", hdr_style());
        if show_status {
            println!("{:<25} {:>4}  {:>5}  {:>3}  {:>5}  {:>4}  {}", "Name", "Pos", "Team", "Age", "PPG", "Trnd", "Status");
            println!("{}", "-".repeat(72));
        } else {
            println!("{:<25} {:>4}  {:>5}  {:>3}  {:>5}  {:>4}", "Name", "Pos", "Team", "Age", "PPG", "Trnd");
            println!("{}", "-".repeat(56));
        }
        for p in players {
            let ppg = p["ppg"].as_f64().unwrap_or(0.0);
            let trend = p["trend"].as_str().unwrap_or("→");
            let risk = if p["risk_flag"].as_bool().unwrap_or(false) { "!" } else { "" };
            let inj = p["injury_status"].as_str().unwrap_or("");
            let status_raw = if inj.is_empty() { p["status"].as_str().unwrap_or("Active") } else { inj };
            let status_str = format!("{}{}", risk, status_raw);
            let ppg_str = if ppg > 0.0 { format!("{ppg:>5.1}") } else { "    -".to_string() };
            if show_status {
                println!(
                    "{:<25} {:>4}  {:>5}  {:>3}  {}  {:>4}  {}",
                    p["name"].as_str().unwrap_or("?"),
                    p["position"].as_str().unwrap_or("?"),
                    p["team"].as_str().unwrap_or("FA"),
                    p["age"].as_i64().map(|a| a.to_string()).unwrap_or("?".to_string()),
                    ppg_str, trend, status_str,
                );
            } else {
                println!(
                    "{:<25} {:>4}  {:>5}  {:>3}  {}  {:>4}",
                    p["name"].as_str().unwrap_or("?"),
                    p["position"].as_str().unwrap_or("?"),
                    p["team"].as_str().unwrap_or("FA"),
                    p["age"].as_i64().map(|a| a.to_string()).unwrap_or("?".to_string()),
                    ppg_str, trend,
                );
            }
            // Print news items indented under the player.
            if let Some(news) = p["news"].as_array() {
                for item in news.iter().take(2) {
                    let src = item["source"].as_str().unwrap_or("?");
                    let hl = item["headline"].as_str().unwrap_or("");
                    let body = item["body"].as_str().unwrap_or("");
                    let date = item["date"].as_str().unwrap_or("");
                    println!("  {} [{}] {}", style(format!("[{src}]")).dim(), date, style(hl).italic());
                    if !body.is_empty() {
                        // Wrap body to 90 chars
                        let wrapped = textwrap(body, 90);
                        for line in wrapped {
                            println!("    {}", style(line).dim());
                        }
                    }
                }
            }
        }
        println!();
        let _ = header; // suppress unused warning; label is in hdr_style closure
    }

    let t1 = data["tier1_cornerstones"].as_array().cloned().unwrap_or_default();
    let t2 = data["tier2_keepers"].as_array().cloned().unwrap_or_default();
    let t3 = data["tier3_trade_bait"].as_array().cloned().unwrap_or_default();
    let t4 = data["tier4_cut_list"].as_array().cloned().unwrap_or_default();

    print_dynasty_tier("t1", || style("TIER 1 — CORNERSTONES (never trade)".to_string()).green().bold(), &t1, true);
    print_dynasty_tier("t2", || style("TIER 2 — KEEPERS (hold)".to_string()).cyan().bold(), &t2, true);
    print_dynasty_tier("t3", || style("TIER 3 — TRADE BAIT (sell now)".to_string()).yellow().bold(), &t3, true);
    print_dynasty_tier("t4", || style("TIER 4 — CUT LIST (drop or release)".to_string()).red().bold(), &t4, false);

    println!("{}", style("TOP DYNASTY WAIVER TARGETS (youngest unrostered)".to_string()).magenta().bold());
    println!("{:<25} {:>4}  {:>5}  {:>3}  {:>5}  {:>4}  {}", "Name", "Pos", "Team", "Age", "PPG", "Trnd", "Status");
    println!("{}", "-".repeat(72));
    for p in data["dynasty_waiver_targets"].as_array().unwrap_or(&vec![]).iter().take(15) {
        let ppg = p["ppg"].as_f64().unwrap_or(0.0);
        let trend = p["trend"].as_str().unwrap_or("→");
        let inj = p["injury_status"].as_str().unwrap_or("");
        let status = if inj.is_empty() { "Active" } else { inj };
        let ppg_str = if ppg > 0.0 { format!("{ppg:>5.1}") } else { "    -".to_string() };
        println!(
            "{:<25} {:>4}  {:>5}  {:>3}  {}  {:>4}  {}",
            p["name"].as_str().unwrap_or("?"),
            p["position"].as_str().unwrap_or("?"),
            p["team"].as_str().unwrap_or("FA"),
            p["age"].as_i64().map(|a| a.to_string()).unwrap_or("?".to_string()),
            ppg_str, trend, status,
        );
    }
    Ok(())
}

async fn cmd_dynasty_scout(
    svc: &AnalysisService,
    league_id: &str,
    position: Option<&str>,
    max_age: u32,
    limit: usize,
) -> Result<()> {
    let players = svc.dynasty_targets(league_id, position, max_age, limit).await?;
    let header = format!(
        "Dynasty Scouting — unrostered players age ≤{}{}",
        max_age,
        position.map(|p| format!(", {p}")).unwrap_or_default()
    );
    println!("{}", style(&header).bold().underlined());
    println!("{:<25} {:>4}  {:>5}  {:>3}  {:>3}yr  {}", "Name", "Pos", "Team", "Age", "Exp", "Status");
    println!("{}", "-".repeat(65));
    for p in &players {
        let inj = p["injury_status"].as_str().unwrap_or("");
        let status = if inj.is_empty() { "Active" } else { inj };
        println!(
            "{:<25} {:>4}  {:>5}  {:>3}  {:>4}  {}",
            p["name"].as_str().unwrap_or("?"),
            p["position"].as_str().unwrap_or("?"),
            p["team"].as_str().unwrap_or("FA"),
            p["age"].as_i64().map(|a| a.to_string()).unwrap_or("?".to_string()),
            p["years_exp"].as_i64().map(|y| y.to_string()).unwrap_or("?".to_string()),
            status,
        );
    }
    Ok(())
}

async fn cmd_find_trades(
    svc: &AnalysisService,
    league_id: &str,
    roster_id: i32,
    limit: usize,
) -> Result<()> {
    let data = svc.find_win_win_trades(league_id, roster_id, limit).await?;

    let my_needs = data["my_needs"].as_array().cloned().unwrap_or_default();
    let offerables = data["my_offerables"].as_array().cloned().unwrap_or_default();
    let proposals = data["trade_proposals"].as_array().cloned().unwrap_or_default();

    println!("{}", style("Win-Win Trade Finder").bold().underlined());

    if my_needs.is_empty() {
        println!("\n{}", style("Your roster is well-balanced — no urgent positional needs.").dim());
    } else {
        let need_str: Vec<_> = my_needs.iter().filter_map(|n| n.as_str()).collect();
        println!("\nYour needs:    {}", style(need_str.join(", ")).yellow().bold());
    }

    if !offerables.is_empty() {
        let offer_str: Vec<_> = offerables
            .iter()
            .take(5)
            .filter_map(|p| p["name"].as_str())
            .collect();
        println!("You can offer: {}", style(offer_str.join(", ")).dim());
    }

    if proposals.is_empty() {
        println!("\n{}", style("No win-win opportunities found with current rosters.").yellow());
        return Ok(());
    }

    println!("\n{} trade proposals found:\n", proposals.len());

    for (i, p) in proposals.iter().enumerate() {
        let partner = p["partner"].as_str().unwrap_or("?");
        let score = p["win_win_score"].as_i64().unwrap_or(0);
        let empty = vec![];
        let their_needs: Vec<_> = p["their_needs"].as_array()
            .unwrap_or(&empty)
            .iter().filter_map(|n| n.as_str()).collect();

        println!(
            "{}  {} {}  (score: {})",
            style(format!("{:>2}.", i + 1)).dim(),
            style(partner).bold().cyan(),
            if their_needs.is_empty() { String::new() } else { format!("— needs {}", their_needs.join(", ")) },
            style(score).bold(),
        );

        let fmt_player = |p: &serde_json::Value| {
            let ppg = p["ppg"].as_f64().unwrap_or(0.0);
            let trend = p["trend"].as_str().unwrap_or("→");
            let ppg_str = if ppg > 0.0 { format!("{ppg:.1}ppg {trend}") } else { "-".to_string() };
            format!(
                "      {:<22} {:>3}  {}  tier {}  {}",
                p["name"].as_str().unwrap_or("?"),
                p["position"].as_str().unwrap_or("?"),
                p["team"].as_str().unwrap_or("FA"),
                p["dynasty_tier"].as_i64().unwrap_or(0),
                ppg_str,
            )
        };

        println!("    {}", style("You give:").red());
        for player in p["you_give"].as_array().unwrap_or(&vec![]) {
            println!("{}", fmt_player(player));
        }

        println!("    {}", style("You receive:").green());
        for player in p["you_receive"].as_array().unwrap_or(&vec![]) {
            println!("{}", fmt_player(player));
        }
        println!();
    }
    Ok(())
}

async fn cmd_sync_league(
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

async fn cmd_league_info(db: &StatsDb, league_id: &str) -> Result<()> {
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

/// Inner sync: runs silently when verbose=false, with progress output when verbose=true.
/// Returns the number of newly stored stat weeks.
async fn do_sync(
    client: &SleeperClient,
    svc: &AnalysisService,
    league_id: &str,
    season_override: Option<&str>,
    verbose: bool,
) -> Result<usize> {
    // Fetch league settings + rosters + users in parallel.
    let lid = league_id.to_string();
    let league_h = { let c = client.clone(); let l = lid.clone(); tokio::spawn(async move { c.get_league(&l).await }) };
    let rosters_h = { let c = client.clone(); let l = lid.clone(); tokio::spawn(async move { c.get_league_rosters(&l).await }) };
    let users_h = { let c = client.clone(); let l = lid.clone(); tokio::spawn(async move { c.get_league_users(&l).await }) };

    let league = league_h.await??;
    let rosters = rosters_h.await??;
    let users = users_h.await??;

    svc.db.upsert_league(&league).await?;

    // Build and store roster → user mapping.
    let user_map: std::collections::HashMap<_, _> = users
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

    if verbose {
        println!(
            "  {} {} | {} | Superflex: {} | {} members",
            style("✓").green().bold(),
            style(&league.name).bold(),
            league.scoring_type(),
            if league.is_superflex() { style("yes").green().to_string() } else { "no".to_string() },
            members.len(),
        );
    }

    let season = if let Some(s) = season_override {
        s.to_string()
    } else {
        client.get_nfl_state().await?.season.clone()
    };

    let already_cached = svc.db.cached_weeks(&season).await?;
    let to_fetch: Vec<i32> = (1..=18)
        .filter(|w| !already_cached.contains(w))
        .collect();

    if to_fetch.is_empty() {
        if verbose {
            println!("  {} All weeks already cached", style("✓").green().bold());
        }
        return Ok(0);
    }

    let pb = if verbose {
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
        match client.get_player_stats("regular", &season, *wk, "nfl").await {
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

async fn cmd_sync(
    client: &SleeperClient,
    svc: &AnalysisService,
    league_id: &str,
    season_override: Option<&str>,
) -> Result<()> {
    println!("{}", style("Syncing league settings…").dim());
    let synced = do_sync(client, svc, league_id, season_override, true).await?;
    if synced > 0 {
        println!("  {} {synced} stat week(s) stored", style("✓").green().bold());
    }
    Ok(())
}

/// Auto-sync if the league data is older than `max_age_secs`. Silent unless new data is found.
async fn maybe_sync(
    client: &SleeperClient,
    svc: &AnalysisService,
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
        let synced = do_sync(client, svc, league_id, None, false).await?;
        if synced > 0 {
            println!("{} synced {} new stat week(s)", style("↻").cyan(), synced);
        }
    }
    Ok(())
}

fn textwrap(s: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in s.split_whitespace() {
        if !current.is_empty() && current.len() + 1 + word.len() > width {
            lines.push(current.trim().to_string());
            current = word.to_string();
        } else {
            if !current.is_empty() { current.push(' '); }
            current.push_str(word);
        }
    }
    if !current.is_empty() { lines.push(current); }
    lines
}
