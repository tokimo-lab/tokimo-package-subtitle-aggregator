use async_trait::async_trait;
use scraper::{Html, Selector};
use std::path::PathBuf;

use super::SubtitleProvider;
use crate::archive::extract_archive;
use crate::models::{
    DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult,
};

const SUBSUNACS_BASE_URL: &str = "https://subsunacs.net";
const SUBSUNACS_USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

pub struct SubsunacsProvider {
    staging_root: PathBuf,
}

impl SubsunacsProvider {
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
            .user_agent(SUBSUNACS_USER_AGENT)
            .build()
            .map_err(|e| format!("Failed to build SubsUnacs HTTP client: {e}"))
    }
}

#[async_trait]
impl SubtitleProvider for SubsunacsProvider {
    fn name(&self) -> &str {
        "subsunacs"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let query = request
            .query
            .as_deref()
            .filter(|q| !q.trim().is_empty())
            .ok_or("SubsUnacs search requires a query")?;

        let language = request
            .languages
            .as_deref()
            .and_then(|l| l.first())
            .map(|l| l.as_str())
            .unwrap_or("bg");

        let lang_param = if language == "en" || language == "eng" {
            "1"
        } else {
            "0"
        };

        let client = Self::build_client()?;

        // Use the Cyrillic search action string as in the Python provider
        let params = [
            ("m", query.trim()),
            ("l", lang_param),
            ("c", ""),
            ("y", ""),
            ("action", "   \u{422}\u{44A}\u{440}\u{441}\u{438}   "), // "Търси" in Bulgarian
            ("a", ""),
            ("d", ""),
            ("u", ""),
            ("g", ""),
            ("t", ""),
            ("imdbcheck", "1"),
        ];

        let response = client
            .post(&format!("{SUBSUNACS_BASE_URL}/search.php"))
            .header("Referer", &format!("{SUBSUNACS_BASE_URL}/index.php"))
            .form(&params)
            .send()
            .await
            .map_err(|e| format!("SubsUnacs search failed: {e}"))?;

        if !response.status().is_success() {
            return Ok(Vec::new());
        }

        let html = response
            .text()
            .await
            .map_err(|e| format!("Failed to read SubsUnacs response: {e}"))?;

        let document = Html::parse_document(&html);
        let row_sel = Selector::parse("tr[onmouseover]")
            .map_err(|e| format!("SubsUnacs selector error: {e}"))?;
        let td_movie_sel =
            Selector::parse("td.tdMovie").map_err(|e| format!("SubsUnacs selector error: {e}"))?;
        let a_tooltip_sel =
            Selector::parse("a.tooltip").map_err(|e| format!("SubsUnacs selector error: {e}"))?;
        let td_sel = Selector::parse("td").map_err(|e| format!("SubsUnacs selector error: {e}"))?;

        let mut results = Vec::new();

        for row in document.select(&row_sel).take(20) {
            let Some(td_movie) = row.select(&td_movie_sel).next() else {
                continue;
            };
            let Some(a) = td_movie.select(&a_tooltip_sel).next() else {
                continue;
            };

            let link = a.value().attr("href").unwrap_or_default().to_string();
            if link.is_empty() {
                continue;
            }

            let title = a.text().collect::<String>().trim().to_string();
            let tds: Vec<_> = row.select(&td_sel).collect();

            let fps = tds
                .get(2)
                .and_then(|td| td.text().collect::<String>().trim().parse::<f64>().ok());

            let rating = tds.get(3).and_then(|td| {
                // Rating is in an img's title attribute
                Selector::parse("img")
                    .ok()
                    .and_then(|s| td.select(&s).next())
                    .and_then(|img| img.value().attr("title"))
                    .and_then(|t| t.parse::<f64>().ok())
            });

            let uploader = tds
                .get(5)
                .map(|td| td.text().collect::<String>().trim().to_string())
                .filter(|s| !s.is_empty());

            let download_url = if link.starts_with("http") {
                link.clone()
            } else {
                format!(
                    "{SUBSUNACS_BASE_URL}{}",
                    if link.starts_with('/') {
                        link.clone()
                    } else {
                        format!("/{link}")
                    }
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
                provider: "subsunacs".into(),
                detail_path: None,
                download_path: Some(download_url),
                download_count: None,
                rating,
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
            .ok_or("SubsUnacs download requires download_path")?;

        let client = Self::build_client()?;
        let response = client
            .get(download_url)
            .header("Referer", &format!("{SUBSUNACS_BASE_URL}/search.php"))
            .send()
            .await
            .map_err(|e| format!("SubsUnacs download failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "SubsUnacs download failed: HTTP {}",
                response.status().as_u16()
            ));
        }

        let content = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read SubsUnacs download: {e}"))?;

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
