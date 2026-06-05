use async_trait::async_trait;
use serde::Deserialize;

use super::SubtitleProvider;
use crate::models::{
    DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult,
};

const JIMAKU_API_BASE: &str = "https://jimaku.cc/api";

// ── Jimaku API types ──

#[derive(Debug, Deserialize)]
struct JimakuEntry {
    id: u64,
    #[serde(default)]
    english_name: Option<String>,
    #[serde(default)]
    japanese_name: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JimakuFile {
    #[serde(default)]
    id: u64,
    name: String,
    url: String,
    #[serde(default)]
    size: Option<u64>,
}

pub struct JimakuProvider {
    api_key: String,
}

impl JimakuProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
        }
    }

    fn build_client(&self) -> Result<reqwest::Client, String> {
        reqwest::Client::builder()
            .build()
            .map_err(|e| format!("Failed to build Jimaku HTTP client: {e}"))
    }

    /// Determine language from filename (Japanese by default, English if detected)
    fn detect_language(filename: &str) -> (&'static str, &'static str) {
        let lower = filename.to_lowercase();
        // Check for English language indicators
        if lower.contains(".en.") || lower.contains("[en]") || lower.contains("(en)") {
            return ("en", "English");
        }
        ("ja", "日本語")
    }
}

#[async_trait]
impl SubtitleProvider for JimakuProvider {
    fn name(&self) -> &str {
        "jimaku"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        if self.api_key.is_empty() {
            return Err("Jimaku API key is not set".into());
        }

        let client = self.build_client()?;

        // Build search params: prefer tmdb_id, fall back to query
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(tmdb_id) = &request.tmdb_id {
            params.push(("tmdb_id", tmdb_id.clone()));
        } else if let Some(query) = &request.query {
            let q = query.trim();
            if q.is_empty() {
                return Err("Jimaku search requires a query or tmdb_id".into());
            }
            params.push(("query", q.to_string()));
        } else {
            return Err("Jimaku search requires a query or tmdb_id".into());
        }

        let search_url = format!("{JIMAKU_API_BASE}/entries/search");
        let entries_resp = client
            .get(&search_url)
            .header("Authorization", &self.api_key)
            .query(&params)
            .send()
            .await
            .map_err(|e| format!("Jimaku entries search request failed: {e}"))?;

        if !entries_resp.status().is_success() {
            let status = entries_resp.status().as_u16();
            let body = entries_resp.text().await.unwrap_or_default();
            return Err(format!("Jimaku entries search failed ({status}): {body}"));
        }

        let entries: Vec<JimakuEntry> = entries_resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse Jimaku entries response: {e}"))?;

        if entries.is_empty() {
            tracing::info!("Jimaku: no entries found");
            return Ok(vec![]);
        }

        // Use the first matching entry
        let entry = &entries[0];
        let movie_name = entry
            .english_name
            .clone()
            .or_else(|| entry.name.clone())
            .or_else(|| entry.japanese_name.clone());

        tracing::info!(
            "Jimaku: matched entry id={}, name={:?}",
            entry.id,
            movie_name
        );

        // Fetch files for this entry
        let files_url = format!("{JIMAKU_API_BASE}/entries/{}/files", entry.id);
        let files_resp = client
            .get(&files_url)
            .header("Authorization", &self.api_key)
            .send()
            .await
            .map_err(|e| format!("Jimaku files request failed: {e}"))?;

        if !files_resp.status().is_success() {
            let status = files_resp.status().as_u16();
            let body = files_resp.text().await.unwrap_or_default();
            return Err(format!("Jimaku files request failed ({status}): {body}"));
        }

        let files: Vec<JimakuFile> = files_resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse Jimaku files response: {e}"))?;

        tracing::info!("Jimaku: found {} files for entry {}", files.len(), entry.id);

        let results = files
            .into_iter()
            .filter(|f| {
                // Skip obviously corrupt files (< 500 bytes)
                if let Some(size) = f.size {
                    if size < 500 {
                        return false;
                    }
                }
                // Skip unhandled archive formats
                !f.name.ends_with(".7z")
            })
            .map(|f| {
                let format =
                    crate::models::normalize_format(&f.name).unwrap_or_else(|| "srt".into());
                let (language, language_name) = Self::detect_language(&f.name);
                SubtitleSearchResult {
                    id: f.id.to_string(),
                    name: f.name.clone(),
                    language: language.to_string(),
                    language_name: language_name.to_string(),
                    format,
                    provider: "jimaku".into(),
                    detail_path: None,
                    download_path: Some(f.url),
                    download_count: None,
                    rating: None,
                    movie_name: movie_name.clone(),
                    release_group: None,
                }
            })
            .collect();

        Ok(results)
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        let download_url = request
            .download_path
            .as_deref()
            .ok_or("Jimaku download missing URL")?;

        let client = self.build_client()?;

        let response = client
            .get(download_url)
            .header("Authorization", &self.api_key)
            .send()
            .await
            .map_err(|e| format!("Jimaku download request failed: {e}"))?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("Jimaku download failed ({status}): {body}"));
        }

        let file_name = request
            .name
            .clone()
            .unwrap_or_else(|| "subtitle.srt".to_string());

        let content = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read Jimaku subtitle content: {e}"))?;

        let format =
            crate::models::normalize_format(&file_name).unwrap_or_else(|| request.format.clone());

        Ok(DownloadedSubtitle {
            name: file_name,
            format,
            content: content.to_vec(),
        })
    }
}
