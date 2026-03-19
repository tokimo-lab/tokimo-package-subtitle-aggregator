/// Gestdown provider — French/multilingual TV series subtitles
/// API: https://api.gestdown.info (JSON, no auth)
/// Search: GET /shows/search/{title}
/// Episodes: GET /subtitles/get/{show_id}/{season}/{episode}/{lang}
use async_trait::async_trait;
use serde::Deserialize;

use super::SubtitleProvider;
use crate::models::{DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult};

const BASE_URL: &str = "https://api.gestdown.info";
const UA: &str = "Bazarr";

fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(UA)
        .build()
        .map_err(|e| format!("gestdown: failed to build client: {e}"))
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    shows: Vec<ShowItem>,
}

#[derive(Debug, Deserialize)]
struct ShowItem {
    id: String,
    #[serde(rename = "showName", default)]
    show_name: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct SubtitlesResponse {
    #[serde(rename = "matchingSubtitles", default)]
    matching_subtitles: Vec<SubtitleItem>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct SubtitleItem {
    #[serde(rename = "subtitleId", default)]
    subtitle_id: String,
    #[serde(rename = "version", default)]
    version: String,
    #[serde(rename = "completed", default)]
    completed: bool,
    #[serde(rename = "hearingImpaired", default)]
    hearing_impaired: bool,
    #[serde(rename = "downloadUri", default)]
    download_uri: String,
}

/// Map a BCP-47/ISO language code to the Addic7ed/Gestdown language name
fn to_gestdown_lang(lang: &str) -> &str {
    match lang {
        "fr" | "fra" => "French",
        "en" | "eng" => "English",
        "de" | "deu" => "German",
        "es" | "spa" => "Spanish",
        "it" | "ita" => "Italian",
        "pt" | "por" => "Portuguese",
        "pt-BR" => "Portuguese (Brazilian)",
        "nl" | "nld" => "Dutch",
        "pl" | "pol" => "Polish",
        "ru" | "rus" => "Russian",
        "tr" | "tur" => "Turkish",
        "sv" | "swe" => "Swedish",
        "da" | "dan" => "Danish",
        "fi" | "fin" => "Finnish",
        "nb" | "nor" => "Norwegian",
        "cs" | "ces" => "Czech",
        "sk" | "slk" => "Slovak",
        "hu" | "hun" => "Hungarian",
        "ro" | "ron" => "Romanian",
        "el" | "ell" => "Greek",
        "ar" | "ara" => "Arabic",
        "he" | "heb" => "Hebrew",
        "zh" | "zho" => "Chinese",
        "ja" | "jpn" => "Japanese",
        "ko" | "kor" => "Korean",
        _ => lang,
    }
}

fn language_name(lang: &str) -> String {
    match lang {
        "fr" => "French".into(),
        "en" => "English".into(),
        "de" => "German".into(),
        "es" => "Spanish".into(),
        "it" => "Italian".into(),
        "pt" => "Portuguese".into(),
        "pt-BR" => "Portuguese (Brazilian)".into(),
        other => other.to_string(),
    }
}

pub struct GestdownProvider;

impl GestdownProvider {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SubtitleProvider for GestdownProvider {
    fn name(&self) -> &str {
        "gestdown"
    }

    async fn search(&self, request: &SubtitleSearchRequest) -> Result<Vec<SubtitleSearchResult>, String> {
        let title = request.query.clone().unwrap_or_default();
        if title.trim().is_empty() {
            return Err("gestdown: search requires a query (series title)".into());
        }

        let client = build_client()?;

        // Search for shows
        let search_url = format!("{BASE_URL}/shows/search/{}", urlencoding::encode(&title));
        let resp = client.get(&search_url).send().await
            .map_err(|e| format!("gestdown: show search failed: {e}"))?;

        if resp.status().as_u16() == 404 {
            return Ok(vec![]);
        }
        if !resp.status().is_success() {
            return Err(format!("gestdown: show search HTTP {}", resp.status().as_u16()));
        }

        let search_result: SearchResponse = resp.json().await
            .map_err(|e| format!("gestdown: parse show search: {e}"))?;

        // Determine target languages
        let langs: Vec<&str> = request.languages.as_deref().unwrap_or(&[])
            .iter()
            .map(|s| s.as_str())
            .collect();
        let effective_langs: Vec<&str> = if langs.is_empty() { vec!["fr", "en"] } else { langs };

        let mut results = Vec::new();

        for show in &search_result.shows {
            for lang in &effective_langs {
                let gestdown_lang = to_gestdown_lang(lang);
                // We don't have season/episode from request, use placeholders that callers can refine
                // Return the show-level result with detail_path pointing to the API
                let id = format!("gestdown_{}_{}", show.id, lang);
                results.push(SubtitleSearchResult {
                    id,
                    name: show.show_name.clone(),
                    language: lang.to_string(),
                    language_name: language_name(lang),
                    format: "srt".into(),
                    provider: "gestdown".into(),
                    detail_path: Some(format!("{BASE_URL}/subtitles/get/{}", show.id)),
                    download_path: None,
                    download_count: None,
                    rating: None,
                    movie_name: Some(show.show_name.clone()),
                    release_group: Some(gestdown_lang.to_string()),
                });
            }
        }

        tracing::info!("gestdown: found {} show results for '{}'", results.len(), title);
        Ok(results)
    }

    async fn download(&self, request: &SubtitleDownloadRequest) -> Result<DownloadedSubtitle, String> {
        // download_path should be the direct subtitle download URI from the API
        // detail_path may be the /subtitles/get/{show_id} base URL
        let url = if let Some(dl) = &request.download_path {
            if dl.starts_with("http") { dl.clone() } else { format!("{BASE_URL}{dl}") }
        } else if let Some(detail) = &request.detail_path {
            detail.clone()
        } else {
            return Err("gestdown: download requires download_path or detail_path".into());
        };

        let client = build_client()?;
        let resp = client.get(&url).send().await
            .map_err(|e| format!("gestdown: download failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("gestdown: download HTTP {}", resp.status().as_u16()));
        }

        let content = resp.bytes().await.map_err(|e| format!("gestdown: read content: {e}"))?;
        let name = request.name.clone()
            .unwrap_or_else(|| format!("gestdown_{}.srt", request.subtitle_id));

        Ok(DownloadedSubtitle {
            name,
            format: request.format.clone(),
            content: content.to_vec(),
        })
    }
}

/// Simple URL percent-encoding (encode path segments)
mod urlencoding {
    pub fn encode(s: &str) -> String {
        url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
    }
}
