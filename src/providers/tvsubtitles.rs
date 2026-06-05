use std::io::{Cursor, Read};

use async_trait::async_trait;
use regex::Regex;
use reqwest::Client;
use scraper::{Html, Selector};
use zip::ZipArchive;

use super::SubtitleProvider;
use crate::models::{
    matches_preferred_language, DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest,
    SubtitleSearchResult,
};

const BASE_URL: &str = "https://www.tvsubtitles.net/";
const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

pub struct TvSubtitlesProvider;

impl TvSubtitlesProvider {
    pub fn new() -> Self {
        Self
    }
}

fn build_client() -> Result<Client, String> {
    Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| format!("Failed to build tvsubtitles HTTP client: {e}"))
}

/// Convert tvsubtitles language image code to our language tag.
/// tvsubtitles uses mostly ISO 639-1 alpha-2 codes but with some exceptions.
fn tvsubtitles_code_to_language(code: &str) -> String {
    match code {
        "br" => "pt-BR",
        "ua" => "uk",
        "gr" => "el",
        "cn" => "zh",
        "jp" => "ja",
        "cz" => "cs",
        "ar" => "ar",
        "bg" => "bg",
        "da" => "da",
        "de" => "de",
        "el" => "el",
        "en" => "en",
        "es" => "es",
        "fi" => "fi",
        "fr" => "fr",
        "hu" => "hu",
        "it" => "it",
        "ja" => "ja",
        "ko" => "ko",
        "nl" => "nl",
        "pl" => "pl",
        "pt" => "pt",
        "ro" => "ro",
        "ru" => "ru",
        "sv" => "sv",
        "tr" => "tr",
        "uk" => "uk",
        "zh" => "zh",
        other => other,
    }
    .to_string()
}

/// Human-readable language name for a tvsubtitles image code.
fn tvsubtitles_code_to_name(code: &str) -> String {
    match code {
        "ar" => "Arabic",
        "bg" => "Bulgarian",
        "br" => "Portuguese (Brazil)",
        "cn" => "Chinese",
        "cs" | "cz" => "Czech",
        "da" => "Danish",
        "de" => "German",
        "el" | "gr" => "Greek",
        "en" => "English",
        "es" => "Spanish",
        "fi" => "Finnish",
        "fr" => "French",
        "hu" => "Hungarian",
        "it" => "Italian",
        "ja" | "jp" => "Japanese",
        "ko" => "Korean",
        "nl" => "Dutch",
        "pl" => "Polish",
        "pt" => "Portuguese",
        "ro" => "Romanian",
        "ru" => "Russian",
        "sv" => "Swedish",
        "tr" => "Turkish",
        "ua" | "uk" => "Ukrainian",
        "zh" => "Chinese",
        other => other,
    }
    .to_string()
}

/// Extract tvsubtitles language code from an img src like `images/flags/en.gif`
/// or `/images/flags/en.gif`.
fn extract_lang_from_img_src(src: &str) -> Option<String> {
    let re = Regex::new(r"flags/([a-z]+)\.[a-z]{2,4}$").ok()?;
    re.captures(src)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// Extract numeric ID from an href like `/tvshow-123.html` or `tvshow-123.html`.
fn extract_id_from_href(href: &str, prefix: &str) -> Option<u64> {
    let href = href.trim_start_matches('/');
    let href = href.strip_prefix(prefix)?;
    let href = href.strip_suffix(".html")?;
    href.parse().ok()
}

async fn http_get(client: &Client, url: &str) -> Result<String, String> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("tvsubtitles GET {url} failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "tvsubtitles GET {url} returned {}",
            resp.status().as_u16()
        ));
    }
    resp.text()
        .await
        .map_err(|e| format!("tvsubtitles read body failed: {e}"))
}

async fn http_get_bytes(client: &Client, url: &str) -> Result<Vec<u8>, String> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("tvsubtitles GET {url} failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "tvsubtitles GET {url} returned {}",
            resp.status().as_u16()
        ));
    }
    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| format!("tvsubtitles read bytes failed: {e}"))
}

