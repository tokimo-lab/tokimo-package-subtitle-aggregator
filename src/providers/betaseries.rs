/// BetaSeries provider — French TV episode subtitles
/// API: https://api.betaseries.com/ — requires API key from BETASERIES_API_KEY env var
use async_trait::async_trait;
use serde::Deserialize;

use super::SubtitleProvider;
use crate::archive::extract_archive;
use crate::models::{DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult};

const SERVER_URL: &str = "https://api.betaseries.com/";
const UA: &str = "Sub-Zero/2";

fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(UA)
        .build()
        .map_err(|e| format!("betaseries: failed to build client: {e}"))
}

fn get_api_key() -> Result<String, String> {
    std::env::var("BETASERIES_API_KEY")
        .map_err(|_| "betaseries: BETASERIES_API_KEY environment variable not set".into())
}

#[derive(Debug, Deserialize)]
struct BetaSeriesEpisodeResponse {
    #[serde(default)]
    errors: Vec<serde_json::Value>,
    episode: Option<EpisodeData>,
    episodes: Option<Vec<EpisodeData>>,
}

#[derive(Debug, Deserialize)]
struct EpisodeData {
    subtitles: Option<Vec<SubtitleData>>,
}

#[derive(Debug, Deserialize)]
struct SubtitleData {
    id: serde_json::Value,
    language: String,
    file: Option<String>,
    url: Option<String>,
    source: Option<serde_json::Value>,
}

fn translate_lang(code: &str) -> Option<(&'static str, &'static str)> {
    match code.to_lowercase().as_str() {
        "vo" | "en" => Some(("en", "English")),
        "vf" | "fr" => Some(("fr", "French")),
        _ => None,
    }
}

pub struct BetaSeriesProvider {
    staging_root: std::path::PathBuf,
}

impl BetaSeriesProvider {
    pub fn new(staging_root: impl Into<std::path::PathBuf>) -> Self {
        Self { staging_root: staging_root.into() }
    }
}

#[async_trait]
impl SubtitleProvider for BetaSeriesProvider {
    fn name(&self) -> &str {
        "betaseries"
    }

    async fn search(&self, request: &SubtitleSearchRequest) -> Result<Vec<SubtitleSearchResult>, String> {
        let api_key = get_api_key()?;
        let client = build_client()?;

        // Prefer TVDB id; fall back to query-based lookup
        let tvdb_id = request.imdb_id.as_deref(); // reuse imdb_id field as tvdb_id if present

        let endpoint;
        let mut params: Vec<(&str, String)> = vec![
            ("key".into(), api_key),
            ("subtitles".into(), "1".into()),
            ("v".into(), "3.0".into()),
        ];

        if let Some(id) = tvdb_id {
            endpoint = format!("{SERVER_URL}episodes/display");
            params.push(("thetvdb_id", id.to_string()));
        } else {
            let query = request.query.clone().unwrap_or_default();
            if query.trim().is_empty() {
                return Err("betaseries: search requires either imdb_id (as tvdb_id) or query".into());
            }
            endpoint = format!("{SERVER_URL}shows/episodes");
            params.push(("title", query));
        }

        let resp = client.get(&endpoint)
            .query(&params)
            .send()
            .await
            .map_err(|e| format!("betaseries: request failed: {e}"))?;

        let status = resp.status().as_u16();
        if status == 400 {
            // Check for known error codes
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            if let Some(code) = body["errors"][0]["code"].as_u64() {
                if code == 4001 { return Ok(vec![]); }
                if code == 1001 { return Err("betaseries: invalid API key".into()); }
            }
            return Ok(vec![]);
        }
        if !resp.status().is_success() {
            return Err(format!("betaseries: HTTP {status}"));
        }

        let result: BetaSeriesEpisodeResponse = resp.json().await
            .map_err(|e| format!("betaseries: parse response: {e}"))?;

        if !result.errors.is_empty() {
            tracing::debug!("betaseries: API errors: {:?}", result.errors);
            return Ok(vec![]);
        }

        let subs = result.episode
            .as_ref()
            .and_then(|ep| ep.subtitles.as_ref())
            .or_else(|| {
                result.episodes.as_ref()
                    .and_then(|eps| eps.first())
                    .and_then(|ep| ep.subtitles.as_ref())
            });

        let Some(subs) = subs else { return Ok(vec![]); };

        // Filter requested languages
        let req_langs: Vec<&str> = request.languages.as_deref().unwrap_or(&[])
            .iter().map(|s| s.as_str()).collect();

        let mut results = Vec::new();
        for sub in subs {
            // Filter out dead seriessub source
            if let Some(src) = &sub.source {
                if src.as_str().map(|s| s == "seriessub").unwrap_or(false) {
                    continue;
                }
            }

            let Some((lang_code, lang_name)) = translate_lang(&sub.language) else { continue };
            if !req_langs.is_empty() && !req_langs.contains(&lang_code) {
                continue;
            }

            let sub_id = match &sub.id {
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let file_name = sub.file.clone().unwrap_or_else(|| format!("betaseries_{sub_id}.srt"));
            let url = sub.url.clone().unwrap_or_default();

            results.push(SubtitleSearchResult {
                id: sub_id,
                name: file_name.clone(),
                language: lang_code.into(),
                language_name: lang_name.into(),
                format: "srt".into(),
                provider: "betaseries".into(),
                detail_path: Some(url.clone()),
                download_path: Some(url),
                download_count: None,
                rating: None,
                movie_name: None,
                release_group: None,
            });
        }

        tracing::info!("betaseries: found {} subtitle results", results.len());
        Ok(results)
    }

    async fn download(&self, request: &SubtitleDownloadRequest) -> Result<DownloadedSubtitle, String> {
        let url = request.download_path.as_deref()
            .or(request.detail_path.as_deref())
            .ok_or("betaseries: download requires download_path")?;

        let client = build_client()?;
        let resp = client.get(url).send().await
            .map_err(|e| format!("betaseries: download request: {e}"))?;

        let status = resp.status().as_u16();
        if status == 404 {
            return Err(format!("betaseries: subtitle not found (404): {url}"));
        }
        if !resp.status().is_success() {
            return Err(format!("betaseries: download HTTP {status}"));
        }

        let bytes = resp.bytes().await.map_err(|e| format!("betaseries: read bytes: {e}"))?;
        let filename = url.rsplit('/').next().unwrap_or("subtitle.srt").to_string();

        // Check if it's an archive
        if filename.ends_with(".zip") || filename.ends_with(".rar") || filename.ends_with(".7z") {
            return extract_archive(&bytes, &filename, &request.language, &self.staging_root).await;
        }

        // Try detect archive by content
        if bytes.starts_with(b"PK") || bytes.starts_with(b"Rar!") {
            return extract_archive(&bytes, &filename, &request.language, &self.staging_root).await;
        }

        Ok(DownloadedSubtitle {
            name: request.name.clone().unwrap_or(filename),
            format: request.format.clone(),
            content: bytes.to_vec(),
        })
    }
}
