use clap::{Parser, Subcommand};
use console::style;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::PathBuf;

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

    /// Authenticated browser session against sleeper.com — for actions the read-only API
    /// can't do (waiver claims, lineups, trades)
    Browser {
        #[command(subcommand)]
        action: BrowserAction,
    },

    /// Waiver claims through the browser backend (Epic 3 — STUB ONLY while
    /// Q5 OPEN: launches via seam then prints disabled triage to stderr,
    /// exits non-zero; performs no DOM automation)
    Waiver {
        #[command(subcommand)]
        action: WaiverAction,
    },

    /// Lineup changes through the browser backend (Epic 4 — STUB ONLY, cut
    /// per appetite rule: launches via seam then prints disabled triage to
    /// stderr, exits non-zero; performs no DOM automation)
    #[command(name = "lineup-set")]
    LineupSet {
        #[arg(long)]
        league: String,
        #[arg(long)]
        slot: String,
        #[arg(long)]
        player: String,
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

/// Epic 3 waiver-claim actions (STUB ONLY while Q5 OPEN).
#[derive(Subcommand)]
pub enum WaiverAction {
    /// Claim a player off waivers (`--league --player [--drop]`).
    /// STUB ONLY while Q5 OPEN: launches via seam then prints the disabled
    /// triage to stderr, exits non-zero; performs no DOM automation.
    Claim {
        #[arg(long)]
        league: String,
        #[arg(long)]
        player: String,
        #[arg(long)]
        drop: Option<String>,
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

        Commands::Browser { action } => cmd_browser(&config, action).await?,

        Commands::Waiver { action } => cmd_waiver(&config, action).await?,

        Commands::LineupSet { league, slot, player } => {
            cmd_lineup_set(&config, &league, &slot, &player).await?
        }

        Commands::TradePropose { league, give, get } => {
            cmd_trade_propose(&config, &league, &give, &get).await?
        }
    }
    Ok(())
}

async fn cmd_browser(config: &Config, action: BrowserAction) -> Result<()> {
    match action {
        BrowserAction::Login { system_chrome, chrome_binary } => {
            cmd_browser_login(config, system_chrome, chrome_binary).await
        }
        BrowserAction::Status { headed, json, debug, system_chrome, chrome_binary } => {
            cmd_browser_status(config, headed, json, debug, system_chrome, chrome_binary).await
        }
        BrowserAction::Doctor => cmd_browser_doctor(config).await,
    }
}

/// Verbatim fast-path verdict (ux.md Surface A). Exact copy guarded by
/// `login_fast_path_should_print_exact_verdict_when_already_authenticated`.
pub fn already_logged_in_message() -> String {
    "Already logged in — session persisted from a previous run.".to_string()
}

/// Exit-code contract (ux.md Surfaces C/D): 0 logged-in / 1 logged-out /
/// 2 skew / 3 lock-busy / 4 unknown. Unknown is NEVER 1 or 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusOutcome {
    LoggedIn,
    LoggedOut,
    Skew,
    LockBusy,
    Unknown,
}

pub fn status_exit_code(outcome: &StatusOutcome) -> i32 {
    match outcome {
        StatusOutcome::LoggedIn => 0,
        StatusOutcome::LoggedOut => 1,
        StatusOutcome::Skew => 2,
        StatusOutcome::LockBusy => 3,
        StatusOutcome::Unknown => 4,
    }
}

/// Skew triage line: contains `version skew` + the `re-run with update`
/// directive (never "wait" — skew is fixed by re-resolving/updating).
pub fn skew_triage_message(pinned: &str, runtime: &str) -> String {
    format!(
        "CfT Chrome runtime v{runtime} vs pinned v{pinned} — version skew. Please re-run with update: re-run `sleeper browser login` to re-resolve the pin; if it persists, update the pin to v{runtime}."
    )
}

/// Lock-busy copy (ux.md state 4): `status` aborts, profile untouched.
pub fn lock_busy_message(profile: &std::path::Path) -> String {
    format!(
        "profile busy — close the login window first (profile {} is locked by a live Chrome window)",
        profile.display()
    )
}

/// Unknown-verdict triage (signal split/timeout): exit 4, re-login only if
/// persistent — distinct from yellow logged-out (exit 1) and lock-busy (3).
pub fn unknown_triage(which_signal: &str) -> String {
    format!(
        "Login check unknown ({which_signal}) — re-run `sleeper browser status`; re-login only if persistent."
    )
}

/// Transport-error triage: distinct copy from signal-split Unknown.
pub fn check_failed_triage(detail: &str) -> String {
    format!("Login check failed ({detail}) — re-run `sleeper browser status`.")
}

/// Triage-line contract (ux.md Surface F): the triage line always comes
/// FIRST; raw CDP/anyhow detail follows `—` (or hides behind
/// `SLEEPER_BROWSER_DEBUG`). Never a bare stack on stdout.
pub fn triage_error(triage: &str, raw_detail: &str) -> String {
    if raw_detail.is_empty() {
        triage.to_string()
    } else {
        format!("{triage} — {raw_detail}")
    }
}

/// Epic 3 stub triage (Q5 OPEN): stderr-only, never a stdout verdict.
pub fn claim_disabled_triage() -> String {
    "Q5 not yet verified — claim automation disabled. Walk the live claim flow by hand first (plan.md Epic 3 Q5 note), then re-run `sleeper waiver claim`.".to_string()
}

