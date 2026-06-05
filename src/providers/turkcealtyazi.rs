use async_trait::async_trait;
use scraper::{Html, Selector};
use std::path::PathBuf;

use super::SubtitleProvider;
use crate::archive::extract_archive;
use crate::models::{
    DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult,
};

const TURKCEALTYAZI_BASE_URL: &str = "https://turkcealtyazi.org";
const TURKCEALTYAZI_USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

pub struct TurkcealtyaziProvider {
    staging_root: PathBuf,
}

impl TurkcealtyaziProvider {
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
            .user_agent(TURKCEALTYAZI_USER_AGENT)
            .build()
            .map_err(|e| format!("Failed to build TurkceAltyazi HTTP client: {e}"))
    }

    fn text_content(element: scraper::ElementRef<'_>) -> String {
        element.text().collect::<String>().trim().to_string()
    }

    fn map_language_class(class: &str) -> &'static str {
        match class {
            "flagtr" => "tr",
            "flagen" => "en",
            _ => "und",
        }
    }
}

#[async_trait]
impl SubtitleProvider for TurkcealtyaziProvider {
    fn name(&self) -> &str {
        "turkcealtyazi"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let imdb_id = request
            .imdb_id
            .as_deref()
            .ok_or("TurkceAltyazi search requires imdb_id")?;

        let client = Self::build_client()?;

        let search_url = format!("{TURKCEALTYAZI_BASE_URL}/find.php?cat=sub&find={imdb_id}");
        let response = client
            .get(&search_url)
            .header("Referer", TURKCEALTYAZI_BASE_URL)
            .send()
            .await
            .map_err(|e| format!("TurkceAltyazi search failed: {e}"))?;

        if response.status().as_u16() == 404 {
            return Ok(Vec::new());
        }
        if !response.status().is_success() {
            return Err(format!(
                "TurkceAltyazi search failed: HTTP {}",
                response.status().as_u16()
            ));
        }

        let html = response
            .text()
            .await
            .map_err(|e| format!("Failed to read TurkceAltyazi response: {e}"))?;

        let document = Html::parse_document(&html);

        // Check for 404 in meta description
        if let Ok(meta_sel) = Selector::parse("meta[name='description']") {
            if let Some(meta) = document.select(&meta_sel).next() {
                if meta
                    .value()
                    .attr("content")
                    .unwrap_or_default()
                    .contains("404 Error")
                {
                    return Ok(Vec::new());
                }
            }
        }

        let mut results = Vec::new();

        // Select subtitle entries (movie: altsonsez2, episode: altsonsez1.sezonX)
        let entry_sels = ["div.altsonsez2", "div.altsonsez1"];
        for sel_str in &entry_sels {
            let Ok(entry_sel) = Selector::parse(sel_str) else {
                continue;
            };

            for entry in document.select(&entry_sel) {
                // Get link to subtitle page
                let link_sel = Selector::parse("div.alisim > div.fl > a")
                    .map_err(|e| format!("TurkceAltyazi selector error: {e}"))?;
                let page_link = entry
                    .select(&link_sel)
                    .next()
                    .and_then(|a| a.value().attr("href"))
                    .map(|h| {
                        if h.starts_with("http") {
                            h.to_string()
                        } else {
                            format!("{TURKCEALTYAZI_BASE_URL}{h}")
                        }
                    });

                let Some(page_link) = page_link else {
                    continue;
                };

                // Language
                let lang_class_sel = Selector::parse("div.aldil > span")
                    .map_err(|e| format!("TurkceAltyazi selector error: {e}"))?;
                let language = entry
                    .select(&lang_class_sel)
                    .next()
                    .and_then(|span| span.value().classes().next())
                    .map(Self::map_language_class)
                    .unwrap_or("tr");

                let language_name = match language {
                    "tr" => "Turkish",
                    "en" => "English",
                    _ => language,
                };

                // Release info
                let rip_sel = Selector::parse("div.ta-container > div.ripdiv")
                    .map_err(|e| format!("TurkceAltyazi selector error: {e}"))?;
                let release_info = entry
                    .select(&rip_sel)
                    .next()
                    .map(|d| Self::text_content(d))
                    .unwrap_or_default();

                // Uploader
                let uploader_sel = Selector::parse("div.alcevirmen")
                    .map_err(|e| format!("TurkceAltyazi selector error: {e}"))?;
                let uploader = entry
                    .select(&uploader_sel)
                    .next()
                    .map(|d| Self::text_content(d))
                    .filter(|s| !s.is_empty());

                let id = page_link.clone();
                let name = if release_info.is_empty() {
                    page_link
                        .rsplit('/')
                        .next()
                        .unwrap_or("subtitle")
                        .to_string()
                } else {
                    release_info.clone()
                };

                results.push(SubtitleSearchResult {
                    id,
                    name,
                    language: language.to_string(),
                    language_name: language_name.to_string(),
                    format: "srt".into(),
                    provider: "turkcealtyazi".into(),
                    detail_path: Some(page_link),
                    download_path: None,
                    download_count: None,
                    rating: None,
                    movie_name: None,
                    release_group: uploader,
                });
            }
        }

        Ok(results)
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        let detail_url = request
            .detail_path
            .as_deref()
            .or(request.download_path.as_deref())
            .ok_or("TurkceAltyazi download requires detail_path")?;

        let client = Self::build_client()?;

        // Fetch the subtitle detail page to extract download form fields
        let page_resp = client
            .get(detail_url)
            .header("Referer", TURKCEALTYAZI_BASE_URL)
            .send()
            .await
            .map_err(|e| format!("TurkceAltyazi detail page fetch failed: {e}"))?;

        if !page_resp.status().is_success() {
            return Err(format!(
                "TurkceAltyazi detail page failed: HTTP {}",
                page_resp.status().as_u16()
            ));
        }

        let html = page_resp
            .text()
            .await
            .map_err(|e| format!("Failed to read TurkceAltyazi detail page: {e}"))?;

        let (idid, altid, sidid) = {
            let document = Html::parse_document(&html);

            let extract_input = |name: &str| -> Result<String, String> {
                let sel = Selector::parse(&format!("input[name='{name}']"))
                    .map_err(|e| format!("TurkceAltyazi selector error: {e}"))?;
                document
                    .select(&sel)
                    .next()
                    .and_then(|i| i.value().attr("value"))
                    .map(|s| s.to_string())
                    .ok_or_else(|| format!("TurkceAltyazi: missing form field '{name}'"))
            };

            let idid = extract_input("idid")?;
            let altid = extract_input("altid")?;
            let sidid = extract_input("sidid")?;
            (idid, altid, sidid)
        };

        let dl_url = format!("{TURKCEALTYAZI_BASE_URL}/ind");
        let content = client
            .post(&dl_url)
            .header("Referer", detail_url)
            .form(&[("idid", &idid), ("altid", &altid), ("sidid", &sidid)])
            .send()
            .await
            .map_err(|e| format!("TurkceAltyazi download POST failed: {e}"))?
            .bytes()
            .await
            .map_err(|e| format!("Failed to read TurkceAltyazi download: {e}"))?;

        extract_archive(
            &content,
            "subtitle.zip",
            &request.language,
            &self.staging_root,
        )
        .await
    }
}
