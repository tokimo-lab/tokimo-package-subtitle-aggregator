use async_trait::async_trait;
use serde::Deserialize;

use super::SubtitleProvider;
use crate::models::{
    DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult,
};

const PODNAPISI_API: &str = "https://www.podnapisi.net/subtitles/search/old";

fn to_podnapisi_lang(lang: &str) -> Option<&str> {
    match lang {
        "zh-CN" | "zh-cn" | "zh" => Some("chn"),
        "zh-TW" | "zh-tw" => Some("cht"),
        "en" => Some("eng"),
        "ja" => Some("jpn"),
        "ko" => Some("kor"),
        _ => None,
    }
}

fn from_podnapisi_lang(lang: &str) -> String {
    match lang {
        "chn" => "zh-CN".into(),
        "cht" => "zh-TW".into(),
        "eng" => "en".into(),
        "jpn" => "ja".into(),
        "kor" => "ko".into(),
        other => other.to_string(),
    }
}

fn from_podnapisi_lang_name(lang: &str) -> String {
    match lang {
        "chn" => "简体中文".into(),
        "cht" => "繁體中文".into(),
        "eng" => "English".into(),
        "jpn" => "日本語".into(),
        "kor" => "한국어".into(),
        other => other.to_string(),
    }
}

#[derive(Debug, Deserialize)]
struct PodnapisiResponse {
    #[serde(default)]
    subtitles: Vec<PodnapisiSubtitle>,
}

#[derive(Debug, Deserialize)]
struct PodnapisiSubtitle {
    id: String,
    #[serde(rename = "release")]
    release: Option<String>,
    language: String,
    #[serde(rename = "upload_count", default)]
    upload_count: u64,
    #[serde(rename = "rating", default)]
    rating: f64,
    url: String,
    #[serde(rename = "movie_year")]
    movie_year: Option<u32>,
    #[serde(rename = "movie_title")]
    movie_title: Option<String>,
}

pub struct PodnapisiProvider;

impl Default for PodnapisiProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl PodnapisiProvider {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SubtitleProvider for PodnapisiProvider {
    fn name(&self) -> &str {
        "podnapisi"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let query = request.query.clone().unwrap_or_default();
        if query.trim().is_empty() {
            return Err("Podnapisi 搜索需要提供 query".into());
        }

        let client = reqwest::Client::builder()
            .user_agent("subtitle-aggregator/0.1")
            .build()
            .map_err(|e| format!("创建客户端失败: {e}"))?;

        // Build language list
        let langs: Vec<&str> = request
            .languages
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .filter_map(|l| to_podnapisi_lang(l))
            .collect();

        let lang_param = if langs.is_empty() {
            "chn,cht,eng".to_string()
        } else {
            langs.join(",")
        };

        let mut params = vec![
            ("keywords", query.as_str()),
            ("language", lang_param.as_str()),
            ("format", "json"),
        ];

        let imdb_id_str;
        if let Some(imdb_id) = &request.imdb_id {
            let numeric = imdb_id.strip_prefix("tt").unwrap_or(imdb_id);
            imdb_id_str = numeric.to_string();
            params.push(("imdbid", imdb_id_str.as_str()));
        }

        let response = client
            .get(PODNAPISI_API)
            .query(&params)
            .send()
            .await
            .map_err(|e| format!("Podnapisi 搜索失败: {e}"))?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            if status == 429 {
                return Err("Podnapisi 请求过于频繁 (429)，请稍后重试".into());
            }
            return Err(format!("Podnapisi 搜索失败: HTTP {status}"));
        }

        let data: PodnapisiResponse = response
            .json()
            .await
            .map_err(|e| format!("解析 Podnapisi 响应失败: {e}"))?;

        tracing::info!("Podnapisi 搜索到 {} 条结果", data.subtitles.len());

        let results = data
            .subtitles
            .into_iter()
            .map(|sub| {
                let language = from_podnapisi_lang(&sub.language);
                let language_name = from_podnapisi_lang_name(&sub.language);
                let movie_name = sub.movie_title.map(|t| {
                    if let Some(y) = sub.movie_year {
                        format!("{t} ({y})")
                    } else {
                        t
                    }
                });
                SubtitleSearchResult {
                    id: sub.id.clone(),
                    name: sub
                        .release
                        .clone()
                        .unwrap_or_else(|| format!("subtitle_{}", sub.id)),
                    language,
                    language_name,
                    format: "srt".into(),
                    provider: "podnapisi".into(),
                    detail_path: None,
                    download_path: Some(format!("{}/download", sub.url)),
                    download_count: Some(sub.upload_count),
                    rating: if sub.rating > 0.0 {
                        Some(sub.rating)
                    } else {
                        None
                    },
                    movie_name,
                    release_group: sub.release,
                }
            })
            .collect();

        Ok(results)
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        let url = request
            .download_path
            .as_deref()
            .ok_or("Podnapisi 下载缺少 URL")?;

        let client = reqwest::Client::builder()
            .user_agent("subtitle-aggregator/0.1")
            .build()
            .map_err(|e| format!("创建客户端失败: {e}"))?;

        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("Podnapisi 下载失败: {e}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "Podnapisi 下载失败: HTTP {}",
                response.status().as_u16()
            ));
        }

        let content = response
            .bytes()
            .await
            .map_err(|e| format!("读取 Podnapisi 字幕内容失败: {e}"))?;

        Ok(DownloadedSubtitle {
            name: request
                .name
                .clone()
                .unwrap_or_else(|| format!("podnapisi_{}.{}", request.subtitle_id, request.format)),
            format: request.format.clone(),
            content: content.to_vec(),
        })
    }
}
