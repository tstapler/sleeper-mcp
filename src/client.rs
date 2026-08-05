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
        let url = format!("{}{}", self.base_url, path);

        // Check cache first
        if let Some((cached, is_fresh)) = self.cache.get("GET", &url).await? {
            if is_fresh {
                tracing::debug!(path, "cache_hit");
                return Ok(serde_json::from_str(&cached.body)?);
            }
            // Stale — try conditional request
            return self
                .do_request_with_validators(&url, cached.etag, cached.last_modified)
                .await;
        }

        self.rate_limiter.acquire().await;

        for attempt in 0..MAX_RETRIES {
            match self.http.get(&url).send().await {
                Ok(resp) if resp.status().as_u16() == 429 => {
                    let retry_after = resp
                        .headers()
                        .get("Retry-After")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(RETRY_BASE_MS * 2u64.pow(attempt) / 1000)
                        * 1000;
                    tracing::warn!(path, attempt, "server_rate_limited");
                    tokio::time::sleep(Duration::from_millis(retry_after)).await;
                    self.rate_limiter.acquire().await;
                }
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    if status >= 500 && attempt < MAX_RETRIES - 1 {
                        let delay = RETRY_BASE_MS * 2u64.pow(attempt);
                        tracing::warn!(path, status, attempt, "server_error_retry");
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        continue;
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
                    let ttl = HttpCache::parse_max_age(cache_control.as_deref())
                        .unwrap_or(self.cache_ttl);

                    let body = resp.text().await?;
                    self.cache
                        .set(
                            "GET",
                            &url,
                            status,
                            &body,
                            etag.as_deref(),
                            last_modified.as_deref(),
                            Some(ttl),
                        )
                        .await?;
                    return Ok(serde_json::from_str(&body)?);
                }
                Err(e) if attempt < MAX_RETRIES - 1 => {
                    let delay = RETRY_BASE_MS * 2u64.pow(attempt);
                    tracing::warn!(path, attempt, error=%e, "request_error_retry");
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
                Err(e) => return Err(e.into()),
            }
        }
        Err(Error::RateLimited { retries: MAX_RETRIES })
    }

    /// Conditional GET using ETag / Last-Modified validators.
    async fn do_request_with_validators(
        &self,
        url: &str,
        etag: Option<String>,
        last_modified: Option<String>,
    ) -> Result<Value> {
        self.rate_limiter.acquire().await;

        let mut req = self.http.get(url);
        if let Some(etag) = &etag {
            req = req.header("If-None-Match", etag);
        }
        if let Some(lm) = &last_modified {
            req = req.header("If-Modified-Since", lm);
        }

        let resp = req.send().await?;
        let status = resp.status().as_u16();

        if status == 304 {
            // Server confirmed cached copy is still current
            tracing::debug!(url, "cache_revalidated");
            let cached = self.cache.get("GET", url).await?.map(|(c, _)| c);
            if let Some(c) = cached {
                return Ok(serde_json::from_str(&c.body)?);
            }
        }

        if status >= 400 {
            return Err(Error::Api {
                status,
                message: resp.text().await.unwrap_or_default(),
            });
        }

        let body = resp.text().await?;
        self.cache.update_body("GET", url, &body).await?;
        Ok(serde_json::from_str(&body)?)
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

    pub async fn get_league_users(&self, league_id: &str) -> Result<Vec<User>> {
        let v = self.get_json(&format!("/league/{league_id}/users")).await?;
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
    async fn get_raw_html(&self, url: &str, ttl_secs: i64) -> Result<String> {
        if let Some((cached, is_fresh)) = self.cache.get("GET", url).await? {
            if is_fresh {
                return Ok(cached.body);
            }
        }
        self.rate_limiter.acquire().await;
        let resp = self.http.get(url)
            .header("User-Agent", "Mozilla/5.0 (compatible; sleeper-mcp/1.0)")
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            return Err(Error::Api { status, message: resp.text().await.unwrap_or_default() });
        }
        let body = resp.text().await?;
        self.cache.set("GET", url, status, &body, None, None, Some(ttl_secs)).await?;
        Ok(body)
    }

    /// RotowWire NFL player news page (25 most recent items, 10-min cache).
    pub async fn get_rotowire_news(&self) -> Result<Vec<NewsItem>> {
        let html = self.get_raw_html(
            "https://www.rotowire.com/football/news.php",
            600,
        ).await?;
        Ok(parse_rotowire_news(&html))
    }

    /// ESPN NFL RSS feed (~25 items, 30-min cache).
    pub async fn get_espn_rss_news(&self) -> Result<Vec<NewsItem>> {
        let xml = self.get_raw_html(
            "https://www.espn.com/espn/rss/nfl/news",
            1800,
        ).await?;
        Ok(parse_espn_rss(&xml))
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

fn parse_espn_rss(xml: &str) -> Vec<NewsItem> {
    let mut items = Vec::new();
    for chunk in xml.split("<item>").skip(1) {
        let title = cdata_or_text(chunk, "title").unwrap_or_default();
        let desc = cdata_or_text(chunk, "description").unwrap_or_default();
        let date = between(chunk, "<pubDate>", "</pubDate>").unwrap_or_default();
        if !title.is_empty() {
            items.push(NewsItem {
                // ESPN articles name players in the title; we match later by name scan.
                player_name: String::new(),
                headline: title,
                body: desc,
                date,
                source: "ESPN".to_string(),
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