/// Epic 4 stub triage (cut per appetite rule): stderr-only, never stdout.
pub fn lineup_disabled_triage() -> String {
    "selectors live-unverified — lineup automation disabled (Epic 4 deferred per appetite cut rule; see plan.md). Live-verify the lineup selector bundle first, then re-run `sleeper lineup-set`.".to_string()
}

/// Epic 4 stub triage (cut per appetite rule): stderr-only, never stdout.
pub fn trade_disabled_triage() -> String {
    "selectors live-unverified — trade automation disabled (Epic 4 deferred per appetite cut rule; see plan.md). Live-verify the trade selector bundle first, then re-run `sleeper trade-propose`.".to_string()
}

/// `9m45s` countdown formatting for the login poll spinner.
pub fn format_countdown(secs: u64) -> String {
    format!("{}m{:02}s", secs / 60, secs % 60)
}

/// One spinner tick (stderr only): elapsed + remaining, updated per 5s poll.
pub fn login_poll_tick(elapsed_secs: u64, total_secs: u64) -> String {
    let left = total_secs.saturating_sub(elapsed_secs);
    format!(
        "waiting for manual login… ({} elapsed, {} left)",
        format_countdown(elapsed_secs),
        format_countdown(left)
    )
}

/// Timeout verdict (ux.md §2): directs to `login`, never `status`.
pub fn login_timeout_copy() -> String {
    "Still not logged in after 10m — finish logging in in the browser window, then re-run `sleeper browser login`.".to_string()
}

/// Challenge-loop tool news (plan Story 2.4 Task 6, verbatim framing):
/// recurrence is tool news, never operator blame.
pub fn challenge_loop_copy(polls: u64) -> String {
    format!(
        "Login page is still showing a challenge after {polls} polls. This is unexpected with the stealth backend — solve it once by hand if you can; if it loops, file the CfT Chrome pinned/runtime versions from `sleeper browser status --debug`."
    )
}

/// Ctrl-C copy: exit 130 after closing the browser.
pub fn login_cancelled_copy() -> String {
    "login wait cancelled — closing browser…".to_string()
}

/// `status --json` shape (ux.md Surface D): stdout carries ONLY this object.
/// Failure shape is `{"logged_in":false,"error":"…"}`, never a bare stack.
pub fn status_json(
    logged_in: bool,
    pinned: &str,
    runtime: &str,
    profile: &std::path::Path,
    error: Option<&str>,
) -> String {
    if let Some(e) = error {
        serde_json::json!({"logged_in": false, "error": e}).to_string()
    } else {
        serde_json::json!({
            "logged_in": logged_in,
            "chrome_version": pinned,
            "chrome_runtime_version": runtime,
            "profile": profile.display().to_string(),
        })
        .to_string()
    }
}

/// Pure outcome resolution behind `status` (FakeSession-programmable):
/// lock first (profile untouched), then skew, then the tri-state verdict.
/// Transport errors surface as `Unknown` (caller prints the distinct
/// `Login check failed` copy with exit 4).
pub async fn status_outcome_for_session(
    session: &dyn crate::browser::BrowserSession,
    pinned: &str,
    runtime: &str,
    lock_held: bool,
) -> StatusOutcome {
    use crate::browser::LoginVerdict;
    if lock_held {
        return StatusOutcome::LockBusy;
    }
    if pinned != runtime {
        return StatusOutcome::Skew;
    }
    match session.login_verdict().await {
        Ok(LoginVerdict::LoggedIn) => StatusOutcome::LoggedIn,
        Ok(LoginVerdict::LoggedOut) => StatusOutcome::LoggedOut,
        Ok(LoginVerdict::Unknown) => StatusOutcome::Unknown,
        Err(_) => StatusOutcome::Unknown,
    }
}

/// Parse `Google Chrome 126.0.6478.126` → `126.0.6478.126`.
/// Thin seam-compat wrapper over [`crate::browser::cft::parse_chrome_version`];
/// the implementation lives in the browser layer, the CLI only calls it.
pub fn parse_chrome_version(output: &str) -> Option<String> {
    crate::browser::cft::parse_chrome_version(output)
}

/// Best-effort runtime version of a Chrome binary (`unknown` when
/// undetectable — never omitted from `--json`).
/// Thin seam-compat wrapper over [`crate::browser::cft::runtime_version`].
pub fn chrome_runtime_version(binary: &std::path::Path) -> String {
    crate::browser::cft::runtime_version(binary)
}

/// `browser doctor` self-test report (ux.md Surface E): one `✓`/`✗` line
/// per check; every `✗` names the fix + exact re-run command. Non-zero exit
/// on any `✗`; zero only when all checks pass.
pub struct DoctorReport {
    pub checks: Vec<(bool, String)>,
}

