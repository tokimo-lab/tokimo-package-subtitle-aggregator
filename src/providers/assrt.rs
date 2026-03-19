use std::path::{Path, PathBuf};

use async_trait::async_trait;
use regex::Regex;
use reqwest::header::{ACCEPT, REFERER};
use scraper::{Html, Selector};
use tokio::{fs, process::Command};
use super::SubtitleProvider;
use crate::archive::extract_archive;
use crate::models::{
    normalize_format, normalize_language, matches_preferred_language, score_subtitle_name,
    DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult,
};

const ASSRT_BASE_URL: &str = "https://assrt.net";
const ASSRT_USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

#[derive(Debug, Clone)]
struct DetailSubtitleFile {
    name: String,
    format: String,
    url: String,
}

pub struct AssrtProvider {
    staging_root: PathBuf,
}

impl AssrtProvider {
    pub fn new(staging_root: impl Into<PathBuf>) -> Self {
        Self {
            staging_root: staging_root.into(),
        }
    }
}

fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(ASSRT_USER_AGENT)
        .build()
        .map_err(|error| format!("创建 assrt HTTP 客户端失败: {error}"))
}

fn absolute_assrt_url(path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else {
        format!(
            "{}{}{}",
            ASSRT_BASE_URL,
            if path.starts_with('/') { "" } else { "/" },
            path
        )
    }
}

fn text_content(element: scraper::ElementRef<'_>) -> String {
    element
        .text()
        .collect::<String>()
        .replace('\u{a0}', " ")
        .trim()
        .to_string()
}

