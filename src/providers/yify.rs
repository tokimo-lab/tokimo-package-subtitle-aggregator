use std::path::PathBuf;

use async_trait::async_trait;
use reqwest::header::{ACCEPT, REFERER};
use scraper::{Html, Selector};

use super::SubtitleProvider;
use crate::archive::extract_archive;
use crate::models::{
    matches_preferred_language, DownloadedSubtitle, SubtitleDownloadRequest,
    SubtitleSearchRequest, SubtitleSearchResult,
};

const YIFY_BASE_URL: &str = "https://yifysubtitles.ch";
const YIFY_USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

pub struct YifyProvider {
    staging_root: PathBuf,
}

impl YifyProvider {
    pub fn new(staging_root: impl Into<PathBuf>) -> Self {
        Self {
            staging_root: staging_root.into(),
        }
    }
}

fn yify_language_to_code(lang_name: &str) -> &'static str {
    match lang_name.trim() {
        "Albanian" => "sq",
        "Arabic" => "ar",
        "Bengali" => "bn",
        "Brazilian Portuguese" => "pt-BR",
        "Bulgarian" => "bg",
        "Chinese" => "zh",
        "Croatian" => "hr",
        "Czech" => "cs",
        "Danish" => "da",
        "Dutch" => "nl",
        "English" => "en",
        "Farsi/Persian" => "fa",
        "Finnish" => "fi",
        "French" => "fr",
        "German" => "de",
        "Greek" => "el",
        "Hebrew" => "he",
        "Hungarian" => "hu",
        "Indonesian" => "id",
        "Italian" => "it",
        "Japanese" => "ja",
        "Korean" => "ko",
        "Lithuanian" => "lt",
        "Macedonian" => "mk",
        "Malay" => "ms",
        "Norwegian" => "no",
        "Polish" => "pl",
        "Portuguese" => "pt",
        "Romanian" => "ro",
        "Russian" => "ru",
        "Serbian" => "sr",
        "Slovenian" => "sl",
        "Spanish" => "es",
        "Swedish" => "sv",
        "Thai" => "th",
        "Turkish" => "tr",
        "Urdu" => "ur",
        "Vietnamese" => "vi",
        _ => "und",
    }
}

fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(YIFY_USER_AGENT)
        .build()
        .map_err(|e| format!("Failed to build YIFY HTTP client: {e}"))
}

fn absolute_yify_url(path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else {
        format!(
            "{}{}{}",
            YIFY_BASE_URL,
            if path.starts_with('/') { "" } else { "/" },
            path
        )
    }
}

fn text_content(element: scraper::ElementRef<'_>) -> String {
    element
        .text()
        .collect::<String>()
        .replace('\u{a0}', " ")
        .trim()
        .to_string()
}

async fn fetch_yify_html(url: &str) -> Result<String, String> {
    let client = build_client()?;
    let response = client
        .get(url)
        .header(
            ACCEPT,
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header(REFERER, YIFY_BASE_URL)
        .send()
        .await
        .map_err(|e| format!("YIFY request failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "YIFY request failed: {}",
            response.status().as_u16()
        ));
    }

    response
        .text()
        .await
        .map_err(|e| format!("Failed to read YIFY response: {e}"))
}

