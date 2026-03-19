use async_trait::async_trait;

use super::SubtitleProvider;
use crate::models::{
    DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult,
};

const SUBTIS_API_BASE: &str = "https://api.subt.is/v1";
const SUBTIS_USER_AGENT: &str = "subtitle-aggregator/Subtis/0.9.2";

pub struct SubtisProvider;

impl SubtisProvider {
    pub fn new() -> Self {
        Self
    }

    fn build_client() -> Result<reqwest::Client, String> {
        reqwest::Client::builder()
            .user_agent(SUBTIS_USER_AGENT)
            .build()
            .map_err(|e| format!("Failed to build Subtis HTTP client: {e}"))
    }

    async fn fetch_subtitle(
        client: &reqwest::Client,
        url: &str,
    ) -> Option<(String, String)> {
        let response = client.get(url).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        let data: serde_json::Value = response.json().await.ok()?;
        let subtitle_link = data
            .get("subtitle")?
            .get("subtitle_link")?
            .as_str()?
            .to_string();
        let title_name = data
            .get("title")
            .and_then(|t| t.get("title_name"))
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown")
            .to_string();
        if subtitle_link.is_empty() {
            return None;
        }
        Some((subtitle_link, title_name))
    }
}

#[async_trait]
impl SubtitleProvider for SubtisProvider {
    fn name(&self) -> &str {
        "subtis"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let client = Self::build_client()?;

        // Build cascade of search URLs: hash -> bytes -> filename -> alternative
        let mut cascade: Vec<(String, &str)> = Vec::new();

        if let Some(hash) = &request.file_hash {
            cascade.push((
                format!("{SUBTIS_API_BASE}/subtitle/find/file/hash/{hash}"),
                "hash",
            ));
        }
        if let Some(size) = request.file_size {
            cascade.push((
                format!("{SUBTIS_API_BASE}/subtitle/find/file/bytes/{size}"),
                "bytes",
            ));
        }
        if let Some(query) = &request.query {
            if !query.trim().is_empty() {
                let encoded =
                    url::form_urlencoded::byte_serialize(query.trim().as_bytes()).collect::<String>();
                cascade.push((
                    format!("{SUBTIS_API_BASE}/subtitle/find/file/name/{encoded}"),
                    "name",
                ));
                cascade.push((
                    format!("{SUBTIS_API_BASE}/subtitle/file/alternative/{encoded}"),
                    "alternative",
                ));
            }
        }

        if cascade.is_empty() {
            return Err("Subtis search requires file_hash, file_size, or query".into());
        }

        for (url, method) in &cascade {
            if let Some((subtitle_link, title_name)) = Self::fetch_subtitle(&client, url).await {
                let is_synced = *method != "alternative";
                let name = if is_synced {
                    title_name.clone()
                } else {
                    format!("{title_name} [fuzzy match]")
                };
                return Ok(vec![SubtitleSearchResult {
                    id: url.clone(),
                    name,
                    language: "es".into(),
                    language_name: "Español".into(),
                    format: "srt".into(),
                    provider: "subtis".into(),
                    detail_path: None,
                    download_path: Some(subtitle_link),
                    download_count: None,
                    rating: None,
                    movie_name: Some(title_name),
                    release_group: None,
                }]);
            }
        }

        Ok(Vec::new())
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        let download_url = request
            .download_path
            .as_deref()
            .ok_or("Subtis download requires download_path")?;

        let client = Self::build_client()?;
        let response = client
            .get(download_url)
            .send()
            .await
            .map_err(|e| format!("Subtis download failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "Subtis download failed: HTTP {}",
                response.status().as_u16()
            ));
        }

        let content = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read Subtis download: {e}"))?;

        let format = request.format.clone();
        let name = request
            .name
            .clone()
            .unwrap_or_else(|| format!("subtitle.{format}"));

        Ok(DownloadedSubtitle {
            name,
            format,
            content: content.to_vec(),
        })
    }
}
