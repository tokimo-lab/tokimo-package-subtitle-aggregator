use async_trait::async_trait;
use scraper::{Html, Selector};
use std::path::PathBuf;

use super::SubtitleProvider;
use crate::archive::extract_archive;
use crate::models::{
    DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult,
};

const SUBSSABBZ_BASE_URL: &str = "http://subs.sab.bz";
const SUBSSABBZ_USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

pub struct SubssabbzProvider {
    staging_root: PathBuf,
}

impl SubssabbzProvider {
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
            .user_agent(SUBSSABBZ_USER_AGENT)
            .build()
            .map_err(|e| format!("Failed to build SubsSabBz HTTP client: {e}"))
    }
}

#[async_trait]
impl SubtitleProvider for SubssabbzProvider {
    fn name(&self) -> &str {
        "subssabbz"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let query = request
            .query
            .as_deref()
            .filter(|q| !q.trim().is_empty())
            .ok_or("SubsSabBz search requires a query")?;

        let language = request
            .languages
            .as_deref()
            .and_then(|l| l.first())
            .map(|l| l.as_str())
            .unwrap_or("bg");

        let lang_param = if language == "en" || language == "eng" {
            "1"
        } else {
            "2"
        };

        let client = Self::build_client()?;

        let params = [
            ("act", "search"),
            ("movie", query.trim()),
            ("select-language", lang_param),
            ("upldr", ""),
            ("yr", ""),
            ("release", ""),
        ];

        let response = client
            .post(&format!("{SUBSSABBZ_BASE_URL}/index.php?"))
            .header("Referer", &format!("{SUBSSABBZ_BASE_URL}/"))
            .form(&params)
            .send()
            .await
            .map_err(|e| format!("SubsSabBz search failed: {e}"))?;

        if !response.status().is_success() {
            return Ok(Vec::new());
        }

        let html = response
            .text()
            .await
            .map_err(|e| format!("Failed to read SubsSabBz response: {e}"))?;

        let document = Html::parse_document(&html);
        let row_sel =
            Selector::parse("tr.subs-row").map_err(|e| format!("SubsSabBz selector error: {e}"))?;
        let td_c2_sel =
            Selector::parse("td.c2field").map_err(|e| format!("SubsSabBz selector error: {e}"))?;
        let a_sel = Selector::parse("a").map_err(|e| format!("SubsSabBz selector error: {e}"))?;
        let td_sel = Selector::parse("td").map_err(|e| format!("SubsSabBz selector error: {e}"))?;

        let mut results = Vec::new();

        for row in document.select(&row_sel).take(25) {
            let Some(td_c2) = row.select(&td_c2_sel).next() else {
                continue;
            };
            let Some(a) = td_c2.select(&a_sel).next() else {
                continue;
            };

            let link = a.value().attr("href").unwrap_or_default().to_string();
            if link.is_empty() {
                continue;
            }

            let title = a.text().collect::<String>().trim().to_string();
            let tds: Vec<_> = row.select(&td_sel).collect();

            let fps = tds
                .get(7)
                .and_then(|td| td.text().collect::<String>().trim().parse::<f64>().ok());

            let uploader = tds
                .get(8)
                .map(|td| td.text().collect::<String>().trim().to_string())
                .filter(|s| !s.is_empty());

            let download_url = if link.starts_with("http") {
                link.clone()
            } else {
                format!(
                    "{SUBSSABBZ_BASE_URL}{}",
                    if link.starts_with('/') {
                        link.clone()
                    } else {
                        format!("/{link}")
                    }
                )
            };

            let release_group = fps.map(|f| format!("{f:.3} fps")).or(uploader);

            results.push(SubtitleSearchResult {
                id: download_url.clone(),
                name: title.clone(),
                language: "bg".into(),
                language_name: "Bulgarian".into(),
                format: "srt".into(),
                provider: "subssabbz".into(),
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
            .ok_or("SubsSabBz download requires download_path")?;

        let client = Self::build_client()?;
        let response = client
            .get(download_url)
            .header("Referer", &format!("{SUBSSABBZ_BASE_URL}/index.php?"))
            .send()
            .await
            .map_err(|e| format!("SubsSabBz download failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "SubsSabBz download failed: HTTP {}",
                response.status().as_u16()
            ));
        }

        let content = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read SubsSabBz download: {e}"))?;

        let archive_name = download_url
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("subtitle.zip");

        extract_archive(
            &content,
            archive_name,
            &request.language,
            &self.staging_root,
        )
        .await
    }
}
