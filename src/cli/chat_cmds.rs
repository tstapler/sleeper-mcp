//! Durable analysis log + chat-transcript import (`sleeper chat ...`). See
//! `ChatAction` in `src/cli/mod.rs` for the subcommand shapes.
use console::style;

use crate::db::StatsDb;
use crate::error::Result;

use super::ChatAction;

pub(super) async fn cmd_chat(db: &StatsDb, action: ChatAction) -> Result<()> {
    match action {
        ChatAction::Log { league, topic, summary, detail } => {
            cmd_log(db, &league, &topic, &summary, detail.as_deref()).await
        }
        ChatAction::List { league, limit } => cmd_list(db, league.as_deref(), limit).await,
        ChatAction::Import { path, league } => cmd_import(db, &path, league.as_deref()).await,
        ChatAction::Transcripts { league, limit } => cmd_transcripts(db, league.as_deref(), limit).await,
    }
}

async fn cmd_log(
    db: &StatsDb,
    league: &str,
    topic: &str,
    summary: &str,
    detail: Option<&str>,
) -> Result<()> {
    let id = db.log_analysis_note(league, topic, summary, detail).await?;
    println!("{} id={id}", style("logged").green());
    Ok(())
}

/// Display summaries longer than this are truncated with an ellipsis — the
/// full text is still what's stored; this only shortens the terminal line.
const SUMMARY_DISPLAY_LEN: usize = 80;

fn truncate_for_display(s: &str) -> String {
    if s.chars().count() <= SUMMARY_DISPLAY_LEN {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(SUMMARY_DISPLAY_LEN).collect();
        format!("{truncated}…")
    }
}

async fn cmd_list(db: &StatsDb, league: Option<&str>, limit: i64) -> Result<()> {
    let notes = db.list_analysis_notes(league, limit).await?;
    if notes.is_empty() {
        println!("no analysis notes logged");
        return Ok(());
    }
    println!("{}", style("Analysis Notes").bold().underlined());
    for n in &notes {
        println!(
            "{:>4}  {}  {:<12}  {}",
            n.id,
            n.created_at,
            style(&n.topic).cyan(),
            truncate_for_display(&n.summary),
        );
    }
    Ok(())
}

async fn cmd_import(db: &StatsDb, path: &std::path::Path, league: Option<&str>) -> Result<()> {
    let raw_text = tokio::fs::read_to_string(path).await?;
    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown-session")
        .to_string();
    let source_path = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned();

    db.import_chat_transcript(&session_id, league, &source_path, &raw_text).await?;
    println!(
        "{} session_id={session_id} bytes={}",
        style("imported").green(),
        raw_text.len(),
    );
    Ok(())
}

fn human_size(bytes: i64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    let b = bytes as f64;
    if b >= MB {
        format!("{:.1}MB", b / MB)
    } else if b >= KB {
        format!("{:.1}KB", b / KB)
    } else {
        format!("{bytes}B")
    }
}

async fn cmd_transcripts(db: &StatsDb, league: Option<&str>, limit: i64) -> Result<()> {
    let transcripts = db.list_chat_transcripts(league, limit).await?;
    if transcripts.is_empty() {
        println!("no transcripts imported");
        return Ok(());
    }
    println!("{}", style("Imported Transcripts").bold().underlined());
    for t in &transcripts {
        println!(
            "{}  {}  {:>8}  {}",
            t.session_id,
            t.imported_at,
            human_size(t.size_bytes),
            t.source_path,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_for_display_should_leave_a_short_summary_unchanged() {
        assert_eq!(truncate_for_display("short summary"), "short summary");
    }

    #[test]
    fn truncate_for_display_should_truncate_a_long_summary_with_an_ellipsis() {
        let long = "x".repeat(200);
        let displayed = truncate_for_display(&long);
        assert_eq!(displayed.chars().count(), SUMMARY_DISPLAY_LEN + 1, "80 chars + ellipsis");
        assert!(displayed.ends_with('…'));
    }

    #[test]
    fn human_size_should_pick_the_right_unit_per_magnitude() {
        assert_eq!(human_size(512), "512B");
        assert_eq!(human_size(2048), "2.0KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0MB");
    }
}
