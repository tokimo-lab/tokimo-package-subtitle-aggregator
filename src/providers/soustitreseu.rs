/// Sous-Titres.eu provider — French subtitle site
/// HTML scraping, no auth. Supports series and movies.
use async_trait::async_trait;
use scraper::{Html, Selector};

use super::SubtitleProvider;
use crate::archive::extract_archive;
use crate::models::{DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult};

const SERVER_URL: &str = "https://www.sous-titres.eu/";
const SEARCH_URL: &str = "https://www.sous-titres.eu/search.html";
const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(UA)
        .default_headers({
            let mut h = reqwest::header::HeaderMap::new();
            h.insert("Referer", SERVER_URL.parse().unwrap());
            h
        })
        .build()
        .map_err(|e| format!("soustitreseu: failed to build client: {e}"))
}

pub struct SoustitreseuProvider {
    staging_root: std::path::PathBuf,
}

impl SoustitreseuProvider {
    pub fn new(staging_root: impl Into<std::path::PathBuf>) -> Self {
        Self { staging_root: staging_root.into() }
    }
}

#[async_trait]
impl SubtitleProvider for SoustitreseuProvider {
    fn name(&self) -> &str {
        "soustitreseu"
    }

    async fn search(&self, request: &SubtitleSearchRequest) -> Result<Vec<SubtitleSearchResult>, String> {
        let query = request.query.clone().unwrap_or_default();
        if query.trim().is_empty() {
            return Err("soustitreseu: search requires a query".into());
        }

        let client = build_client()?;

        let resp = client
            .get(SEARCH_URL)
            .query(&[("q", &query)])
            .send()
            .await
            .map_err(|e| format!("soustitreseu: search request failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("soustitreseu: HTTP {}", resp.status().as_u16()));
        }

        let html = resp.text().await.map_err(|e| format!("soustitreseu: read response: {e}"))?;
        let document = Html::parse_document(&html);

        let mut results = Vec::new();
        let mut id_counter = 0u64;

        // Series: .serie > h3 > a
        if let Ok(series_sel) = Selector::parse(".serie > h3 > a") {
            for anchor in document.select(&series_sel) {
                let text = anchor.text().collect::<String>();
                if !text.to_lowercase().contains(&query.to_lowercase()) {
                    continue;
                }
                let href = anchor.value().attr("href").unwrap_or("");
                let page_url = format!("{SERVER_URL}{href}");
                id_counter += 1;
                results.push(SubtitleSearchResult {
                    id: id_counter.to_string(),
                    name: text.trim().to_string(),
                    language: "fr".into(),
                    language_name: "French".into(),
                    format: "srt".into(),
                    provider: "soustitreseu".into(),
                    detail_path: Some(page_url),
                    download_path: None,
                    download_count: None,
                    rating: None,
                    movie_name: Some(query.clone()),
                    release_group: None,
                });
            }
        }

        // Movies: .film > h3 > a
        if let Ok(film_sel) = Selector::parse(".film > h3 > a") {
            for anchor in document.select(&film_sel) {
                let text = anchor.text().collect::<String>();
                if !text.to_lowercase().contains(&query.to_lowercase()) {
                    continue;
                }
                let href = anchor.value().attr("href").unwrap_or("");
                let page_url = format!("{SERVER_URL}{href}");
                id_counter += 1;
                results.push(SubtitleSearchResult {
                    id: id_counter.to_string(),
                    name: text.trim().to_string(),
                    language: "fr".into(),
                    language_name: "French".into(),
                    format: "srt".into(),
                    provider: "soustitreseu".into(),
                    detail_path: Some(page_url),
                    download_path: None,
                    download_count: None,
                    rating: None,
                    movie_name: Some(query.clone()),
                    release_group: None,
                });
            }
        }

        tracing::info!("soustitreseu: found {} results for '{}'", results.len(), query);
        Ok(results)
    }

    async fn download(&self, request: &SubtitleDownloadRequest) -> Result<DownloadedSubtitle, String> {
        // detail_path holds the series/movie page URL; download_path may hold direct zip URL
        let client = build_client()?;

        let zip_url = if let Some(dl) = &request.download_path {
            dl.clone()
        } else if let Some(detail) = &request.detail_path {
            // Fetch detail page to find first .subList href
            let resp = client.get(detail).send().await
                .map_err(|e| format!("soustitreseu: fetch detail page: {e}"))?;
            let html = resp.text().await.map_err(|e| format!("soustitreseu: read detail: {e}"))?;
            let doc = Html::parse_document(&html);
            let sub_sel = Selector::parse("a.subList").map_err(|_| "soustitreseu: selector error")?;
            let first = doc.select(&sub_sel).next()
                .ok_or("soustitreseu: no subtitle archives found on detail page")?;
            let href = first.value().attr("href").unwrap_or("");
            // determine prefix: series vs films
            if detail.contains("series") || detail.contains("serie") {
                format!("{SERVER_URL}series/{href}")
            } else {
                format!("{SERVER_URL}films/{href}")
            }
        } else {
            return Err("soustitreseu: download requires detail_path or download_path".into());
        };

        let resp = client.get(&zip_url).send().await
            .map_err(|e| format!("soustitreseu: download archive: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("soustitreseu: download HTTP {}", resp.status().as_u16()));
        }
        let bytes = resp.bytes().await.map_err(|e| format!("soustitreseu: read archive: {e}"))?;
        let filename = zip_url.rsplit('/').next().unwrap_or("subtitle.zip").to_string();
        extract_archive(&bytes, &filename, &request.language, &self.staging_root).await
    }
}