fn parse_movie_page(
    html: &str,
    requested_languages: Option<&[String]>,
) -> Result<Vec<SubtitleSearchResult>, String> {
    let document = Html::parse_document(html);

    let table_sel = Selector::parse("table.other-subs")
        .map_err(|e| format!("YIFY selector error: {e}"))?;
    let tbody_sel =
        Selector::parse("tbody").map_err(|e| format!("YIFY selector error: {e}"))?;
    let tr_sel = Selector::parse("tr").map_err(|e| format!("YIFY selector error: {e}"))?;
    let td_sel = Selector::parse("td").map_err(|e| format!("YIFY selector error: {e}"))?;
    let a_sel = Selector::parse("a").map_err(|e| format!("YIFY selector error: {e}"))?;

    let Some(table) = document.select(&table_sel).next() else {
        return Ok(Vec::new());
    };

    let mut results = Vec::new();

    let rows: Vec<_> = if let Some(tbody) = table.select(&tbody_sel).next() {
        tbody.select(&tr_sel).collect()
    } else {
        table.select(&tr_sel).collect()
    };

    for row in rows {
        let tds: Vec<_> = row.select(&td_sel).collect();
        if tds.len() < 5 {
            continue;
        }

        let rating_text = text_content(tds[0]);
        let rating = rating_text.trim().parse::<f64>().ok();

        let lang_name = text_content(tds[1]);
        let lang_code = yify_language_to_code(&lang_name).to_string();

        if !matches_preferred_language(&lang_code, requested_languages) {
            continue;
        }

        // Release name: strip leading "\nsubtitle " prefix from bazarr
        let release_raw = text_content(tds[2]);
        let release = release_raw
            .trim_start_matches('\n')
            .trim_start_matches("subtitle ")
            .trim()
            .to_string();

        let detail_path = tds[2]
            .select(&a_sel)
            .next()
            .and_then(|a| a.value().attr("href"))
            .map(|s| s.to_string());

        let uploader = text_content(tds[4]);

        let id = detail_path
            .clone()
            .unwrap_or_else(|| format!("yify-{lang_code}-{}", release.replace(' ', "-")));

        results.push(SubtitleSearchResult {
            id,
            name: if release.is_empty() {
                lang_name.clone()
            } else {
                release
            },
            language: lang_code,
            language_name: lang_name,
            format: "srt".into(),
            provider: "yify".into(),
            detail_path,
            download_path: None,
            download_count: None,
            rating,
            movie_name: None,
            release_group: if uploader.is_empty() {
                None
            } else {
                Some(uploader)
            },
        });
    }

    results.sort_by(|a, b| {
        b.rating
            .partial_cmp(&a.rating)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(results)
}

#[async_trait]
impl SubtitleProvider for YifyProvider {
    fn name(&self) -> &str {
        "yify"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let movie_html = if let Some(imdb_id) = &request.imdb_id {
            let url = format!("{YIFY_BASE_URL}/movie-imdb/{imdb_id}");
            let client = build_client()?;
            let response = client
                .get(&url)
                .header(
                    ACCEPT,
                    "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
                )
                .header(REFERER, YIFY_BASE_URL)
                .send()
                .await
                .map_err(|e| format!("YIFY request failed: {e}"))?;

            if response.status().as_u16() == 404 {
                return Ok(Vec::new());
            }
            if !response.status().is_success() {
                return Err(format!(
                    "YIFY request failed: {}",
                    response.status().as_u16()
                ));
            }

            response
                .text()
                .await
                .map_err(|e| format!("Failed to read YIFY response: {e}"))?
        } else if let Some(query) = &request.query {
            if query.trim().is_empty() {
                return Err("Query is empty".into());
            }

            let search_url = format!(
                "{YIFY_BASE_URL}/search?q={}",
                url::form_urlencoded::byte_serialize(query.trim().as_bytes())
                    .collect::<String>()
            );
            let search_html = fetch_yify_html(&search_url).await?;
            let movie_path = {
                let document = Html::parse_document(&search_html);

                // YIFY search results list movie links under .media-body or similar containers
                let movie_link_sel = Selector::parse(
                    "div.media-body a, ul.media-list a, .subtitle-page a, a[href*='/subtitles/']",
                )
                .map_err(|e| format!("YIFY selector error: {e}"))?;

                document
                    .select(&movie_link_sel)
                    .next()
                    .and_then(|a| a.value().attr("href"))
                    .ok_or_else(|| format!("No YIFY movie found for query: {query}"))?
                    .to_string()
            };

            fetch_yify_html(&absolute_yify_url(&movie_path)).await?
        } else {
            return Err("Either imdb_id or query is required for YIFY search".into());
        };

        parse_movie_page(&movie_html, request.languages.as_deref())
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        let zip_url = if let Some(dl_path) = &request.download_path {
            absolute_yify_url(dl_path)
        } else if let Some(detail_path) = &request.detail_path {
            let detail_url = absolute_yify_url(detail_path);
            let html = fetch_yify_html(&detail_url).await?;
            let document = Html::parse_document(&html);
            let dl_sel = Selector::parse("a.download-subtitle")
                .map_err(|e| format!("YIFY selector error: {e}"))?;
            let href = document
                .select(&dl_sel)
                .next()
                .and_then(|a| a.value().attr("href"))
                .ok_or_else(|| {
                    "No download link found on YIFY subtitle detail page".to_string()
                })?
                .to_string();
            absolute_yify_url(&href)
        } else {
            return Err(
                "Either download_path or detail_path is required for YIFY download".into(),
            );
        };

        let client = build_client()?;
        let response = client
            .get(&zip_url)
            .header(ACCEPT, "*/*")
            .header(REFERER, YIFY_BASE_URL)
            .send()
            .await
            .map_err(|e| format!("YIFY download failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "YIFY download failed: {}",
                response.status().as_u16()
            ));
        }

        let content = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read YIFY download content: {e}"))?;

        let archive_name = zip_url
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("subtitle.zip")
            .to_string();

        extract_archive(&content, &archive_name, &request.language, &self.staging_root).await
    }
}
