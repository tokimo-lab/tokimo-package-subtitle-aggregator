use std::path::PathBuf;

use async_trait::async_trait;
use regex::Regex;

use super::SubtitleProvider;
use crate::models::{
    matches_preferred_language, DownloadedSubtitle, SubtitleDownloadRequest,
    SubtitleSearchRequest, SubtitleSearchResult,
};

const XSUBS_BASE: &str = "http://xsubs.tv";
const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

pub struct XSubsProvider {
    #[allow(dead_code)]
    staging_root: PathBuf,
}

impl XSubsProvider {
    pub fn new(staging_root: impl Into<PathBuf>) -> Self {
        Self {
            staging_root: staging_root.into(),
        }
    }
}

fn build_client() -> Result<reqwest::Client, String> {
    let builder = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .cookie_store(true);

    // Optional basic auth via env vars
    if let (Ok(user), Ok(pass)) = (
        std::env::var("XSUBS_USER"),
        std::env::var("XSUBS_PASS"),
    ) {
        // Some xsubs setups use HTTP basic auth; provide as default headers if needed
        let _ = (user, pass); // Store for potential future use
    }

    builder
        .build()
        .map_err(|e| format!("Failed to build xsubs client: {e}"))
}

fn sanitize_title(title: &str) -> String {
    title.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_season_episode(query: &str) -> Option<(u32, u32)> {
    let re = Regex::new(r"[Ss](\d+)[Ee](\d+)").unwrap();
    re.captures(query).map(|cap| {
        let season = cap[1].parse::<u32>().unwrap_or(1);
        let episode = cap[2].parse::<u32>().unwrap_or(1);
        (season, episode)
    })
}

async fn fetch_text(client: &reqwest::Client, url: &str) -> Result<String, String> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("xsubs request failed for {url}: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("xsubs request failed: {} for {url}", resp.status()));
    }

    resp.text()
        .await
        .map_err(|e| format!("xsubs read error: {e}"))
}

#[async_trait]
impl SubtitleProvider for XSubsProvider {
    fn name(&self) -> &str {
        "xsubs"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let query = request
            .query
            .as_deref()
            .ok_or("xsubs requires query")?;

        let (season, episode) = parse_season_episode(query)
            .ok_or("xsubs: query must contain season/episode pattern (e.g. S01E02)")?;

        let title_re = Regex::new(r"\s*[Ss]\d+[Ee]\d+.*").unwrap();
        let show_title = title_re.replace(query, "").trim().to_string();
        let show_title_sanitized = sanitize_title(&show_title);

        let client = build_client()?;

        // Step 1: Get all series
        let all_series_url = format!("{XSUBS_BASE}/series/all.xml");
        let all_xml = fetch_text(&client, &all_series_url).await?;

        // Parse <series srsid="123">Show Name</series>
        let series_re =
            Regex::new(r#"<series\s+srsid="(\d+)"[^>]*>([^<]+)</series>"#).unwrap();

        let show_id = series_re
            .captures_iter(&all_xml)
            .find_map(|cap| {
                let id = cap[1].to_string();
                let name = cap[2].trim().to_string();
                if sanitize_title(&name) == show_title_sanitized {
                    Some(id)
                } else {
                    None
                }
            })
            .ok_or_else(|| format!("xsubs: show not found for '{show_title}'"))?;

        // Step 2: Get show main.xml for season list
        let main_url = format!("{XSUBS_BASE}/series/{show_id}/main.xml");
        let main_xml = fetch_text(&client, &main_url).await?;

        // Parse <series_group ssnnum="1" ssnid="456"/>
        let season_re =
            Regex::new(r#"<series_group\s+ssnnum="(\d+)"\s+ssnid="(\d+)"[^/]*/>"#).unwrap();

        let season_id = season_re
            .captures_iter(&main_xml)
            .find_map(|cap| {
                let num = cap[1].parse::<u32>().unwrap_or(0);
                let id = cap[2].to_string();
                if num == season {
                    Some(id)
                } else {
                    None
                }
            })
            .ok_or_else(|| format!("xsubs: season {season} not found for show {show_id}"))?;

        // Step 3: Get season XML
        let season_url = format!("{XSUBS_BASE}/series/{show_id}/{season_id}.xml");
        let season_xml = fetch_text(&client, &season_url).await?;

        // Parse subtitle entries
        // <subg>
        //   <etitle number="1" title="..."/>
        //   <sgt ssnnum="1" epsid="5"/>
        //   <sr rlsid="789" published_on="..."><fmt>HDTV</fmt><team>Name</team></sr>
        // </subg>

        let subg_re = Regex::new(r"(?s)<subg>(.*?)</subg>").unwrap();
        let etitle_re = Regex::new(r#"<etitle\s+number="(\d+)"[^>]*/>"#).unwrap();
        let sgt_re = Regex::new(r#"<sgt\s+ssnnum="(\d+)"\s+epsid="(\d+)"[^/]*/>"#).unwrap();
        let sr_re = Regex::new(
            r#"<sr\s+rlsid="(\d+)"[^>]*>(?:[^<]*<fmt>([^<]*)</fmt>)?(?:[^<]*<team>([^<]*)</team>)?"#,
        )
        .unwrap();

        let mut results = Vec::new();

        for subg_cap in subg_re.captures_iter(&season_xml) {
            let block = &subg_cap[1];

            // Find episode number
            let ep_num = etitle_re
                .captures(block)
                .and_then(|c| c[1].parse::<u32>().ok())
                .unwrap_or(0);

            // Check sgt for episode match
            let matches_episode = sgt_re.captures_iter(block).any(|c| {
                c[1].parse::<u32>().unwrap_or(0) == season
                    && c[2].parse::<u32>().unwrap_or(0) == episode
            });

            if !matches_episode && ep_num != episode {
                continue;
            }

            if !matches_preferred_language("el", request.languages.as_deref()) {
                continue;
            }

            for sr_cap in sr_re.captures_iter(block) {
                let rlsid = sr_cap[1].to_string();
                let fmt = sr_cap.get(2).map(|m| m.as_str()).unwrap_or("").to_string();
                let team = sr_cap.get(3).map(|m| m.as_str()).unwrap_or("").to_string();

                let version = if !team.is_empty() && !fmt.is_empty() {
                    format!("{fmt}.{team}")
                } else if !team.is_empty() {
                    team.clone()
                } else {
                    rlsid.clone()
                };

                let download_link = format!("{XSUBS_BASE}/xthru/getsub/{rlsid}");
                let id = format!("xsubs-{rlsid}");

                results.push(SubtitleSearchResult {
                    id,
                    name: version,
                    language: "el".into(),
                    language_name: "Greek".into(),
                    format: "srt".into(),
                    provider: "xsubs".into(),
                    detail_path: None,
                    download_path: Some(download_link),
                    download_count: None,
                    rating: None,
                    movie_name: None,
                    release_group: if team.is_empty() { None } else { Some(team) },
                });
            }
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
            .ok_or("xsubs: download_path required")?;

        let client = build_client()?;

        let resp = client
            .get(download_url)
            .send()
            .await
            .map_err(|e| format!("xsubs download failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("xsubs download failed: {}", resp.status()));
        }

        let content = resp
            .bytes()
            .await
            .map_err(|e| format!("xsubs read content error: {e}"))?;

        let name = request
            .name
            .clone()
            .unwrap_or_else(|| "subtitle.srt".to_string());

        Ok(DownloadedSubtitle {
            name,
            format: "srt".into(),
            content: content.to_vec(),
        })
    }
}