/// POST search and return (show_id, show_title) for results matching `query`.
async fn search_show_id(client: &Client, query: &str) -> Result<Option<(u64, String)>, String> {
    let url = format!("{BASE_URL}search.php");
    let resp = client
        .post(&url)
        .form(&[("q", query)])
        .send()
        .await
        .map_err(|e| format!("tvsubtitles search POST failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "tvsubtitles search returned {}",
            resp.status().as_u16()
        ));
    }
    let html = resp
        .text()
        .await
        .map_err(|e| format!("tvsubtitles search read failed: {e}"))?;

    let document = Html::parse_document(&html);
    let anchor_sel = Selector::parse(r#"div.left li div a[href]"#)
        .map_err(|e| format!("parse selector failed: {e}"))?;

    // Pattern: "Series Name (2010-2020)" or "Series Name (US) (2010-2020)"
    let link_re = Regex::new(
        r"^(?P<series>.+?)(?: \(?\d{4}\)?| \((?:US|UK)\))? \((?P<first_year>\d{4})-\d{4}\)$",
    )
    .map_err(|e| format!("link regex failed: {e}"))?;

    for anchor in document.select(&anchor_sel) {
        let href = match anchor.value().attr("href") {
            Some(h) if h.contains("tvshow-") => h,
            _ => continue,
        };
        let show_id = match extract_id_from_href(href, "tvshow-") {
            Some(id) => id,
            None => continue,
        };
        let text: String = anchor.text().collect::<String>().trim().to_string();
        if let Some(caps) = link_re.captures(&text) {
            let series_name = caps.name("series").map(|m| m.as_str()).unwrap_or("");
            if series_name.to_ascii_lowercase() == query.to_ascii_lowercase() {
                return Ok(Some((show_id, series_name.to_string())));
            }
        }
    }

    // Fallback: return first show found even if name doesn't match exactly
    for anchor in document.select(&anchor_sel) {
        let href = match anchor.value().attr("href") {
            Some(h) if h.contains("tvshow-") => h,
            _ => continue,
        };
        let show_id = match extract_id_from_href(href, "tvshow-") {
            Some(id) => id,
            None => continue,
        };
        let text: String = anchor.text().collect::<String>().trim().to_string();
        if let Some(caps) = link_re.captures(&text) {
            let series_name = caps.name("series").map(|m| m.as_str()).unwrap_or(&text);
            return Ok(Some((show_id, series_name.to_string())));
        }
    }

    Ok(None)
}

/// Get episode IDs for a show's season. Returns map from episode_number → episode_id.
async fn get_episode_ids(
    client: &Client,
    show_id: u64,
    season: u32,
) -> Result<Vec<(u32, u64)>, String> {
    let url = format!("{BASE_URL}tvshow-{show_id}-{season}.html");
    let html = http_get(client, &url).await?;
    let document = Html::parse_document(&html);
    let row_sel = Selector::parse("table#table5 tr")
        .map_err(|e| format!("parse table row selector failed: {e}"))?;

    let mut episodes = Vec::new();
    let episode_href_re =
        Regex::new(r"^/?episode-\d+\.html$").map_err(|e| format!("episode href regex: {e}"))?;

    for row in document.select(&row_sel) {
        let anchor = row
            .select(
                &Selector::parse("a[href]").map_err(|e| format!("anchor selector failed: {e}"))?,
            )
            .find(|a| {
                a.value()
                    .attr("href")
                    .map(|h| episode_href_re.is_match(h))
                    .unwrap_or(false)
            });
        let anchor = match anchor {
            Some(a) => a,
            None => continue,
        };

        let href = anchor.value().attr("href").unwrap_or("");
        let episode_id = match extract_id_from_href(href, "episode-") {
            Some(id) => id,
            None => continue,
        };

        let td_sel = Selector::parse("td").map_err(|e| format!("td selector: {e}"))?;
        let cells: Vec<_> = row.select(&td_sel).collect();
        if cells.is_empty() {
            continue;
        }
        let cell_text: String = cells[0].text().collect();
        // Format: "1x01" or "S01E01"
        let episode_num: u32 = if let Some(pos) = cell_text.find('x') {
            cell_text[pos + 1..].trim().parse().unwrap_or(0)
        } else if let Some(pos) = cell_text.to_ascii_lowercase().find('e') {
            cell_text[pos + 1..].trim().parse().unwrap_or(0)
        } else {
            cell_text.trim().parse().unwrap_or(0)
        };
        if episode_num == 0 {
            continue;
        }
        episodes.push((episode_num, episode_id));
    }

    Ok(episodes)
}

