use clap::{Parser, Subcommand};
use std::path::PathBuf;

use crate::analysis::AnalysisService;
use crate::config::Config;
use crate::db::StatsDb;
use crate::client::SleeperClient;
use crate::error::Result;

pub mod browser;
mod dynasty_cmds;
mod roster_cmds;
mod sync;

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

    /// At-a-glance view of every roster in the league: owner, record, and
    /// position-group counts (so IDP/skill-position depth is comparable
    /// across teams at a glance, not just for one roster). Also writes the
    /// full resolved JSON to `<data_dir>/snapshots/<league_id>.json` and
    /// appends a row per roster to `stats.db` for `sqlite3`-queryable history.
    #[command(name = "league-snapshot")]
    LeagueSnapshot { league_id: String },

    /// Sync everything: league settings + player stats for the current season (auto-detected)
    Sync {
        /// League ID to sync settings for
        league_id: String,
        /// Override the auto-detected season (e.g. 2025)
        #[arg(long)]
        season: Option<String>,
    },

    /// Authenticated browser session against sleeper.com — for actions the read-only API
    /// can't do (waiver claims, lineups, trades)
    Browser {
        #[command(subcommand)]
        action: BrowserAction,
    },

    /// Waiver claims through the browser backend (live-verified selectors,
    /// plan.md Story 3.1/3.2 — see `sleeper waiver claim --help`)
    Waiver {
        #[command(subcommand)]
        action: WaiverAction,
    },

    /// Lineup changes through the browser backend (live-verified selectors,
    /// plan.md Epic 4 lineup mechanism spot-check). `--out`/`--in` take
    /// numeric Sleeper player IDs rather than an abstract slot name: the
    /// live UI swaps by clicking the currently-started player's row then
    /// the bench replacement's row, not by an addressable slot.
    #[command(name = "lineup-set")]
    LineupSet {
        #[arg(long)]
        league: String,
        /// Player_id currently in the starting slot to bench.
        #[arg(long)]
        out: String,
        /// Player_id on the bench to start in their place.
        #[arg(long = "in")]
        in_: String,
    },

    /// Trade proposals through the browser backend (Epic 4 — STUB ONLY, cut
    /// per appetite rule: launches via seam then prints disabled triage to
    /// stderr, exits non-zero; performs no DOM automation)
    #[command(name = "trade-propose")]
    TradePropose {
        #[arg(long)]
        league: String,
        #[arg(long)]
        give: String,
        #[arg(long)]
        get: String,
    },
}

#[derive(Subcommand)]
pub enum BrowserAction {
    /// Open a Chrome window and wait while you log into sleeper.com by hand (including any
    /// CAPTCHA) — the session persists for future runs once you're in
    Login {
        /// Explicit opt-in: fall back to system Chrome (CfT resolver pending
        /// in Story 2.1; never silent)
        #[arg(long)]
        system_chrome: bool,
        /// Explicit CfT Chrome binary path (Story 2.1 resolver pending)
        #[arg(long)]
        chrome_binary: Option<PathBuf>,
    },
    /// Check whether the persisted Chrome profile has an active Sleeper login
    Status {
        /// Escape hatch: run the same is_logged_in verdict path headed
        /// (Story 1.4 AC5 agreement gate)
        #[arg(long)]
        headed: bool,
        /// Machine-readable verdict on stdout (ux.md Surface D) + exit
        /// codes 0/1/2/3/4 (unknown=4, never 3)
        #[arg(long)]
        json: bool,
        /// Same as SLEEPER_BROWSER_DEBUG=1: stealth-signal dump to stderr
        #[arg(long)]
        debug: bool,
        /// Explicit opt-in: fall back to system Chrome (never silent)
        #[arg(long)]
        system_chrome: bool,
        /// Explicit CfT Chrome binary path
        #[arg(long)]
        chrome_binary: Option<PathBuf>,
    },
    /// Self-test: pinned/runtime versions, stealth-set hash, lock state,
    /// login-check probes (ux.md Surface E). Non-zero exit on any ✗.
    Doctor,
}

