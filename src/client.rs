/// Sleeper API client with SQLite HTTP cache and goroutine-actor rate limiter.
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::cache::HttpCache;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::models::*;
use crate::rate_limit::RateLimiter;

const MAX_RETRIES: u32 = 3;
const RETRY_BASE_MS: u64 = 2000;
const USER_AGENT: &str = "Mozilla/5.0 (compatible; sleeper-mcp/1.0)";

/// Whether a GET may be served from (or revalidated against) this process's own local
/// cache, or must go straight to the network and ask the upstream to skip its own caching
/// too. `ForceFresh` exists for read-after-write — see [`SleeperClient::get_json_fresh`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CachePolicy {
    PreferCache,
    ForceFresh,
}

#[derive(Clone)]
pub struct SleeperClient {
    http: reqwest::Client,
    cache: Arc<HttpCache>,
    rate_limiter: RateLimiter,
    base_url: String,
    cache_ttl: i64,
}

impl SleeperClient {
    pub async fn new(config: &Config) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .gzip(true)
            .build()?;

        let cache = Arc::new(
            HttpCache::open(&config.cache_db_path(), config.cache_ttl_secs).await?,
        );
        let rate_limiter =
            RateLimiter::spawn(&config.rate_limit_db_path(), config.rate_limit_per_minute)
                .await?;

