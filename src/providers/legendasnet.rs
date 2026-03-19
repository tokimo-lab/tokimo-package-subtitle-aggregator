/// LegendasNet provider — Portuguese subtitle site (legendas.net)
/// REST JSON API: https://legendas.net/api/v1/ — requires login via LEGENDASNET_USER / LEGENDASNET_PASS
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::SubtitleProvider;
use crate::archive::extract_archive;
use crate::models::{DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult};

const SERVER_HOSTNAME: &str = "legendas.net/api";
const UA: &str = "Sub-Zero/2";

fn server_url() -> String {
    format!("https://{SERVER_HOSTNAME}/v1/")
}

fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(UA)
        .build()
        .map_err(|e| format!("legendasnet: failed to build client: {e}"))
}

fn get_credentials() -> Result<(String, String), String> {
    let user = std::env::var("LEGENDASNET_USER")
        .map_err(|_| "legendasnet: LEGENDASNET_USER environment variable not set")?;
    let pass = std::env::var("LEGENDASNET_PASS")
        .map_err(|_| "legendasnet: LEGENDASNET_PASS environment variable not set")?;
    Ok((user, pass))
}

#[derive(Debug, Serialize)]
struct LoginRequest {
    email: String,
    password: String,
}

#[derive(Debug, Deserialize)]
struct LoginResponse {
    access_token: Option<String>,
}

