use async_trait::async_trait;
use serde::Deserialize;

use super::SubtitleProvider;
use crate::models::{
    DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult,
};

const SHOOTER_API_URL: &str = "https://www.shooter.cn/api/subapi.php";

/// Shooter (射手网) subtitle provider
///
/// Uses file-hash based matching. Requires the video file to calculate hashes.
/// Hash algorithm: Take 4 blocks of 4KB from the file at specific positions,
/// compute MD5 of each block, join with ";".
pub struct ShooterProvider;

impl Default for ShooterProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ShooterProvider {
    pub fn new() -> Self {
        Self
    }

    /// Compute shooter-style hash from file bytes.
    /// Takes 4 blocks of 4096 bytes at positions:
    ///   - 4096 from start
    ///   - 1/3 of file size
    ///   - 2/3 of file size
    ///   - 4096 bytes from end (file_size - 8192)
    pub fn compute_file_hash(file_data: &[u8]) -> Option<String> {
        let file_size = file_data.len();
        if file_size < 12288 {
            // File too small for hash computation (need at least ~12KB)
            return None;
        }

        let block_size = 4096usize;
        let offsets = [
            4096usize,
            file_size / 3,
            file_size * 2 / 3,
            file_size.saturating_sub(8192),
        ];

        let hashes: Vec<String> = offsets
            .iter()
            .filter_map(|&offset| {
                if offset + block_size > file_size {
                    return None;
                }
                let block = &file_data[offset..offset + block_size];
                let hash = format!("{:x}", md5::compute(block));
                Some(hash)
            })
            .collect();

        if hashes.len() == 4 {
            Some(hashes.join(";"))
        } else {
            None
        }
    }
}

#[derive(Debug, Deserialize)]
struct ShooterSubtitle {
    #[serde(rename = "Desc")]
    desc: Option<String>,
    #[serde(rename = "Delay")]
    #[allow(dead_code)]
    delay: Option<i64>,
    #[serde(rename = "Files")]
    files: Vec<ShooterFile>,
}

#[derive(Debug, Deserialize)]
struct ShooterFile {
    #[serde(rename = "Ext")]
    ext: String,
    #[serde(rename = "Link")]
    link: String,
}

#[async_trait]
impl SubtitleProvider for ShooterProvider {
    fn name(&self) -> &str {
        "shooter"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let file_hash = request
            .file_hash
            .as_deref()
            .ok_or("射手网搜索需要提供 file_hash (文件哈希)")?;

        let file_name = request.query.clone().unwrap_or_else(|| "video".into());

        let client = reqwest::Client::builder()
            .build()
            .map_err(|error| format!("创建 HTTP 客户端失败: {error}"))?;

        let params = [
            ("filehash", file_hash),
            ("pathinfo", file_name.as_str()),
            ("format", "json"),
            ("lang", "chn"),
        ];

        let response = client
            .post(SHOOTER_API_URL)
            .form(&params)
            .send()
            .await
            .map_err(|error| format!("射手网搜索请求失败: {error}"))?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            if status == 200 {
                // Shooter returns 200 even for no results, but with empty body
            }
            return Err(format!("射手网搜索失败: HTTP {status}"));
        }

        let body = response
            .text()
            .await
            .map_err(|error| format!("读取射手网响应失败: {error}"))?;

        // Shooter returns literal -1 when no matches found
        if body.trim() == "-1" || body.trim().is_empty() {
            return Ok(Vec::new());
        }

        let subtitles: Vec<ShooterSubtitle> = serde_json::from_str(&body)
            .map_err(|error| format!("解析射手网搜索结果失败: {error}"))?;

        let mut results = Vec::new();

        for (index, subtitle) in subtitles.iter().enumerate() {
            for (file_index, file) in subtitle.files.iter().enumerate() {
                let ext = file.ext.to_ascii_lowercase();
                let format = match ext.as_str() {
                    "srt" => "srt",
                    "ass" => "ass",
                    "ssa" => "ssa",
                    "vtt" => "vtt",
                    _ => "srt",
                };
                let name = format!(
                    "shooter_{}_{}_{}.{}",
                    index,
                    file_index,
                    subtitle
                        .desc
                        .as_deref()
                        .unwrap_or("subtitle")
                        .replace(['/', '\\', ':', ' '], "_"),
                    ext
                );

                results.push(SubtitleSearchResult {
                    id: format!("shooter_{index}_{file_index}"),
                    name,
                    language: "zh".into(),
                    language_name: "中文".into(),
                    format: format.into(),
                    provider: "shooter".into(),
                    detail_path: None,
                    download_path: Some(file.link.clone()),
                    download_count: None,
                    rating: None,
                    movie_name: Some(file_name.clone()),
                    release_group: subtitle.desc.clone(),
                });
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
            .ok_or("射手网下载缺少下载地址")?;

        let client = reqwest::Client::builder()
            .build()
            .map_err(|error| format!("创建 HTTP 客户端失败: {error}"))?;

        let response = client
            .get(download_url)
            .send()
            .await
            .map_err(|error| format!("下载射手网字幕失败: {error}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "下载射手网字幕失败: HTTP {}",
                response.status().as_u16()
            ));
        }

        let content = response
            .bytes()
            .await
            .map_err(|error| format!("读取射手网字幕内容失败: {error}"))?;

        let format = request.format.clone();
        let name = request
            .name
            .clone()
            .unwrap_or_else(|| format!("subtitle.{format}"));

        Ok(DownloadedSubtitle {
            name,
            format,
            content: content.to_vec(),
        })
    }
}
