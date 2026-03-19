/// SubtitrariNoi provider — Romanian subtitles
/// Site: https://www.subtitrari-noi.ro/
/// HTML scraping via POST, no authentication required
use async_trait::async_trait;
use scraper::{Html, Selector};

use super::SubtitleProvider;
use crate::archive::extract_archive;
use crate::models::{
    normalize_format, DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest,
    SubtitleSearchResult,
};

const SERVER_URL: &str = "https://www.subtitrari-noi.ro/";
const API_URL: &str = "https://www.subtitrari-noi.ro/paginare_filme.php";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like \
                  Gecko) Chrome/93.0.4535.2 Safari/537.36";

fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(UA)
        .build()
        .map_err(|e| format!("SubtitrariNoi: failed to build HTTP client: {e}"))
}

pub struct SubtitrariNoiProvider {
    staging_root: std::path::PathBuf,
}

impl SubtitrariNoiProvider {
    pub fn new(staging_root: impl Into<std::path::PathBuf>) -> Self {
        Self {
            staging_root: staging_root.into(),
        }
    }
}

#[async_trait]
impl SubtitleProvider for SubtitrariNoiProvider {
    fn name(&self) -> &str {
        "subtitrarinoi"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let client = build_client()?;

        let query = request.query.clone().unwrap_or_default();
        let search_term = if let Some(imdb_id) = &request.imdb_id {
            // Strip leading "tt" if present for this provider
            imdb_id.trim_start_matches("tt").to_string()
        } else if !query.is_empty() {
            query.clone()
        } else {
            return Err("SubtitrariNoi: search requires a query or IMDB ID".into());
        };

        let params = [
            ("search_q", "1"),
            ("tip", "2"),
            ("an", "Toti anii"),
            ("gen", "Toate"),
            ("cautare", &search_term),
            ("query_q", &search_term),
        ];

        let resp = client
            .post(API_URL)
            .header("X-Requested-With", "XMLHttpRequest")
            .header("Referer", SERVER_URL)
            .form(&params)
            .send()
            .await
            .map_err(|e| format!("SubtitrariNoi: search request failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!(
                "SubtitrariNoi: search failed with HTTP {}",
                resp.status().as_u16()
            ));
        }

        let html = resp
            .text()
            .await
            .map_err(|e| format!("SubtitrariNoi: failed to read search response: {e}"))?;

        parse_subtitrarinoi_results(&html, &query)
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        let url = request
            .download_path
            .as_deref()
            .ok_or("SubtitrariNoi: missing download_path")?;

        let client = build_client()?;

        let resp = client
            .get(url)
            .header("Referer", API_URL)
            .send()
            .await
            .map_err(|e| format!("SubtitrariNoi: download request failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!(
                "SubtitrariNoi: download failed with HTTP {}",
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
            .unwrap_or_else(|| format!("subtitrarinoi_{}.zip", request.subtitle_id));

        let content = resp
            .bytes()
            .await
            .map_err(|e| format!("SubtitrariNoi: failed to read download content: {e}"))?;

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

fn parse_subtitrarinoi_results(
    html: &str,
    query: &str,
) -> Result<Vec<SubtitleSearchResult>, String> {
    let document = Html::parse_document(html);
    let mut results = Vec::new();

    let round_sel = Selector::parse(r#"div[id="round"]"#)
        .map_err(|e| format!("SubtitrariNoi: selector error: {e}"))?;
    let title_sel = Selector::parse(r#"#content-main a"#)
        .map_err(|e| format!("SubtitrariNoi: selector error: {e}"))?;
    let download_sel = Selector::parse(".buton a")
        .map_err(|e| format!("SubtitrariNoi: selector error: {e}"))?;
    let dl_count_sel = Selector::parse(r#"#content-right p"#)
        .map_err(|e| format!("SubtitrariNoi: selector error: {e}"))?;

    let year_re =
        regex::Regex::new(r"\((\d{4})\)").map_err(|e| format!("SubtitrariNoi: regex error: {e}"))?;

    let rows: Vec<_> = document.select(&round_sel).collect();

    if rows.is_empty() {
        tracing::debug!("SubtitrariNoi: no results found");
        return Ok(Vec::new());
    }

    for (index, row) in rows.iter().enumerate() {
        // Download link
        let download_href = match row.select(&download_sel).next() {
            Some(a) => match a.value().attr("href") {
                Some(h) => {
                    if h.starts_with("http") {
                        h.to_string()
                    } else {
                        format!("{SERVER_URL}{}", h.trim_start_matches('/'))
                    }
                }
                None => continue,
            },
            None => continue,
        };

        // Title and year
        let full_title = row
            .select(&title_sel)
            .next()
            .map(|el| el.text().collect::<String>())
            .unwrap_or_default();

        if full_title.is_empty() {
            continue;
        }

        let title = full_title
            .split('(')
            .next()
            .unwrap_or(&full_title)
            .trim()
            .to_string();

        let year: Option<u64> = year_re
            .captures(&full_title)
            .and_then(|c| c.get(1))
            .and_then(|m| m.as_str().parse().ok());

        // Download count
        let download_count: Option<u64> = row
            .select(&dl_count_sel)
            .next()
            .and_then(|el| {
                let text = el.text().collect::<String>();
                // Text format: "Descarcari: 123"
                text.split(':')
                    .nth(1)
                    .and_then(|n| n.trim().parse().ok())
            });

        let name = match year {
            Some(y) => format!("{title} ({y})"),
            None => title.clone(),
        };

        results.push(SubtitleSearchResult {
            id: format!("subtitrarinoi_{index}"),
            name,
            language: "ro".into(),
            language_name: "Romanian".into(),
            format: "srt".into(),
            provider: "subtitrarinoi".into(),
            detail_path: None,
            download_path: Some(download_href),
            download_count,
            rating: None,
            movie_name: Some(if query.is_empty() {
                title
            } else {
                query.to_string()
            }),
            release_group: None,
        });
    }

    tracing::info!("SubtitrariNoi: found {} results", results.len());
    Ok(results)
}