#[derive(Debug, Serialize)]
struct TvSearchRequest {
    name: String,
    page: u32,
    per_page: u32,
    tv_episode: Option<u32>,
    tv_season: Option<u32>,
    imdb_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct MovieSearchRequest {
    name: String,
    page: u32,
    per_page: u32,
    imdb_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TvSearchResponse {
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    status: Option<bool>,
    #[serde(rename = "tv_shows", default)]
    tv_shows: Vec<SubtitleItem>,
}

#[derive(Debug, Deserialize)]
struct MovieSearchResponse {
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    status: Option<bool>,
    #[serde(default)]
    movies: Vec<SubtitleItem>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct SubtitleItem {
    id: serde_json::Value,
    #[serde(default)]
    release_name: Option<String>,
    #[serde(default)]
    uploader: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    comment: Option<String>,
    #[serde(default)]
    tmdb_id: Option<serde_json::Value>,
    #[serde(default)]
    season: Option<serde_json::Value>,
    #[serde(default)]
    episode: Option<serde_json::Value>,
}

async fn login(client: &reqwest::Client, email: &str, password: &str) -> Result<String, String> {
    let payload = LoginRequest {
        email: email.to_string(),
        password: password.to_string(),
    };

    let resp = client
        .post(format!("{}login", server_url()))
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("legendasnet: login request failed: {e}"))?;

    if resp.status().as_u16() != 200 {
        return Err(format!(
            "legendasnet: login failed with HTTP {}",
            resp.status().as_u16()
        ));
    }

    let body: LoginResponse = resp.json().await
        .map_err(|e| format!("legendasnet: parse login response: {e}"))?;

    body.access_token.ok_or_else(|| "legendasnet: access token not found in login response".into())
}

fn is_forced(item: &SubtitleItem) -> bool {
    let comment = item.comment.as_deref().unwrap_or("").to_lowercase();
    comment.contains("forced") || comment.contains("foreign")
}

fn item_id(item: &SubtitleItem) -> String {
    match &item.id {
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

pub struct LegendasNetProvider {
    staging_root: std::path::PathBuf,
}

impl LegendasNetProvider {
    pub fn new(staging_root: impl Into<std::path::PathBuf>) -> Self {
        Self { staging_root: staging_root.into() }
    }
}

#[async_trait]
impl SubtitleProvider for LegendasNetProvider {
    fn name(&self) -> &str {
        "legendasnet"
    }

    async fn search(&self, request: &SubtitleSearchRequest) -> Result<Vec<SubtitleSearchResult>, String> {
        let (email, password) = get_credentials()?;
        let client = build_client()?;
        let token = login(&client, &email, &password).await?;

        let auth_client = reqwest::Client::builder()
            .user_agent(UA)
            .default_headers({
                let mut h = reqwest::header::HeaderMap::new();
                h.insert(
                    reqwest::header::AUTHORIZATION,
                    format!("Bearer {token}").parse().unwrap(),
                );
                h
            })
            .build()
            .map_err(|e| format!("legendasnet: build auth client: {e}"))?;

        let query = request.query.clone().unwrap_or_default();
        // Determine if TV or movie search based on presence of season info in query
        // Since we only have a generic query, use imdb_id hint if present
        let is_tv = query.contains("S0") || query.contains("Season") || query.contains("s0");
        let mut results = Vec::new();

        if is_tv {
            let payload = TvSearchRequest {
                name: query.clone(),
                page: 1,
                per_page: 25,
                tv_episode: None,
                tv_season: None,
                imdb_id: request.imdb_id.clone(),
            };

            let resp = auth_client
                .get(format!("{}search/tv", server_url()))
                .header("Content-Type", "application/json")
                .json(&payload)
                .send()
                .await
                .map_err(|e| format!("legendasnet: TV search request: {e}"))?;

            let status = resp.status().as_u16();
            if status == 403 { return Err("legendasnet: invalid access token".into()); }
            if status == 429 { return Err("legendasnet: too many requests (429)".into()); }
            if status == 404 { return Ok(vec![]); }
            if status != 200 { return Err(format!("legendasnet: TV search HTTP {status}")); }

            let body: TvSearchResponse = resp.json().await
                .map_err(|e| format!("legendasnet: parse TV search: {e}"))?;

            if body.success == Some(false) || body.status == Some(false) {
                return Ok(vec![]);
            }

            for item in body.tv_shows {
                let forced = is_forced(&item);
                let id = item_id(&item);
                let release = item.release_name.clone().unwrap_or_default();
                let uploader = item.uploader.clone().unwrap_or_else(|| "unknown".into());
                let download_path = item.path.clone().unwrap_or_default();
                let tmdb_id = item.tmdb_id.as_ref().map(|v| v.to_string()).unwrap_or_default();
                let page_link = format!("https://legendas.net/tv_legenda?movie_id={tmdb_id}&legenda_id={id}");
                let lang = if forced { "pt-forced" } else { "pt-BR" };
                let lang_name = if forced { "Portuguese (Forced)" } else { "Portuguese (Brazilian)" };

                results.push(SubtitleSearchResult {
                    id: id.clone(),
                    name: release.clone(),
                    language: lang.into(),
                    language_name: lang_name.into(),
                    format: "srt".into(),
                    provider: "legendasnet".into(),
                    detail_path: Some(page_link),
                    download_path: Some(format!("https://legendas.net{}", download_path)),
                    download_count: None,
                    rating: None,
                    movie_name: Some(query.clone()),
                    release_group: Some(uploader),
                });
            }
        } else {
            let payload = MovieSearchRequest {
                name: query.clone(),
                page: 1,
                per_page: 25,
                imdb_id: request.imdb_id.clone(),
            };

            let resp = auth_client
                .get(format!("{}search/movie", server_url()))
                .header("Content-Type", "application/json")
                .json(&payload)
                .send()
                .await
                .map_err(|e| format!("legendasnet: movie search request: {e}"))?;

            let status = resp.status().as_u16();
            if status == 403 { return Err("legendasnet: invalid access token".into()); }
            if status == 429 { return Err("legendasnet: too many requests (429)".into()); }
            if status == 404 { return Ok(vec![]); }
            if status != 200 { return Err(format!("legendasnet: movie search HTTP {status}")); }

            let body: MovieSearchResponse = resp.json().await
                .map_err(|e| format!("legendasnet: parse movie search: {e}"))?;

            if body.success == Some(false) || body.status == Some(false) {
                return Ok(vec![]);
            }

            for item in body.movies {
                let forced = is_forced(&item);
                let id = item_id(&item);
                let release = item.release_name.clone().unwrap_or_default();
                let uploader = item.uploader.clone().unwrap_or_else(|| "unknown".into());
                let download_path = item.path.clone().unwrap_or_default();
                let tmdb_id = item.tmdb_id.as_ref().map(|v| v.to_string()).unwrap_or_default();
                let page_link = format!("https://legendas.net/legenda?movie_id={tmdb_id}&legenda_id={id}");
                let lang = if forced { "pt-forced" } else { "pt-BR" };
                let lang_name = if forced { "Portuguese (Forced)" } else { "Portuguese (Brazilian)" };

                results.push(SubtitleSearchResult {
                    id: id.clone(),
                    name: release.clone(),
                    language: lang.into(),
                    language_name: lang_name.into(),
                    format: "srt".into(),
                    provider: "legendasnet".into(),
                    detail_path: Some(page_link),
                    download_path: Some(format!("https://legendas.net{}", download_path)),
                    download_count: None,
                    rating: None,
                    movie_name: Some(query.clone()),
                    release_group: Some(uploader),
                });
            }
        }

        tracing::info!("legendasnet: found {} results for '{}'", results.len(), query);
        Ok(results)
    }

    async fn download(&self, request: &SubtitleDownloadRequest) -> Result<DownloadedSubtitle, String> {
        let (email, password) = get_credentials()?;
        let client = build_client()?;
        let token = login(&client, &email, &password).await?;

        let url = request.download_path.as_deref()
            .or(request.detail_path.as_deref())
            .ok_or("legendasnet: download requires download_path")?;

        let resp = client
            .get(url)
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
            .send()
            .await
            .map_err(|e| format!("legendasnet: download request: {e}"))?;

        let status = resp.status().as_u16();
        if status == 429 { return Err("legendasnet: daily download limit exceeded".into()); }
        if status == 403 { return Err("legendasnet: invalid access token".into()); }
        if status != 200 { return Err(format!("legendasnet: download HTTP {status}")); }

        let bytes = resp.bytes().await.map_err(|e| format!("legendasnet: read bytes: {e}"))?;
        let filename = url.rsplit('/').next().unwrap_or("subtitle.zip").to_string();

        if bytes.starts_with(b"PK") || filename.ends_with(".zip") {
            return extract_archive(&bytes, &filename, &request.language, &self.staging_root).await;
        }

        Ok(DownloadedSubtitle {
            name: request.name.clone().unwrap_or(filename),
            format: request.format.clone(),
            content: bytes.to_vec(),
        })
    }
}
