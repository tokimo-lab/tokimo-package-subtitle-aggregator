use async_trait::async_trait;
use reqwest::header::USER_AGENT;
use serde::{Deserialize, Serialize};

use super::SubtitleProvider;
use crate::models::{
    DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult,
};

const OS_API_BASE: &str = "https://api.opensubtitles.com/api/v1";
const OS_USER_AGENT: &str = "subtitle-aggregator v0.1.0";

/// Language code mapping for OpenSubtitles REST API
fn to_os_language(lang: &str) -> &str {
    match lang {
        "zh-CN" | "zh-cn" | "zho" | "chi" => "zh-cn",
        "zh-TW" | "zh-tw" => "zh-tw",
        "zh" => "zh-cn",
        "en" | "eng" => "en",
        "ja" | "jpn" => "ja",
        "ko" | "kor" => "ko",
        other => other,
    }
}

fn from_os_language(lang: &str) -> String {
    match lang {
        "zh-cn" => "zh-CN".into(),
        "zh-tw" => "zh-TW".into(),
        other => other.to_string(),
    }
}

fn from_os_language_name(lang: &str) -> String {
    match lang {
        "zh-cn" => "简体中文".into(),
        "zh-tw" => "繁體中文".into(),
        "en" => "English".into(),
        "ja" => "日本語".into(),
        "ko" => "한국어".into(),
        other => other.to_string(),
    }
}

// ── OpenSubtitles REST API types ──

#[derive(Debug, Deserialize)]
struct OsSearchResponse {
    data: Vec<OsSubtitle>,
    #[serde(default)]
    total_count: u64,
}

#[derive(Debug, Deserialize)]
struct OsSubtitle {
    id: String,
    attributes: OsSubtitleAttributes,
}

#[derive(Debug, Deserialize)]
struct OsSubtitleAttributes {
    #[serde(default)]
    subtitle_id: Option<String>,
    language: String,
    #[serde(default)]
    download_count: u64,
    #[serde(default)]
    ratings: f64,
    #[serde(default)]
    release: Option<String>,
    #[serde(default)]
    feature_details: Option<OsFeatureDetails>,
    files: Vec<OsFile>,
}

#[derive(Debug, Deserialize)]
struct OsFeatureDetails {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    movie_name: Option<String>,
    #[serde(default)]
    year: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct OsFile {
    file_id: u64,
    #[serde(default)]
    file_name: Option<String>,
}

#[derive(Debug, Serialize)]
struct OsDownloadRequest {
    file_id: u64,
}

#[derive(Debug, Deserialize)]
struct OsDownloadResponse {
    link: String,
    file_name: String,
    #[serde(default)]
    remaining: u32,
}

pub struct OpenSubtitlesProvider {
    api_key: String,
}

impl OpenSubtitlesProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
        }
    }

    fn build_client(&self) -> Result<reqwest::Client, String> {
        reqwest::Client::builder()
            .build()
            .map_err(|error| format!("创建 OpenSubtitles HTTP 客户端失败: {error}"))
    }
}