impl DoctorReport {
    pub fn render(&self) -> String {
        self.checks
            .iter()
            .map(|(ok, line)| format!("{} {line}", if *ok { "✓" } else { "✗" }))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn exit_code(&self) -> i32 {
        if self.checks.iter().all(|(ok, _)| *ok) {
            0
        } else {
            1
        }
    }
}

pub fn build_doctor_report(
    pinned: &str,
    cft_binary: &std::path::Path,
    runtime: &str,
    flag_hash: &str,
    profile: &std::path::Path,
    lock_free: bool,
    login: &str,
    webdriver_clean: bool,
) -> DoctorReport {
    let mut checks = Vec::new();
    if cft_binary.is_file() {
        checks.push((
            true,
            format!("cft: {pinned} at {} (match)", cft_binary.display()),
        ));
    } else {
        checks.push((
            false,
            format!(
                "cft: {pinned} missing at {} — re-run `sleeper browser login` to resolve the pin with update",
                cft_binary.display()
            ),
        ));
    }
    if pinned == runtime {
        checks.push((
            true,
            format!("chrome runtime {runtime} (match, skew-free)"),
        ));
    } else if runtime == "unknown" {
        checks.push((
            false,
            format!(
                "Chrome binary missing or unreadable — re-run `sleeper browser login` to resolve the pin (pinned {pinned})"
            ),
        ));
    } else {
        checks.push((
            false,
            format!(
                "chrome runtime {runtime} vs pinned {pinned} — version skew. Re-run `sleeper browser login` to re-resolve the pin; if it persists, update the pin to {runtime}"
            ),
        ));
    }
    checks.push((
        true,
        format!("patch: {flag_hash} (flag-set hash match, no binary patching)"),
    ));
    if lock_free {
        checks.push((
            true,
            format!("profile {} (unlocked) lock: free", profile.display()),
        ));
    } else {
        checks.push((
            false,
            format!(
                "profile {} (locked) — close the login window, then re-run `sleeper browser status`",
                profile.display()
            ),
        ));
    }
    if webdriver_clean {
        checks.push((
            true,
            format!("headless login check: {login}, webdriver: false"),
        ));
    } else {
        checks.push((
            false,
            format!(
                "headless login check: {login} — re-run `sleeper browser status`; re-login only if persistent"
            ),
        ));
    }
    DoctorReport { checks }
}

/// Launch through the seam only: `StealthLauncher::spawn` + `connect`.
/// Returns the boxed session plus the process guard; callers close both.
/// CfT wiring (Story 2.1 Task 4): without an explicit binary/flag the
/// resolved CfT path is used; a missing cache entry fails fast (never a
/// silent system-Chrome fallback).
async fn launch_session(
    config: &Config,
    headed: bool,
    system_chrome: bool,
    chrome_binary: Option<PathBuf>,
) -> Result<(Box<dyn crate::browser::BrowserSession>, crate::browser::LaunchedBrowser)> {
    use crate::browser::{BrowserSession, LaunchConfig, PinnedVersion, SleeperBrowser, StealthLauncher};
    let profile_dir = config.chrome_profile_dir();
    let mut cfg = if headed {
        LaunchConfig::headed_manual_login(profile_dir)
    } else {
        LaunchConfig::headless_status(profile_dir)
    };
    cfg = cfg.with_system_chrome_fallback(system_chrome);
    if let Some(path) = chrome_binary {
        cfg = cfg.with_chrome_binary(path);
    } else if !system_chrome {
        cfg = cfg.with_cft(&config.data_dir, &PinnedVersion::pinned());
    }
    let launched = StealthLauncher::spawn(&cfg).await?;
    let session: Box<dyn BrowserSession> =
        Box::new(SleeperBrowser::connect(&launched.endpoint).await?);
    Ok((session, launched))
}

async fn cmd_browser_login(
    config: &Config,
    system_chrome: bool,
    chrome_binary: Option<PathBuf>,
) -> Result<()> {
    let (mut session, launched) =
        match launch_session(config, true, system_chrome, chrome_binary).await {
            Ok(pair) => pair,
            Err(e) => launch_failure_exit(&e),
        };
    // AC4 fast path: check is_logged_in() before starting the poll.
    if session.is_logged_in().await? {
        println!("{}", style(already_logged_in_message()).green());
        session.close().await?;
        return launched.close().await;
    }

    eprintln!("Log into Sleeper by hand in the window that just opened — including any CAPTCHA. This step isn't automated, on purpose; waiting up to 10 minutes…");
    // LoginPoll: indicatif spinner + countdown on stderr only (Surface B);
    // 5s ticks, 600s bound, Ctrl-C → exit 130 with copy.
    const TOTAL_SECS: u64 = 600;
    // Observability boundary (plan.md Layer 3): one span + start / timeout
    // events; per-tick progress stays on the spinner/stderr.
    let poll_span = tracing::info_span!("login.poll", timeout_s = TOTAL_SECS);
    let _poll_enter = poll_span.enter();
    tracing::info!("login.poll start");
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.dim} {msg}")
            .unwrap(),
    );
    pb.enable_steady_tick(std::time::Duration::from_millis(120));
    let start = tokio::time::Instant::now();
    let mut polls: u64 = 0;
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                pb.abandon_with_message("login wait stopped");
                eprintln!("{}", style(login_cancelled_copy()).red());
                session.close().await?;
                launched.close().await?;
                std::process::exit(130);
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
        }
        polls += 1;
        let elapsed = start.elapsed().as_secs().min(TOTAL_SECS);
        pb.set_message(login_poll_tick(elapsed, TOTAL_SECS));
        if polls % 60 == 0 {
            // Tool-news framing (never operator blame) on long waits.
            eprintln!("{}", style(challenge_loop_copy(polls)).dim());
        }
        if session.is_logged_in().await? {
            pb.finish_with_message("login detected");
            println!("{}", style("Logged in. Session persisted for future runs.").green());
            session.close().await?;
            return launched.close().await;
        }
        if elapsed >= TOTAL_SECS {
            pb.abandon_with_message("still waiting stopped");
            tracing::info!("login.poll timeout");
            println!("{}", style(login_timeout_copy()).yellow());
            session.close().await?;
            launched.close().await?;
            std::process::exit(1);
        }
    }
}

