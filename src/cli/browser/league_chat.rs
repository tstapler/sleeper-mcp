//! League chat read path (discovery, 2026-11-12): Sleeper's League Chat has
//! no league-specific query. Live GraphQL introspection (enabled in prod —
//! schema is fully walkable via `__schema`/`__type`, with every meta-field
//! snake_cased too, e.g. `query_type` not `queryType`) showed the `League`
//! type carries `last_message_id`/`last_message_text`, the same shape as
//! the `Channel`/`Topic` types, both of which resolve their feed through a
//! generic `messages(parent_id: ...)` field. Tried the league_id itself as
//! `parent_id` directly — confirmed live: it returns the real league chat,
//! a mix of human messages and system-authored transaction/trade-block
//! notices. See `league_chat_via_seam` (`src/browser/session/flows.rs`) for
//! the query and pagination details.
use crate::browser::session::league_chat_via_seam;
use crate::config::Config;
use crate::db::{LeagueChatMessageRow, StatsDb};
use crate::error::Result;

use super::super::LeagueChatAction;
use super::login::{debug_detail, launch_failure_error, launch_session, triage_error, LaunchOptions};
use super::pending::graphql_error_message;

/// Page size Sleeper's `messages` field returns (confirmed live, no `limit`
/// arg exists on it) — a page with fewer rows than this means the feed's
/// beginning has been reached.
const PAGE_SIZE: usize = 50;

/// One chat message, parsed. `author_id`/`author_name` of
/// `"166666666666666666"`/`"sys"` are Sleeper's own system-generated
/// transaction/trade-block notices mixed into the same feed as real human
/// messages — stored as-is rather than filtered out, since they're
/// genuinely part of the league's chat history.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ChatMessage {
    pub(crate) message_id: String,
    pub(crate) author_id: Option<String>,
    pub(crate) author_name: Option<String>,
    pub(crate) text: String,
    pub(crate) created: i64,
    pub(crate) pinned: bool,
}

/// Pure, no I/O: parses one `messages` GraphQL response page.
pub(crate) fn parse_chat_messages(raw: &str) -> Result<Vec<ChatMessage>> {
    let v: serde_json::Value = serde_json::from_str(raw).map_err(crate::browser::other)?;
    if let Some(msg) = graphql_error_message(&v) {
        return Err(crate::browser::other(format!("messages query failed: {msg}")));
    }
    let entries = v.pointer("/data/messages").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    Ok(entries
        .into_iter()
        .filter_map(|e| {
            Some(ChatMessage {
                message_id: e.get("message_id")?.as_str()?.to_string(),
                author_id: e.get("author_id").and_then(|v| v.as_str()).map(str::to_string),
                author_name: e.get("author_display_name").and_then(|v| v.as_str()).map(str::to_string),
                text: e.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                created: e.get("created").and_then(|v| v.as_i64()).unwrap_or(0),
                pinned: e.get("pinned").and_then(|v| v.as_bool()).unwrap_or(false),
            })
        })
        .collect())
}

fn chat_failed_triage() -> String {
    "Reading the league chat failed — the live chat GraphQL shape may have changed. Re-run with SLEEPER_BROWSER_DEBUG=1 for detail.".to_string()
}

/// Whether to stop walking backward through pages. Reaching the feed's real
/// start always stops. Otherwise, a page with nothing new stops a normal
/// sync (already caught up) — but not a `force_deep` one, which only stops
/// at the real start or the `--pages` cap, so a deliberate backfill can walk
/// past a prior sync's already-known frontier instead of being blocked by
/// it (see `run_sync`'s doc comment for why that frontier is otherwise a
/// dead end). The `pages_fetched > 1` guard lets page 1 be fully known
/// (e.g. an idle re-sync with nothing new posted) without stopping before
/// a second page is even tried.
fn should_stop_paging(reached_start: bool, page_has_new: bool, pages_fetched: usize, force_deep: bool) -> bool {
    let caught_up = !force_deep && !page_has_new && pages_fetched > 1;
    reached_start || caught_up
}

/// Outcome of one `sync` run, reported back to the CLI.
pub(crate) struct SyncOutcome {
    pub(crate) pages_fetched: usize,
    pub(crate) messages_fetched: usize,
    pub(crate) new_messages: usize,
}

