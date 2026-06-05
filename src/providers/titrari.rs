/// Titrari provider — Romanian subtitles
/// Site: https://www.titrari.ro
/// HTML scraping, no authentication required
use async_trait::async_trait;
use regex::Regex;
use scraper::{Html, Selector};

use super::SubtitleProvider;
use crate::archive::extract_archive;
use crate::models::{
    normalize_format, DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest,
    SubtitleSearchResult,
};

const BASE_URL: &str = "https://www.titrari.ro/";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like \
                  Gecko) Chrome/122.0.0.0 Safari/537.36";
/// Hardcoded advanced search page param (fetched dynamically in the Python version)
const ADV_SEARCH_PAGE: &str = "numaicautamcaneiesepenas";

fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(UA)
        .build()
        .map_err(|e| format!("Titrari: failed to build HTTP client: {e}"))
}

pub struct TitrariProvider {
    staging_root: std::path::PathBuf,
}

impl TitrariProvider {
    pub fn new(staging_root: impl Into<std::path::PathBuf>) -> Self {
        Self {
            staging_root: staging_root.into(),
        }
    }
}

#[async_trait]
impl SubtitleProvider for TitrariProvider {
    fn name(&self) -> &str {
        "titrari"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let client = build_client()?;

        let query = request.query.clone().unwrap_or_default();
        let lang_code = match &request.languages {
            Some(l) if l.iter().any(|x| x == "ro") => "1",
            Some(l) if l.iter().any(|x| x == "en") => "2",
            _ => "-1",
        };

        let mut params = vec![
            ("page", ADV_SEARCH_PAGE.to_string()),
            ("z2", String::new()),
            ("z3", "-1".to_string()),
            ("z4", "-1".to_string()),
            ("z5", String::new()),
            ("z6", "0".to_string()),
            ("z7", String::new()),
            ("z8", lang_code.to_string()),
            ("z9", "All".to_string()),
            ("z11", "0".to_string()),
        ];

        if let Some(imdb_id) = &request.imdb_id {
            // Strip leading "tt" if present
            let raw_id = imdb_id.trim_start_matches("tt");
            params.iter_mut().for_each(|(k, v)| {
                if *k == "z5" {
                    *v = raw_id.to_string();
                }
            });
        } else if !query.is_empty() {
            params.iter_mut().for_each(|(k, v)| {
                if *k == "z7" {
                    *v = query.clone();
                }
            });
        }

        let resp = client
            .get(BASE_URL)
            .query(&params)
            .send()
            .await
            .map_err(|e| format!("Titrari: search request failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!(
                "Titrari: search failed with HTTP {}",
                resp.status().as_u16()
            ));
        }

        let html = resp
            .text()
            .await
            .map_err(|e| format!("Titrari: failed to read search response: {e}"))?;

        parse_titrari_results(&html, &query)
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        let url = request
            .download_path
            .as_deref()
            .ok_or("Titrari: missing download_path")?;

        let client = build_client()?;

        let resp = client
            .get(url)
            .header("Referer", BASE_URL)
            .send()
            .await
            .map_err(|e| format!("Titrari: download request failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!(
                "Titrari: download failed with HTTP {}",
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
            .unwrap_or_else(|| format!("titrari_{}.zip", request.subtitle_id));

        let content = resp
            .bytes()
            .await
            .map_err(|e| format!("Titrari: failed to read download content: {e}"))?;

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

fn parse_titrari_results(html: &str, query: &str) -> Result<Vec<SubtitleSearchResult>, String> {
    let document = Html::parse_document(html);
    let mut results = Vec::new();

    // Each subtitle is in a td[rowspan="5"] cell
    let row_sel = Selector::parse(r#"td[rowspan="5"]"#)
        .map_err(|e| format!("Titrari: selector parse error: {e}"))?;
    let _title_sel =
        Selector::parse("h1 a").map_err(|e| format!("Titrari: selector parse error: {e}"))?;
    let comment_sel =
        Selector::parse(".comment").map_err(|e| format!("Titrari: selector parse error: {e}"))?;

    let year_re = Regex::new(r"\((\d{4})\)").map_err(|e| format!("Titrari: regex error: {e}"))?;
    let imdb_re = Regex::new(r"tt(\d+)").map_err(|e| format!("Titrari: regex error: {e}"))?;

    for (index, row) in document.select(&row_sel).enumerate() {
        // The first <a> in the row is the download link
        let anchor = match row.select(&Selector::parse("a").unwrap()).next() {
            Some(a) => a,
            None => continue,
        };
        let href = anchor.value().attr("href").unwrap_or("");
        if href.is_empty() {
            continue;
        }
        let download_url = if href.starts_with("http") {
            href.to_string()
        } else {
            format!("{BASE_URL}{}", href.trim_start_matches('/'))
        };

        // Title and year from parent table's h1 a
        let parent_html = row.parent().and_then(|p| {
            // Walk up to find the table row containing h1 a
            // In practice, the h1 a is a sibling td's child
            Some(p)
        });

        // Try to get title from the document by looking at h1 a near this row
        // The Python code does: row.parent.select('h1 a')[0].text
        // We'll use the enclosing table structure
        let full_title = {
            // The Python code does: row.parent.select('h1 a')[0].text
            let h1_sel = Selector::parse("h1 a").unwrap();
            document
                .select(&h1_sel)
                .nth(index)
                .map(|el| el.text().collect::<String>())
                .unwrap_or_default()
        };

        let title = full_title
            .split('(')
            .next()
            .unwrap_or(&full_title)
            .trim()
            .to_string();

        if title.is_empty() {
            continue;
        }

        let year: Option<u64> = year_re
            .captures(&full_title)
            .and_then(|c| c.get(1))
            .and_then(|m| m.as_str().parse().ok());

        // Comments / release info
        let comments = document
            .select(&comment_sel)
            .nth(1 + index * 2) // skip first comment on page, alternate structure
            .map(|el| el.text().collect::<String>())
            .unwrap_or_default();

        // IMDB ID from page (try to find in the surrounding HTML)
        let imdb_id: Option<String> = {
            let all_text = row.html();
            imdb_re.captures(&all_text).map(|c| format!("tt{}", &c[1]))
        };

        let name = if year.is_some() {
            format!("{} ({})", title, year.unwrap())
        } else {
            title.clone()
        };

        results.push(SubtitleSearchResult {
            id: format!("titrari_{index}"),
            name,
            language: "ro".into(),
            language_name: "Romanian".into(),
            format: "srt".into(),
            provider: "titrari".into(),
            detail_path: None,
            download_path: Some(download_url),
            download_count: None,
            rating: None,
            movie_name: Some(if query.is_empty() {
                title
            } else {
                query.to_string()
            }),
            release_group: if comments.is_empty() {
                None
            } else {
                Some(comments.trim().to_string())
            },
        });
        let _ = parent_html;
        let _ = imdb_id;
    }

    tracing::info!("Titrari: found {} results", results.len());
    Ok(results)
}
