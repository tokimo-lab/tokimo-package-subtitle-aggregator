use std::path::PathBuf;

use async_trait::async_trait;
use scraper::{Html, Selector};

use super::SubtitleProvider;
use crate::archive::extract_archive;
use crate::models::{
    matches_preferred_language, DownloadedSubtitle, SubtitleDownloadRequest,
    SubtitleSearchRequest, SubtitleSearchResult,
};

const SUBS4FREE_BASE: &str = "https://www.subs4free.info";
const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

pub struct Subs4FreeProvider {
    staging_root: PathBuf,
}

impl Subs4FreeProvider {
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
        .map_err(|e| format!("Failed to build subs4free client: {e}"))
}

#[async_trait]
impl SubtitleProvider for Subs4FreeProvider {
    fn name(&self) -> &str {
        "subs4free"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let query = request
            .query
            .as_deref()
            .ok_or("subs4free requires query")?;

        let encoded = url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>();
        let search_url = format!("{SUBS4FREE_BASE}/search_report.php?search={encoded}&searchType=1");

        let client = build_client()?;
        let resp = client
            .get(&search_url)
            .send()
            .await
            .map_err(|e| format!("subs4free search request failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("subs4free search failed: {}", resp.status()));
        }

        let html = resp
            .text()
            .await
            .map_err(|e| format!("subs4free read error: {e}"))?;

        let show_link = {
            let document = Html::parse_document(&html);
            let option_sel = Selector::parse("select[name=\"Mov_sel\"] > option[value]")
                .map_err(|e| format!("subs4free selector error: {e}"))?;
            document
                .select(&option_sel)
                .find_map(|opt| {
                    let val = opt.value().attr("value").unwrap_or("").trim().to_string();
                    if !val.is_empty() && val != "#" {
                        Some(val)
                    } else {
                        None
                    }
                })
                .ok_or("subs4free: no show options found")?
        };

        // Fetch show page
        let show_url = if show_link.starts_with("http") {
            show_link
        } else {
            format!("{SUBS4FREE_BASE}/{}", show_link.trim_start_matches('/'))
        };

        let show_resp = client
            .get(&show_url)
            .send()
            .await
            .map_err(|e| format!("subs4free show request failed: {e}"))?;

        if !show_resp.status().is_success() {
            return Err(format!("subs4free show request failed: {}", show_resp.status()));
        }

        let show_html = show_resp
            .text()
            .await
            .map_err(|e| format!("subs4free read show error: {e}"))?;

        let show_doc = Html::parse_document(&show_html);

        let details_sel = Selector::parse(".movie-details")
            .map_err(|e| format!("subs4free selector error: {e}"))?;
        let span_sel =
            Selector::parse("span").map_err(|e| format!("subs4free selector error: {e}"))?;
        let a_sel = Selector::parse("a").map_err(|e| format!("subs4free selector error: {e}"))?;
        let img_sel =
            Selector::parse("img").map_err(|e| format!("subs4free selector error: {e}"))?;

        let mut results = Vec::new();

        for subs_tag in show_doc.select(&details_sel) {
            let version = subs_tag
                .select(&span_sel)
                .next()
                .map(|s| s.text().collect::<String>().trim().to_string())
                .unwrap_or_default();

            let href = subs_tag
                .select(&a_sel)
                .next()
                .and_then(|a| a.value().attr("href"))
                .unwrap_or("");

            if href.is_empty() {
                continue;
            }

            let download_link = if href.starts_with("http") {
                href.to_string()
            } else {
                format!("{SUBS4FREE_BASE}/{}", href.trim_start_matches('/'))
            };

            // Language from .sprite class
            let lang_code = subs_tag
                .select(&img_sel)
                .find_map(|img| {
                    let classes = img.value().attr("class").unwrap_or("");
                    // second class name, strip 'gif' suffix
                    classes.split_whitespace().nth(1).map(|c| {
                        c.trim_end_matches("gif")
                            .trim_end_matches('.')
                            .to_lowercase()
                    })
                })
                .unwrap_or_else(|| "el".to_string());

            if !matches_preferred_language(&lang_code, request.languages.as_deref()) {
                continue;
            }

            let id = format!(
                "subs4free-{}",
                href.trim_end_matches('/').rsplit('/').next().unwrap_or(&version)
            );

            results.push(SubtitleSearchResult {
                id,
                name: version,
                language: lang_code,
                language_name: "Greek".into(),
                format: "srt".into(),
                provider: "subs4free".into(),
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
            .ok_or("subs4free: download_path required")?;

        let client = build_client()?;

        let resp = client
            .get(download_url)
            .send()
            .await
            .map_err(|e| format!("subs4free GET failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("subs4free GET failed: {}", resp.status()));
        }

        let html = resp
            .text()
            .await
            .map_err(|e| format!("subs4free read error: {e}"))?;

        let subtitle_id = {
            let document = Html::parse_document(&html);
            let id_sel = Selector::parse("input[name=\"id\"]")
                .map_err(|e| format!("subs4free selector error: {e}"))?;
            document
                .select(&id_sel)
                .next()
                .and_then(|el| el.value().attr("value"))
                .ok_or("subs4free: no subtitle id found")?
                .to_string()
        };

        let post_url = format!("{SUBS4FREE_BASE}/getSub.php");
        let form_data = [("id", subtitle_id.as_str()), ("x", "5"), ("y", "5")];

        let post_resp = client
            .post(&post_url)
            .header("Referer", download_url)
            .form(&form_data)
            .send()
            .await
            .map_err(|e| format!("subs4free POST failed: {e}"))?;

        if !post_resp.status().is_success() {
            return Err(format!("subs4free POST failed: {}", post_resp.status()));
        }

        let content = post_resp
            .bytes()
            .await
            .map_err(|e| format!("subs4free read content error: {e}"))?;

        extract_archive(&content, "subtitle.zip", &request.language, &self.staging_root).await
    }
}