/// One page's worth of work: fetch via the seam, parse, and persist.
/// Returns the parsed page so the caller can decide whether to keep paging.
async fn fetch_and_store_page(
    session: &dyn crate::browser::BrowserSession,
    db: &StatsDb,
    league_id: &str,
    cursor: Option<&str>,
) -> Result<Vec<ChatMessage>> {
    let raw = league_chat_via_seam(session, league_id, cursor).await?;
    let page = parse_chat_messages(&raw)?;
    let rows: Vec<LeagueChatMessageRow<'_>> = page
        .iter()
        .map(|m| LeagueChatMessageRow {
            message_id: &m.message_id,
            author_id: m.author_id.as_deref(),
            author_name: m.author_name.as_deref(),
            text: &m.text,
            created: m.created,
            pinned: m.pinned,
        })
        .collect();
    db.upsert_league_chat_messages(league_id, &rows).await?;
    Ok(page)
}

/// Launches one browser session and paginates backward (newest first via
/// `before`) until either a page comes back with no message this `db`
/// doesn't already have (already caught up with a prior sync — the common
/// case), a page has fewer than `PAGE_SIZE` rows (the feed's real
/// beginning), or `max_pages` is hit (safety cap on a first-ever sync
/// against a league with a long history).
///
/// `force_deep` disables the already-caught-up stop: a normal sync always
/// restarts from the newest message, so without this it can never advance
/// past a prior sync's frontier (every page in that already-known range
/// looks "fully known" and stops the walk immediately) — leaving anything
/// older than the first sync's `--pages` cap permanently unreachable. With
/// `force_deep`, only `reached_start` or `max_pages` stops the walk.
pub(crate) async fn run_sync(
    config: &Config,
    db: &StatsDb,
    league_id: &str,
    max_pages: usize,
    force_deep: bool,
) -> Result<SyncOutcome> {
    let known_ids = db.league_chat_message_ids(league_id).await?;

    let (mut session, launched) = match launch_session(config, LaunchOptions::headless()).await {
        Ok(pair) => pair,
        Err(e) => return Err(launch_failure_error(&e)),
    };

    let mut cursor: Option<String> = None;
    let mut pages_fetched = 0usize;
    let mut messages_fetched = 0usize;
    let mut new_messages = 0usize;
    let mut sync_err = None;

    for _ in 0..max_pages {
        let page = match fetch_and_store_page(&*session, db, league_id, cursor.as_deref()).await {
            Ok(page) => page,
            Err(e) => {
                sync_err = Some(e);
                break;
            }
        };
        pages_fetched += 1;
        messages_fetched += page.len();
        new_messages += page.iter().filter(|m| !known_ids.contains(&m.message_id)).count();

        let page_has_new = page.iter().any(|m| !known_ids.contains(&m.message_id));
        let reached_start = page.len() < PAGE_SIZE;
        let Some(oldest) = page.last().map(|m| m.message_id.clone()) else { break };
        if should_stop_paging(reached_start, page_has_new, pages_fetched, force_deep) {
            break;
        }
        cursor = Some(oldest);
    }

    session.close().await?;
    launched.close().await?;

    if let Some(e) = sync_err {
        if e.to_string() == crate::browser::session::DEAD_SESSION_HINT {
            return Err(e);
        }
        return Err(crate::browser::other(triage_error(&chat_failed_triage(), &debug_detail(&e.to_string()))));
    }
    Ok(SyncOutcome { pages_fetched, messages_fetched, new_messages })
}

