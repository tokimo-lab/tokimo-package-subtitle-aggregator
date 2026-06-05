use async_trait::async_trait;
use serde::Deserialize;

use super::SubtitleProvider;
use crate::models::{
    DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult,
};

const SUBX_API_BASE: &str = "https://subx-api.duckdns.org/api/subtitles";
const STAGING_ROOT: &str = "/tmp/subtitle-aggregator";

#[derive(Debug, Deserialize)]
struct SubxResponse {
    items: Vec<SubxItem>,
    #[allow(dead_code)]
    total: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct SubxItem {
    id: serde_json::Value,
    title: Option<String>,
    description: Option<String>,
    season: Option<serde_json::Value>,
    episode: Option<serde_json::Value>,
    uploader_name: Option<String>,
    #[allow(dead_code)]
    page_url: Option<String>,
}

fn val_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

pub struct SubxProvider {
    api_key: Option<String>,
}

impl SubxProvider {
    pub fn new() -> Self {
        let api_key = std::env::var("SUBX_API_KEY").ok();
        Self { api_key }
    }
}

#[async_trait]
impl SubtitleProvider for SubxProvider {
    fn name(&self) -> &str {
        "subx"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let api_key = self
            .api_key
            .as_deref()
            .ok_or("subx: SUBX_API_KEY environment variable is required")?;

        let client = reqwest::Client::builder()
            .user_agent("subtitle-aggregator/0.1")
            .build()
            .map_err(|e| format!("subx: failed to build client: {e}"))?;

        let query = request.query.clone().unwrap_or_default();
        let (series_name, season, episode) = parse_series_query(&query);

        let video_type = if season > 0 { "episode" } else { "movie" };

        let mut params: Vec<(&str, String)> = vec![
            ("limit", "200".to_string()),
            ("video_type", video_type.to_string()),
        ];

        if let Some(imdb_id) = &request.imdb_id {
            params.push(("imdb_id", imdb_id.clone()));
        }

        if !series_name.is_empty() {
            params.push(("title", series_name.clone()));
        }

        if season > 0 {
            params.push(("season", season.to_string()));
            params.push(("episode", episode.to_string()));
        }

        let url = format!("{}/search", SUBX_API_BASE);

        let response = client
            .get(&url)
            .header("Authorization", format!("Bearer {api_key}"))
            .query(&params)
            .send()
            .await
            .map_err(|e| format!("subx: request failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!("subx: HTTP {}", response.status().as_u16()));
        }

        let data: SubxResponse = response
            .json()
            .await
            .map_err(|e| format!("subx: failed to parse response: {e}"))?;

        let results = data
            .items
            .into_iter()
            .map(|item| {
                let id_str = val_to_string(&item.id);
                let title = item.title.clone().unwrap_or_else(|| id_str.clone());
                let _description = item.description.clone().unwrap_or_default();
                let season_str = item.season.as_ref().map(val_to_string).unwrap_or_default();
                let episode_str = item.episode.as_ref().map(val_to_string).unwrap_or_default();

                let name = if !season_str.is_empty() && !episode_str.is_empty() {
                    format!("{} S{}E{}", title, season_str, episode_str)
                } else {
                    title.clone()
                };

                let download_url = format!("{}/{}/download", SUBX_API_BASE, id_str);

                SubtitleSearchResult {
                    id: id_str,
                    name,
                    language: "hu".into(),
                    language_name: "Hungarian".into(),
                    format: "srt".into(),
                    provider: "subx".into(),
                    detail_path: None,
                    download_path: Some(download_url),
                    download_count: None,
                    rating: None,
                    movie_name: Some(title),
                    release_group: item.uploader_name,
                }
            })
            .collect();

        Ok(results)
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        let api_key = self
            .api_key
            .as_deref()
            .ok_or("subx: SUBX_API_KEY environment variable is required")?;

        let url = request
            .download_path
            .as_deref()
            .map(|p| p.to_string())
            .unwrap_or_else(|| format!("{}/{}/download", SUBX_API_BASE, request.subtitle_id));

        let client = reqwest::Client::builder()
            .user_agent("subtitle-aggregator/0.1")
            .build()
            .map_err(|e| format!("subx: failed to build client: {e}"))?;

        let response = client
            .get(&url)
            .header("Authorization", format!("Bearer {api_key}"))
            .send()
            .await
            .map_err(|e| format!("subx: download failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!("subx: HTTP {}", response.status().as_u16()));
        }

        let content = response
            .bytes()
            .await
            .map_err(|e| format!("subx: failed to read content: {e}"))?;

        let name = request
            .name
            .clone()
            .unwrap_or_else(|| format!("subx_{}.zip", request.subtitle_id));

        let staging = std::path::Path::new(STAGING_ROOT);
        crate::archive::extract_archive(&content, &name, &request.language, staging).await
    }
}

/// Parse "Series Name S01E02" into ("Series Name", season, episode).
/// Returns (query, 0, 0) if no season/episode found.
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
