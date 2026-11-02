//! Dynasty-format command output: tiered roster report, waiver scouting, and
//! the win-win trade finder's table rendering.
use console::style;

use crate::analysis::AnalysisService;
use crate::error::Result;

pub(super) async fn cmd_dynasty(svc: &AnalysisService, league_id: &str, roster_id: i32) -> Result<()> {
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

pub(super) async fn cmd_dynasty_scout(
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

pub(super) async fn cmd_find_trades(
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
