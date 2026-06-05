use async_trait::async_trait;
use scraper::{Html, Selector};

use super::SubtitleProvider;
use crate::models::{
    DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult,
};

const SUBTITRIID_BASE: &str = "https://subtitri.do.am";
const STAGING_ROOT: &str = "/tmp/subtitle-aggregator";

pub struct SubtitriIdProvider;

impl Default for SubtitriIdProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SubtitriIdProvider {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SubtitleProvider for SubtitriIdProvider {
    fn name(&self) -> &str {
        "subtitriid"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let title = request
            .query
            .as_deref()
            .ok_or("subtitriid: query is required")?;

        let client = reqwest::Client::builder()
            .user_agent("subtitle-aggregator/0.1")
            .build()
            .map_err(|e| format!("subtitriid: failed to build client: {e}"))?;

        let search_url = format!("{}/search/?q={}", SUBTITRIID_BASE, encode_query(title));

        let response = client
            .get(&search_url)
            .send()
            .await
            .map_err(|e| format!("subtitriid: search request failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!("subtitriid: HTTP {}", response.status().as_u16()));
        }

        let html = response
            .text()
            .await
            .map_err(|e| format!("subtitriid: failed to read HTML: {e}"))?;

        // Parse the HTML document in a block so `document` (non-Send) is
        // dropped before the first `.await` below.
        let page_links: Vec<(String, String)> = {
            let document = Html::parse_document(&html);

            let block_sel = Selector::parse(".eBlock")
                .map_err(|e| format!("subtitriid: selector error: {e}"))?;
            let title_sel = Selector::parse(".eTitle > a")
                .map_err(|e| format!("subtitriid: selector error: {e}"))?;

            let mut links: Vec<(String, String)> = Vec::new();

            for block in document.select(&block_sel) {
                let anchor = block.select(&title_sel).next();
                let Some(anchor) = anchor else {
                    continue;
                };
                let anchor_title = anchor.text().collect::<String>().trim().to_string();
                let href = anchor.value().attr("href").unwrap_or("");
                let page_url = if href.starts_with("http") {
                    href.to_string()
                } else {
                    format!("{}/{}", SUBTITRIID_BASE, href.trim_start_matches('/'))
                };
                links.push((page_url, anchor_title));
            }
            links
        };

        // Follow each page link to get download details (document is dropped here)
        let mut results = Vec::new();

        for (page_url, page_title) in page_links.iter().take(10) {
            match self.fetch_detail_page(&client, page_url, page_title).await {
                Ok(mut items) => results.append(&mut items),
                Err(e) => tracing::warn!("subtitriid: failed to fetch detail {page_url}: {e}"),
            }
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
            .ok_or("subtitriid: download_path is required")?;

        let client = reqwest::Client::builder()
            .user_agent("subtitle-aggregator/0.1")
            .build()
            .map_err(|e| format!("subtitriid: failed to build client: {e}"))?;

        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("subtitriid: download failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!("subtitriid: HTTP {}", response.status().as_u16()));
        }

        let content = response
            .bytes()
            .await
            .map_err(|e| format!("subtitriid: failed to read content: {e}"))?;

        let name = request
            .name
            .clone()
            .unwrap_or_else(|| format!("subtitriid_{}.zip", request.subtitle_id));

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

impl SubtitriIdProvider {
    async fn fetch_detail_page(
        &self,
        client: &reqwest::Client,
        page_url: &str,
        fallback_title: &str,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let response = client
            .get(page_url)
            .send()
            .await
            .map_err(|e| format!("subtitriid: detail request failed: {e}"))?;

        if !response.status().is_success() {
            return Ok(vec![]);
        }

        let html = response
            .text()
            .await
            .map_err(|e| format!("subtitriid: failed to read detail HTML: {e}"))?;

        let document = Html::parse_document(&html);

        let header_sel = Selector::parse(".main-header")
            .map_err(|e| format!("subtitriid: selector error: {e}"))?;
        let dl_sel =
            Selector::parse(".hvr").map_err(|e| format!("subtitriid: selector error: {e}"))?;

        // Extract title
        let title = document
            .select(&header_sel)
            .next()
            .map(|el| {
                let text = el.text().collect::<String>();
                // Take last part after ' / '
                text.split(" / ").last().unwrap_or(&text).trim().to_string()
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| fallback_title.to_string());

        let mut results = Vec::new();

        for (i, dl_el) in document.select(&dl_sel).enumerate() {
            let href = dl_el.value().attr("href").unwrap_or("");
            if href.is_empty() {
                continue;
            }

            let download_url = if href.starts_with("http") {
                href.to_string()
            } else {
                format!("{}/{}", SUBTITRIID_BASE, href.trim_start_matches('/'))
            };

            // ID from last segment of page_url + index
            let page_id = page_url
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("unknown");

            let sub_id = format!("{}_{}", page_id, i);
            let dl_text = dl_el.text().collect::<String>().trim().to_string();
            let name = if !dl_text.is_empty() {
                dl_text
            } else {
                format!("{}.zip", title)
            };

            results.push(SubtitleSearchResult {
                id: sub_id,
                name,
                language: "et".into(),
                language_name: "Estonian".into(),
                format: "srt".into(),
                provider: "subtitriid".into(),
                detail_path: Some(page_url.to_string()),
                download_path: Some(download_url),
                download_count: None,
                rating: None,
                movie_name: Some(title.clone()),
                release_group: None,
            });
        }

        Ok(results)
    }
}

fn encode_query(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}
