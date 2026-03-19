/// RegieLive provider — Romanian subtitles
/// API: https://api.regielive.ro/bazarr/search.php
/// No authentication required, uses a fixed API key header
use async_trait::async_trait;
use serde::Deserialize;
use std::collections::HashMap;

use super::SubtitleProvider;
use crate::archive::extract_archive;
use crate::models::{
    normalize_format, DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest,
    SubtitleSearchResult,
};

const API_URL: &str = "https://api.regielive.ro/bazarr/search.php";
const SITE_URL: &str = "https://subtitrari.regielive.ro";
const RL_API_KEY: &str = "API-BAZARR-YTZ-SL";
const UA: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:72.0) Gecko/20100101 Firefox/72.0";

fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(UA)
        .cookie_store(true)
        .build()
        .map_err(|e| format!("RegieLive: failed to build HTTP client: {e}"))
}

pub struct RegieLiveProvider {
    staging_root: std::path::PathBuf,
}

impl RegieLiveProvider {
    pub fn new(staging_root: impl Into<std::path::PathBuf>) -> Self {
        Self {
            staging_root: staging_root.into(),
        }
    }
}

/// Nested JSON structures from RegieLive API
#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct RegieLiveResponse {
    rezultate: Option<HashMap<String, FilmEntry>>,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct FilmEntry {
    subtitrari: Option<HashMap<String, SubEntry>>,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct SubEntry {
    titlu: Option<String>,
    url: Option<String>,
    rating: Option<RatingEntry>,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct RatingEntry {
    nota: Option<f64>,
}

#[async_trait]
impl SubtitleProvider for RegieLiveProvider {
    fn name(&self) -> &str {
        "regielive"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let client = build_client()?;
        let query = request.query.clone().unwrap_or_default();

        if query.is_empty() {
            return Err("RegieLive: search requires a query (title)".into());
        }

        let params = vec![("nume", query.clone())];
        // RegieLive does not support searching without a title, season/episode are optional
        // (search request doesn't have season/episode fields, so we skip them)

        let url = {
            let encoded: String = params
                .iter()
                .map(|(k, v)| {
                    format!(
                        "{}={}",
                        url::form_urlencoded::byte_serialize(k.as_bytes()).collect::<String>(),
                        url::form_urlencoded::byte_serialize(v.as_bytes()).collect::<String>()
                    )
                })
                .collect::<Vec<_>>()
                .join("&");
            format!("{API_URL}?{encoded}")
        };
        let _ = params;

        let resp = client
            .get(&url)
            .header("RL-API", RL_API_KEY)
            .send()
            .await
            .map_err(|e| format!("RegieLive: search request failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!(
                "RegieLive: search failed with HTTP {}",
                resp.status().as_u16()
            ));
        }

        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("RegieLive: failed to parse JSON response: {e}"))?;

        let mut results = Vec::new();
        let mut idx = 0usize;

        if let Some(rezultate) = data.get("rezultate").and_then(|r| r.as_object()) {
            for (_film_key, film_val) in rezultate {
                if let Some(subtitrari) = film_val
                    .get("subtitrari")
                    .and_then(|s| s.as_object())
                {
                    for (_sub_key, sub_val) in subtitrari {
                        let titlu = sub_val
                            .get("titlu")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string();
                        let sub_url = sub_val
                            .get("url")
                            .and_then(|u| u.as_str())
                            .unwrap_or("")
                            .to_string();
                        let rating = sub_val
                            .get("rating")
                            .and_then(|r| r.get("nota"))
                            .and_then(|n| n.as_f64());

                        if sub_url.is_empty() {
                            continue;
                        }

                        results.push(SubtitleSearchResult {
                            id: format!("regielive_{idx}"),
                            name: if titlu.is_empty() {
                                query.clone()
                            } else {
                                titlu.clone()
                            },
                            language: "ro".into(),
                            language_name: "Romanian".into(),
                            format: "srt".into(),
                            provider: "regielive".into(),
                            detail_path: None,
                            download_path: Some(sub_url),
                            download_count: None,
                            rating,
                            movie_name: Some(query.clone()),
                            release_group: if titlu.is_empty() {
                                None
                            } else {
                                Some(titlu)
                            },
                        });
                        idx += 1;
                    }
                }
            }
        }

        tracing::info!("RegieLive: found {} results", results.len());
        Ok(results)
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        let sub_url = request
            .download_path
            .as_deref()
            .ok_or("RegieLive: missing download_path")?;

        let client = build_client()?;

        // First fetch the site to get cookies
        client
            .get(SITE_URL)
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8")
            .header("Accept-Language", "en-US,en;q=0.5")
            .send()
            .await
            .map_err(|e| format!("RegieLive: failed to fetch site for cookies: {e}"))?;

        let resp = client
            .get(sub_url)
            .header("Referer", SITE_URL)
            .header("Origin", SITE_URL)
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8")
            .header("Accept-Language", "en-US,en;q=0.5")
            .send()
            .await
            .map_err(|e| format!("RegieLive: download request failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!(
                "RegieLive: download failed with HTTP {}",
                resp.status().as_u16()
            ));
        }

        let file_name = resp
            .headers()
            .get("content-disposition")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| {
                let re =
                    regex::Regex::new(r#"filename[^;=\n]*=(?:(['"])(.+?)\1|([^;\n]*))"#).ok()?;
                re.captures(v)
                    .and_then(|c| c.get(2).or_else(|| c.get(3)))
                    .map(|m| m.as_str().trim().to_string())
            })
            .unwrap_or_else(|| format!("regielive_{}.zip", request.subtitle_id));

        let content = resp
            .bytes()
            .await
            .map_err(|e| format!("RegieLive: failed to read download content: {e}"))?;

        if let Some(format) = normalize_format(&file_name) {
            return Ok(DownloadedSubtitle {
                name: file_name,
                format,
                content: content.to_vec(),
            });
        }

        extract_archive(&content, &file_name, &request.language, &self.staging_root).await
    }
}
