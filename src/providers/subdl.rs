use async_trait::async_trait;
use serde::Deserialize;
use std::path::PathBuf;

use super::SubtitleProvider;
use crate::archive::extract_archive;
use crate::models::{
    normalize_format, DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest,
    SubtitleSearchResult,
};

const SUBDL_API_BASE: &str = "https://api.subdl.com/api/v1";
const SUBDL_DOWNLOAD_BASE: &str = "https://dl.subdl.com";
const SUBDL_USER_AGENT: &str = "subtitle-aggregator v0.1.0";

/// Map ISO 639-1 language codes to SubDL language codes.
fn to_subdl_language(lang: &str) -> &str {
    match lang {
        "zh-CN" | "zh-cn" | "zh" | "zho" | "chi" => "SC",
        "zh-TW" | "zh-tw" => "TC",
        "en" | "eng" => "EN",
        "ja" | "jpn" => "JA",
        "ko" | "kor" => "KO",
        "fr" | "fra" => "FR",
        "de" | "deu" | "ger" => "DE",
        "es" | "spa" => "ES",
        "pt" | "por" => "PT",
        "it" | "ita" => "IT",
        "ru" | "rus" => "RU",
        "ar" | "ara" => "AR",
        "nl" | "nld" => "NL",
        "pl" | "pol" => "PL",
        "sv" | "swe" => "SV",
        "tr" | "tur" => "TR",
        "vi" | "vie" => "VI",
        "th" | "tha" => "TH",
        "id" | "ind" => "ID",
        "ms" | "msa" => "MS",
        other => other,
    }
}

/// Map SubDL language codes back to ISO 639-1.
fn from_subdl_language(lang: &str) -> String {
    match lang {
        "SC" | "ZH" => "zh-CN".into(),
        "TC" => "zh-TW".into(),
        "EN" => "en".into(),
        "JA" => "ja".into(),
        "KO" => "ko".into(),
        "FR" => "fr".into(),
        "DE" => "de".into(),
        "ES" => "es".into(),
        "PT" => "pt".into(),
        "IT" => "it".into(),
        "RU" => "ru".into(),
        "AR" => "ar".into(),
        "NL" => "nl".into(),
        "PL" => "pl".into(),
        "SV" => "sv".into(),
        "TR" => "tr".into(),
        "VI" => "vi".into(),
        "TH" => "th".into(),
        "ID" => "id".into(),
        "MS" => "ms".into(),
        other => other.to_ascii_lowercase(),
    }
}

fn from_subdl_language_name(lang: &str) -> String {
    match lang {
        "SC" | "ZH" => "简体中文".into(),
        "TC" => "繁體中文".into(),
        "EN" => "English".into(),
        "JA" => "日本語".into(),
        "KO" => "한국어".into(),
        "FR" => "Français".into(),
        "DE" => "Deutsch".into(),
        "ES" => "Español".into(),
        "PT" => "Português".into(),
        "IT" => "Italiano".into(),
        "RU" => "Русский".into(),
        "AR" => "العربية".into(),
        other => other.to_string(),
    }
}

// ── SubDL API response types ──

#[derive(Debug, Deserialize)]
struct SubdlSearchResponse {
    #[serde(default)]
    status: Option<bool>,
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    subtitles: Vec<SubdlSubtitleItem>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct SubdlSubtitleItem {
    name: String,
    language: String,
    url: String,
    #[serde(default, rename = "subtitlePage")]
    subtitle_page: Option<String>,
    #[serde(default)]
    releases: Vec<String>,
    #[serde(default)]
    hi: Option<bool>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    episode_from: Option<serde_json::Value>,
    #[serde(default)]
    episode_end: Option<serde_json::Value>,
}

pub struct SubdlProvider {
    api_key: String,
    staging_root: PathBuf,
}

impl SubdlProvider {
    pub fn new(api_key: Option<String>) -> Self {
        let api_key = api_key.unwrap_or_default();
        let staging_root = std::env::temp_dir();
        Self {
            api_key,
            staging_root,
        }
    }