        Ok(Self {
            http,
            cache,
            rate_limiter,
            base_url: config.base_url.clone(),
            cache_ttl: config.cache_ttl_secs,
        })
    }

    async fn get_json(&self, path: &str) -> Result<Value> {
        self.get_json_with(path, CachePolicy::PreferCache).await
    }

    /// Bypasses both this process's own local cache AND asks the upstream
    /// server to skip its own caching (`Cache-Control`/`Pragma: no-cache`) —
    /// for the one real use case normal reads can't serve: confirming a
    /// write actually landed, right after making it (read-after-write).
    /// Normal reads should keep using [`get_json`]; forcing on every call
    /// would defeat the point of caching and burn rate-limit budget for no
    /// reason the vast majority of the time.
    async fn get_json_fresh(&self, path: &str) -> Result<Value> {
        self.get_json_with(path, CachePolicy::ForceFresh).await
    }

    async fn get_json_with(&self, path: &str, policy: CachePolicy) -> Result<Value> {
        let url = format!("{}{}", self.base_url, path);
        let body = self.get_cached_text(&url, self.cache_ttl, policy).await?;
        Ok(serde_json::from_str(&body)?)
    }

    /// Read-through HTTP cache: serves a fresh cached body directly, revalidates a stale one
    /// with ETag/If-None-Match + Last-Modified/If-Modified-Since (cheap 304 on no change,
    /// which also extends the cache's freshness window without a full re-download), and only
    /// does a full fetch when there's nothing cached at all.
    ///
    /// Freshness comes from the response's own `Cache-Control: max-age`, falling back to
    /// `default_ttl_secs` only when the server doesn't send one — this is what makes it a real
    /// read-through cache rather than one that just ignores the site's own caching metadata.
    ///
    /// `CachePolicy::ForceFresh` skips the local-cache read entirely (straight to
    /// [`fetch_fresh`]) — see [`get_json_fresh`]'s doc comment for when that's warranted.
    async fn get_cached_text(&self, url: &str, default_ttl_secs: i64, policy: CachePolicy) -> Result<String> {
        if policy == CachePolicy::PreferCache {
            if let Some((cached, is_fresh)) = self.cache.get("GET", url).await? {
                if is_fresh {
                    tracing::debug!(url, "cache_hit");
                    return Ok(cached.body);
                }
                return self
                    .revalidate(url, cached.etag, cached.last_modified, default_ttl_secs)
                    .await;
            }
        }
        self.fetch_fresh(url, default_ttl_secs, policy).await
    }

    /// `CachePolicy::ForceFresh` adds `Cache-Control`/`Pragma: no-cache` to the outbound
    /// request — best-effort against any CDN/edge cache Sleeper's own API sits behind, which
    /// this process has no other control over (confirmed live, 2026-11-11: a roster write can
    /// take several minutes to show up even on a from-scratch GET with no local caching
    /// involved at all — that lag is on Sleeper's side, not this cache layer's).
    async fn fetch_fresh(&self, url: &str, default_ttl_secs: i64, policy: CachePolicy) -> Result<String> {
        self.rate_limiter.acquire().await;

        for attempt in 0..MAX_RETRIES {
            let mut req = self.http.get(url).header("User-Agent", USER_AGENT);
            if policy == CachePolicy::ForceFresh {
                req = req.header("Cache-Control", "no-cache").header("Pragma", "no-cache");
            }
            match req.send().await {
                Ok(resp) if resp.status().as_u16() == 429 => {
                    let retry_after = resp
                        .headers()
                        .get("Retry-After")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(RETRY_BASE_MS * 2u64.pow(attempt) / 1000)
                        * 1000;
                    tracing::warn!(url, attempt, "server_rate_limited");
                    tokio::time::sleep(Duration::from_millis(retry_after)).await;
                    self.rate_limiter.acquire().await;
                }
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    if status >= 500 && attempt < MAX_RETRIES - 1 {
                        let delay = RETRY_BASE_MS * 2u64.pow(attempt);
                        tracing::warn!(url, status, attempt, "server_error_retry");
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        continue;
                    }
                    if status >= 400 {
                        return Err(Error::Api {
                            status,
                            message: resp.text().await.unwrap_or_default(),
                        });
                    }

                    let headers = resp.headers().clone();
                    let body = resp.text().await?;
                    self.store_response(url, status, &body, &headers, default_ttl_secs).await?;
                    return Ok(body);
                }
                Err(e) if attempt < MAX_RETRIES - 1 => {
                    let delay = RETRY_BASE_MS * 2u64.pow(attempt);
                    tracing::warn!(url, attempt, error=%e, "request_error_retry");
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
                Err(e) => return Err(e.into()),
            }
        }
        Err(Error::RateLimited { retries: MAX_RETRIES })
    }

    /// Shared by [`fetch_fresh`] and [`revalidate`]'s success paths: derives the cache TTL
    /// from the response's own `Cache-Control` (falling back to `default_ttl_secs`) and
    /// persists the body plus validators.
    async fn store_response(
        &self,
        url: &str,
        status: u16,
        body: &str,
        headers: &reqwest::header::HeaderMap,
        default_ttl_secs: i64,
    ) -> Result<()> {
        let etag = header_str(headers, "etag");
        let last_modified = header_str(headers, "last-modified");
        let cache_control = header_str(headers, "cache-control");
        let ttl = HttpCache::parse_max_age(cache_control.as_deref()).unwrap_or(default_ttl_secs);
        self.cache
            .set("GET", url, status, body, etag.as_deref(), last_modified.as_deref(), Some(ttl))
            .await
    }

    /// Conditional GET using ETag / Last-Modified validators.
    async fn revalidate(
        &self,
        url: &str,
        etag: Option<String>,
        last_modified: Option<String>,
        default_ttl_secs: i64,
    ) -> Result<String> {
        self.rate_limiter.acquire().await;

        let mut req = self.http.get(url).header("User-Agent", USER_AGENT);
        if let Some(etag) = &etag {
            req = req.header("If-None-Match", etag);
        }
        if let Some(lm) = &last_modified {
            req = req.header("If-Modified-Since", lm);
        }

        let resp = req.send().await?;
        let status = resp.status().as_u16();

        if status == 304 {
            // Server confirmed the cached copy is still current. Re-write the same body to
            // bump cached_at, extending the freshness window instead of revalidating again on
            // every single call — a 304 is a signal to trust the cache longer, not just once.
            tracing::debug!(url, "cache_revalidated");
            if let Some((cached, _)) = self.cache.get("GET", url).await? {
                self.cache.update_body("GET", url, &cached.body).await?;
                return Ok(cached.body);
            }
        }

        if status >= 400 {
            return Err(Error::Api {
                status,
                message: resp.text().await.unwrap_or_default(),
            });
        }

        let etag = header_str(resp.headers(), "etag");
        let last_modified = header_str(resp.headers(), "last-modified");
        let cache_control = header_str(resp.headers(), "cache-control");
        let ttl = HttpCache::parse_max_age(cache_control.as_deref()).unwrap_or(default_ttl_secs);

        let body = resp.text().await?;
        self.cache
            .set("GET", url, status, &body, etag.as_deref(), last_modified.as_deref(), Some(ttl))
            .await?;
        Ok(body)
    }

    // -----------------------------------------------------------------------
    // API methods
    // -----------------------------------------------------------------------

    pub async fn get_user(&self, identifier: &str) -> Result<User> {
        let v = self.get_json(&format!("/user/{identifier}")).await?;
        Ok(serde_json::from_value(v)?)
    }

    pub async fn get_user_leagues(
        &self,
        user_id: &str,
        season: &str,
        sport: &str,
    ) -> Result<Vec<League>> {
        let v = self
            .get_json(&format!("/user/{user_id}/leagues/{sport}/{season}"))
            .await?;
        Ok(serde_json::from_value(v)?)
    }

    pub async fn get_league(&self, league_id: &str) -> Result<League> {
        let v = self.get_json(&format!("/league/{league_id}")).await?;
        Ok(serde_json::from_value(v)?)
    }

    pub async fn get_league_rosters(&self, league_id: &str) -> Result<Vec<Roster>> {
        let v = self.get_json(&format!("/league/{league_id}/rosters")).await?;
        Ok(serde_json::from_value(v)?)
    }

    /// Read-after-write escape hatch (see [`get_json_fresh`]'s doc comment) — the one read
    /// this project has repeatedly needed right after a lineup/waiver write, to confirm it
    /// actually landed rather than trusting the write call's own success message.
    pub async fn get_league_rosters_fresh(&self, league_id: &str) -> Result<Vec<Roster>> {
        let v = self.get_json_fresh(&format!("/league/{league_id}/rosters")).await?;
        Ok(serde_json::from_value(v)?)
    }

    pub async fn get_league_users(&self, league_id: &str) -> Result<Vec<User>> {
        let v = self.get_json(&format!("/league/{league_id}/users")).await?;
        Ok(serde_json::from_value(v)?)
    }

    /// Only picks that have changed hands at least once appear in the
    /// response (an untraded pick has no row) — see `TradedPick`'s doc
    /// comment for the field meanings, live-verified against this exact
    /// league (22 rows, 2026-11-11).
    pub async fn get_traded_picks(&self, league_id: &str) -> Result<Vec<TradedPick>> {
        let v = self.get_json(&format!("/league/{league_id}/traded_picks")).await?;
        Ok(serde_json::from_value(v)?)
    }

    pub async fn get_nfl_state(&self) -> Result<NflState> {
        let v = self.get_json("/state/nfl").await?;
        Ok(serde_json::from_value(v)?)
    }

    pub async fn get_league_matchups(
        &self,
        league_id: &str,
        week: i32,
    ) -> Result<Vec<Matchup>> {
        let v = self
            .get_json(&format!("/league/{league_id}/matchups/{week}"))
            .await?;
        Ok(serde_json::from_value(v)?)
    }

    pub async fn get_league_transactions(
        &self,
        league_id: &str,
        week: i32,
    ) -> Result<Vec<Transaction>> {
        let v = self
            .get_json(&format!("/league/{league_id}/transactions/{week}"))
            .await?;
        Ok(serde_json::from_value(v)?)
    }

    pub async fn get_trending_players(
        &self,
        kind: &str,
        sport: &str,
        lookback_hours: i32,
        limit: i32,
    ) -> Result<Vec<TrendingPlayer>> {
        let v = self
            .get_json(&format!(
                "/players/{sport}/trending/{kind}?lookback_hours={lookback_hours}&limit={limit}"
            ))
            .await?;
        Ok(serde_json::from_value(v)?)
    }

    /// Returns the full player map (player_id → info). ~2MB, cached aggressively.
    pub async fn get_all_players(&self, sport: &str) -> Result<HashMap<String, Value>> {
        let v = self.get_json(&format!("/players/{sport}")).await?;
        Ok(serde_json::from_value(v)?)
    }

    pub async fn get_player_stats(
        &self,
        season_type: &str,
        season: &str,
        week: i32,
        sport: &str,
    ) -> Result<HashMap<String, Value>> {
        let v = self
            .get_json(&format!("/stats/{sport}/{season_type}/{season}/{week}"))
            .await?;
        Ok(serde_json::from_value(v)?)
    }

    pub async fn get_player_projections(
        &self,
        season_type: &str,
        season: &str,
        week: i32,
        sport: &str,
    ) -> Result<HashMap<String, Value>> {
        let v = self
            .get_json(&format!(
                "/projections/{sport}/{season_type}/{season}/{week}"
            ))
            .await?;
        Ok(serde_json::from_value(v)?)
    }

    pub async fn get_league_drafts(&self, league_id: &str) -> Result<Vec<Draft>> {
        let v = self.get_json(&format!("/league/{league_id}/drafts")).await?;
        Ok(serde_json::from_value(v)?)
    }

    pub async fn get_draft(&self, draft_id: &str) -> Result<Draft> {
        let v = self.get_json(&format!("/draft/{draft_id}")).await?;
        Ok(serde_json::from_value(v)?)
    }

    pub async fn get_draft_picks(&self, draft_id: &str) -> Result<Vec<DraftPick>> {
        let v = self.get_json(&format!("/draft/{draft_id}/picks")).await?;
        Ok(serde_json::from_value(v)?)
    }

    /// Fetch raw HTML/XML from any URL, going through the shared HTTP cache.
    /// Fetch a non-Sleeper URL (news pages, RSS feeds, third-party APIs) through the same
    /// read-through cache as `get_json` — `ttl_secs` is only the fallback used when the site
    /// doesn't send its own `Cache-Control`.
    async fn get_raw_html(&self, url: &str, ttl_secs: i64) -> Result<String> {
        self.get_cached_text(url, ttl_secs, CachePolicy::PreferCache).await
    }

    /// RotowWire NFL player news page (25 most recent items). RotoWire sends
    /// `Cache-Control: no-store, no-cache, must-revalidate` — this is injury/beat news where
    /// staleness has a real cost, so the 10-min default stays short rather than being extended.
    pub async fn get_rotowire_news(&self) -> Result<Vec<NewsItem>> {
        let html = self.get_raw_html(
            "https://www.rotowire.com/football/news.php",
            600,
        ).await?;
        Ok(parse_rotowire_news(&html))
    }

    /// ESPN NFL RSS feed (~25 items). ESPN actually advertises `max-age=184` (~3 min) via
    /// Cache-Control, which now governs real freshness — the 30-min value here is only the
    /// fallback for if that header ever goes missing.
    pub async fn get_espn_rss_news(&self) -> Result<Vec<NewsItem>> {
        let xml = self.get_raw_html(
            "https://www.espn.com/espn/rss/nfl/news",
            1800,
        ).await?;
        Ok(parse_rss_feed(&xml, "ESPN"))
    }

    /// CBS Sports NFL RSS feed. Sends an ETag, so a stale cache revalidates cheaply (304) most
    /// of the time instead of a full re-download.
    pub async fn get_cbssports_news(&self) -> Result<Vec<NewsItem>> {
        let xml = self.get_raw_html(
            "https://www.cbssports.com/rss/headlines/nfl/",
            900,
        ).await?;
        Ok(parse_rss_feed(&xml, "CBS Sports"))
    }

    /// ProFootballTalk (NBC Sports) RSS feed. No Cache-Control/ETag observed from this host, so
    /// the 30-min default fully governs freshness — general beat news, not injury-critical.
    pub async fn get_pft_news(&self) -> Result<Vec<NewsItem>> {
        let xml = self.get_raw_html(
            "https://www.nbcsports.com/profootballtalk.rss",
            1800,
        ).await?;
        Ok(parse_rss_feed(&xml, "ProFootballTalk"))
    }

    /// Yahoo Sports NFL RSS feed. No Cache-Control/ETag observed from this host either.
    pub async fn get_yahoo_news(&self) -> Result<Vec<NewsItem>> {
        let xml = self.get_raw_html(
            "https://sports.yahoo.com/nfl/rss/",
            1800,
        ).await?;
        Ok(parse_rss_feed(&xml, "Yahoo Sports"))
    }

    /// FantasyCalc dynasty trade values, keyed by `sleeperId` in the response — no crosswalk
    /// needed. Offense-only (QB/RB/WR/TE); does not cover IDP. 1-hour cache since values move
    /// slowly day-to-day.
    pub async fn get_trade_values(
        &self,
        is_dynasty: bool,
        num_qbs: u32,
        num_teams: u32,
        ppr: f64,
    ) -> Result<Vec<Value>> {
        let url = format!(
            "https://api.fantasycalc.com/values/current?isDynasty={}&numQbs={}&numTeams={}&ppr={}",
            is_dynasty, num_qbs, num_teams, ppr
        );
        let body = self.get_raw_html(&url, 3600).await?;
        Ok(serde_json::from_str(&body)?)
    }
}

