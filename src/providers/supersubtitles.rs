use async_trait::async_trait;
use serde::Deserialize;

use super::SubtitleProvider;
use crate::models::{
    DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult,
};

const SUPERSUBTITLES_BASE: &str = "https://www.feliratok.eu";
const STAGING_ROOT: &str = "/tmp/subtitle-aggregator";

#[derive(Debug, Deserialize)]
struct AutocompleteItem {
    name: String,
    #[serde(rename = "ID")]
    id: String,
}

#[derive(Debug, Deserialize)]
struct XbmcItem {
    language: Option<String>,
    nev: Option<String>,
    fnev: Option<String>,
    felirat: Option<serde_json::Value>,
    evad: Option<serde_json::Value>,
    ep: Option<serde_json::Value>,
    feltolto: Option<String>,
    #[allow(dead_code)]
    evadpakk: Option<serde_json::Value>,
}

fn language_code(lang: &str) -> (&str, &str) {
    match lang {
        "Magyar" => ("hu", "Hungarian"),
        "Angol" => ("en", "English"),
        _ => ("hu", "Hungarian"),
    }
}

fn val_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

pub struct SuperSubtitlesProvider;

impl Default for SuperSubtitlesProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SuperSubtitlesProvider {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SubtitleProvider for SuperSubtitlesProvider {
    fn name(&self) -> &str {
        "supersubtitles"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let query = request
            .query
            .as_deref()
            .ok_or("supersubtitles: query is required")?;

        let client = reqwest::Client::builder()
            .user_agent("subtitle-aggregator/0.1")
            .build()
            .map_err(|e| format!("supersubtitles: failed to build client: {e}"))?;

        // Try to detect season/episode from query (e.g. "Series Name S01E02")
        let (series_name, season, episode) = parse_series_query(query);

        if season > 0 {
            // Series search
            self.search_series(&client, &series_name, season, episode)
                .await
        } else {
            // Movie search
            self.search_movie(&client, query).await
        }
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        let url = request
            .download_path
            .as_deref()
            .ok_or("supersubtitles: download_path is required")?;

        let client = reqwest::Client::builder()
            .user_agent("subtitle-aggregator/0.1")
            .build()
            .map_err(|e| format!("supersubtitles: failed to build client: {e}"))?;

        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("supersubtitles: download failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "supersubtitles: HTTP {}",
                response.status().as_u16()
            ));
        }

        let content = response
            .bytes()
            .await
            .map_err(|e| format!("supersubtitles: failed to read content: {e}"))?;

        let name = request
            .name
            .clone()
            .unwrap_or_else(|| format!("supersubtitles_{}.zip", request.subtitle_id));

        let staging = std::path::Path::new(STAGING_ROOT);
        crate::archive::extract_archive(&content, &name, &request.language, staging).await
    }
}