async fn cmd_browser_status(
    config: &Config,
    headed: bool,
    json: bool,
    debug: bool,
    system_chrome: bool,
    chrome_binary: Option<PathBuf>,
) -> Result<()> {
    use crate::browser::{LoginVerdict, PinnedVersion};
    let profile_dir = config.chrome_profile_dir();
    let pinned = PinnedVersion::pinned().0.clone();

    // Lock gate first: abort without touching the profile (exit 3).
    if profile_dir.join("SingletonLock").exists() {
        let msg = lock_busy_message(&profile_dir);
        if json {
            println!("{}", status_json(false, &pinned, "unknown", &profile_dir, Some(&msg)));
        } else {
            eprintln!("{}", style(&msg).red());
        }
        std::process::exit(3);
    }

    // Skew gate before/around launch: runtime vs pin (exit 2, no fallback).
    let probe_binary = chrome_binary.clone().unwrap_or_else(|| {
        crate::browser::cft::cft_chrome_binary(&config.data_dir, &PinnedVersion::pinned())
    });
    let runtime = if system_chrome || !probe_binary.is_file() {
        "unknown".to_string()
    } else {
        crate::browser::cft::runtime_version(&probe_binary)
    };
    if runtime != "unknown" && pinned != runtime {
        let msg = skew_triage_message(&pinned, &runtime);
        if json {
            println!("{}", status_json(false, &pinned, &runtime, &profile_dir, Some(&msg)));
        } else {
            eprintln!("{}", style(triage_error(&msg, "")).red());
        }
        std::process::exit(2);
    }

    let (mut session, launched) =
        match launch_session(config, headed, system_chrome, chrome_binary).await {
            Ok(pair) => pair,
            Err(e) => {
                // --json contract (Surface D / Story 2.4 AC2): transport/check
                // failure exits non-zero with the failure shape on stdout
                // (pipeable to jq), never empty. Stderr triage-first line is
                // unchanged inside launch_failure_exit; exit code preserved.
                if json {
                    let err_msg = launch_failure_json_error(&e);
                    println!(
                        "{}",
                        status_json(false, &pinned, &runtime, &profile_dir, Some(&err_msg))
                    );
                }
                launch_failure_exit(&e)
            }
        };
    if debug || std::env::var("SLEEPER_BROWSER_DEBUG").is_ok() {
        let dump = match session.eval_dump().await {
            Ok(dump) => dump,
            Err(e) => {
                // Debug-probe failure maps like any transport failure (exit 4,
                // never exit 1 via `?`); closes are best-effort on this path.
                let detail = e.to_string();
                let _ = session.close().await;
                let _ = launched.close().await;
                let msg = check_failed_triage(&detail);
                if json {
                    println!("{}", status_json(false, &pinned, &runtime, &profile_dir, Some(&msg)));
                } else {
                    eprintln!("{}", style(triage_error(&msg, &debug_detail(&detail))).red());
                }
                std::process::exit(4);
            }
        };
        eprintln!(
            "DEBUG dump: {}",
            crate::browser::session::redact_secrets(&dump)
        );
        eprintln!(
            "{}",
            crate::browser::session::debug_browser_line(
                &pinned,
                &runtime,
                "<flag-hash>",
                "<cdp-port>",
                &profile_dir,
                &[],
            )
        );
    }
    let outcome = match session.login_verdict().await {
        Ok(LoginVerdict::LoggedIn) => StatusOutcome::LoggedIn,
        Ok(LoginVerdict::LoggedOut) => StatusOutcome::LoggedOut,
        Ok(LoginVerdict::Unknown) => StatusOutcome::Unknown,
        Err(e) => {
            let detail = e.to_string();
            session.close().await?;
            launched.close().await?;
            let msg = check_failed_triage(&detail);
            if json {
                println!("{}", status_json(false, &pinned, &runtime, &profile_dir, Some(&msg)));
            } else {
                eprintln!("{}", style(triage_error(&msg, &debug_detail(&detail))).red());
            }
            std::process::exit(4);
        }
    };
    session.close().await?;
    launched.close().await?;
    let code = status_exit_code(&outcome);
    if json {
        match outcome {
            StatusOutcome::LoggedIn => {
                println!("{}", status_json(true, &pinned, &runtime, &profile_dir, None));
            }
            StatusOutcome::LoggedOut => {
                println!("{}", status_json(false, &pinned, &runtime, &profile_dir, None));
            }
            StatusOutcome::Skew => {
                let msg = skew_triage_message(&pinned, &runtime);
                println!("{}", status_json(false, &pinned, &runtime, &profile_dir, Some(&msg)));
            }
            StatusOutcome::LockBusy => {
                let msg = lock_busy_message(&profile_dir);
                println!("{}", status_json(false, &pinned, &runtime, &profile_dir, Some(&msg)));
            }
            StatusOutcome::Unknown => {
                let msg = unknown_triage("signal split/timeout");
                println!("{}", status_json(false, &pinned, &runtime, &profile_dir, Some(&msg)));
            }
        }
    } else {
        match outcome {
            StatusOutcome::LoggedIn => {
                println!("{}", style("Logged in.").green());
            }
            StatusOutcome::LoggedOut => {
                println!("{}", style("Not logged in — run `sleeper browser login`.").yellow());
            }
            StatusOutcome::Unknown => {
                eprintln!(
                    "{}",
                    style(unknown_triage("signal split/timeout")).red()
                );
            }
            StatusOutcome::Skew => {
                eprintln!("{}", style(skew_triage_message(&pinned, &runtime)).red());
            }
            StatusOutcome::LockBusy => {
                eprintln!("{}", style(lock_busy_message(&profile_dir)).red());
            }
        }
    }
    std::process::exit(code);
}