#[derive(Debug)]
struct SubtitleEntry {
    subtitle_id: u64,
    lang_code: String,
    rip: String,
    release: String,
}

/// Get subtitle entries from an episode page.
async fn get_episode_subtitles(
    client: &Client,
    episode_id: u64,
) -> Result<Vec<SubtitleEntry>, String> {
    let url = format!("{BASE_URL}episode-{episode_id}.html");
    let html = http_get(client, &url).await?;
    let document = Html::parse_document(&html);
    let row_sel =
        Selector::parse(".subtitlen").map_err(|e| format!("subtitlen selector failed: {e}"))?;

    let mut entries = Vec::new();
    for row in document.select(&row_sel) {
        // subtitle_id from parent anchor href: /subtitle-123.html
        let subtitle_id = {
            let parent_html = row.parent().and_then(|p| scraper::ElementRef::wrap(p));
            match parent_html {
                Some(parent) => {
                    let href = parent.value().attr("href").unwrap_or("");
                    extract_id_from_href(href, "subtitle-")
                }
                None => None,
            }
        };
        let subtitle_id = match subtitle_id {
            Some(id) => id,
            None => continue,
        };

        // Language from img src inside h5
        let img_sel = Selector::parse("h5 img").map_err(|e| format!("img selector: {e}"))?;
        let lang_code = row
            .select(&img_sel)
            .next()
            .and_then(|img| img.value().attr("src"))
            .and_then(|src| extract_lang_from_img_src(src))
            .unwrap_or_else(|| "en".to_string());

        // Rip from <p title="rip">
        let rip_sel =
            Selector::parse(r#"p[title="rip"]"#).map_err(|e| format!("rip selector: {e}"))?;
        let rip = row
            .select(&rip_sel)
            .next()
            .map(|p| p.text().collect::<String>().trim().to_string())
            .unwrap_or_default();

        // Release from first <h5>
        let h5_sel = Selector::parse("h5").map_err(|e| format!("h5 selector: {e}"))?;
        let release = row
            .select(&h5_sel)
            .next()
            .map(|h| h.text().collect::<String>().trim().to_string())
            .unwrap_or_default();

        entries.push(SubtitleEntry {
            subtitle_id,
            lang_code,
            rip,
            release,
        });
    }

    Ok(entries)
}

/// Fetch `download-{subtitle_id}.html`, extract JS-concatenated URL parts,
/// then download and return the raw ZIP/SRT bytes and filename.
async fn fetch_subtitle_file(
    client: &Client,
    subtitle_id: u64,
) -> Result<(Vec<u8>, String), String> {
    let download_page_url = format!("{BASE_URL}download-{subtitle_id}.html");
    let html = http_get(client, &download_page_url).await?;

    // Find JS parts: s1 = 'abc'; s2 = 'def'; …
    let parts_re =
        Regex::new(r#"s\d\s*=\s*'([^']*)'"#).map_err(|e| format!("js parts regex: {e}"))?;
    let parts: Vec<&str> = parts_re
        .captures_iter(&html)
        .filter_map(|c| c.get(1).map(|m| m.as_str()))
        .collect();

    if parts.is_empty() {
        return Err(format!(
            "tvsubtitles: could not find download link for subtitle {subtitle_id}"
        ));
    }

    let path: String = parts.concat();
    let download_url = format!("{BASE_URL}{path}");
    let bytes = http_get_bytes(client, &download_url).await?;

    // Derive a filename from the last path segment
    let filename = path
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("subtitle.zip")
        .to_string();

    Ok((bytes, filename))
}

/// Extract the first subtitle file from a ZIP archive.
fn extract_from_zip(bytes: &[u8]) -> Result<(Vec<u8>, String), String> {
    let cursor = Cursor::new(bytes);
    let mut zip =
        ZipArchive::new(cursor).map_err(|e| format!("tvsubtitles: zip open failed: {e}"))?;
    if zip.len() == 0 {
        return Err("tvsubtitles: zip archive is empty".to_string());
    }
    let mut file = zip
        .by_index(0)
        .map_err(|e| format!("tvsubtitles: zip entry failed: {e}"))?;
    let name = file.name().to_string();
    let mut content = Vec::new();
    file.read_to_end(&mut content)
        .map_err(|e| format!("tvsubtitles: zip read failed: {e}"))?;
    Ok((content, name))
}

fn subtitle_format_from_name(name: &str) -> String {
    name.rsplit('.')
        .next()
        .map(|ext| ext.to_ascii_lowercase())
        .filter(|ext| matches!(ext.as_str(), "srt" | "ass" | "ssa" | "vtt" | "sub"))
        .unwrap_or_else(|| "srt".to_string())
}

#[async_trait]
impl SubtitleProvider for TvSubtitlesProvider {
    fn name(&self) -> &str {
        "tvsubtitles"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let query = request
            .query
            .as_deref()
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .ok_or_else(|| "tvsubtitles: search query is required".to_string())?
            .to_string();

        let client = build_client()?;

        let (show_id, show_title) = search_show_id(&client, &query)
            .await?
            .ok_or_else(|| format!("tvsubtitles: no show found for '{query}'"))?;

        // Get season 1 episodes
        let episodes = get_episode_ids(&client, show_id, 1).await?;
        if episodes.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = Vec::new();

        for (ep_num, ep_id) in &episodes {
            let entries = match get_episode_subtitles(&client, *ep_id).await {
                Ok(e) => e,
                Err(_) => continue,
            };

            for entry in entries {
                let language = tvsubtitles_code_to_language(&entry.lang_code);
                if !matches_preferred_language(&language, request.languages.as_deref()) {
                    continue;
                }
                let language_name = tvsubtitles_code_to_name(&entry.lang_code);
                let name = if !entry.release.is_empty() && !entry.rip.is_empty() {
                    format!("{} ({})", entry.release, entry.rip)
                } else if !entry.release.is_empty() {
                    entry.release.clone()
                } else if !entry.rip.is_empty() {
                    entry.rip.clone()
                } else {
                    format!("{show_title} S01E{ep_num:02}")
                };

                results.push(SubtitleSearchResult {
                    id: entry.subtitle_id.to_string(),
                    name,
                    language,
                    language_name,
                    format: "srt".to_string(),
                    provider: "tvsubtitles".to_string(),
                    detail_path: Some(format!("subtitle-{}.html", entry.subtitle_id)),
                    download_path: Some(format!("download-{}.html", entry.subtitle_id)),
                    download_count: None,
                    rating: None,
                    movie_name: Some(show_title.clone()),
                    release_group: (!entry.release.is_empty()).then_some(entry.release),
                });
            }
        }

        Ok(results)
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        let subtitle_id: u64 = request
            .subtitle_id
            .parse()
            .map_err(|_| format!("tvsubtitles: invalid subtitle id '{}'", request.subtitle_id))?;

        let client = build_client()?;
        let (bytes, filename) = fetch_subtitle_file(&client, subtitle_id).await?;

        // If the downloaded file is a ZIP, extract the first entry
        let (content, name) = if filename.to_ascii_lowercase().ends_with(".zip")
            || bytes.starts_with(b"PK\x03\x04")
        {
            extract_from_zip(&bytes)?
        } else {
            (bytes, filename)
        };

        let format = subtitle_format_from_name(&name);
        Ok(DownloadedSubtitle {
            name: request.name.clone().unwrap_or(name),
            format,
            content,
        })
    }
}
