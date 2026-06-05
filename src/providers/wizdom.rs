use std::path::PathBuf;

use async_trait::async_trait;
use regex::Regex;

use super::SubtitleProvider;
use crate::archive::extract_archive;
use crate::models::{
    matches_preferred_language, DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest,
    SubtitleSearchResult,
};

const WIZDOM_BASE: &str = "http://wizdom.xyz";
const WIZDOM_API_BASE: &str = "https://wizdom.xyz/api";
const TMDB_API_KEY: &str = "a51ee051bcd762543373903de296e0a3";
const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

pub struct WizdomProvider {
    staging_root: PathBuf,
}

impl WizdomProvider {
    pub fn new(staging_root: impl Into<PathBuf>) -> Self {
        Self {
            staging_root: staging_root.into(),
        }
    }
}

fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .cookie_store(true)
        .build()
        .map_err(|e| format!("Failed to build wizdom client: {e}"))
}

fn parse_season_episode(query: &str) -> Option<(u32, u32)> {
    let re = Regex::new(r"[Ss](\d+)[Ee](\d+)").unwrap();
    re.captures(query).map(|cap| {
        let season = cap[1].parse::<u32>().unwrap_or(1);
        let episode = cap[2].parse::<u32>().unwrap_or(1);
        (season, episode)
    })
}

async fn get_imdb_id_from_tmdb(
    client: &reqwest::Client,
    title: &str,
    is_tv: bool,
) -> Result<String, String> {
    let kind = if is_tv { "tv" } else { "movie" };
    let encoded = url::form_urlencoded::byte_serialize(title.as_bytes()).collect::<String>();
    let search_url = format!(
        "http://api.tmdb.org/3/search/{kind}?api_key={TMDB_API_KEY}&query={encoded}&language=en"
    );

    let resp = client
        .get(&search_url)
        .send()
        .await
        .map_err(|e| format!("wizdom TMDB search failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("wizdom TMDB search failed: {}", resp.status()));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("wizdom TMDB JSON parse error: {e}"))?;

    let tmdb_id = json["results"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|r| r["id"].as_u64())
        .ok_or("wizdom: no TMDB result found")?;

    // Get IMDB ID from TMDB
    let ext_url = if is_tv {
        format!("http://api.tmdb.org/3/tv/{tmdb_id}/external_ids?api_key={TMDB_API_KEY}")
    } else {
        format!("http://api.tmdb.org/3/movie/{tmdb_id}?api_key={TMDB_API_KEY}")
    };

    let ext_resp = client
        .get(&ext_url)
        .send()
        .await
        .map_err(|e| format!("wizdom TMDB external_ids failed: {e}"))?;

    if !ext_resp.status().is_success() {
        return Err(format!(
            "wizdom TMDB external_ids failed: {}",
            ext_resp.status()
        ));
    }

    let ext_json: serde_json::Value = ext_resp
        .json()
        .await
        .map_err(|e| format!("wizdom TMDB external_ids JSON parse error: {e}"))?;

    let imdb_id = ext_json["imdb_id"]
        .as_str()
        .ok_or("wizdom: no IMDB ID found in TMDB response")?
        .to_string();

    Ok(imdb_id)
}

#[async_trait]
impl SubtitleProvider for WizdomProvider {
    fn name(&self) -> &str {
        "wizdom"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        if !matches_preferred_language("he", request.languages.as_deref()) {
            return Ok(Vec::new());
        }

        let client = build_client()?;

        let imdb_id = if let Some(id) = &request.imdb_id {
            id.clone()
        } else {
            let query = request
                .query
                .as_deref()
                .ok_or("wizdom requires imdb_id or query")?;

            let se_opt = parse_season_episode(query);
            let is_tv = se_opt.is_some();
            let title_re = Regex::new(r"\s*[Ss]\d+[Ee]\d+.*").unwrap();
            let title = title_re.replace(query, "").trim().to_string();

            get_imdb_id_from_tmdb(&client, &title, is_tv).await?
        };

        let api_url = format!("{WIZDOM_API_BASE}/releases/{imdb_id}");
        let resp = client
            .get(&api_url)
            .send()
            .await
            .map_err(|e| format!("wizdom API request failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("wizdom API request failed: {}", resp.status()));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("wizdom API JSON parse error: {e}"))?;

        let query_str = request.query.as_deref().unwrap_or("");
        let se_opt = parse_season_episode(query_str);

        let mut results = Vec::new();

        if let Some((season, episode)) = se_opt {
            // TV series: result["subs"][season_str][episode_str]
            let season_str = season.to_string();
            let episode_str = episode.to_string();

            if let Some(subs) = json["subs"][&season_str][&episode_str].as_array() {
                for sub in subs {
                    let subtitle_id = sub["id"]
                        .as_u64()
                        .map(|v| v.to_string())
                        .or_else(|| sub["id"].as_str().map(|s| s.to_string()))
                        .unwrap_or_default();
                    let version = sub["version"].as_str().unwrap_or("").to_string();

                    if subtitle_id.is_empty() {
                        continue;
                    }

                    let download_path = format!("{WIZDOM_BASE}/api/files/sub/{subtitle_id}");

                    results.push(SubtitleSearchResult {
                        id: format!("wizdom-{subtitle_id}"),
                        name: version,
                        language: "he".into(),
                        language_name: "Hebrew".into(),
                        format: "srt".into(),
                        provider: "wizdom".into(),
                        detail_path: None,
                        download_path: Some(download_path),
                        download_count: None,
                        rating: None,
                        movie_name: None,
                        release_group: None,
                    });
                }
            }
        } else {
            // Movie: result["subs"] is a list
            if let Some(subs) = json["subs"].as_array() {
                for sub in subs {
                    let subtitle_id = sub["id"]
                        .as_u64()
                        .map(|v| v.to_string())
                        .or_else(|| sub["id"].as_str().map(|s| s.to_string()))
                        .unwrap_or_default();
                    let version = sub["version"].as_str().unwrap_or("").to_string();

                    if subtitle_id.is_empty() {
                        continue;
                    }

                    let download_path = format!("{WIZDOM_BASE}/api/files/sub/{subtitle_id}");

                    results.push(SubtitleSearchResult {
                        id: format!("wizdom-{subtitle_id}"),
                        name: version,
                        language: "he".into(),
                        language_name: "Hebrew".into(),
                        format: "srt".into(),
                        provider: "wizdom".into(),
                        detail_path: None,
                        download_path: Some(download_path),
                        download_count: None,
                        rating: None,
                        movie_name: None,
                        release_group: None,
                    });
                }
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
            .ok_or("wizdom: download_path required")?;

        let client = build_client()?;

        let resp = client
            .get(download_url)
            .send()
            .await
            .map_err(|e| format!("wizdom download failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("wizdom download failed: {}", resp.status()));
        }

        let content = resp
            .bytes()
            .await
            .map_err(|e| format!("wizdom read content error: {e}"))?;

        extract_archive(
            &content,
            "subtitle.zip",
            &request.language,
            &self.staging_root,
        )
        .await
    }
}
