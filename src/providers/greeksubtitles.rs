use std::path::PathBuf;

use async_trait::async_trait;
use scraper::{Html, Selector};

use super::SubtitleProvider;
use crate::archive::extract_archive;
use crate::models::{
    matches_preferred_language, DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest,
    SubtitleSearchResult,
};

const GREEKSUBTITLES_BASE: &str = "http://gr.greek-subtitles.com/";
const GREEKSUBTITLES_DL_BASE: &str = "http://www.greeksubtitles.info/getp.php?id=";
const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

pub struct GreekSubtitlesProvider {
    staging_root: PathBuf,
}

impl GreekSubtitlesProvider {
    pub fn new(staging_root: impl Into<PathBuf>) -> Self {
        Self {
            staging_root: staging_root.into(),
        }
    }
}

fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .cookie_store(true)
        .build()
        .map_err(|e| format!("Failed to build greeksubtitles client: {e}"))
}

async fn fetch_page(
    client: &reqwest::Client,
    url: &str,
    requested_languages: Option<&[String]>,
    results: &mut Vec<SubtitleSearchResult>,
) -> Result<Option<String>, String> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("greeksubtitles request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("greeksubtitles request failed: {}", resp.status()));
    }

    let html = resp
        .text()
        .await
        .map_err(|e| format!("greeksubtitles read error: {e}"))?;

    let document = Html::parse_document(&html);

    let cell_sel = Selector::parse("td.latest_name > a")
        .map_err(|e| format!("greeksubtitles selector error: {e}"))?;
    let img_sel =
        Selector::parse("img").map_err(|e| format!("greeksubtitles selector error: {e}"))?;

    for cell in document.select(&cell_sel) {
        let href = match cell.value().attr("href") {
            Some(h) => h,
            None => continue,
        };

        // Extract subtitle_id: second segment from right after splitting by '/'
        // e.g. /something/12345/title/ -> id = "12345"
        let parts: Vec<&str> = href.trim_end_matches('/').split('/').collect();
        let subtitle_id = if parts.len() >= 2 {
            parts[parts.len() - 2].to_string()
        } else {
            continue;
        };

        // Language from img src
        let lang_code = cell
            .select(&img_sel)
            .next()
            .and_then(|img| img.value().attr("src"))
            .and_then(|src| src.rsplit('/').next())
            .map(|fname| {
                fname
                    .trim_end_matches(".png")
                    .trim_end_matches(".gif")
                    .to_lowercase()
            })
            .unwrap_or_else(|| "el".to_string());

        if !matches_preferred_language(&lang_code, requested_languages) {
            continue;
        }

        let version = cell.text().collect::<String>().trim().to_string();

        let download_path = format!("{GREEKSUBTITLES_DL_BASE}{subtitle_id}");
        let id = format!("greeksubtitles-{subtitle_id}");

        results.push(SubtitleSearchResult {
            id,
            name: version,
            language: lang_code,
            language_name: "Greek".into(),
            format: "srt".into(),
            provider: "greeksubtitles".into(),
            detail_path: None,
            download_path: Some(download_path),
            download_count: None,
            rating: None,
            movie_name: None,
            release_group: None,
        });
    }

    // Check for next page
    let a_sel = Selector::parse("a").map_err(|e| format!("greeksubtitles selector error: {e}"))?;
    let next_url = document.select(&a_sel).find_map(|a| {
        let text = a.text().collect::<String>();
        let href = a.value().attr("href").unwrap_or("");
        if text.contains("Next") && href.contains("search.php") {
            Some(format!(
                "{GREEKSUBTITLES_BASE}{}",
                href.trim_start_matches('/')
            ))
        } else {
            None
        }
    });

    Ok(next_url)
}

#[async_trait]
impl SubtitleProvider for GreekSubtitlesProvider {
    fn name(&self) -> &str {
        "greeksubtitles"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let query = request
            .query
            .as_deref()
            .ok_or("greeksubtitles requires query")?;

        let encoded = url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>();
        let start_url = format!("{GREEKSUBTITLES_BASE}search.php?name={encoded}");

        let client = build_client()?;
        let mut results = Vec::new();
        let mut current_url = Some(start_url);

        while let Some(url) = current_url {
            current_url =
                fetch_page(&client, &url, request.languages.as_deref(), &mut results).await?;
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
            .ok_or("greeksubtitles: download_path required")?;

        let client = build_client()?;
        let resp = client
            .get(download_url)
            .send()
            .await
            .map_err(|e| format!("greeksubtitles download failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("greeksubtitles download failed: {}", resp.status()));
        }

        let content = resp
            .bytes()
            .await
            .map_err(|e| format!("greeksubtitles read content error: {e}"))?;

        extract_archive(
            &content,
            "subtitle.zip",
            &request.language,
            &self.staging_root,
        )
        .await
    }
}
