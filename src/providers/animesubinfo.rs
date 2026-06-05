use async_trait::async_trait;
use scraper::{Html, Selector};
use std::path::PathBuf;

use super::SubtitleProvider;
use crate::archive::extract_archive;
use crate::models::{
    DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult,
};

const ANIMESUBINFO_BASE_URL: &str = "http://animesub.info";
const ANIMESUBINFO_SEARCH_URL: &str = "http://animesub.info/szukaj.php";
const ANIMESUBINFO_USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

pub struct AnimesubinfoProvider {
    staging_root: PathBuf,
}

impl AnimesubinfoProvider {
    pub fn new() -> Self {
        Self {
            staging_root: std::env::temp_dir(),
        }
    }

    pub fn with_staging_root(mut self, staging_root: impl Into<PathBuf>) -> Self {
        self.staging_root = staging_root.into();
        self
    }

    fn build_client() -> Result<reqwest::Client, String> {
        reqwest::Client::builder()
            .user_agent(ANIMESUBINFO_USER_AGENT)
            .build()
            .map_err(|e| format!("Failed to build AnimeSubInfo HTTP client: {e}"))
    }
}

#[async_trait]
impl SubtitleProvider for AnimesubinfoProvider {
    fn name(&self) -> &str {
        "animesubinfo"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let query = request
            .query
            .as_deref()
            .filter(|q| !q.trim().is_empty())
            .ok_or("AnimeSubInfo search requires a query")?;

        let client = Self::build_client()?;

        let response = client
            .get(ANIMESUBINFO_SEARCH_URL)
            .query(&[
                ("szukane", query.trim()),
                ("pTitle", "org"),
                ("pSortuj", "pobrn"),
            ])
            .send()
            .await
            .map_err(|e| format!("AnimeSubInfo search failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "AnimeSubInfo search failed: HTTP {}",
                response.status().as_u16()
            ));
        }

        // Site uses ISO-8859-2 encoding
        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read AnimeSubInfo response: {e}"))?;
        let html = encoding_to_utf8(&bytes);

        let document = Html::parse_document(&html);

        // Each subtitle is in a table with class "Napisy" that has rows with class "KNap"
        let table_sel = Selector::parse("table.Napisy")
            .map_err(|e| format!("AnimeSubInfo selector error: {e}"))?;
        let row_sel =
            Selector::parse("tr.KNap").map_err(|e| format!("AnimeSubInfo selector error: {e}"))?;
        let td_sel =
            Selector::parse("td").map_err(|e| format!("AnimeSubInfo selector error: {e}"))?;
        let a_sel =
            Selector::parse("a").map_err(|e| format!("AnimeSubInfo selector error: {e}"))?;

        let mut results = Vec::new();
        let mut index = 0usize;

        for table in document.select(&table_sel) {
            let rows: Vec<_> = table.select(&row_sel).collect();
            if rows.len() < 2 {
                continue;
            }

            let row1_tds: Vec<_> = rows[0].select(&td_sel).collect();
            let row2_tds: Vec<_> = rows[1].select(&td_sel).collect();

            let title_org = row1_tds
                .first()
                .map(|td| td.text().collect::<String>().trim().to_string())
                .unwrap_or_default();

            let title_eng = row2_tds
                .first()
                .map(|td| td.text().collect::<String>().trim().to_string())
                .unwrap_or_default();

            let format_type = row1_tds
                .get(3)
                .map(|td| td.text().collect::<String>().trim().to_string())
                .unwrap_or_default();

            let format = if format_type.to_lowercase().contains("ass")
                || format_type.to_lowercase().contains("ssa")
            {
                "ass"
            } else {
                "srt"
            };

            // Download link from row 2 or 3 - look for link containing 'sciagnij'
            let download_hash = rows
                .iter()
                .flat_map(|r| r.select(&a_sel))
                .find(|a| {
                    a.value()
                        .attr("href")
                        .map(|h| h.contains("sciagnij") || h.contains("hash="))
                        .unwrap_or(false)
                })
                .and_then(|a| a.value().attr("href"))
                .map(|h| h.to_string());

            // Download count
            let download_count = row2_tds
                .get(3)
                .and_then(|td| td.text().collect::<String>().trim().parse::<u64>().ok());

            let name = if !title_eng.is_empty() {
                format!("{title_org} / {title_eng}")
            } else {
                title_org.clone()
            };

            let download_url = download_hash.as_deref().map(|h| {
                if h.starts_with("http") {
                    h.to_string()
                } else {
                    format!("{ANIMESUBINFO_BASE_URL}/{}", h.trim_start_matches('/'))
                }
            });

            let id = download_url
                .clone()
                .unwrap_or_else(|| format!("animesubinfo-{index}"));

            results.push(SubtitleSearchResult {
                id,
                name,
                language: "tr".into(),
                language_name: "Turkish".into(),
                format: format.into(),
                provider: "animesubinfo".into(),
                detail_path: None,
                download_path: download_url,
                download_count,
                rating: None,
                movie_name: Some(title_org),
                release_group: None,
            });

            index += 1;
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
            .ok_or("AnimeSubInfo download requires download_path")?;

        let client = Self::build_client()?;
        let response = client
            .get(download_url)
            .header("Referer", ANIMESUBINFO_BASE_URL)
            .send()
            .await
            .map_err(|e| format!("AnimeSubInfo download failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "AnimeSubInfo download failed: HTTP {}",
                response.status().as_u16()
            ));
        }

        let content = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read AnimeSubInfo download: {e}"))?;

        let archive_name = download_url
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("subtitle.zip");

        // Try archive extraction; if it fails, return as raw content
        match extract_archive(
            &content,
            archive_name,
            &request.language,
            &self.staging_root,
        )
        .await
        {
            Ok(sub) => Ok(sub),
            Err(_) => {
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
    }
}

/// Decode bytes that may be ISO-8859-2 encoded into UTF-8.
fn encoding_to_utf8(bytes: &[u8]) -> String {
    // Try UTF-8 first; fall back to latin2 (ISO-8859-2) mapping
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    // Simple lossy conversion: map each byte as windows-1250 / ISO-8859-2
    bytes.iter().map(|&b| b as char).collect()
}