    pub fn with_staging_root(mut self, staging_root: impl Into<PathBuf>) -> Self {
        self.staging_root = staging_root.into();
        self
    }

    fn build_client(&self) -> Result<reqwest::Client, String> {
        reqwest::Client::builder()
            .user_agent(SUBDL_USER_AGENT)
            .build()
            .map_err(|e| format!("创建 SubDL HTTP 客户端失败: {e}"))
    }
}

#[async_trait]
impl SubtitleProvider for SubdlProvider {
    fn name(&self) -> &str {
        "subdl"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let client = self.build_client()?;

        // Build language list
        let langs = request
            .languages
            .as_deref()
            .map(|langs| {
                let mut unique: Vec<&str> = langs.iter().map(|l| to_subdl_language(l)).collect();
                unique.dedup();
                unique.join(",")
            })
            .unwrap_or_default();

        let mut params: Vec<(&str, String)> = vec![
            ("api_key", self.api_key.clone()),
            ("subs_per_page", "30".into()),
            ("comment", "1".into()),
            ("releases", "1".into()),
        ];

        if !langs.is_empty() {
            params.push(("languages", langs));
        }

        // Determine movie vs TV and identifier
        // We default to movie; callers with TV content should include that info via query
        let has_imdb = request.imdb_id.is_some();
        let has_tmdb = request.tmdb_id.is_some();

        if let Some(imdb_id) = &request.imdb_id {
            params.push(("imdb_id", imdb_id.clone()));
        } else if let Some(tmdb_id) = &request.tmdb_id {
            params.push(("tmdb_id", tmdb_id.clone()));
        } else if let Some(query) = &request.query {
            let q = query.trim();
            if !q.is_empty() {
                params.push(("film_name", q.to_string()));
            }
        } else {
            return Err("SubDL 搜索需要提供 imdb_id、tmdb_id 或 query".into());
        }

        // type param: default movie; could be made configurable
        params.push(("type", "movie".into()));

        let url = format!("{SUBDL_API_BASE}/subtitles");
        let response = client
            .get(&url)
            .query(&params)
            .send()
            .await
            .map_err(|e| format!("SubDL 搜索请求失败: {e}"))?;

        match response.status().as_u16() {
            429 => return Err("SubDL 请求过于频繁 (429 Too Many Requests)".into()),
            403 => return Err("SubDL API Key 无效 (403 Forbidden)".into()),
            200 => {}
            status => {
                let body = response.text().await.unwrap_or_default();
                return Err(format!("SubDL 搜索失败 ({status}): {body}"));
            }
        }

        let search_response: SubdlSearchResponse = response
            .json()
            .await
            .map_err(|e| format!("解析 SubDL 搜索结果失败: {e}"))?;

        // Check for API-level errors
        let api_ok =
            search_response.status.unwrap_or(true) && search_response.success.unwrap_or(true);
        if !api_ok {
            if let Some(error_msg) = &search_response.error {
                let lower = error_msg.to_ascii_lowercase();
                if lower.contains("can't find") || lower.contains("not found") {
                    tracing::debug!("SubDL: 未找到字幕 - {error_msg}");
                    return Ok(vec![]);
                }
                // If we used imdb_id and got an error, try falling back to tmdb_id
                if has_imdb && has_tmdb {
                    tracing::debug!("SubDL: IMDB 搜索失败，尝试 TMDB fallback");
                    return self
                        .search_by_tmdb(&client, &url, &params, request.tmdb_id.as_deref().unwrap())
                        .await;
                }
                return Err(format!("SubDL API 错误: {error_msg}"));
            }
            return Ok(vec![]);
        }

        tracing::info!("SubDL 搜索到 {} 条字幕", search_response.subtitles.len());

        let results = search_response
            .subtitles
            .into_iter()
            .map(|item| {
                let lang_code = item.language.to_uppercase();
                let language = from_subdl_language(&lang_code);
                let language_name = from_subdl_language_name(&lang_code);
                let format = normalize_format(&item.name).unwrap_or_else(|| "srt".into());
                let release_group = if item.releases.is_empty() {
                    None
                } else {
                    Some(item.releases.join(", "))
                };
                let detail_path = item.subtitle_page.map(|page| {
                    if page.starts_with("http") {
                        page
                    } else {
                        format!("https://subdl.com{page}")
                    }
                });

                SubtitleSearchResult {
                    id: item.name.clone(),
                    name: item.name,
                    language,
                    language_name,
                    format,
                    provider: "subdl".into(),
                    detail_path,
                    download_path: Some(item.url),
                    download_count: None,
                    rating: None,
                    movie_name: None,
                    release_group,
                }
            })
            .collect();

        Ok(results)
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        let download_link = request
            .download_path
            .as_deref()
            .ok_or("SubDL 下载缺少 download_path")?;

        // Build full URL: if it already starts with http use as-is, else prepend base
        let url = if download_link.starts_with("http") {
            download_link.to_string()
        } else {
            format!("{SUBDL_DOWNLOAD_BASE}{download_link}")
        };

        let client = self.build_client()?;
        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("SubDL 下载请求失败: {e}"))?;

