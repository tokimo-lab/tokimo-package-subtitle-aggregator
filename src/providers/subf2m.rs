/// Subf2m subtitle provider - subf2m.co
/// HTML scraping provider, no API key required
use async_trait::async_trait;
use scraper::{Html, Selector};

use super::SubtitleProvider;
use crate::archive::extract_archive;
use crate::models::{
    normalize_format, DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest,
    SubtitleSearchResult,
};

const BASE_URL: &str = "https://subf2m.co";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/111.0.0.0 Safari/537.36";

/// Map from our language codes to subf2m URL path segments
fn lang_to_subf2m_path(lang: &str) -> Option<&'static str> {
    match lang {
        "zh-CN" | "zh" => Some("chinese-bg-code"),
        "zh-TW" => Some("chinese-bg-code"),
        "en" => Some("english"),
        "ja" => Some("japanese"),
        "ko" => Some("korean"),
        "ar" => Some("arabic"),
        "es" => Some("spanish"),
        "pt" => Some("portuguese"),
        "pt-BR" => Some("brazillian-portuguese"),
        "it" => Some("italian"),
        "nl" => Some("dutch"),
        "he" => Some("hebrew"),
        "id" => Some("indonesian"),
        "da" => Some("danish"),
        "no" => Some("norwegian"),
        "bn" => Some("bengali"),
        "bg" => Some("bulgarian"),
        "hr" => Some("croatian"),
        "sv" => Some("swedish"),
        "vi" => Some("vietnamese"),
        "cs" => Some("czech"),
        "fi" => Some("finnish"),
        "fr" => Some("french"),
        "de" => Some("german"),
        "el" => Some("greek"),
        "hu" => Some("hungarian"),
        "is" => Some("icelandic"),
        "mk" => Some("macedonian"),
        "ms" => Some("malay"),
        "pl" => Some("polish"),
        "ro" => Some("romanian"),
        "ru" => Some("russian"),
        "sr" => Some("serbian"),
        "th" => Some("thai"),
        "tr" => Some("turkish"),
        "fa" => Some("farsi_persian"),
        _ => None,
    }
}

fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .default_headers({
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert("authority", "subf2m.co".parse().unwrap());
            headers.insert("referer", "https://subf2m.co".parse().unwrap());
            headers.insert(
                "accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"
                    .parse()
                    .unwrap(),
            );
            headers.insert("accept-language", "en-US,en;q=0.9".parse().unwrap());
            headers
        })
        .build()
        .map_err(|e| format!("Failed to build subf2m client: {e}"))
}

pub struct Subf2mProvider {
    staging_root: std::path::PathBuf,
}

impl Subf2mProvider {
    pub fn new(staging_root: impl Into<std::path::PathBuf>) -> Self {
        Self {
            staging_root: staging_root.into(),
        }
    }
}

/// Fetch HTML text from a URL, retrying on 404/503
async fn safe_get_text(client: &reqwest::Client, url: &str) -> Result<String, String> {
    let mut last_err = String::new();
    for attempt in 0..3u8 {
        let resp = client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("subf2m GET {url} failed: {e}"))?;

        let status = resp.status();
        if status == 403 {
            return Err(format!("subf2m: access forbidden for {url}"));
        }
        if status == 404 || status == 503 {
            last_err = format!("subf2m: HTTP {} for {url}", status.as_u16());
            if attempt < 2 {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                continue;
            }
            return Err(last_err);
        }
        if !status.is_success() {
            return Err(format!("subf2m: HTTP {} for {url}", status.as_u16()));
        }
        return resp
            .text()
            .await
            .map_err(|e| format!("subf2m: failed to read body from {url}: {e}"));
    }
    Err(last_err)
}

/// Search subf2m for movie slugs matching the query, return up to `limit` hrefs
fn parse_search_results(html: &str) -> Vec<String> {
    let document = Html::parse_document(html);
    let Ok(sel) = Selector::parse("li div.title a") else {
        return vec![];
    };
    document
        .select(&sel)
        .filter_map(|a| a.value().attr("href").map(|h| h.to_string()))
        .filter(|href| href.starts_with("/subtitles/"))
        .collect()
}

/// Parse subtitle items from a language page, return (release_name, download_href)
fn parse_subtitle_items(html: &str) -> Vec<(String, String)> {
    let document = Html::parse_document(html);
    let Ok(item_sel) = Selector::parse("li.item") else {
        return vec![];
    };
    let Ok(dl_sel) = Selector::parse("a.download.icon-download") else {
        return vec![];
    };
    let Ok(scroll_sel) = Selector::parse("ul.scrolllist") else {
        return vec![];
    };

    let mut results = Vec::new();

    for item in document.select(&item_sel) {
        // Get download href
        let Some(dl_anchor) = item.select(&dl_sel).next() else {
            continue;
        };
        let Some(dl_href) = dl_anchor.value().attr("href") else {
            continue;
        };

        // Build release name from scrolllist entries
        let release_name: String = item
            .select(&scroll_sel)
            .flat_map(|ul| {
                ul.text()
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
            })
            .collect::<Vec<_>>()
            .join(" / ");

        results.push((release_name, dl_href.to_string()));
    }

    results
}

