use async_trait::async_trait;
use scraper::{Html, Selector};

use super::SubtitleProvider;
use crate::models::{
    DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult,
};

const NEKUR_BASE: &str = "http://subtitri.nekur.net";
const NEKUR_API: &str = "http://subtitri.nekur.net/modules/Subtitles.php";
const STAGING_ROOT: &str = "/tmp/subtitle-aggregator";

pub struct NekurProvider;

impl NekurProvider {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SubtitleProvider for NekurProvider {
    fn name(&self) -> &str {
        "nekur"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let title = request.query.as_deref().ok_or("nekur: query is required")?;

        let client = reqwest::Client::builder()
            .user_agent("subtitle-aggregator/0.1")
            .build()
            .map_err(|e| format!("nekur: failed to build client: {e}"))?;

        let params = [("ajax", "1"), ("sSearch", title)];

        let response = client
            .post(NEKUR_API)
            .form(&params)
            .send()
            .await
            .map_err(|e| format!("nekur: request failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!("nekur: HTTP {}", response.status().as_u16()));
        }

        let html = response
            .text()
            .await
            .map_err(|e| format!("nekur: failed to read HTML: {e}"))?;

        let document = Html::parse_document(&html);

        let row_sel =
            Selector::parse("tbody > tr").map_err(|e| format!("nekur: selector error: {e}"))?;
        let title_sel =
            Selector::parse(".title > a").map_err(|e| format!("nekur: selector error: {e}"))?;
        let year_sel =
            Selector::parse(".year").map_err(|e| format!("nekur: selector error: {e}"))?;
        let notes_sel =
            Selector::parse(".notes").map_err(|e| format!("nekur: selector error: {e}"))?;

        let mut results = Vec::new();

        for row in document.select(&row_sel) {
            let title_anchor = row.select(&title_sel).next();
            let Some(anchor) = title_anchor else {
                continue;
            };

            let sub_title = anchor.text().collect::<String>().trim().to_string();
            let href = anchor.value().attr("href").unwrap_or("");

            let download_url = if href.starts_with("http") {
                href.to_string()
            } else {
                format!("{}/{}", NEKUR_BASE, href.trim_start_matches('/'))
            };

            // Extract ID from last path segment
            let sub_id = download_url
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("unknown")
                .to_string();

            let year = row
                .select(&year_sel)
                .next()
                .map(|el| {
                    el.text()
                        .collect::<String>()
                        .trim()
                        .trim_matches('(')
                        .trim_matches(')')
                        .to_string()
                })
                .filter(|s| !s.is_empty());

            let notes = row
                .select(&notes_sel)
                .next()
                .map(|el| el.text().collect::<String>().trim().to_string())
                .filter(|s| !s.is_empty());

            let movie_name = if let Some(y) = &year {
                Some(format!("{sub_title} ({y})"))
            } else {
                Some(sub_title.clone())
            };

            results.push(SubtitleSearchResult {
                id: sub_id,
                name: sub_title.clone(),
                language: "lv".into(),
                language_name: "Latvian".into(),
                format: "srt".into(),
                provider: "nekur".into(),
                detail_path: None,
                download_path: Some(download_url),
                download_count: None,
                rating: None,
                movie_name,
                release_group: notes,
            });
        }

        Ok(results)
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        let url = request
            .download_path
            .as_deref()
            .ok_or("nekur: download_path is required")?;

        let client = reqwest::Client::builder()
            .user_agent("subtitle-aggregator/0.1")
            .build()
            .map_err(|e| format!("nekur: failed to build client: {e}"))?;

        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("nekur: download failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!("nekur: HTTP {}", response.status().as_u16()));
        }

        let content = response
            .bytes()
            .await
            .map_err(|e| format!("nekur: failed to read content: {e}"))?;

        let name = request
            .name
            .clone()
            .unwrap_or_else(|| format!("nekur_{}.zip", request.subtitle_id));

        // Try archive extraction; fall back to raw content if not an archive
        if content.starts_with(b"PK") || content.starts_with(b"Rar!") {
            let staging = std::path::Path::new(STAGING_ROOT);
            return crate::archive::extract_archive(&content, &name, &request.language, staging)
                .await;
        }

        let format = request.format.clone();
        Ok(DownloadedSubtitle {
            name,
            format,
            content: content.to_vec(),
        })
    }
}