        match response.status().as_u16() {
            429 => return Err("SubDL 下载超出每日限额 (429)".into()),
            500 => {
                let body = response.text().await.unwrap_or_default();
                if body.contains("Download limit exceeded") {
                    return Err("SubDL 每日下载限额已达上限".into());
                }
                return Err(format!("SubDL 下载服务器错误: {body}"));
            }
            403 => return Err("SubDL API Key 无效 (403 Forbidden)".into()),
            200 => {}
            status => {
                let body = response.text().await.unwrap_or_default();
                return Err(format!("SubDL 下载失败 ({status}): {body}"));
            }
        }

        let content = response
            .bytes()
            .await
            .map_err(|e| format!("读取 SubDL 下载内容失败: {e}"))?;

        // The download_link path looks like /subtitles/xxx.zip
        let archive_name = download_link
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("subtitle.zip");

        extract_archive(
            &content,
            archive_name,
            &request.language,
            &self.staging_root,
        )
        .await
    }
}

impl SubdlProvider {
    async fn search_by_tmdb(
        &self,
        client: &reqwest::Client,
        url: &str,
        base_params: &[(&str, String)],
        tmdb_id: &str,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let mut params: Vec<(&str, String)> = base_params
            .iter()
            .filter(|(k, _)| *k != "imdb_id" && *k != "film_name")
            .cloned()
            .collect();
        params.push(("tmdb_id", tmdb_id.to_string()));

        let response = client
            .get(url)
            .query(&params)
            .send()
            .await
            .map_err(|e| format!("SubDL TMDB fallback 请求失败: {e}"))?;

        if !response.status().is_success() {
            return Ok(vec![]);
        }

        let search_response: SubdlSearchResponse = response
            .json()
            .await
            .map_err(|e| format!("解析 SubDL TMDB fallback 结果失败: {e}"))?;

        let api_ok =
            search_response.status.unwrap_or(true) && search_response.success.unwrap_or(true);
        if !api_ok {
            return Ok(vec![]);
        }

        let results = search_response
            .subtitles
            .into_iter()
            .map(|item| {
                let lang_code = item.language.to_uppercase();
                let language = from_subdl_language(&lang_code);
                let language_name = from_subdl_language_name(&lang_code);
                let format = normalize_format(&item.name).unwrap_or_else(|| "srt".into());
                let release_group = if item.releases.is_empty() {
                    None
                } else {
                    Some(item.releases.join(", "))
                };
                let detail_path = item.subtitle_page.map(|page| {
                    if page.starts_with("http") {
                        page
                    } else {
                        format!("https://subdl.com{page}")
                    }
                });
                SubtitleSearchResult {
                    id: item.name.clone(),
                    name: item.name,
                    language,
                    language_name,
                    format,
                    provider: "subdl".into(),
                    detail_path,
                    download_path: Some(item.url),
                    download_count: None,
                    rating: None,
                    movie_name: None,
                    release_group,
                }
            })
            .collect();

        Ok(results)
    }
}