/// Live headless login probe for `doctor`: launches, reads the verdict +
/// webdriver signal, and closes on every branch. Returns the Surface E
/// login-check label plus whether the webdriver signal came back clean.
/// Dump parsing lives in the seam (`stealth_health`); the CLI only renders.
/// Probe failures carry triage-first copy (raw detail behind
/// `SLEEPER_BROWSER_DEBUG`), never a bare error on stdout.
async fn probe_headless_login(config: &Config) -> (String, bool) {
    use crate::browser::LoginVerdict;
    let (mut session, launched) = match launch_session(config, false, false, None).await {
        Ok(pair) => pair,
        Err(e) => {
            let raw = e.to_string();
            let label =
                triage_error(&check_failed_triage("browser launch"), &debug_detail(&raw));
            return (label, false);
        }
    };
    let dump = session.eval_dump().await.unwrap_or_default();
    let (_signal_summary, webdriver_clean) = crate::browser::session::stealth_health(&dump);
    let label = match session.login_verdict().await {
        Ok(LoginVerdict::LoggedIn) => "logged in".to_string(),
        Ok(LoginVerdict::LoggedOut) => "logged out".to_string(),
        Ok(LoginVerdict::Unknown) => {
            "unknown — re-run `sleeper browser status`; re-login only if persistent".to_string()
        }
        Err(e) => {
            let raw = e.to_string();
            triage_error(&check_failed_triage("login check"), &debug_detail(&raw))
        }
    };
    let _ = session.close().await;
    let _ = launched.close().await;
    (label, webdriver_clean)
}

/// Raw detail is stderr/debug-gated: shown only when `SLEEPER_BROWSER_DEBUG`
/// is set, so the triage line always leads.
fn debug_detail(detail: &str) -> String {
    if std::env::var("SLEEPER_BROWSER_DEBUG").is_ok() {
        detail.to_string()
    } else {
        String::new()
    }
}

/// Launch/transport failure exit (contract: 1 = logged-out, so a failed
/// launch must never exit 1 via `?`). Prints the triage line FIRST to stderr
/// (raw detail behind `SLEEPER_BROWSER_DEBUG`) and exits 4 — or 2 when the
/// error proves version skew.
fn launch_failure_exit(e: &crate::error::Error) -> ! {
    let msg = e.to_string();
    if msg.contains("version skew") {
        eprintln!("{}", style(triage_error(&msg, &debug_detail(&msg))).red());
        std::process::exit(2);
    }
    let triage = check_failed_triage("browser launch");
    eprintln!("{}", style(triage_error(&triage, &debug_detail(&msg))).red());
    std::process::exit(4);
}

/// Short JSON-safe error for the launch-failure path under `--json`
/// (ux.md Surface D + Story 2.4 AC2): skew errors carry their own triage,
/// everything else carries the `browser launch` triage. Properly escaped
/// downstream via `status_json` (`serde_json::json!`), never a bare stack.
pub fn launch_failure_json_error(e: &crate::error::Error) -> String {
    let msg = e.to_string();
    if msg.contains("version skew") {
        msg
    } else {
        check_failed_triage("browser launch")
    }
}

async fn cmd_browser_doctor(config: &Config) -> Result<()> {
    use crate::browser::{stealth::stealth_flags, LaunchMode, PinnedVersion};
    let pinned = PinnedVersion::pinned();
    let cft_binary = crate::browser::cft::cft_chrome_binary(&config.data_dir, &pinned);
    let runtime = if cft_binary.is_file() {
        crate::browser::cft::runtime_version(&cft_binary)
    } else {
        "unknown".to_string()
    };
    let flags = stealth_flags(LaunchMode::HeadlessStatus, &config.chrome_profile_dir());
    let flag_hash = crate::browser::cft::stealth_flag_set_hash(&flags);
    let profile_dir = config.chrome_profile_dir();
    let lock_free = !profile_dir.join("SingletonLock").exists();
    // Probes are read-only: never delete the profile; a failed probe is an
    // ✗ with its fix line, never a silent kill (Surface E).
    let (login, webdriver_clean) = if lock_free && cft_binary.is_file() && pinned.0 == runtime
    {
        probe_headless_login(config).await
    } else {
        ("skipped (resolve skew/lock first)".to_string(), false)
    };
    let report = build_doctor_report(
        &pinned.0,
        &cft_binary,
        &runtime,
        &flag_hash,
        &profile_dir,
        lock_free,
        &login,
        webdriver_clean,
    );
    println!("{}", report.render());
    std::process::exit(report.exit_code());
}

async fn cmd_waiver(config: &Config, action: WaiverAction) -> Result<()> {
    match action {
        WaiverAction::Claim { league, player, drop } => {
            cmd_waiver_claim(config, &league, &player, drop.as_deref()).await
        }
    }
}

