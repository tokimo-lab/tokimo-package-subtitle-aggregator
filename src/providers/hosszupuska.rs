use async_trait::async_trait;
use scraper::{Html, Selector};

use super::SubtitleProvider;
use crate::models::{
    DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult,
};

const HOSSZUPUSKA_BASE: &str = "http://hosszupuskasub.com";
const STAGING_ROOT: &str = "/tmp/subtitle-aggregator";

/// Parse season/episode from a string like "S01E03" or "s01e03".
/// Returns (season, episode) defaulting to (1, 1).
fn parse_season_episode(s: &str) -> (u32, u32) {
    let upper = s.to_uppercase();
    if let Some(s_pos) = upper.find('S') {
        if let Some(e_pos) = upper[s_pos..].find('E') {
            let season_str = &upper[s_pos + 1..s_pos + e_pos];
            let episode_str = &upper[s_pos + e_pos + 1..];
            // episode_str may have trailing non-digit characters
            let episode_digits: String = episode_str
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            let season = season_str.parse::<u32>().unwrap_or(1);
            let episode = episode_digits.parse::<u32>().unwrap_or(1);
            return (season, episode);
        }
    }
    (1, 1)
}

pub struct HosszupuskaProvider;

impl HosszupuskaProvider {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SubtitleProvider for HosszupuskaProvider {
    fn name(&self) -> &str {
        "hosszupuska"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let query = request
            .query
            .as_deref()
            .ok_or("hosszupuska: query is required")?;

        let (season, episode) = parse_season_episode(query);

        // Remove the SxxExx part from series name for the URL parameter
        let series_name = {
            let upper = query.to_uppercase();
            let name = if let Some(pos) = upper.find(" S") {
                query[..pos].trim()
            } else if let Some(pos) = upper.find('S') {
                // If the query starts with an SxxExx pattern, use full query
                if pos == 0 {
                    query
                } else {
                    query[..pos].trim()
                }
            } else {
                query
            };
            name.replace(' ', "+")
        };

        let url = format!(
            "{}/sorozatok.php?cim={}&evad={:02}&resz={:02}&nyelvtipus=%25&x=24&y=8",
            HOSSZUPUSKA_BASE, series_name, season, episode
        );

        let client = reqwest::Client::builder()
            .user_agent("subtitle-aggregator/0.1")
            .build()
            .map_err(|e| format!("hosszupuska: failed to build client: {e}"))?;

        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("hosszupuska: request failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!("hosszupuska: HTTP {}", response.status().as_u16()));
        }

        let html = response
            .text()
            .await
            .map_err(|e| format!("hosszupuska: failed to read HTML: {e}"))?;

        let document = Html::parse_document(&html);

        // Find rows that contain the subtitle indicator
        // Rows that match: tr containing `this.style.backgroundImage='url(css/over2.jpg)`
        let row_sel =
            Selector::parse("tr").map_err(|e| format!("hosszupuska: selector error: {e}"))?;
        let td_sel =
            Selector::parse("td").map_err(|e| format!("hosszupuska: selector error: {e}"))?;
        let img_sel =
            Selector::parse("img").map_err(|e| format!("hosszupuska: selector error: {e}"))?;
        let a_sel =
            Selector::parse("a").map_err(|e| format!("hosszupuska: selector error: {e}"))?;

        let mut results = Vec::new();

        for row in document.select(&row_sel) {
            let row_html = row.html();
            // Only process rows with the subtitle marker
            if !row_html.contains("css/over2.jpg") || !row_html.contains("css/infooldal.png") {
                continue;
            }

            let tds: Vec<_> = row.select(&td_sel).collect();
            if tds.len() < 6 {
                continue;
            }

            let title = tds[0].text().collect::<String>().trim().to_string();

            // Language detection from column 2 img src
            let language = tds[1]
                .select(&img_sel)
                .next()
                .and_then(|img| img.value().attr("src"))
                .map(|src| {
                    if src.ends_with("1.gif") {
                        "hu"
                    } else if src.ends_with("2.gif") {
                        "en"
                    } else {
                        "hu"
                    }
                })
                .unwrap_or("hu");

            // Download link from column 6 (index 5)
            let download_link = tds[5]
                .select(&a_sel)
                .last()
                .and_then(|a| a.value().attr("href"))
                .map(|href| {
                    if href.starts_with("http") {
                        href.to_string()
                    } else {
                        format!("{}/{}", HOSSZUPUSKA_BASE, href.trim_start_matches('/'))
                    }
                });

            let Some(dl_link) = download_link else {
                continue;
            };

            // Extract ID from download link: split on '=' take [1], split on '.' take [0]
            let sub_id = dl_link
                .split('=')
                .nth(1)
                .map(|s| s.split('.').next().unwrap_or(s))
                .unwrap_or("unknown")
                .to_string();

            results.push(SubtitleSearchResult {
                id: sub_id,
                name: title.clone(),
                language: language.to_string(),
                language_name: if language == "hu" {
                    "Hungarian".into()
                } else {
                    "English".into()
                },
                format: "srt".into(),
                provider: "hosszupuska".into(),
                detail_path: None,
                download_path: Some(dl_link),
                download_count: None,
                rating: None,
                movie_name: Some(title),
                release_group: None,
            });
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
            .ok_or("hosszupuska: download_path is required")?;

        let client = reqwest::Client::builder()
            .user_agent("subtitle-aggregator/0.1")
            .build()
            .map_err(|e| format!("hosszupuska: failed to build client: {e}"))?;

        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("hosszupuska: download failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!("hosszupuska: HTTP {}", response.status().as_u16()));
        }

        let content = response
            .bytes()
            .await
            .map_err(|e| format!("hosszupuska: failed to read content: {e}"))?;

        let name = request
            .name
            .clone()
            .unwrap_or_else(|| format!("hosszupuska_{}.zip", request.subtitle_id));

        let staging = std::path::Path::new(STAGING_ROOT);
        crate::archive::extract_archive(&content, &name, &request.language, staging).await
    }
}
