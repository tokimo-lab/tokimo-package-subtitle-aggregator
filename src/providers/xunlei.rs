/// Xunlei subtitle provider (迅雷字幕)
/// API: http://sub.xmp.sandai.net:8000/subxl/{cid}.json
/// CID = SHA1 hash of the video file content (similar to ed2k CID)
/// Note: different from XunleiClient (which is for torrent downloading)
use async_trait::async_trait;
use sha1::{Digest, Sha1};
use serde::Deserialize;

use super::SubtitleProvider;
use crate::models::{
    DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult,
};

const XUNLEI_SUB_API: &str = "http://sub.xmp.sandai.net:8000/subxl";

#[derive(Debug, Deserialize)]
struct XunleiSubResponse {
    #[serde(rename = "sublist")]
    sublist: Vec<XunleiSubItem>,
}

#[derive(Debug, Deserialize)]
struct XunleiSubItem {
    sname: String,
    surl: String,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    rate: Option<f64>,
    #[serde(default)]
    count: Option<u64>,
}

pub struct XunleiSubtitleProvider;

impl XunleiSubtitleProvider {
    pub fn new() -> Self {
        Self
    }

    /// Compute Xunlei CID: SHA1 of the entire file
    /// (simplified; full xunlei CID uses a more complex hash but SHA1 works for many files)
    pub fn compute_cid(data: &[u8]) -> String {
        let mut hasher = Sha1::new();
        hasher.update(data);
        format!("{:X}", hasher.finalize())
    }
}

#[async_trait]
impl SubtitleProvider for XunleiSubtitleProvider {
    fn name(&self) -> &str {
        "xunlei"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let cid = request
            .file_hash
            .as_deref()
            .ok_or("迅雷字幕搜索需要提供 file_hash (CID)")?;

        let url = format!("{XUNLEI_SUB_API}/{cid}.json");
        let query_title = request.query.clone().unwrap_or_else(|| "video".into());

        let client = reqwest::Client::builder()
            .user_agent("Thunder/3.0")
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| format!("创建迅雷客户端失败: {e}"))?;

        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("迅雷字幕搜索失败: {e}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "迅雷字幕搜索失败: HTTP {}",
                response.status().as_u16()
            ));
        }

        let data: XunleiSubResponse = response
            .json()
            .await
            .map_err(|e| format!("解析迅雷字幕响应失败: {e}"))?;

        tracing::info!("迅雷字幕搜索到 {} 条结果", data.sublist.len());

        let results = data
            .sublist
            .into_iter()
            .enumerate()
            .filter_map(|(i, item)| {
                if item.surl.is_empty() {
                    return None;
                }
                let ext = item
                    .sname
                    .rsplit('.')
                    .next()
                    .map(|e| e.to_ascii_lowercase())
                    .unwrap_or_else(|| "srt".into());
                let format = match ext.as_str() {
                    "srt" | "ass" | "ssa" | "vtt" => ext.clone(),
                    _ => "srt".into(),
                };
                let language = item
                    .language
                    .as_deref()
                    .map(|l| match l {
                        "chs" | "zh-CN" | "zh_CN" | "简体" => "zh-CN".to_string(),
                        "cht" | "zh-TW" | "zh_TW" | "繁体" => "zh-TW".to_string(),
                        "eng" | "en" => "en".to_string(),
                        _ => "zh".to_string(),
                    })
                    .unwrap_or_else(|| "zh".into());

                Some(SubtitleSearchResult {
                    id: format!("xunlei_{cid}_{i}"),
                    name: item.sname.clone(),
                    language: language.clone(),
                    language_name: match language.as_str() {
                        "zh-CN" => "简体中文".into(),
                        "zh-TW" => "繁體中文".into(),
                        "en" => "English".into(),
                        _ => "中文".into(),
                    },
                    format,
                    provider: "xunlei".into(),
                    detail_path: None,
                    download_path: Some(item.surl),
                    download_count: item.count,
                    rating: item.rate,
                    movie_name: Some(query_title.clone()),
                    release_group: None,
                })
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
            .ok_or("迅雷字幕下载缺少 URL")?;

        let client = reqwest::Client::builder()
            .user_agent("Thunder/3.0")
            .build()
            .map_err(|e| format!("创建迅雷客户端失败: {e}"))?;

        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("迅雷字幕下载失败: {e}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "迅雷字幕下载失败: HTTP {}",
                response.status().as_u16()
            ));
        }

        let content = response
            .bytes()
            .await
            .map_err(|e| format!("读取迅雷字幕内容失败: {e}"))?;

        Ok(DownloadedSubtitle {
            name: request
                .name
                .clone()
                .unwrap_or_else(|| format!("xunlei.{}", request.format)),
            format: request.format.clone(),
            content: content.to_vec(),
        })
    }
}
