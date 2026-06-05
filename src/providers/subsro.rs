/// SubsRo provider — Romanian subtitles
/// Site: https://subs.ro/subtitrari/imdbid/{imdb_id}
/// HTML scraping, requires IMDB ID, no authentication required
use async_trait::async_trait;
use regex::Regex;
use scraper::{Html, Selector};

use super::SubtitleProvider;
use crate::archive::extract_archive;
use crate::models::{
    normalize_format, DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest,
    SubtitleSearchResult,
};

const BASE_URL: &str = "https://subs.ro";
const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) \
                  Chrome/122.0.0.0 Safari/537.36";

fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(UA)
        .build()
        .map_err(|e| format!("SubsRo: failed to build HTTP client: {e}"))
}

pub struct SubsRoProvider {
    staging_root: std::path::PathBuf,
}

impl SubsRoProvider {
    pub fn new(staging_root: impl Into<std::path::PathBuf>) -> Self {
        Self {
            staging_root: staging_root.into(),
        }
    }
}

#[async_trait]
impl SubtitleProvider for SubsRoProvider {
    fn name(&self) -> &str {
        "subsro"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let imdb_id = request
            .imdb_id
            .as_deref()
            .ok_or("SubsRo: search requires an IMDB ID")?;

        // Strip leading "tt" prefix to get numeric ID
        let raw_id = imdb_id.trim_start_matches("tt");
        if raw_id.is_empty() {
            return Err("SubsRo: invalid IMDB ID".into());
        }

        let client = build_client()?;
        let url = format!("{BASE_URL}/subtitrari/imdbid/{raw_id}");

        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("SubsRo: search request failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!(
                "SubsRo: search failed with HTTP {}",
                resp.status().as_u16()
            ));
        }

        let html = resp
            .text()
            .await
            .map_err(|e| format!("SubsRo: failed to read search response: {e}"))?;

        // Determine desired language filter
        let want_ro = match &request.languages {
            None => true,
            Some(l) if l.is_empty() => true,
            Some(l) => l.iter().any(|x| x == "ro"),
        };
        let want_en = match &request.languages {
            Some(l) => l.iter().any(|x| x == "en"),
            None => false,
        };

        parse_subsro_results(&html, &format!("tt{raw_id}"), want_ro, want_en)
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        let url = request
            .download_path
            .as_deref()
            .ok_or("SubsRo: missing download_path")?;

        let client = build_client()?;

        let resp = client
            .get(url)
            .header("Referer", BASE_URL)
            .send()
            .await
            .map_err(|e| format!("SubsRo: download request failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!(
                "SubsRo: download failed with HTTP {}",
                resp.status().as_u16()
            ));
        }

        let file_name = resp
            .headers()
            .get("content-disposition")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| {
                let re =
                    regex::Regex::new(r#"filename[^;=\n]*=(?:(['"])(.+?)\1|([^;\n]*))"#).ok()?;
                re.captures(v)
                    .and_then(|c| c.get(2).or_else(|| c.get(3)))
                    .map(|m| m.as_str().trim().to_string())
            })
            .unwrap_or_else(|| format!("subsro_{}.zip", request.subtitle_id));

        let content = resp
            .bytes()
            .await
            .map_err(|e| format!("SubsRo: failed to read download content: {e}"))?;

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

fn parse_subsro_results(
    html: &str,
    imdb_id: &str,
    want_ro: bool,
    want_en: bool,
) -> Result<Vec<SubtitleSearchResult>, String> {
    let document = Html::parse_document(html);
    let mut results = Vec::new();

    // Each subtitle result is in div.md:col-span-6
    let item_sel = Selector::parse("div.md\\:col-span-6")
        .map_err(|e| format!("SubsRo: selector error: {e}"))?;
    let img_sel = Selector::parse("img").map_err(|e| format!("SubsRo: selector error: {e}"))?;
    let h1_sel = Selector::parse("h1 a").map_err(|e| format!("SubsRo: selector error: {e}"))?;
    let dl_link_sel =
        Selector::parse("div a").map_err(|e| format!("SubsRo: selector error: {e}"))?;

    let year_re = Regex::new(r"\((\d{4})\)").map_err(|e| format!("SubsRo: regex error: {e}"))?;
    let season_re =
        Regex::new(r"Sezonul\s*(\d+)").map_err(|e| format!("SubsRo: regex error: {e}"))?;

    for (index, item) in document.select(&item_sel).enumerate() {
        // Determine language from flag image
        let img_src = item
            .select(&img_sel)
            .next()
            .and_then(|el| el.value().attr("src"))
            .unwrap_or("");

        let language = if img_src.contains("flag-rom") {
            if !want_ro {
                continue;
            }
            "ro"
        } else if img_src.contains("flag-eng") {
            if !want_en {
                continue;
            }
            "en"
        } else {
            // Unknown flag, skip
            continue;
        };

        let language_name = if language == "ro" {
            "Romanian"
        } else {
            "English"
        };

        // Download link
        let download_href = match item.select(&dl_link_sel).next() {
            Some(a) => match a.value().attr("href") {
                Some(h) if !h.is_empty() => {
                    if h.starts_with("http") {
                        h.to_string()
                    } else {
                        format!("{BASE_URL}{h}")
                    }
                }
                _ => continue,
            },
            None => continue,
        };

        // Title and year from h1 a
        let title_raw = item
            .select(&h1_sel)
            .next()
            .map(|el| el.text().collect::<String>())
            .unwrap_or_default();

        if title_raw.is_empty() {
            continue;
        }

        // Strip season suffix and year: "Title - Sezonul 2 (2021)" → "Title"
        let title = Regex::new(r"\s*(-\s*Sezonul\s*\d+)?\s*\(\d{4}\).*$")
            .ok()
            .and_then(|re| Some(re.replace(&title_raw, "").trim().to_string()))
            .unwrap_or_else(|| title_raw.clone());

        let year: Option<u64> = year_re
            .captures(&title_raw)
            .and_then(|c| c.get(1))
            .and_then(|m| m.as_str().parse().ok());

        // Release info from p span with blue color
        let release_info = item
            .select(&Selector::parse("p span").unwrap_or_else(|_| Selector::parse("p").unwrap()))
            .find(|el| {
                el.value()
                    .attr("style")
                    .map(|s| s.contains("color: blue") || s.contains("color:blue"))
                    .unwrap_or(false)
            })
            .map(|el| el.text().collect::<String>())
            .or_else(|| {
                item.select(&Selector::parse("p").unwrap())
                    .next()
                    .map(|el| el.text().collect::<String>())
            });

        let name = match year {
            Some(y) => format!("{title} ({y})"),
            None => title.clone(),
        };

        results.push(SubtitleSearchResult {
            id: format!("subsro_{index}"),
            name,
            language: language.into(),
            language_name: language_name.into(),
            format: "srt".into(),
            provider: "subsro".into(),
            detail_path: None,
            download_path: Some(download_href),
            download_count: None,
            rating: None,
            movie_name: Some(title),
            release_group: release_info
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        });
        let _ = imdb_id;
        let _ = season_re;
    }

    tracing::info!("SubsRo: found {} results", results.len());
    Ok(results)
}