/// Epic 3 waiver-claim actions (live-verified selectors, plan.md Story 3.1).
#[derive(Subcommand)]
pub enum WaiverAction {
    /// Claim a player off waivers (`--league --player [--drop] [--bid]`).
    /// `--player`/`--drop` take numeric Sleeper player IDs, resolved to
    /// display names before touching the DOM — the live player list has no
    /// player_id to select on (plan.md Story 3.1).
    Claim {
        #[arg(long)]
        league: String,
        #[arg(long)]
        player: String,
        #[arg(long)]
        drop: Option<String>,
        /// FAAB bid amount; ignored for a free agent not on waivers.
        #[arg(long, default_value_t = 0)]
        bid: u32,
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
            | Commands::LeagueSnapshot { league_id }
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
        sync::maybe_sync(&svc, &config, league_id, STALE_SECS).await?;
    }

    match cli.command {
        Commands::User { identifier } => roster_cmds::cmd_user(&client, &identifier).await?,

        Commands::Leagues { user_id, season } => {
            roster_cmds::cmd_leagues(&client, &user_id, &season).await?
        }

        Commands::Standings { league_id } => roster_cmds::cmd_standings(&svc, &league_id).await?,

        Commands::Matchups { league_id, week } => {
            roster_cmds::cmd_matchups(&svc, &league_id, week).await?
        }

        Commands::Waivers { league_id, lookback_hours, limit } => {
            roster_cmds::cmd_waivers(&svc, &league_id, lookback_hours, limit).await?
        }

        Commands::Lineup { league_id, roster_id, week } => {
            let rid = resolve_rid(&svc.db, &league_id, &roster_id).await?;
            roster_cmds::cmd_lineup(&svc, &league_id, rid, week).await?
        }

        Commands::Trade { give, receive, season, week } => {
            roster_cmds::cmd_trade(&svc, &give, &receive, season.as_deref(), week).await?
        }

        Commands::Search { query, position, limit } => {
            roster_cmds::cmd_search(&svc, &query, position.as_deref(), limit).await?
        }

        Commands::Transactions { league_id, week, kind } => {
            roster_cmds::cmd_transactions(&svc, &league_id, week, kind.as_deref()).await?
        }

        Commands::Roster { league_id, roster_id } => {
            let rid = resolve_rid(&svc.db, &league_id, &roster_id).await?;
            roster_cmds::cmd_roster(&svc, &league_id, rid).await?
        }

        Commands::Analyze { league_id, roster_id } => {
            let rid = resolve_rid(&svc.db, &league_id, &roster_id).await?;
            roster_cmds::cmd_analyze(&svc, &league_id, rid).await?
        }

        Commands::Available { league_id, position, season, limit } => {
            roster_cmds::cmd_available(&svc, &league_id, position.as_deref(), season.as_deref(), limit)
                .await?
        }

        Commands::SyncStats { season, weeks } => {
            roster_cmds::cmd_sync_stats(&client, &config, &season, weeks).await?
        }

        Commands::DbStatus => roster_cmds::cmd_db_status(&config).await?,

        Commands::Dynasty { league_id, roster_id } => {
            let rid = resolve_rid(&svc.db, &league_id, &roster_id).await?;
            dynasty_cmds::cmd_dynasty(&svc, &league_id, rid).await?
        }

        Commands::DynastyScout { league_id, position, max_age, limit } => {
            dynasty_cmds::cmd_dynasty_scout(&svc, &league_id, position.as_deref(), max_age, limit).await?
        }

        Commands::FindTrades { league_id, roster_id, limit } => {
            let rid = resolve_rid(&svc.db, &league_id, &roster_id).await?;
            dynasty_cmds::cmd_find_trades(&svc, &league_id, rid, limit).await?
        }

        Commands::SyncLeague { league_id } => {
            sync::cmd_sync_league(&client, &*svc.db, &league_id).await?
        }

        Commands::LeagueInfo { league_id } => {
            sync::cmd_league_info(&*svc.db, &league_id).await?
        }

        Commands::LeagueSnapshot { league_id } => {
            sync::cmd_league_snapshot(&svc, &config, &league_id).await?
        }

        Commands::Sync { league_id, season } => {
            sync::cmd_sync(&svc, &config, &league_id, season.as_deref()).await?
        }

        Commands::Browser { action } => browser::cmd_browser(&config, action).await?,

        Commands::Waiver { action } => browser::cmd_waiver(&config, &client, action).await?,

        Commands::LineupSet { league, out, in_ } => {
            browser::cmd_lineup_set(&config, &client, &league, &out, &in_).await?
        }

        Commands::TradePropose { league, give, get } => {
            browser::cmd_trade_propose(&config, &league, &give, &get).await?
        }
    }
    Ok(())
}