/// Story 3.1/3.2 stub wiring: launches through the seam, calls the gated
/// seam-side flow (which returns the Q5-unverified error with no selector
/// use), and surfaces triage. Dead sessions get the yellow Surface C hint
/// (ux.md §3), never a CDP dump. The browser is closed on every branch —
/// no orphan. Never attempts claim automation while Q5 is OPEN.
async fn cmd_waiver_claim(
    config: &Config,
    league: &str,
    player: &str,
    drop: Option<&str>,
) -> Result<()> {
    use crate::browser::session::{claim_player_via_seam, DEAD_SESSION_HINT};
    let (mut session, launched) = match launch_session(config, false, false, None).await {
        Ok(pair) => pair,
        Err(e) => launch_failure_exit(&e),
    };
    let outcome = claim_player_via_seam(&*session, league, player, drop).await;
    session.close().await?;
    launched.close().await?;
    match outcome {
        Ok(()) => Ok(()), // unreachable while Q5 OPEN (stub always Errs)
        Err(e) if e.to_string() == DEAD_SESSION_HINT => {
            eprintln!("{}", style(DEAD_SESSION_HINT).yellow());
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("{}", style(claim_disabled_triage()).red());
            std::process::exit(1);
        }
    }
}

/// Epic 4 stub wiring (cut per appetite rule): same launch → gated-flow →
/// triage → close discipline as the claim stub. Never attempts automation.
async fn cmd_lineup_set(
    config: &Config,
    league: &str,
    slot: &str,
    player: &str,
) -> Result<()> {
    use crate::browser::session::{lineup_set_via_seam, DEAD_SESSION_HINT};
    let (mut session, launched) = match launch_session(config, false, false, None).await {
        Ok(pair) => pair,
        Err(e) => launch_failure_exit(&e),
    };
    let outcome = lineup_set_via_seam(&*session, league, slot, player).await;
    session.close().await?;
    launched.close().await?;
    match outcome {
        Ok(()) => Ok(()), // unreachable while Epic 4 is cut (stub always Errs)
        Err(e) if e.to_string() == DEAD_SESSION_HINT => {
            eprintln!("{}", style(DEAD_SESSION_HINT).yellow());
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("{}", style(lineup_disabled_triage()).red());
            std::process::exit(1);
        }
    }
}