pub(crate) async fn cmd_league_chat(config: &Config, db: &StatsDb, action: LeagueChatAction) -> Result<()> {
    match action {
        LeagueChatAction::Sync { league, pages, force_deep } => {
            let outcome = run_sync(config, db, &league, pages, force_deep).await?;
            println!(
                "synced {} pages, {} messages fetched, {} new",
                outcome.pages_fetched, outcome.messages_fetched, outcome.new_messages,
            );
            Ok(())
        }
        LeagueChatAction::List { league, limit } => {
            let messages = db.list_league_chat_messages(&league, limit).await?;
            if messages.is_empty() {
                println!("no league chat messages cached — run `sleeper league-chat sync --league {league}` first");
                return Ok(());
            }
            for m in &messages {
                let when = chrono::DateTime::from_timestamp_millis(m.created)
                    .map(|t| t.to_rfc3339())
                    .unwrap_or_default();
                let who = m.author_name.as_deref().unwrap_or("?");
                println!("{when}  {who:<16}  {}", m.text);
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_stop_paging_should_stop_at_the_real_feed_start_regardless_of_force_deep() {
        assert!(should_stop_paging(true, true, 5, false));
        assert!(should_stop_paging(true, true, 5, true));
    }

    #[test]
    fn should_stop_paging_should_stop_a_normal_sync_once_a_page_has_nothing_new() {
        assert!(should_stop_paging(false, false, 2, false));
    }

    #[test]
    fn should_stop_paging_should_not_stop_page_one_even_when_fully_known() {
        // An idle re-sync where the newest page has nothing new shouldn't
        // stop before a second page is even tried.
        assert!(!should_stop_paging(false, false, 1, false));
    }

    #[test]
    fn should_stop_paging_should_keep_walking_past_a_fully_known_page_when_force_deep() {
        assert!(!should_stop_paging(false, false, 5, true));
    }

    #[test]
    fn parse_chat_messages_should_parse_a_real_shaped_page() {
        let raw = r#"{"data":{"messages":[
            {"message_id":"1426375922830839808","author_id":"166666666666666666","author_display_name":"sys","text":"dehnfranklincj made a roster move: Added D. Uiagalelei (QB - LAC)","created":1794437030088,"edited":null,"pinned":false,"parent_type":"league"},
            {"message_id":"1413320350720180224","author_id":"710246355305558016","author_display_name":"dehnfranklincj","text":"you almost got me there","created":1791324339136,"edited":null,"pinned":false,"parent_type":"league"}
        ]}}"#;
        let msgs = parse_chat_messages(raw).expect("must parse");
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].author_name.as_deref(), Some("sys"));
        assert_eq!(msgs[1].author_name.as_deref(), Some("dehnfranklincj"));
        assert_eq!(msgs[1].text, "you almost got me there");
        assert!(!msgs[0].pinned);
    }

    #[test]
    fn parse_chat_messages_should_default_missing_text_to_empty_string_not_fail() {
        // Live-observed: a message with only an attachment (no text) comes
        // back with no `text` field at all, not an empty string.
        let raw = r#"{"data":{"messages":[
            {"message_id":"1","author_id":"2","author_display_name":"BlakeMoen","created":1791324385942,"pinned":false}
        ]}}"#;
        let msgs = parse_chat_messages(raw).expect("must parse");
        assert_eq!(msgs[0].text, "");
    }

    #[test]
    fn parse_chat_messages_should_surface_graphql_error_when_unauthorized() {
        let raw = r#"{"errors":[{"message":"Unauthorized"}]}"#;
        let err = parse_chat_messages(raw).expect_err("must error");
        assert!(err.to_string().contains("Unauthorized"));
    }

    #[test]
    fn parse_chat_messages_should_return_empty_when_no_messages_key_present() {
        let raw = r#"{"data":{}}"#;
        assert_eq!(parse_chat_messages(raw).expect("must parse"), vec![]);
    }

    /// Operator-gated live run: real browser + real league. Live-verified
    /// 2026-11-12 against this exact league via a throwaway discovery
    /// script (deleted) — the league chat feed round-trips end to end,
    /// including real human messages mixed with system-generated
    /// transaction notices, and `before`-cursor pagination. Re-run by hand
    /// with `cargo test --lib -- --ignored league_chat_live --nocapture`.
    #[tokio::test]
    #[ignore = "operator-gated: needs live Chrome + authenticated profile"]
    async fn league_chat_live_should_sync_real_messages_from_a_real_league() {
        let config = Config::default();
        let db = crate::db::StatsDb::open(&config.stats_db_path()).await.expect("db must open");

        let outcome = run_sync(&config, &db, "1352883872131481600", 5, false)
            .await
            .expect("sync must succeed against a live, logged-in session");
        println!(
            "{} pages, {} messages, {} new",
            outcome.pages_fetched, outcome.messages_fetched, outcome.new_messages
        );
        assert!(outcome.messages_fetched > 0, "this league's real chat is not empty");

        let messages = db.list_league_chat_messages("1352883872131481600", 10).await.expect("list must succeed");
        assert!(!messages.is_empty());
        for m in &messages {
            println!("{:?}", m);
        }
    }
}
