/// TheSubDB provider
/// Hash-based subtitle DB - 完全免费，无需账号
/// 算法: MD5(首 64KB + 末 64KB) → 16进制
/// API: http://api.thesubdb.com/?action=search&hash=xxx
use async_trait::async_trait;

use super::SubtitleProvider;
use crate::models::{
    DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult,
};

const THESUBDB_API: &str = "http://api.thesubdb.com/";
const HASH_CHUNK: usize = 64 * 1024; // 64KB

pub struct TheSubDbProvider;

impl TheSubDbProvider {
    pub fn new() -> Self {
        Self
    }

    /// Compute TheSubDB hash: MD5(first 64KB + last 64KB)
    pub fn compute_hash(data: &[u8]) -> Option<String> {
        if data.len() < HASH_CHUNK * 2 {
            return None;
        }
        let first = &data[..HASH_CHUNK];
        let last = &data[data.len() - HASH_CHUNK..];
        let mut combined = Vec::with_capacity(HASH_CHUNK * 2);
        combined.extend_from_slice(first);
        combined.extend_from_slice(last);
        Some(format!("{:x}", md5::compute(&combined)))
    }

    fn build_client() -> Result<reqwest::Client, String> {
        reqwest::Client::builder()
            .user_agent("SubDB/1.0 (subtitle-aggregator/0.1; https://github.com/)")
            .build()
            .map_err(|e| format!("创建 thesubdb 客户端失败: {e}"))
    }
}

#[async_trait]
impl SubtitleProvider for TheSubDbProvider {
    fn name(&self) -> &str {
        "thesubdb"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let hash = request
            .file_hash
            .as_deref()
            .ok_or("TheSubDB 搜索需要提供 file_hash")?;

        let client = Self::build_client()?;
        let url = format!("{THESUBDB_API}?action=search&hash={hash}");

        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("TheSubDB 搜索失败: {e}"))?;

        if response.status().as_u16() == 404 {
            return Ok(Vec::new()); // no results
        }

        if !response.status().is_success() {
            return Err(format!(
                "TheSubDB 搜索失败: HTTP {}",
                response.status().as_u16()
            ));
        }

        // Response is comma-separated language codes: "en,pt,es"
        let body = response
            .text()
            .await
            .map_err(|e| format!("读取 TheSubDB 响应失败: {e}"))?;

        let query_title = request.query.clone().unwrap_or_else(|| "video".into());
        let preferred = request.languages.as_deref().unwrap_or(&[]);

        let results = body
            .split(',')
            .map(|lang| lang.trim())
            .filter(|lang| !lang.is_empty())
            .filter(|lang| {
                if preferred.is_empty() {
                    return true;
                }
                // map thesubdb lang → our lang and check
                let our_lang = thesubdb_to_lang(lang);
                preferred
                    .iter()
                    .any(|p| p == &our_lang || (p.starts_with("zh") && our_lang.starts_with("zh")))
            })
            .map(|lang| {
                let our_lang = thesubdb_to_lang(lang);
                SubtitleSearchResult {
                    id: format!("thesubdb_{hash}_{lang}"),
                    name: format!("{}.{lang}.srt", query_title.replace(['/', '\\', ':'], "_")),
                    language: our_lang.clone(),
                    language_name: lang_display_name(&our_lang),
                    format: "srt".into(),
                    provider: "thesubdb".into(),
                    detail_path: None,
                    // Encode hash+lang into download_path
                    download_path: Some(format!("{hash}|{lang}")),
                    download_count: None,
                    rating: None,
                    movie_name: Some(query_title.clone()),
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
        let dl_path = request
            .download_path
            .as_deref()
            .ok_or("TheSubDB 下载缺少 download_path")?;

        let (hash, lang) = dl_path
            .split_once('|')
            .ok_or("TheSubDB download_path 格式错误")?;

        let client = Self::build_client()?;
        let url = format!("{THESUBDB_API}?action=download&hash={hash}&language={lang}");

        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("TheSubDB 下载失败: {e}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "TheSubDB 下载失败: HTTP {}",
                response.status().as_u16()
            ));
        }

        let content = response
            .bytes()
            .await
            .map_err(|e| format!("读取 TheSubDB 字幕内容失败: {e}"))?;

        let name = request
            .name
            .clone()
            .unwrap_or_else(|| format!("{hash}.{lang}.srt"));

        Ok(DownloadedSubtitle {
            name,
            format: "srt".into(),
            content: content.to_vec(),
        })
    }
}

fn thesubdb_to_lang(lang: &str) -> String {
    match lang {
        "zh" => "zh-CN".into(),
        "en" => "en".into(),
        "ja" => "ja".into(),
        "ko" => "ko".into(),
        other => other.to_string(),
    }
}

fn lang_display_name(lang: &str) -> String {
    match lang {
        "zh-CN" | "zh" => "简体中文".into(),
        "en" => "English".into(),
        "ja" => "日本語".into(),
        "ko" => "한국어".into(),
        other => other.to_string(),
    }
}