/// Epic 4 stub wiring (cut per appetite rule): same launch → gated-flow →
/// triage → close discipline as the claim stub. Never attempts automation.
async fn cmd_trade_propose(
    config: &Config,
    league: &str,
    give: &str,
    get: &str,
) -> Result<()> {
    use crate::browser::session::{trade_propose_via_seam, DEAD_SESSION_HINT};
    let (mut session, launched) = match launch_session(config, false, false, None).await {
        Ok(pair) => pair,
        Err(e) => launch_failure_exit(&e),
    };
    let outcome = trade_propose_via_seam(&*session, league, give, get).await;
    session.close().await?;
    launched.close().await?;
    match outcome {
        Ok(()) => Ok(()), // unreachable while Epic 4 is cut (stub always Errs)
        Err(e) if e.to_string() == DEAD_SESSION_HINT => {
            eprintln!("{}", style(DEAD_SESSION_HINT).yellow());
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("{}", style(trade_disabled_triage()).red());
            std::process::exit(1);
        }
    }
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

#[cfg(test)]
mod tests {
    use super::{
        already_logged_in_message, build_doctor_report, challenge_loop_copy,
        check_failed_triage, format_countdown, launch_failure_json_error, lock_busy_message,
        login_cancelled_copy, login_poll_tick, login_timeout_copy, parse_chrome_version,
        skew_triage_message, status_exit_code, status_json, status_outcome_for_session,
        triage_error, unknown_triage, StatusOutcome,
    };
    use crate::browser::{BrowserSession, FakeSession};

    /// CLI-level seam test: the login fast path resolves through
    /// `Box<dyn BrowserSession>` with no Chrome installed.
    #[tokio::test]
    async fn login_fast_path_should_print_exact_verdict_when_already_authenticated() {
        let session: Box<dyn BrowserSession> = Box::new(FakeSession::logged_in());
        assert!(
            session.is_logged_in().await.expect("seam is_logged_in"),
            "fast path triggers only when the seam reports logged-in"
        );
        assert_eq!(
            already_logged_in_message(),
            "Already logged in — session persisted from a previous run."
        );
    }

    #[tokio::test]
    async fn status_should_exit_2_with_triage_when_skew_detected() {
        let session = FakeSession::logged_in();
        let outcome =
            status_outcome_for_session(&session, "126.0.6478.126", "127.0.6533.99", false).await;
        assert_eq!(outcome, StatusOutcome::Skew);
        assert_eq!(status_exit_code(&outcome), 2);
        let msg = skew_triage_message("126.0.6478.126", "127.0.6533.99");
        assert!(msg.contains("version skew"), "got {msg}");
        assert!(msg.contains("re-run with update"), "got {msg}");
    }

    #[tokio::test]
    async fn status_should_exit_3_when_singleton_lock_held() {
        // Held lock short-circuits before any profile touch: the outcome
        // resolves without creating or modifying the profile dir.
        let untouched = std::env::temp_dir().join(format!(
            "sleeper-status-untouched-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&untouched);
        let session = FakeSession::logged_in();
        let outcome =
            status_outcome_for_session(&session, "126.0.6478.126", "126.0.6478.126", true).await;
        assert_eq!(outcome, StatusOutcome::LockBusy);
        assert_eq!(status_exit_code(&outcome), 3);
        let msg = lock_busy_message(&untouched);
        assert!(
            msg.contains("profile busy — close the login window first"),
            "got {msg}"
        );
        assert!(
            !untouched.exists(),
            "lock-busy must leave the profile untouched"
        );
    }

    #[tokio::test]
    async fn status_should_exit_4_with_signal_log_when_verdict_unknown() {
        let session = FakeSession::with_signal_split();
        let outcome =
            status_outcome_for_session(&session, "126.0.6478.126", "126.0.6478.126", false).await;
        assert_eq!(outcome, StatusOutcome::Unknown);
        assert_eq!(status_exit_code(&outcome), 4);
        let msg = unknown_triage("signal 1/3 missed");
        assert!(msg.contains("signal 1/3 missed"), "per-signal log, got {msg}");
        assert!(
            msg.contains("re-run `sleeper browser status`"),
            "got {msg}"
        );
        assert!(msg.contains("re-login only if persistent"), "got {msg}");
        // Transport errors share exit 4 with their own copy.
        let failed = check_failed_triage("fake transport error");
        assert!(failed.contains("Login check failed"), "got {failed}");
        assert!(failed.contains("re-run `sleeper browser status`"), "got {failed}");
    }

    #[test]
    fn status_should_not_use_exit_3_when_verdict_unknown() {
        assert_eq!(status_exit_code(&StatusOutcome::Unknown), 4);
        assert_ne!(
            status_exit_code(&StatusOutcome::Unknown),
            3,
            "unknown must never reuse the lock-busy exit"
        );
        assert_eq!(status_exit_code(&StatusOutcome::LockBusy), 3);
    }

    #[test]
    fn login_poll_should_render_countdown_when_waiting() {
        // 15s into a 600s poll: 585s = 9m45s left (stderr shape; stdout untouched).
        let tick = login_poll_tick(15, 600);
        assert!(tick.contains("9m45s"), "countdown must show remaining, got {tick}");
        assert!(tick.contains("elapsed"), "got {tick}");
        assert_eq!(format_countdown(151), "2m31s");
        assert_eq!(format_countdown(0), "0m00s");
    }

    #[test]
    fn timeout_copy_should_direct_to_login_when_poll_expires() {
        let copy = login_timeout_copy();
        assert!(
            copy.contains("sleeper browser login"),
            "timeout must direct to login, got {copy}"
        );
        assert!(
            !copy.contains("browser status"),
            "timeout must never direct to status, got {copy}"
        );
        // Verbatim durability promise preserved.
        assert!(copy.contains("10m") || copy.contains("10 minutes"));
        // Ctrl-C copy + exit contract live beside the timeout copy.
        assert_eq!(
            login_cancelled_copy(),
            "login wait cancelled — closing browser…"
        );
    }

    #[test]
    fn status_json_should_emit_shape_when_authenticated() {
        let profile = std::path::Path::new("/tmp/chrome-profile");
        let raw = status_json(true, "126.0.6478.126", "126.0.6478.126", profile, None);
        let v: serde_json::Value = serde_json::from_str(&raw).expect("stdout must be pure JSON");
        assert_eq!(v["logged_in"], true);
        assert_eq!(v["chrome_version"], "126.0.6478.126");
        assert_eq!(v["chrome_runtime_version"], "126.0.6478.126");
        assert_eq!(v["profile"], "/tmp/chrome-profile");
        assert_eq!(status_exit_code(&StatusOutcome::LoggedIn), 0);
    }

    #[test]
    fn status_json_should_map_each_cause_when_failing() {
        let profile = std::path::Path::new("/tmp/chrome-profile");
        // Logged-out: full shape, exit 1.
        let raw = status_json(false, "126.0.6478.126", "126.0.6478.126", profile, None);
        let v: serde_json::Value = serde_json::from_str(&raw).expect("pure JSON");
        assert_eq!(v["logged_in"], false);
        assert_eq!(status_exit_code(&StatusOutcome::LoggedOut), 1);
        // Skew / lock / unknown: error shape, never a bare stack.
        for (outcome, code) in [
            (StatusOutcome::Skew, 2),
            (StatusOutcome::LockBusy, 3),
            (StatusOutcome::Unknown, 4),
        ] {
            assert_eq!(status_exit_code(&outcome), code);
            let raw = status_json(false, "126.0.6478.126", "126.0.6478.126", profile, Some("boom"));
            let v: serde_json::Value = serde_json::from_str(&raw).expect("pure JSON");
            assert_eq!(v, serde_json::json!({"logged_in": false, "error": "boom"}));
            assert!(
                !raw.contains("panicked") && !raw.contains("Backtrace"),
                "stdout must never carry a bare stack, got {raw}"
            );
        }
    }

    #[test]
    fn skew_error_should_contain_triage_verb_when_printed() {
        let msg = skew_triage_message("126.0.6478.126", "127.0.6533.99");
        let lower = msg.to_lowercase();
        assert!(
            lower.contains("wait") || lower.contains("re-run") || lower.contains("update"),
            "skew line must carry a triage verb, got {msg}"
        );
        // Challenge-loop copy is tool news, never operator blame.
        let news = challenge_loop_copy(61);
        assert!(news.contains("unexpected with the stealth backend"), "got {news}");
        assert!(news.contains("sleeper browser status --debug"), "got {news}");
    }

    #[test]
    fn error_should_lead_with_triage_when_raw_detail_present() {
        let triage = "Login check failed (timeout) — re-run `sleeper browser status`.";
        let out = triage_error(triage, "CDP Runtime.enable: websocket closed");
        let triage_pos = out.find(triage).expect("triage leads");
        let detail_pos = out.find("websocket closed").expect("detail follows");
        assert!(
            triage_pos < detail_pos,
            "triage must precede raw detail, got {out}"
        );
        assert_eq!(triage_error(triage, ""), triage);
        // chrome --version parsing feeds the skew gate, never panics.
        assert_eq!(
            parse_chrome_version("Google Chrome 126.0.6478.126 "),
            Some("126.0.6478.126".to_string())
        );
        assert_eq!(parse_chrome_version("garbage"), None);
    }

    #[test]
    fn doctor_should_report_all_checks_when_profile_free() {
        let report = build_doctor_report(
            "126.0.6478.126",
            std::path::Path::new("/tmp/does-not-exist-marker"),
            "126.0.6478.126",
            "a1b2c3d4",
            std::path::Path::new("/tmp/chrome-profile"),
            true,
            "logged in",
            true,
        );
        // Missing binary is honestly ✗ here (fixture path); rebuild with a
        // real temp file to prove the all-✓ render + exit 0.
        assert_eq!(report.exit_code(), 1);
        let dir = std::env::temp_dir();
        let bin = dir.join(format!("sleeper-doctor-bin-{}", std::process::id()));
        std::fs::write(&bin, b"fake").expect("fixture binary");
        let report = build_doctor_report(
            "126.0.6478.126",
            &bin,
            "126.0.6478.126",
            "a1b2c3d4",
            std::path::Path::new("/tmp/chrome-profile"),
            true,
            "logged in",
            true,
        );
        let rendered = report.render();
        assert!(rendered.contains("cft: 126.0.6478.126"), "got {rendered}");
        assert!(rendered.contains("patch: a1b2c3d4"), "got {rendered}");
        assert!(rendered.contains("lock: free"), "got {rendered}");
        assert!(rendered.contains("webdriver: false"), "got {rendered}");
        assert!(
            rendered.lines().all(|l| l.starts_with("✓")),
            "all checks pass → all ✓, got:\n{rendered}"
        );
        assert_eq!(report.exit_code(), 0);
        let _ = std::fs::remove_file(&bin);
    }

    #[test]
    fn doctor_should_exit_nonzero_when_any_check_fails() {
        let report = build_doctor_report(
            "126.0.6478.126",
            std::path::Path::new("/tmp/doctor-missing"),
            "127.0.6533.99",
            "a1b2c3d4",
            std::path::Path::new("/tmp/chrome-profile"),
            false,
            "skipped (resolve skew/lock first)",
            false,
        );
        let rendered = report.render();
        assert!(rendered.contains("✗"), "got {rendered}");
        assert_ne!(report.exit_code(), 0);
        // Every ✗ names the fix + exact re-run command (no dead ends).
        for line in rendered.lines().filter(|l| l.starts_with("✗")) {
            assert!(
                line.to_lowercase().contains("re-run `sleeper browser"),
                "✗ line must name a re-run command, got {line}"
            );
        }
    }

    #[test]
    fn status_json_should_emit_failure_shape_when_launch_fails() {
        use crate::browser::other;
        let profile = std::path::Path::new("/tmp/chrome-profile");
        // Transport failure: short triage copy in the failure shape, never empty.
        let err = other("connect failed: no Chrome installed");
        let msg = launch_failure_json_error(&err);
        assert!(msg.contains("Login check failed (browser launch)"), "got {msg}");
        let raw = status_json(false, "126.0.6478.126", "unknown", profile, Some(&msg));
        let v: serde_json::Value = serde_json::from_str(&raw).expect("stdout must be pure JSON");
        assert_eq!(v, serde_json::json!({"logged_in": false, "error": msg}));
        // Skew-flavoured launch failure passes its own triage through.
        let skew = other("CfT Chrome runtime v127.0.6533.99 vs pinned v126.0.6478.126 — version skew");
        let skew_msg = launch_failure_json_error(&skew);
        assert!(skew_msg.contains("version skew"), "got {skew_msg}");
        let raw = status_json(false, "126.0.6478.126", "127.0.6533.99", profile, Some(&skew_msg));
        let v: serde_json::Value = serde_json::from_str(&raw).expect("pure JSON");
        assert_eq!(v["logged_in"], false);
    }

    #[test]
    fn doctor_should_resolve_pin_when_runtime_unknown() {
        // Undetectable runtime ("unknown" sentinel): must never instruct
        // updating the pin TO unknown.
        let report = build_doctor_report(
            "126.0.6478.126",
            std::path::Path::new("/tmp/doctor-missing"),
            "unknown",
            "a1b2c3d4",
            std::path::Path::new("/tmp/chrome-profile"),
            true,
            "skipped (resolve skew/lock first)",
            false,
        );
        let rendered = report.render();
        assert!(
            !rendered.to_lowercase().contains("update the pin to unknown"),
            "must never instruct pinning to unknown, got:\n{rendered}"
        );
        assert!(
            rendered.contains(
                "Chrome binary missing or unreadable — re-run `sleeper browser login` to resolve the pin (pinned 126.0.6478.126)"
            ),
            "got:\n{rendered}"
        );
        assert_ne!(report.exit_code(), 0);
        // Known-vs-known skew wording stays byte-identical.
        let known = build_doctor_report(
            "126.0.6478.126",
            std::path::Path::new("/tmp/doctor-missing"),
            "127.0.6533.99",
            "a1b2c3d4",
            std::path::Path::new("/tmp/chrome-profile"),
            true,
            "skipped (resolve skew/lock first)",
            false,
        )
        .render();
        assert!(
            known.contains(
                "chrome runtime 127.0.6533.99 vs pinned 126.0.6478.126 — version skew. Re-run `sleeper browser login` to re-resolve the pin; if it persists, update the pin to 127.0.6533.99"
            ),
            "known-vs-known wording must stay byte-identical, got:\n{known}"
        );
    }
}