#[async_trait]
impl SubtitleProvider for Subf2mProvider {
    fn name(&self) -> &str {
        "subf2m"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let query = request.query.clone().unwrap_or_default();
        if query.trim().is_empty() {
            return Err("subf2m search requires a query".into());
        }

        let client = build_client()?;

        // Determine which language paths to query
        let languages = request.languages.clone().unwrap_or_default();
        let lang_paths: Vec<(&str, &'static str)> = if languages.is_empty() {
            // Default to English
            vec![("en", "english")]
        } else {
            languages
                .iter()
                .filter_map(|l| lang_to_subf2m_path(l).map(|p| (l.as_str(), p)))
                .collect()
        };

        // Step 1: search for movie slugs
        let encoded_query =
            url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>();
        let search_url = format!("{BASE_URL}/subtitles/searchbytitle?query={encoded_query}&l=");

        let search_html = safe_get_text(&client, &search_url).await?;
        let slugs = parse_search_results(&search_html);

        if slugs.is_empty() {
            return Ok(vec![]);
        }

        // Use only the top 3 results
        let slugs: Vec<String> = slugs.into_iter().take(3).collect();

        let mut results: Vec<SubtitleSearchResult> = Vec::new();

        // Step 2: for each slug × language, fetch and parse subtitle items
        'slug_loop: for slug in &slugs {
            let mut found_any = false;

            for &(lang_code, lang_path) in &lang_paths {
                let lang_page_url = format!("{BASE_URL}{slug}/{lang_path}");

                let lang_html = match safe_get_text(&client, &lang_page_url).await {
                    Ok(h) => h,
                    Err(_) => continue,
                };

                let items = parse_subtitle_items(&lang_html);

                for (idx, (release_name, dl_href)) in items.iter().enumerate() {
                    let id = format!(
                        "subf2m:{}:{}:{}",
                        slug.trim_start_matches('/'),
                        lang_path,
                        idx
                    );

                    // Determine language name
                    let language_name = match lang_code {
                        "zh-CN" => "Chinese Simplified",
                        "zh-TW" => "Chinese Traditional",
                        "zh" => "Chinese",
                        "en" => "English",
                        "ja" => "Japanese",
                        "ko" => "Korean",
                        _ => lang_path,
                    }
                    .to_string();

                    // Guess format from release name
                    let format =
                        normalize_format(release_name).unwrap_or_else(|| "srt".to_string());

                    let download_path = if dl_href.starts_with("http") {
                        dl_href.clone()
                    } else {
                        format!("{BASE_URL}{dl_href}")
                    };

                    results.push(SubtitleSearchResult {
                        id,
                        name: if release_name.is_empty() {
                            format!("{} [{}]", query, lang_code)
                        } else {
                            release_name.clone()
                        },
                        language: lang_code.to_string(),
                        language_name,
                        format,
                        provider: "subf2m".into(),
                        detail_path: Some(lang_page_url.clone()),
                        download_path: Some(download_path),
                        download_count: None,
                        rating: None,
                        movie_name: Some(query.clone()),
                        release_group: None,
                    });

                    found_any = true;
                }
            }

            // Stop after first slug that has results
            if found_any {
                break 'slug_loop;
            }
        }

        Ok(results)
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        // Prefer download_path (direct ZIP link); fall back to detail_path which
        // requires extracting the real download button link
        let client = build_client()?;

        let download_url = if let Some(dp) = &request.download_path {
            dp.clone()
        } else if let Some(detail) = &request.detail_path {
            // Fetch detail page and find the download button
            let detail_html = safe_get_text(&client, detail).await?;
            let document = Html::parse_document(&detail_html);
            let sel = Selector::parse("a#downloadButton")
                .map_err(|e| format!("selector parse error: {e}"))?;
            let href = document
                .select(&sel)
                .next()
                .and_then(|a| a.value().attr("href"))
                .ok_or_else(|| format!("subf2m: couldn't find download button on {detail}"))?;
            if href.starts_with("http") {
                href.to_string()
            } else {
                format!("{BASE_URL}{href}")
            }
        } else {
            return Err("subf2m download requires download_path or detail_path".into());
        };

        let resp = client
            .get(&download_url)
            .send()
            .await
            .map_err(|e| format!("subf2m download failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!(
                "subf2m download HTTP {}: {download_url}",
                resp.status().as_u16()
            ));
        }

        let file_name = resp
            .headers()
            .get("content-disposition")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| {
                // filename="foo.zip" or filename=foo.zip
                v.split(';').find_map(|part| {
                    let part = part.trim();
                    if part.to_ascii_lowercase().starts_with("filename") {
                        part.split_once('=')
                            .map(|x| x.1)
                            .map(|s| s.trim().trim_matches('"').to_string())
                    } else {
                        None
                    }
                })
            })
            .unwrap_or_else(|| format!("subf2m_{}.zip", request.subtitle_id));

        let content = resp
            .bytes()
            .await
            .map_err(|e| format!("subf2m: failed to read download bytes: {e}"))?;

        // If the content itself is already a subtitle file, return it directly
        if let Some(format) = normalize_format(&file_name) {
            return Ok(DownloadedSubtitle {
                name: file_name,
                format,
                content: content.to_vec(),
            });
        }

        // Otherwise extract from archive
        extract_archive(&content, &file_name, &request.language, &self.staging_root).await
    }
}
