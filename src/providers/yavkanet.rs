use async_trait::async_trait;
use scraper::{Html, Selector};
use std::path::PathBuf;

use super::SubtitleProvider;
use crate::archive::extract_archive;
use crate::models::{
    DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult,
};

const YAVKANET_BASE_URL: &str = "https://yavka.net";
const YAVKANET_USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

pub struct YavkanetProvider {
    staging_root: PathBuf,
}

impl YavkanetProvider {
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
            .user_agent(YAVKANET_USER_AGENT)
            .build()
            .map_err(|e| format!("Failed to build YavkaNet HTTP client: {e}"))
    }
}

#[async_trait]
impl SubtitleProvider for YavkanetProvider {
    fn name(&self) -> &str {
        "yavkanet"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let imdb_id = request
            .imdb_id
            .as_deref()
            .ok_or("YavkaNet search requires imdb_id")?;

        let client = Self::build_client()?;

        let search_url = format!("{YAVKANET_BASE_URL}/imdb/{imdb_id}");
        let response = client
            .get(&search_url)
            .header("Referer", YAVKANET_BASE_URL)
            .send()
            .await
            .map_err(|e| format!("YavkaNet search failed: {e}"))?;

        if !response.status().is_success() {
            return Ok(Vec::new());
        }

        let html = response
            .text()
            .await
            .map_err(|e| format!("Failed to read YavkaNet response: {e}"))?;

        let document = Html::parse_document(&html);
        let row_sel = Selector::parse("tr")
            .map_err(|e| format!("YavkaNet selector error: {e}"))?;
        let a_balon_sel = Selector::parse("a.balon, a.selector")
            .map_err(|e| format!("YavkaNet selector error: {e}"))?;
        let td_sel =
            Selector::parse("td").map_err(|e| format!("YavkaNet selector error: {e}"))?;
        let _span_sel = Selector::parse("span.smGray, span")
            .map_err(|e| format!("YavkaNet selector error: {e}"))?;

        let mut results = Vec::new();

        let rows: Vec<_> = document.select(&row_sel).collect();
        // The Python provider searches last 50 rows
        let start = rows.len().saturating_sub(50);

        for row in rows[start..].iter() {
            let Some(a) = row.select(&a_balon_sel).next() else {
                continue;
            };

            let link = a.value().attr("href").unwrap_or_default().to_string();
            if link.is_empty() {
                continue;
            }

            let title = a.text().collect::<String>().trim().to_string();
            let tds: Vec<_> = row.select(&td_sel).collect();

            let fps = tds
                .get(4)
                .and_then(|td| td.text().collect::<String>().trim().parse::<f64>().ok());

            let uploader = tds
                .get(5)
                .map(|td| td.text().collect::<String>().trim().to_string())
                .filter(|s| !s.is_empty());

            let download_url = if link.starts_with("http") {
                link.clone()
            } else {
                format!(
                    "{YAVKANET_BASE_URL}{}{}",
                    if link.starts_with('/') { "" } else { "/" },
                    link
                )
            };

            let release_group = fps
                .map(|f| format!("{f:.3} fps"))
                .or_else(|| uploader.clone());

            results.push(SubtitleSearchResult {
                id: download_url.clone(),
                name: title.clone(),
                language: "bg".into(),
                language_name: "Bulgarian".into(),
                format: "srt".into(),
                provider: "yavkanet".into(),
                detail_path: None,
                download_path: Some(download_url),
                download_count: None,
                rating: None,
                movie_name: Some(title),
                release_group,
            });
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
            .ok_or("YavkaNet download requires download_path")?;

        let client = Self::build_client()?;

        // YavkaNet download may require a POST with form data (subs_form_data)
        // Try GET first; the form data approach is for when the response is a page
        let response = client
            .get(download_url)
            .header("Referer", YAVKANET_BASE_URL)
            .send()
            .await
            .map_err(|e| format!("YavkaNet download failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "YavkaNet download failed: HTTP {}",
                response.status().as_u16()
            ));
        }

        let content = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read YavkaNet download: {e}"))?;

        let archive_name = download_url
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("subtitle.zip");

        extract_archive(&content, archive_name, &request.language, &self.staging_root).await
    }
}
