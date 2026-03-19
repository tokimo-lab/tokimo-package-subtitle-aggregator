use std::path::PathBuf;

use async_trait::async_trait;
use regex::Regex;
use scraper::{Html, Selector};

use super::SubtitleProvider;
use crate::archive::extract_archive;
use crate::models::{
    matches_preferred_language, DownloadedSubtitle, SubtitleDownloadRequest,
    SubtitleSearchRequest, SubtitleSearchResult,
};

const SUBS4SERIES_BASE: &str = "https://www.subs4series.com";
const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

pub struct Subs4SeriesProvider {
    staging_root: PathBuf,
}

impl Subs4SeriesProvider {
    pub fn new(staging_root: impl Into<PathBuf>) -> Self {
        Self {
            staging_root: staging_root.into(),
        }
    }
}

fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .cookie_store(true)
        .build()
        .map_err(|e| format!("Failed to build subs4series client: {e}"))
}

fn parse_season_episode(query: &str) -> Option<(u32, u32)> {
    let re = Regex::new(r"[Ss](\d+)[Ee](\d+)").unwrap();
    re.captures(query).map(|cap| {
        let season = cap[1].parse::<u32>().unwrap_or(1);
        let episode = cap[2].parse::<u32>().unwrap_or(1);
        (season, episode)
    })
}

