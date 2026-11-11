use clap::{Parser, Subcommand};
use std::path::PathBuf;

use crate::analysis::AnalysisService;
use crate::config::Config;
use crate::db::StatsDb;
use crate::client::SleeperClient;
use crate::error::Result;

pub mod browser;
mod chat_cmds;
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

    /// Show transactions for a week (REST-only — never shows a pending
    /// claim's status/bid; see `transactions` for the cache-backed command
    /// that does)
    #[command(name = "transactions-week")]
    TransactionsWeek {
        league_id: String,
        #[arg(short, long)]
        week: i32,
        #[arg(short, long)]
        kind: Option<String>,
    },

    /// Epic 2.3 / Story 2.3.1: transaction history read through the unified
    /// cache (`cli::browser::transactions::read_through`) — triggers a live
    /// browser refresh only when a row's status/age genuinely warrants it,
    /// never on a fixed interval. Unlike `transactions-week`, this also
    /// surfaces `pending` claims and their FAAB bids.
    Transactions {
        #[arg(long)]
        league: String,
        /// Restrict to one kind (`waiver`, `trade`, `free_agent`); omitted returns all three.
        #[arg(long)]
        kind: Option<String>,
        /// Explicit cache tolerance in seconds; omitted uses the Tuesday-window-aware
        /// default per row's status (ux.md §7).
        #[arg(long = "max-age-secs")]
        max_age_secs: Option<i64>,
        /// Force a live refresh regardless of cache age or status.
        #[arg(long)]
        force: bool,
    },

    /// Show roster details for a team
    Roster {
        league_id: String,
        /// Roster ID, username, or display name
        #[arg(short, long)]
        roster_id: String,
        /// Bypass this process's own cache and ask Sleeper to skip its own caching too —
        /// for checking a write (lineup/waiver) right after making it, since Sleeper's own
        /// API can lag several minutes on an unforced read.
        #[arg(long)]
        fresh: bool,
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
    /// live UI swaps by clicking one player's row then the other's, not by
    /// an addressable slot. This also covers IR/Taxi moves — live-verified
    /// 2026-11-11 (round-tripped a real IR<->BN swap and back): `--out` the
    /// player currently in IR/Taxi, `--in` the bench player taking that
    /// slot (or the reverse, to activate someone off IR/Taxi back to the
    /// bench) — same generic swap mechanism, no slot-type restriction.
    #[command(name = "lineup-set")]
    LineupSet {
        #[arg(long)]
        league: String,
        /// Player_id currently occupying the target slot (starter, IR, or Taxi) to bench.
        #[arg(long)]
        out: String,
        /// Player_id on the bench to move into that slot in their place.
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

    /// Durable analysis log + chat-transcript import — so a session's
    /// findings (standings reviews, trade scans) survive past the session
    /// itself, in the local stats DB rather than only in chat scrollback.
    Chat {
        #[command(subcommand)]
        action: ChatAction,
    },
}

#[derive(Subcommand)]
pub enum ChatAction {
    /// Persist a dated analysis finding.
    Log {
        #[arg(long)]
        league: String,
        #[arg(long)]
        topic: String,
        #[arg(long)]
        summary: String,
        #[arg(long)]
        detail: Option<String>,
    },

    /// List logged analysis findings, most recent first.
    List {
        #[arg(long)]
        league: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },

    /// Import a chat session transcript file (e.g. a Claude Code session
    /// JSONL) verbatim, keyed by filename so re-importing an
    /// actively-growing session file updates rather than duplicates.
    Import {
        path: PathBuf,
        #[arg(long)]
        league: Option<String>,
    },

    /// List imported transcripts (metadata only), most recent first.
    Transcripts {
        #[arg(long)]
        league: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: i64,
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
    /// Read your own pending waiver claims (root-cause fix, 2026-11-10): the
    /// public REST transactions endpoint only ever returns `status:
    /// "complete"` — pending claims are invisible there, for every roster.
    /// This reads them through the authenticated browser session instead,
    /// via the same GraphQL query the Sleeper web app itself fires.
    Pending {
        #[arg(long)]
        league: String,
        /// Your numeric Sleeper roster_id in this league.
        #[arg(long)]
        roster: i64,
    },
    /// Epic 3.2 / Story 3.2.1: FAAB bid-projection ("what should I bid") —
    /// pure analysis over already-cached data (Epic 3.1's five-signal
    /// model); never launches a browser. Default output is the one-line
    /// `render_short()`; `--full` prints the full structured object.
    BidProjection {
        #[arg(long)]
        league: String,
        #[arg(long)]
        player: String,
        #[arg(long)]
        roster: i64,
        /// Print the full structured object (drill-down) instead of the
        /// default one-line render.
        #[arg(long)]
        full: bool,
    },
    /// Epic 4.3 / Story 4.3.1: edit the FAAB bid on a real pending waiver
    /// claim (`sleeper waiver edit --league --transaction --bid`). LIVE
    /// WRITE ACTION — always launches a browser, never a simulation. Prints
    /// `claim bid updated to $N` only once an independent post-write
    /// re-check confirms the new bid landed (ux.md §5).
    Edit {
        #[arg(long)]
        league: String,
        /// The Sleeper transaction_id of the pending claim to edit.
        #[arg(long)]
        transaction: String,
        /// New FAAB bid amount.
        #[arg(long)]
        bid: u32,
    },
    /// Epic 4.3 / Story 4.3.1: cancel a real pending waiver claim
    /// (`sleeper waiver cancel --league --transaction`). LIVE WRITE ACTION —
    /// always launches a browser, never a simulation. Prints
    /// `claim cancelled` only once an independent post-write re-check
    /// confirms the cancel landed (ux.md §5).
    Cancel {
        #[arg(long)]
        league: String,
        /// The Sleeper transaction_id of the pending claim to cancel.
        #[arg(long)]
        transaction: String,
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
            | Commands::TransactionsWeek { league_id, .. }
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
            Commands::Transactions { league, .. } => Some(league),
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

        Commands::TransactionsWeek { league_id, week, kind } => {
            roster_cmds::cmd_transactions(&svc, &league_id, week, kind.as_deref()).await?
        }

        Commands::Transactions { league, kind, max_age_secs, force } => {
            browser::transactions::cmd_transactions(&config, &svc, &league, kind.as_deref(), max_age_secs, force)
                .await?
        }

        Commands::Roster { league_id, roster_id, fresh } => {
            let rid = resolve_rid(&svc.db, &league_id, &roster_id).await?;
            roster_cmds::cmd_roster(&svc, &league_id, rid, fresh).await?
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

        // BidProjection is pure analysis over AnalysisService (`svc`), unlike
        // Claim/Pending which run through the browser backend — intercepted
        // here rather than threaded into `browser::cmd_waiver`, which only
        // has `client`, not `svc`.
        Commands::Waiver { action: WaiverAction::BidProjection { league, player, roster, full } } => {
            dynasty_cmds::cmd_waiver_bid_projection(&svc, &league, &player, roster, full).await?
        }

        // Epic 4.3: also intercepted here rather than threaded into
        // `browser::cmd_waiver` (same reason as `BidProjection` above) —
        // `run_edit_cancel_claim` needs `AnalysisService` (`svc`), which
        // `cmd_waiver` only has as `client`.
        Commands::Waiver { action: WaiverAction::Edit { league, transaction, bid } } => {
            browser::cmd_waiver_edit_cancel(
                &config,
                &svc,
                &league,
                &transaction,
                crate::browser::session::EditCancelAction::EditBid(bid),
            )
            .await?
        }
        Commands::Waiver { action: WaiverAction::Cancel { league, transaction } } => {
            browser::cmd_waiver_edit_cancel(
                &config,
                &svc,
                &league,
                &transaction,
                crate::browser::session::EditCancelAction::Cancel,
            )
            .await?
        }

        Commands::Waiver { action } => browser::cmd_waiver(&config, &client, action).await?,

        Commands::LineupSet { league, out, in_ } => {
            browser::cmd_lineup_set(&config, &client, &league, &out, &in_).await?
        }

        Commands::TradePropose { league, give, get } => {
            browser::cmd_trade_propose(&config, &league, &give, &get).await?
        }

        Commands::Chat { action } => chat_cmds::cmd_chat(&*svc.db, action).await?,
    }
    Ok(())
}
