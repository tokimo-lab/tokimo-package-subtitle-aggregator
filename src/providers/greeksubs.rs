use std::path::PathBuf;

use async_trait::async_trait;
use regex::Regex;
use scraper::{Html, Selector};

use super::SubtitleProvider;
use crate::archive::extract_archive;
use crate::models::{
    matches_preferred_language, DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest,
    SubtitleSearchResult,
};

const GREEKSUBS_BASE: &str = "https://greeksubs.net/";
const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

pub struct GreekSubsProvider {
    staging_root: PathBuf,
}

impl GreekSubsProvider {
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
        .map_err(|e| format!("Failed to build greeksubs client: {e}"))
}

#[async_trait]
impl SubtitleProvider for GreekSubsProvider {
    fn name(&self) -> &str {
        "greeksubs"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let imdb_id = request
            .imdb_id
            .as_deref()
            .ok_or("greeksubs requires imdb_id")?;

        let url = format!("{GREEKSUBS_BASE}en/view/{imdb_id}");
        let client = build_client()?;
        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("greeksubs request failed: {e}"))?;

        if response.status().as_u16() == 404 {
            return Ok(Vec::new());
        }
        if !response.status().is_success() {
            return Err(format!("greeksubs request failed: {}", response.status()));
        }

        let html = response
            .text()
            .await
            .map_err(|e| format!("greeksubs read error: {e}"))?;

        let document = Html::parse_document(&html);

        // Extract secCode
        let sec_sel = Selector::parse("input#secCode")
            .map_err(|e| format!("greeksubs selector error: {e}"))?;
        let sec_code = document
            .select(&sec_sel)
            .next()
            .and_then(|el| el.value().attr("value"))
            .unwrap_or("")
            .to_string();

        // Parse subtitle rows
        let row_sel = Selector::parse("#elSub > tbody > tr")
            .map_err(|e| format!("greeksubs selector error: {e}"))?;
        let img_sel =
            Selector::parse("img").map_err(|e| format!("greeksubs selector error: {e}"))?;

        let re_dl = Regex::new(r"downloadMe\('([^']+)'\)").unwrap();

        let mut results = Vec::new();

        for row in document.select(&row_sel) {
            // Find onclick with downloadMe
            let onclick = row
                .value()
                .attr("onclick")
                .or_else(|| {
                    row.children()
                        .filter_map(scraper::ElementRef::wrap)
                        .find_map(|el| el.value().attr("onclick"))
                })
                .unwrap_or("");

            let subtitle_id = match re_dl.captures(onclick) {
                Some(cap) => cap[1].to_string(),
                None => continue,
            };

            // Language from img alt
            let lang_code = row
                .select(&img_sel)
                .next()
                .and_then(|img| img.value().attr("alt"))
                .map(|alt| alt.to_lowercase())
                .unwrap_or_else(|| "el".to_string());

            let lang_code = if lang_code == "greek" || lang_code == "gr" {
                "el".to_string()
            } else if lang_code == "english" || lang_code == "en" {
                "en".to_string()
            } else {
                lang_code
            };

            if !matches_preferred_language(&lang_code, request.languages.as_deref()) {
                continue;
            }

            let download_link = format!("{GREEKSUBS_BASE}dll/{subtitle_id}/0/{sec_code}");

            // Version from text
            let version = row
                .text()
                .collect::<Vec<_>>()
                .join(" ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");

            let id = format!("greeksubs-{subtitle_id}");
            results.push(SubtitleSearchResult {
                id,
                name: version,
                language: lang_code,
                language_name: "Greek".into(),
                format: "srt".into(),
                provider: "greeksubs".into(),
                detail_path: None,
                download_path: Some(download_link),
                download_count: None,
                rating: None,
                movie_name: None,
                release_group: None,
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
            .ok_or("greeksubs: download_path required")?;

        let client = build_client()?;

        // GET first to get the form
        let resp = client
            .get(download_url)
            .header("Referer", GREEKSUBS_BASE)
            .send()
            .await
            .map_err(|e| format!("greeksubs GET failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("greeksubs GET failed: {}", resp.status()));
        }

        let html = resp
            .text()
            .await
            .map_err(|e| format!("greeksubs read error: {e}"))?;

        let form_data = {
            let document = Html::parse_document(&html);
            let input_sel = Selector::parse("input[type='hidden']")
                .map_err(|e| format!("greeksubs selector error: {e}"))?;
            let mut data = std::collections::HashMap::new();
            for input in document.select(&input_sel) {
                if let (Some(name), Some(value)) =
                    (input.value().attr("name"), input.value().attr("value"))
                {
                    data.insert(name.to_string(), value.to_string());
                }
            }
            data
        };

        // POST with form fields
        let post_resp = client
            .post(download_url)
            .header("Referer", download_url)
            .form(&form_data)
            .send()
            .await
            .map_err(|e| format!("greeksubs POST failed: {e}"))?;

        if !post_resp.status().is_success() {
            return Err(format!("greeksubs POST failed: {}", post_resp.status()));
        }

        let content = post_resp
            .bytes()
            .await
            .map_err(|e| format!("greeksubs read content error: {e}"))?;

        extract_archive(
            &content,
            "subtitle.zip",
            &request.language,
            &self.staging_root,
        )
        .await
    }
}