#[async_trait]
impl SubtitleProvider for Subs4SeriesProvider {
    fn name(&self) -> &str {
        "subs4series"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let query = request
            .query
            .as_deref()
            .ok_or("subs4series requires query")?;

        let (season, episode) = parse_season_episode(query).ok_or(
            "subs4series: query must contain season/episode pattern (e.g. S01E02)",
        )?;

        // Strip S##E## from query for show title search
        let title_re = Regex::new(r"\s*[Ss]\d+[Ee]\d+.*").unwrap();
        let show_title = title_re.replace(query, "").trim().to_string();

        let encoded =
            url::form_urlencoded::byte_serialize(show_title.as_bytes()).collect::<String>();
        let search_url =
            format!("{SUBS4SERIES_BASE}/search_report.php?search={encoded}&searchType=1");

        let client = build_client()?;
        let resp = client
            .get(&search_url)
            .send()
            .await
            .map_err(|e| format!("subs4series search request failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("subs4series search failed: {}", resp.status()));
        }

        let html = resp
            .text()
            .await
            .map_err(|e| format!("subs4series read error: {e}"))?;

        let (show_link, show_path) = {
            let document = Html::parse_document(&html);
            let option_sel = Selector::parse("select[name=\"Mov_sel\"] > option[value]")
                .map_err(|e| format!("subs4series selector error: {e}"))?;
            let link = document
                .select(&option_sel)
                .find_map(|opt| {
                    let val = opt.value().attr("value").unwrap_or("").trim().to_string();
                    if !val.is_empty() && val != "#" {
                        Some(val)
                    } else {
                        None
                    }
                })
                .ok_or("subs4series: no show options found")?;

            // Extract show path parts: tv-series/show-name-123
            let parts: Vec<&str> = link.trim_end_matches('/').split('/').rev().take(2).collect();
            let path = if parts.len() == 2 {
                format!("{}/{}", parts[1], parts[0])
            } else {
                link.trim_start_matches('/').to_string()
            };
            (link, path)
        };
        let _ = show_link;

        let episode_url = format!(
            "{SUBS4SERIES_BASE}/{show_path}/season-{season}/episode-{episode}"
        );

        let ep_resp = client
            .get(&episode_url)
            .send()
            .await
            .map_err(|e| format!("subs4series episode request failed: {e}"))?;

        if ep_resp.status().as_u16() == 404 {
            return Ok(Vec::new());
        }
        if !ep_resp.status().is_success() {
            return Err(format!("subs4series episode request failed: {}", ep_resp.status()));
        }

        let ep_html = ep_resp
            .text()
            .await
            .map_err(|e| format!("subs4series read episode error: {e}"))?;

        let ep_doc = Html::parse_document(&ep_html);

        let row_sel =
            Selector::parse("table .seeDark, table .seeMedium")
                .map_err(|e| format!("subs4series selector error: {e}"))?;
        let b_sel =
            Selector::parse("b").map_err(|e| format!("subs4series selector error: {e}"))?;
        let a_sel =
            Selector::parse("a").map_err(|e| format!("subs4series selector error: {e}"))?;
        let img_sel =
            Selector::parse("img").map_err(|e| format!("subs4series selector error: {e}"))?;

        let mut results = Vec::new();

        for row in ep_doc.select(&row_sel) {
            let version = row
                .select(&b_sel)
                .next()
                .map(|b| b.text().collect::<String>().trim().to_string())
                .unwrap_or_default();

            let href = row
                .select(&a_sel)
                .next()
                .and_then(|a| a.value().attr("href"))
                .unwrap_or("");

            if href.is_empty() {
                continue;
            }

            let download_link = if href.starts_with("http") {
                href.to_string()
            } else {
                format!("{SUBS4SERIES_BASE}/{}", href.trim_start_matches('/'))
            };

            let lang_code = row
                .select(&img_sel)
                .next()
                .and_then(|img| img.value().attr("src"))
                .and_then(|src| src.rsplit('/').next())
                .map(|fname| {
                    fname
                        .trim_end_matches(".png")
                        .trim_end_matches(".gif")
                        .to_lowercase()
                })
                .unwrap_or_else(|| "el".to_string());

            if !matches_preferred_language(&lang_code, request.languages.as_deref()) {
                continue;
            }

            let id = format!(
                "subs4series-{}",
                href.trim_end_matches('/').rsplit('/').next().unwrap_or(&version)
            );

            results.push(SubtitleSearchResult {
                id,
                name: version,
                language: lang_code,
                language_name: "Greek".into(),
                format: "srt".into(),
                provider: "subs4series".into(),
                detail_path: None,
                download_path: Some(download_link),
                download_count: None,
                rating: None,
                movie_name: None,
                release_group: None,
            });
        }

        Ok(results)
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        let download_url = request
            .download_path
            .as_deref()
            .ok_or("subs4series: download_path required")?;

        let client = build_client()?;

        let resp = client
            .get(download_url)
            .send()
            .await
            .map_err(|e| format!("subs4series GET failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("subs4series GET failed: {}", resp.status()));
        }

        let html = resp
            .text()
            .await
            .map_err(|e| format!("subs4series read error: {e}"))?;

        let post_url = {
            let document = Html::parse_document(&html);
            let link_sel = Selector::parse("a.style55ws")
                .map_err(|e| format!("subs4series selector error: {e}"))?;
            let form_sel = Selector::parse("form[method=\"post\"]")
                .map_err(|e| format!("subs4series selector error: {e}"))?;

            let target = if let Some(a) = document.select(&link_sel).next() {
                a.value().attr("href").unwrap_or("").to_string()
            } else if let Some(form) = document.select(&form_sel).next() {
                form.value().attr("action").unwrap_or("").to_string()
            } else {
                return Err("subs4series: no download target found".into());
            };

            if target.starts_with("http") {
                target
            } else {
                format!("{SUBS4SERIES_BASE}/{}", target.trim_start_matches('/'))
            }
        };

        let post_resp = client
            .post(&post_url)
            .header("Referer", download_url)
            .send()
            .await
            .map_err(|e| format!("subs4series POST failed: {e}"))?;

        if !post_resp.status().is_success() {
            return Err(format!("subs4series POST failed: {}", post_resp.status()));
        }

        let content = post_resp
            .bytes()
            .await
            .map_err(|e| format!("subs4series read content error: {e}"))?;

        extract_archive(&content, "subtitle.zip", &request.language, &self.staging_root).await
    }
}
