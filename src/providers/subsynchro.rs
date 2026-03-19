/// Subsynchro provider — French subtitle site for movies
/// API: https://www.subsynchro.com/include/ajax/subMarin.php (JSON)
use async_trait::async_trait;
use serde::Deserialize;

use super::SubtitleProvider;
use crate::archive::extract_archive;
use crate::models::{DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult};

const SERVER_URL: &str = "https://www.subsynchro.com/include/ajax/subMarin.php";
const PAGE_URL: &str = "https://www.subsynchro.com";
const UA: &str = "Bazarr";

fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(UA)
        .default_headers({
            let mut h = reqwest::header::HeaderMap::new();
            h.insert("Referer", PAGE_URL.parse().unwrap());
            h
        })
        .build()
        .map_err(|e| format!("subsynchro: failed to build client: {e}"))
}

#[derive(Debug, Deserialize)]
struct SubsynchroResponse {
    #[serde(default)]
    status: Option<u32>,
    #[serde(default)]
    data: Vec<SubsynchroItem>,
}

#[derive(Debug, Deserialize)]
struct SubsynchroItem {
    #[serde(default)]
    release: Option<String>,
    #[serde(default)]
    filename: Option<String>,
    #[serde(default)]
    telechargement: Option<String>,
    #[serde(default)]
    fichier: Option<String>,
    #[serde(default)]
    titre: Option<String>,
    #[serde(default)]
    titre_original: Option<String>,
}

pub struct SubsynchroProvider {
    staging_root: std::path::PathBuf,
}

impl SubsynchroProvider {
    pub fn new(staging_root: impl Into<std::path::PathBuf>) -> Self {
        Self { staging_root: staging_root.into() }
    }
}

#[async_trait]
impl SubtitleProvider for SubsynchroProvider {
    fn name(&self) -> &str {
        "subsynchro"
    }

    async fn search(&self, request: &SubtitleSearchRequest) -> Result<Vec<SubtitleSearchResult>, String> {
        let title = request.query.clone().unwrap_or_default();
        if title.trim().is_empty() {
            return Err("subsynchro: search requires a query (movie title)".into());
        }

        let client = build_client()?;

        let resp = client
            .get(SERVER_URL)
            .query(&[("title", title.as_str())])
            .send()
            .await
            .map_err(|e| format!("subsynchro: request failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("subsynchro: HTTP {}", resp.status().as_u16()));
        }

        let body: SubsynchroResponse = resp.json().await
            .map_err(|e| format!("subsynchro: parse response: {e}"))?;

        if body.status.unwrap_or(0) != 200 {
            tracing::debug!("subsynchro: no subtitles found (status {:?})", body.status);
            return Ok(vec![]);
        }

        let mut results = Vec::new();
        for (idx, item) in body.data.into_iter().enumerate() {
            let Some(download_url) = item.telechargement else { continue };
            let release = item.release.unwrap_or_default();
            let filename = item.filename.unwrap_or_default();
            let name = if release.len() >= filename.len() { release.clone() } else { filename.clone() };
            let movie_name = item.titre
                .or(item.titre_original)
                .unwrap_or_else(|| title.clone());

            results.push(SubtitleSearchResult {
                id: format!("subsynchro_{idx}"),
                name,
                language: "fr".into(),
                language_name: "French".into(),
                format: item.fichier.unwrap_or_else(|| "zip".into()),
                provider: "subsynchro".into(),
                detail_path: Some(download_url.clone()),
                download_path: Some(download_url),
                download_count: None,
                rating: None,
                movie_name: Some(movie_name),
                release_group: Some(release),
            });
        }

        tracing::info!("subsynchro: found {} results for '{}'", results.len(), title);
        Ok(results)
    }

    async fn download(&self, request: &SubtitleDownloadRequest) -> Result<DownloadedSubtitle, String> {
        let url = request.download_path.as_deref()
            .or(request.detail_path.as_deref())
            .ok_or("subsynchro: download requires download_path")?;

        let client = build_client()?;

        let resp = client.get(url).send().await
            .map_err(|e| format!("subsynchro: download request: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("subsynchro: download HTTP {}", resp.status().as_u16()));
        }

        let bytes = resp.bytes().await.map_err(|e| format!("subsynchro: read response: {e}"))?;
        let filename = url.rsplit('/').next().unwrap_or("subtitle.zip").to_string();
        extract_archive(&bytes, &filename, &request.language, &self.staging_root).await
    }
}