fn filename_from_path(path: &str, fallback: &str) -> String {
    path.rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn is_assrt_block_page(content: &[u8]) -> bool {
    content.starts_with(b"<?xml")
        || content.starts_with(b"<html>")
        || String::from_utf8_lossy(content).contains("/errpage/40x")
}

async fn fetch_assrt_html(url: &str) -> Result<String, String> {
    let client = build_client()?;
    let response = client
        .get(url)
        .header(
            ACCEPT,
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .send()
        .await
        .map_err(|error| format!("assrt 请求失败: {error}"))?;

    if !response.status().is_success() {
        return Err(format!("assrt 请求失败: {}", response.status().as_u16()));
    }

    response
        .text()
        .await
        .map_err(|error| format!("读取 assrt 响应失败: {error}"))
}

fn parse_detail_download_files(
    detail_html: &str,
    _subtitle_id: &str,
) -> Result<Vec<DetailSubtitleFile>, String> {
    let onthefly_regex = Regex::new(r#"onthefly\("(\d+)","(\d+)","([^"]+)"\)"#)
        .map_err(|error| format!("detail file regex failed: {error}"))?;
    let mut files = Vec::new();

    for captures in onthefly_regex.captures_iter(detail_html) {
        let Some(subtitle_id_value) = captures.get(1).map(|value| value.as_str()) else {
            continue;
        };
        let Some(index) = captures.get(2).map(|value| value.as_str()) else {
            continue;
        };
        let Some(name) = captures
            .get(3)
            .map(|value| value.as_str().trim().to_string())
        else {
            continue;
        };
        let Some(format) = normalize_format(&name) else {
            continue;
        };
        let url = {
            let mut download_url = url::Url::parse(ASSRT_BASE_URL)
                .map_err(|error| format!("解析 assrt 下载基础地址失败: {error}"))?;
            download_url
                .path_segments_mut()
                .map_err(|_| "构造 assrt 下载地址失败".to_string())?
                .extend(["download", subtitle_id_value, "-", index, name.as_str()]);
            download_url.to_string()
        };
        files.push(DetailSubtitleFile {
            format,
            name: name.clone(),
            url,
        });
    }

    Ok(files)
}

fn assrt_cookie_jar_path(staging_root: &Path) -> PathBuf {
    staging_root.join("assrt-subtitles").join("cookies.txt")
}

async fn warm_assrt_session(cookie_jar: &Path) -> Result<(), String> {
    if let Some(parent) = cookie_jar.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|error| format!("创建 assrt cookie 目录失败: {error}"))?;
    }

    let cookie_jar_str = cookie_jar.to_string_lossy().to_string();
    let accept_html = "Accept: text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";
    let warmup_output = Command::new("curl")
        .arg("-sSL")
        .arg("-A")
        .arg(ASSRT_USER_AGENT)
        .arg("-H")
        .arg(accept_html)
        .arg("-b")
        .arg(&cookie_jar_str)
        .arg("-c")
        .arg(&cookie_jar_str)
        .arg("-o")
        .arg("/dev/null")
        .arg(ASSRT_BASE_URL)
        .output()
        .await
        .map_err(|error| format!("调用 curl 预热 assrt 会话失败: {error}"))?;

    if !warmup_output.status.success() {
        return Err(format!(
            "curl 预热 assrt 会话失败: {}",
            String::from_utf8_lossy(&warmup_output.stderr).trim()
        ));
    }

    Ok(())
}

async fn fetch_assrt_detail_html_via_curl(
    detail_url: &str,
    cookie_jar: &Path,
) -> Result<String, String> {
    let cookie_jar_str = cookie_jar.to_string_lossy().to_string();
    let accept_html = "Accept: text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";
    let detail_output = Command::new("curl")
        .arg("-sSL")
        .arg("-A")
        .arg(ASSRT_USER_AGENT)
        .arg("-H")
        .arg(accept_html)
        .arg("-b")
        .arg(&cookie_jar_str)
        .arg("-c")
        .arg(&cookie_jar_str)
        .arg("-e")
        .arg(ASSRT_BASE_URL)
        .arg(detail_url)
        .output()
        .await
        .map_err(|error| format!("调用 curl 获取 assrt 详情页失败: {error}"))?;

    if !detail_output.status.success() {
        return Err(format!(
            "curl 获取 assrt 详情页失败: {}",
            String::from_utf8_lossy(&detail_output.stderr).trim()
        ));
    }

    String::from_utf8(detail_output.stdout)
        .map_err(|error| format!("解码 assrt 详情页失败: {error}"))
}

async fn download_detail_subtitle_via_curl(
    detail_path: &str,
    subtitle_id: &str,
    preferred_language: &str,
    staging_root: &Path,
) -> Result<DownloadedSubtitle, String> {
    let cookie_jar = assrt_cookie_jar_path(staging_root);
    let cookie_jar_str = cookie_jar.to_string_lossy().to_string();
    let detail_url = absolute_assrt_url(detail_path);

    warm_assrt_session(&cookie_jar).await?;

    let mut detail_html = fetch_assrt_detail_html_via_curl(&detail_url, &cookie_jar).await?;
    if detail_html.contains("/errpage/40x") {
        warm_assrt_session(&cookie_jar).await?;
        detail_html = fetch_assrt_detail_html_via_curl(&detail_url, &cookie_jar).await?;
    }
    if detail_html.contains("/errpage/40x") {
        return Err("assrt 详情页请求失败: 493".into());
    }

    let detail_files = parse_detail_download_files(&detail_html, subtitle_id)?;
    let selected_file = detail_files
        .into_iter()
        .max_by_key(|file| score_subtitle_name(&file.name, &file.format, preferred_language))
        .ok_or_else(|| "assrt 详情页未提供可下载字幕文件".to_string())?;

    let mut file_output = Command::new("curl")
        .arg("-sSL")
        .arg("-A")
        .arg(ASSRT_USER_AGENT)
        .arg("-H")
        .arg("Accept: */*")
        .arg("-b")
        .arg(&cookie_jar_str)
        .arg("-c")
        .arg(&cookie_jar_str)
        .arg("-e")
        .arg(&detail_url)
        .arg(&selected_file.url)
        .output()
        .await
        .map_err(|error| format!("调用 curl 下载 assrt 字幕失败: {error}"))?;

    if !file_output.status.success() {
        return Err(format!(
            "curl 下载 assrt 字幕失败: {}",
            String::from_utf8_lossy(&file_output.stderr).trim()
        ));
    }

    if is_assrt_block_page(&file_output.stdout) {
        warm_assrt_session(&cookie_jar).await?;
        file_output = Command::new("curl")
            .arg("-sSL")
            .arg("-A")
            .arg(ASSRT_USER_AGENT)
            .arg("-H")
            .arg("Accept: */*")
            .arg("-b")
            .arg(&cookie_jar_str)
            .arg("-c")
            .arg(&cookie_jar_str)
            .arg("-e")
            .arg(&detail_url)
            .arg(&selected_file.url)
            .output()
            .await
            .map_err(|error| format!("调用 curl 重试下载 assrt 字幕失败: {error}"))?;
    }

    if !file_output.status.success() {
        return Err(format!(
            "curl 重试下载 assrt 字幕失败: {}",
            String::from_utf8_lossy(&file_output.stderr).trim()
        ));
    }

    if is_assrt_block_page(&file_output.stdout) {
        return Err(format!("assrt 单文件下载失败: 493 ({})", selected_file.url));
    }

    Ok(DownloadedSubtitle {
        name: selected_file.name,
        format: selected_file.format,
        content: file_output.stdout,
    })
}

async fn download_archive_subtitle(
    request: &SubtitleDownloadRequest,
    staging_root: &Path,
) -> Result<DownloadedSubtitle, String> {
    let client = build_client()?;
    let download_path = request
        .download_path
        .clone()
        .ok_or_else(|| "assrt 搜索结果缺少下载地址".to_string())?;
    let download_url = absolute_assrt_url(&download_path);
    let response = client
        .get(&download_url)
        .header(ACCEPT, "*/*")
        .header(REFERER, ASSRT_BASE_URL)
        .send()
        .await
        .map_err(|error| format!("assrt 请求失败: {error}"))?;

    if !response.status().is_success() {
        return Err(format!("assrt 请求失败: {}", response.status().as_u16()));
    }

    let archive_file_name = filename_from_path(&download_path, &request.subtitle_id);
    let content = response
        .bytes()
        .await
        .map_err(|error| format!("读取 assrt 下载内容失败: {error}"))?;
    let subtitle_name = request
        .name
        .clone()
        .unwrap_or_else(|| archive_file_name.clone());

    if let Some(format) = normalize_format(&archive_file_name) {
        return Ok(DownloadedSubtitle {
            name: subtitle_name,
            format,
            content: content.to_vec(),
        });
    }

    extract_archive(
        &content,
        &archive_file_name,
        &request.language,
        staging_root,
    )
    .await
}

#[async_trait]
impl SubtitleProvider for AssrtProvider {
    fn name(&self) -> &str {
        "assrt"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let query = request.query.clone().unwrap_or_default().trim().to_string();
        if query.is_empty() {
            return Err("请输入片名或文件名后再搜索 assrt 字幕".into());
        }

        let search_url = format!(
            "{ASSRT_BASE_URL}/sub/?searchword={}&sort=rank&no_redir=1",
            url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>()
        );
        let html = fetch_assrt_html(&search_url).await?;
        let document = Html::parse_document(&html);
        let row_selector = Selector::parse(".resultcard .subitem")
            .map_err(|error| format!("解析 assrt 搜索选择器失败: {error}"))?;
        let title_selector = Selector::parse(".introtitle")
            .map_err(|error| format!("解析 assrt 标题选择器失败: {error}"))?;
        let meta_span_selector = Selector::parse("#sublist_div span")
            .map_err(|error| format!("解析 assrt 元数据选择器失败: {error}"))?;
        let version_selector = Selector::parse("#meta_top b")
            .map_err(|error| format!("解析 assrt 版本选择器失败: {error}"))?;
        let rating_selector =
            Selector::parse(r#"img[alt*="用户评分"], img[title*="用户评分"]"#)
                .map_err(|error| format!("解析 assrt 评分选择器失败: {error}"))?;
        let detail_id_regex = Regex::new(r"/(\d+)\.xml$")
            .map_err(|error| format!("detail id regex failed: {error}"))?;
        let download_regex = Regex::new(r"location\.href='([^']+)'")
            .map_err(|error| format!("download path regex failed: {error}"))?;
        let number_regex =
            Regex::new(r"([\d.]+)").map_err(|error| format!("number regex failed: {error}"))?;
        let download_count_regex = Regex::new(r"下载次数：\s*(\d+)")
            .map_err(|error| format!("download count regex failed: {error}"))?;

        let mut results = Vec::new();

        for row in document.select(&row_selector) {
            let Some(detail_anchor) = row.select(&title_selector).next() else {
                continue;
            };
            let detail_path = detail_anchor
                .value()
                .attr("href")
                .map(|value| value.to_string());
            let Some(detail_path_value) = detail_path.as_deref() else {
                continue;
            };
            let Some(id_match) = detail_id_regex.captures(detail_path_value) else {
                continue;
            };
            let Some(id) = id_match.get(1).map(|value| value.as_str().to_string()) else {
                continue;
            };

            let span_texts = row
                .select(&meta_span_selector)
                .map(text_content)
                .collect::<Vec<_>>();
            let format_text = span_texts
                .iter()
                .find_map(|text| text.strip_prefix("格式："))
                .map(str::trim)
                .unwrap_or_default()
                .to_string();
            let Some(format) = normalize_format(&format_text) else {
                continue;
            };
            let language_text = span_texts
                .iter()
                .find_map(|text| text.strip_prefix("语言："))
                .map(str::trim)
                .unwrap_or("未知")
                .to_string();
            let version_text = row
                .select(&version_selector)
                .next()
                .map(text_content)
                .unwrap_or_default();
            let download_count = span_texts
                .iter()
                .find_map(|text| download_count_regex.captures(text))
                .and_then(|captures| captures.get(1))
                .and_then(|value| value.as_str().parse::<u64>().ok());
            let rating = row
                .select(&rating_selector)
                .next()
                .and_then(|node| {
                    node.value()
                        .attr("alt")
                        .or_else(|| node.value().attr("title"))
                        .map(str::to_string)
                })
                .and_then(|text| {
                    number_regex
                        .captures(&text)
                        .and_then(|captures| {
                            captures.get(1).map(|value| value.as_str().to_string())
                        })
                })
                .and_then(|value| value.parse::<f64>().ok());
            let download_path = download_regex
                .captures(&row.html())
                .and_then(|captures| captures.get(1))
                .map(|value| value.as_str().to_string());
            let language = normalize_language(&language_text);
            if !matches_preferred_language(&language, request.languages.as_deref()) {
                continue;
            }

            results.push(SubtitleSearchResult {
                id,
                name: {
                    let title = text_content(detail_anchor);
                    if title.is_empty() {
                        if version_text.is_empty() {
                            query.clone()
                        } else {
                            version_text.clone()
                        }
                    } else {
                        title
                    }
                },
                language,
                language_name: language_text,
                format,
                provider: "assrt".into(),
                detail_path,
                download_path,
                download_count,
                rating,
                movie_name: detail_anchor
                    .value()
                    .attr("title")
                    .map(|value| value.trim().to_string()),
                release_group: (!version_text.is_empty()).then_some(version_text),
            });
        }

        Ok(results)
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        if let Some(detail_path) = request.detail_path.as_deref() {
            match download_detail_subtitle_via_curl(
                detail_path,
                &request.subtitle_id,
                &request.language,
                &self.staging_root,
            )
            .await
            {
                Ok(subtitle) => return Ok(subtitle),
                Err(detail_error) => {
                    if request.download_path.is_none() {
                        return Err(detail_error);
                    }
                }
            }
        }

        download_archive_subtitle(request, &self.staging_root).await
    }
}