fn header_str(headers: &reqwest::header::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

fn parse_rotowire_news(html: &str) -> Vec<NewsItem> {
    use scraper::{Html, Selector};

    let doc = Html::parse_document(html);
    let update_sel = Selector::parse(".news-update").unwrap();
    let player_sel = Selector::parse(".news-update__player-link").unwrap();
    let headline_sel = Selector::parse(".news-update__headline").unwrap();
    let main_sel = Selector::parse(".news-update__main").unwrap();

    let mut items = Vec::new();
    for update in doc.select(&update_sel) {
        let player_name = update
            .select(&player_sel)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .unwrap_or_default();
        if player_name.is_empty() {
            continue;
        }

        let headline = update
            .select(&headline_sel)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .unwrap_or_default();

        let raw = update
            .select(&main_sel)
            .next()
            .map(|el| el.text().collect::<String>())
            .unwrap_or_default();

        // Strip the paywalled "ANALYSISSubscribe..." suffix.
        let raw = raw.split("ANALYSIS").next().unwrap_or("").trim().to_string();

        // First line is the date ("May 15, 2026"), rest is the body.
        let (date, body) = match raw.find('\n') {
            Some(i) => (raw[..i].trim().to_string(), raw[i..].trim().to_string()),
            None => (String::new(), raw),
        };
        // Collapse internal whitespace in body.
        let body = body.split_whitespace().collect::<Vec<_>>().join(" ");

        items.push(NewsItem { player_name, headline, body, date, source: "RotowWire".to_string() });
    }
    items
}

/// Generic RSS 2.0 `<item>` parser shared by every RSS-shaped source (ESPN, CBS Sports,
/// ProFootballTalk, Yahoo Sports). None of these feeds name a specific player in a dedicated
/// field, so items carry an empty `player_name` — `fetch_news_for_players` matches them by
/// scanning the headline for a known player name instead.
fn parse_rss_feed(xml: &str, source: &str) -> Vec<NewsItem> {
    let mut items = Vec::new();
    for chunk in xml.split("<item>").skip(1) {
        let title = cdata_or_text(chunk, "title").unwrap_or_default();
        let desc = cdata_or_text(chunk, "description").unwrap_or_default();
        let date = between(chunk, "<pubDate>", "</pubDate>")
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        if !title.is_empty() {
            items.push(NewsItem {
                player_name: String::new(),
                headline: title,
                body: desc,
                date,
                source: source.to_string(),
            });
        }
    }
    items
}

fn cdata_or_text(html: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let inner = between(html, &open, &close)?;
    // Strip CDATA wrapper if present.
    if let Some(s) = inner.strip_prefix("<![CDATA[") {
        Some(s.trim_end_matches("]]>").trim().to_string())
    } else {
        Some(inner.trim().to_string())
    }
}

fn between<'a>(s: &'a str, start: &str, end: &str) -> Option<String> {
    let i = s.find(start)? + start.len();
    let j = s[i..].find(end)?;
    Some(s[i..i + j].to_string())
}