impl SuperSubtitlesProvider {
    async fn search_series(
        &self,
        client: &reqwest::Client,
        series_name: &str,
        season: u32,
        episode: u32,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        // Step 1: Autocomplete to get series ID
        let ac_url = format!(
            "{}/index.php?term={}&nyelv=0&action=autoname",
            SUPERSUBTITLES_BASE,
            urlencoding::encode(series_name)
        );

        let ac_resp = client
            .get(&ac_url)
            .send()
            .await
            .map_err(|e| format!("supersubtitles autocomplete failed: {e}"))?;

        if !ac_resp.status().is_success() {
            return Err(format!(
                "supersubtitles autocomplete HTTP {}",
                ac_resp.status().as_u16()
            ));
        }

        let items: Vec<AutocompleteItem> = ac_resp
            .json()
            .await
            .map_err(|e| format!("supersubtitles: failed to parse autocomplete: {e}"))?;

        let series_name_lower = series_name.to_lowercase();
        let series_id = items
            .iter()
            .find(|item| item.name.to_lowercase().contains(&series_name_lower))
            .or_else(|| items.first())
            .map(|item| item.id.clone())
            .ok_or("supersubtitles: series not found")?;

        // Step 2: Get episode subtitles
        let xbmc_url = format!(
            "{}/index.php?action=xbmc&sid={}&ev={}&rtol={}",
            SUPERSUBTITLES_BASE, series_id, season, episode
        );

        let xbmc_resp = client
            .get(&xbmc_url)
            .send()
            .await
            .map_err(|e| format!("supersubtitles xbmc request failed: {e}"))?;

        if !xbmc_resp.status().is_success() {
            return Err(format!(
                "supersubtitles xbmc HTTP {}",
                xbmc_resp.status().as_u16()
            ));
        }

        let data: serde_json::Value = xbmc_resp
            .json()
            .await
            .map_err(|e| format!("supersubtitles: failed to parse xbmc response: {e}"))?;

        let mut results = Vec::new();

        if let serde_json::Value::Object(map) = data {
            for (_key, val) in map {
                let item: XbmcItem = match serde_json::from_value(val) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                let lang_str = item.language.as_deref().unwrap_or("Magyar");
                let (lang_code, lang_name) = language_code(lang_str);
                let nev = item.nev.clone().unwrap_or_default();
                let fnev = item.fnev.clone().unwrap_or_else(|| nev.clone());
                let felirat = item.felirat.as_ref().map(val_to_string).unwrap_or_default();
                let ep = item.ep.as_ref().map(val_to_string).unwrap_or_default();
                let evad = item.evad.as_ref().map(val_to_string).unwrap_or_default();

                if felirat.is_empty() {
                    continue;
                }

                let download_url = format!(
                    "{}/index.php?action=letolt&fnev={}&felirat={}",
                    SUPERSUBTITLES_BASE,
                    urlencoding::encode(&fnev),
                    felirat
                );

                let sub_name = if !nev.is_empty() {
                    nev.clone()
                } else {
                    format!("s{}e{}_{}", evad, ep, felirat)
                };

                results.push(SubtitleSearchResult {
                    id: felirat.clone(),
                    name: sub_name.clone(),
                    language: lang_code.to_string(),
                    language_name: lang_name.to_string(),
                    format: "srt".into(),
                    provider: "supersubtitles".into(),
                    detail_path: None,
                    download_path: Some(download_url),
                    download_count: None,
                    rating: None,
                    movie_name: Some(series_name.to_string()),
                    release_group: item.feltolto,
                });
            }
        }

        Ok(results)
    }

    async fn search_movie(
        &self,
        client: &reqwest::Client,
        title: &str,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let url = format!(
            "{}/index.php?search={}&soriSorszam=&nyelv=&action=subtitle",
            SUPERSUBTITLES_BASE,
            urlencoding::encode(title)
        );

        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("supersubtitles movie search failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "supersubtitles movie search HTTP {}",
                response.status().as_u16()
            ));
        }

        // For movies, just return a basic result pointing at the search page
        // A more complete implementation would parse the HTML
        let _html = response
            .text()
            .await
            .map_err(|e| format!("supersubtitles: failed to read HTML: {e}"))?;

        // Return empty — movie HTML parsing would require scraper but is complex
        Ok(vec![])
    }
}

/// Parse "Series Name S01E02" into ("Series Name", season, episode).
/// If no season/episode found, returns (query, 0, 0).
fn parse_series_query(query: &str) -> (String, u32, u32) {
    let upper = query.to_uppercase();
    if let Some(s_pos) = upper.find(" S") {
        let after = &upper[s_pos + 2..];
        if let Some(e_pos) = after.find('E') {
            let season_str = &after[..e_pos];
            let episode_str: String = after[e_pos + 1..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let (Ok(season), Ok(episode)) =
                (season_str.parse::<u32>(), episode_str.parse::<u32>())
            {
                let series = query[..s_pos].trim().to_string();
                return (series, season, episode);
            }
        }
    }
    (query.to_string(), 0, 0)
}

mod urlencoding {
    pub fn encode(s: &str) -> String {
        let mut out = String::new();
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(b as char);
                }
                b' ' => out.push('+'),
                other => out.push_str(&format!("%{other:02X}")),
            }
        }
        out
    }
}
