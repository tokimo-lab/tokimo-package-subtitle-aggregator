/// Zimuku (字幕库) provider - zimuku.cn
/// 中国最大的字幕网站之一，支持简繁中文、英文字幕，HTML scraping
use async_trait::async_trait;
use regex::Regex;
use scraper::{Html, Selector};

use super::SubtitleProvider;
use crate::archive::extract_archive;
use crate::models::{
    normalize_format, normalize_language, DownloadedSubtitle, SubtitleDownloadRequest,
    SubtitleSearchRequest, SubtitleSearchResult,
};

const ZIMUKU_BASE: &str = "https://zimuku.cn";
const ZIMUKU_UA: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(ZIMUKU_UA)
        .build()
        .map_err(|e| format!("创建 zimuku 客户端失败: {e}"))
}

pub struct ZimukuProvider {
    staging_root: std::path::PathBuf,
}

impl ZimukuProvider {
    pub fn new(staging_root: impl Into<std::path::PathBuf>) -> Self {
        Self {
            staging_root: staging_root.into(),
        }
    }
}

#[async_trait]
impl SubtitleProvider for ZimukuProvider {
    fn name(&self) -> &str {
        "zimuku"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let query = request.query.clone().unwrap_or_default();
        if query.trim().is_empty() {
            return Err("字幕库搜索需要提供 query".into());
        }

        let client = build_client()?;
        let search_url = format!(
            "{ZIMUKU_BASE}/search?q={}",
            url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>()
        );

        let response = client
            .get(&search_url)
            .header("Referer", ZIMUKU_BASE)
            .send()
            .await
            .map_err(|e| format!("字幕库搜索失败: {e}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "字幕库搜索失败: HTTP {}",
                response.status().as_u16()
            ));
        }

        let html = response
            .text()
            .await
            .map_err(|e| format!("读取字幕库响应失败: {e}"))?;

        parse_zimuku_search_results(&html, &query)
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        let detail_url = request
            .detail_path
            .as_deref()
            .ok_or("字幕库下载缺少详情页地址")?;

        let client = build_client()?;

        // Fetch detail page to get real download link
        let detail_response = client
            .get(detail_url)
            .header("Referer", ZIMUKU_BASE)
            .send()
            .await
            .map_err(|e| format!("获取字幕库详情页失败: {e}"))?;

        if !detail_response.status().is_success() {
            return Err(format!(
                "获取字幕库详情页失败: HTTP {}",
                detail_response.status().as_u16()
            ));
        }

        let detail_html = detail_response
            .text()
            .await
            .map_err(|e| format!("读取字幕库详情页失败: {e}"))?;

        let download_url = extract_zimuku_download_url(&detail_html, detail_url)?;

        let dl_response = client
            .get(&download_url)
            .header("Referer", detail_url)
            .send()
            .await
            .map_err(|e| format!("下载字幕库字幕失败: {e}"))?;

        if !dl_response.status().is_success() {
            return Err(format!(
                "下载字幕库字幕失败: HTTP {}",
                dl_response.status().as_u16()
            ));
        }

        let file_name = dl_response
            .headers()
            .get("content-disposition")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| {
                let re = Regex::new(r#"filename[^;=\n]*=(?:(['"])(.+?)\1|([^;\n]*))"#).ok()?;
                re.captures(v)
                    .and_then(|c| c.get(2).or_else(|| c.get(3)))
                    .map(|m| m.as_str().trim().to_string())
            })
            .unwrap_or_else(|| format!("zimuku_{}.zip", request.subtitle_id));

        let content = dl_response
            .bytes()
            .await
            .map_err(|e| format!("读取字幕库字幕内容失败: {e}"))?;

        if let Some(format) = normalize_format(&file_name) {
            return Ok(DownloadedSubtitle {
                name: file_name,
                format,
                content: content.to_vec(),
            });
        }

        extract_archive(&content, &file_name, &request.language, &self.staging_root).await
    }
}

fn parse_zimuku_search_results(
    html: &str,
    query: &str,
) -> Result<Vec<SubtitleSearchResult>, String> {
    let document = Html::parse_document(html);
    let mut results = Vec::new();

    let item_sel = Selector::parse(".item.prel")
        .or_else(|_| Selector::parse(".subitem"))
        .map_err(|e| format!("解析字幕库选择器失败: {e}"))?;

    let title_sel = Selector::parse("a.title, .title a, h2 a, h3 a")
        .map_err(|e| format!("解析字幕库标题选择器失败: {e}"))?;

    let lang_sel = Selector::parse(".other span, .label, .type span")
        .map_err(|e| format!("解析字幕库语言选择器失败: {e}"))?;

    let id_re = Regex::new(r"/detail/(\d+)").map_err(|e| format!("regex failed: {e}"))?;
    let dl_re = Regex::new(r"(\d+)次").map_err(|e| format!("regex failed: {e}"))?;

    for item in document.select(&item_sel) {
        let Some(anchor) = item.select(&title_sel).next() else {
            continue;
        };
        let href = anchor.value().attr("href").unwrap_or("");
        let Some(id_cap) = id_re.captures(href) else {
            continue;
        };
        let id = id_cap[1].to_string();
        let name = anchor
            .text()
            .collect::<String>()
            .replace('\u{a0}', " ")
            .trim()
            .to_string();
        if name.is_empty() {
            continue;
        }

        // Collect language labels
        let lang_texts: Vec<String> = item
            .select(&lang_sel)
            .map(|e| e.text().collect::<String>())
            .collect();
        let combined_lang = lang_texts.join(" ");
        let language = normalize_language(&combined_lang);
        let language_name = if language.starts_with("zh-CN") {
            "简体中文".into()
        } else if language.starts_with("zh-TW") {
            "繁體中文".into()
        } else if language == "zh" {
            "中文双语".into()
        } else if language == "en" {
            "English".into()
        } else {
            combined_lang.trim().to_string()
        };

        let download_count = item
            .text()
            .collect::<String>()
            .lines()
            .find_map(|line| dl_re.captures(line))
            .and_then(|c| c[1].parse::<u64>().ok());

        let detail_path = if href.starts_with("http") {
            href.to_string()
        } else {
            format!("{ZIMUKU_BASE}{href}")
        };

        results.push(SubtitleSearchResult {
            id,
            name: name.clone(),
            language,
            language_name,
            format: "srt".into(), // actual format determined at download
            provider: "zimuku".into(),
            detail_path: Some(detail_path),
            download_path: None,
            download_count,
            rating: None,
            movie_name: Some(query.to_string()),
            release_group: None,
        });
    }

    tracing::info!("字幕库搜索到 {} 条结果", results.len());
    Ok(results)
}

fn extract_zimuku_download_url(html: &str, detail_url: &str) -> Result<String, String> {
    let document = Html::parse_document(html);

    // Try known download button selectors
    let selectors = [
        "a.btn-success",
        "a.down-btn",
        "a[href*='/download/']",
        "#subtitle-title ~ * a[href*='download']",
    ];

    for sel_str in &selectors {
        if let Ok(sel) = Selector::parse(sel_str) {
            if let Some(anchor) = document.select(&sel).next() {
                if let Some(href) = anchor.value().attr("href") {
                    let url = if href.starts_with("http") {
                        href.to_string()
                    } else {
                        format!("{ZIMUKU_BASE}{href}")
                    };
                    return Ok(url);
                }
            }
        }
    }

    Err(format!("未能从字幕库详情页提取下载地址: {detail_url}"))
}