#[async_trait]
impl SubtitleProvider for OpenSubtitlesProvider {
    fn name(&self) -> &str {
        "opensubtitles"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let client = self.build_client()?;

        let mut params: Vec<(&str, String)> = Vec::new();

        if let Some(query) = &request.query {
            let q = query.trim();
            if !q.is_empty() {
                params.push(("query", q.to_string()));
            }
        }
        if let Some(imdb_id) = &request.imdb_id {
            // OpenSubtitles expects numeric IMDB ID (strip "tt" prefix)
            let numeric = imdb_id.strip_prefix("tt").unwrap_or(imdb_id);
            params.push(("imdb_id", numeric.to_string()));
        }
        if let Some(tmdb_id) = &request.tmdb_id {
            params.push(("tmdb_id", tmdb_id.clone()));
        }

        // Build language filter
        if let Some(languages) = &request.languages {
            let os_langs: Vec<&str> = languages.iter().map(|l| to_os_language(l)).collect();
            if !os_langs.is_empty() {
                params.push(("languages", os_langs.join(",")));
            }
        }

        if params.is_empty() {
            return Err("OpenSubtitles 搜索需要提供 query 或 imdb_id".into());
        }

        let url = format!("{OS_API_BASE}/subtitles");
        let response = client
            .get(&url)
            .header("Api-Key", &self.api_key)
            .header(USER_AGENT, OS_USER_AGENT)
            .query(&params)
            .send()
            .await
            .map_err(|error| format!("OpenSubtitles 搜索请求失败: {error}"))?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("OpenSubtitles 搜索失败 ({status}): {body}"));
        }

        let os_response: OsSearchResponse = response
            .json()
            .await
            .map_err(|error| format!("解析 OpenSubtitles 搜索结果失败: {error}"))?;

        tracing::info!(
            "OpenSubtitles 搜索到 {} 条结果 (total: {})",
            os_response.data.len(),
            os_response.total_count
        );

        let results = os_response
            .data
            .into_iter()
            .filter_map(|sub| {
                let file = sub.attributes.files.first()?;
                let file_name = file
                    .file_name
                    .clone()
                    .unwrap_or_else(|| format!("subtitle_{}", file.file_id));
                let format = crate::models::normalize_format(&file_name)
                    .unwrap_or_else(|| "srt".into());
                let language = from_os_language(&sub.attributes.language);
                let language_name = from_os_language_name(&sub.attributes.language);
                let movie_name = sub
                    .attributes
                    .feature_details
                    .as_ref()
                    .and_then(|fd| {
                        fd.movie_name
                            .clone()
                            .or_else(|| fd.title.clone())
                            .map(|name| {
                                if let Some(year) = fd.year {
                                    format!("{name} ({year})")
                                } else {
                                    name
                                }
                            })
                    });

                Some(SubtitleSearchResult {
                    id: sub
                        .attributes
                        .subtitle_id
                        .unwrap_or_else(|| sub.id.clone()),
                    name: file_name,
                    language,
                    language_name,
                    format,
                    provider: "opensubtitles".into(),
                    detail_path: None,
                    download_path: Some(file.file_id.to_string()),
                    download_count: Some(sub.attributes.download_count),
                    rating: if sub.attributes.ratings > 0.0 {
                        Some(sub.attributes.ratings)
                    } else {
                        None
                    },
                    movie_name,
                    release_group: sub.attributes.release,
                })
            })
            .collect();

        Ok(results)
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        let client = self.build_client()?;

        let file_id: u64 = request
            .download_path
            .as_deref()
            .ok_or("OpenSubtitles 下载缺少 file_id")?
            .parse()
            .map_err(|_| "OpenSubtitles file_id 解析失败")?;

        let download_req = OsDownloadRequest { file_id };
        let url = format!("{OS_API_BASE}/download");
        let response = client
            .post(&url)
            .header("Api-Key", &self.api_key)
            .header(USER_AGENT, OS_USER_AGENT)
            .header("Content-Type", "application/json")
            .json(&download_req)
            .send()
            .await
            .map_err(|error| format!("OpenSubtitles 下载请求失败: {error}"))?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("OpenSubtitles 下载失败 ({status}): {body}"));
        }

        let dl_response: OsDownloadResponse = response
            .json()
            .await
            .map_err(|error| format!("解析 OpenSubtitles 下载响应失败: {error}"))?;

        tracing::info!(
            "OpenSubtitles 下载链接获取成功, 剩余配额: {}",
            dl_response.remaining
        );

        // Download the actual file
        let file_response = client
            .get(&dl_response.link)
            .send()
            .await
            .map_err(|error| format!("下载 OpenSubtitles 字幕文件失败: {error}"))?;

        if !file_response.status().is_success() {
            return Err(format!(
                "下载 OpenSubtitles 字幕文件失败: {}",
                file_response.status().as_u16()
            ));
        }

        let content = file_response
            .bytes()
            .await
            .map_err(|error| format!("读取 OpenSubtitles 字幕内容失败: {error}"))?;

        let format = crate::models::normalize_format(&dl_response.file_name)
            .unwrap_or_else(|| request.format.clone());

        Ok(DownloadedSubtitle {
            name: dl_response.file_name,
            format,
            content: content.to_vec(),
        })
    }
}
