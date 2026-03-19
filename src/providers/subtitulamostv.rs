use async_trait::async_trait;
use scraper::{Html, Selector};

use super::SubtitleProvider;
use crate::models::{
    DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult,
};

const SUBTITULAMOS_BASE_URL: &str = "https://www.subtitulamos.tv";
const SUBTITULAMOS_USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

pub struct SubtitulamosTvProvider;

impl SubtitulamosTvProvider {
    pub fn new() -> Self {
        Self
    }

    fn build_client() -> Result<reqwest::Client, String> {
        reqwest::Client::builder()
            .user_agent(SUBTITULAMOS_USER_AGENT)
            .build()
            .map_err(|e| format!("Failed to build SubtitulamosTV HTTP client: {e}"))
    }

    fn absolute_url(path: &str) -> String {
        if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else {
            format!(
                "{SUBTITULAMOS_BASE_URL}{}{}",
                if path.starts_with('/') { "" } else { "/" },
                path
            )
        }
    }
}

#[async_trait]
impl SubtitleProvider for SubtitulamosTvProvider {
    fn name(&self) -> &str {
        "subtitulamostv"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let query = request
            .query
            .as_deref()
            .filter(|q| !q.trim().is_empty())
            .ok_or("SubtitulamosTV search requires a query (series name)")?;

        let client = Self::build_client()?;

        // Search for the series
        let search_url = format!("{SUBTITULAMOS_BASE_URL}/search/query");
        let response = client
            .get(&search_url)
            .query(&[("q", query.trim())])
            .send()
            .await
            .map_err(|e| format!("SubtitulamosTV search failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "SubtitulamosTV search failed: HTTP {}",
                response.status().as_u16()
            ));
        }

        let shows: Vec<serde_json::Value> = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse SubtitulamosTV search response: {e}"))?;

        let mut results = Vec::new();

        for show in &shows {
            let show_name = show
                .get("show_name")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let show_id = show.get("show_id").and_then(|v| v.as_u64()).unwrap_or(0);

            if show_id == 0 {
                continue;
            }

            // Fetch show page to find seasons/episodes
            let show_url = format!("{SUBTITULAMOS_BASE_URL}/shows/{show_id}");
            let show_resp = client
                .get(&show_url)
                .send()
                .await
                .map_err(|e| format!("SubtitulamosTV show fetch failed: {e}"))?;

            if !show_resp.status().is_success() {
                continue;
            }

            let html = show_resp
                .text()
                .await
                .map_err(|e| format!("Failed to read SubtitulamosTV show page: {e}"))?;

            let document = Html::parse_document(&html);

            // Find all language containers
            let lang_sel = Selector::parse("div.language-container")
                .map_err(|e| format!("SubtitulamosTV selector error: {e}"))?;
            let lang_name_sel = Selector::parse("div.language-name")
                .map_err(|e| format!("SubtitulamosTV selector error: {e}"))?;
            let version_sel = Selector::parse("div.version-container")
                .map_err(|e| format!("SubtitulamosTV selector error: {e}"))?;
            let dl_sel = Selector::parse("a[href*='/download']")
                .map_err(|e| format!("SubtitulamosTV selector error: {e}"))?;
            let p_sel = Selector::parse("p")
                .map_err(|e| format!("SubtitulamosTV selector error: {e}"))?;

            for lang_container in document.select(&lang_sel) {
                let lang_name = lang_container
                    .select(&lang_name_sel)
                    .next()
                    .map(|e| e.text().collect::<String>())
                    .unwrap_or_default();

                let language = if lang_name.contains("English") {
                    "en"
                } else if lang_name.contains("Español") || lang_name.contains("Spanish") {
                    "es"
                } else {
                    continue;
                };

                let language_name = if language == "en" { "English" } else { "Español" };

                for version in lang_container.select(&version_sel) {
                    let dl_links: Vec<_> = version.select(&dl_sel).collect();
                    if dl_links.len() != 1 {
                        continue; // incomplete translation
                    }

                    let release_url = dl_links[0]
                        .value()
                        .attr("href")
                        .map(|h| Self::absolute_url(h))
                        .unwrap_or_default();

                    let release_name = version
                        .select(&p_sel)
                        .nth(1)
                        .map(|e| e.text().collect::<String>().trim().to_string())
                        .unwrap_or_else(|| show_name.to_string());

                    let id = release_url.clone();

                    results.push(SubtitleSearchResult {
                        id,
                        name: release_name.clone(),
                        language: language.into(),
                        language_name: language_name.into(),
                        format: "srt".into(),
                        provider: "subtitulamostv".into(),
                        detail_path: Some(show_url.clone()),
                        download_path: Some(release_url),
                        download_count: None,
                        rating: None,
                        movie_name: Some(show_name.to_string()),
                        release_group: Some(release_name),
                    });
                }
            }
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
            .ok_or("SubtitulamosTV download requires download_path")?;

        let client = Self::build_client()?;
        let response = client
            .get(download_url)
            .send()
            .await
            .map_err(|e| format!("SubtitulamosTV download failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "SubtitulamosTV download failed: HTTP {}",
                response.status().as_u16()
            ));
        }

        let content = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read SubtitulamosTV download: {e}"))?;

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
