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

const SUBSCENTER_BASE: &str = "http://www.subscenter.info/he/";
const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

pub struct SubsCenterProvider {
    staging_root: PathBuf,
}

impl SubsCenterProvider {
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
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("Failed to build subscenter client: {e}"))
}

fn parse_season_episode(query: &str) -> Option<(u32, u32)> {
    let re = Regex::new(r"[Ss](\d+)[Ee](\d+)").unwrap();
    re.captures(query).map(|cap| {
        let season = cap[1].parse::<u32>().unwrap_or(1);
        let episode = cap[2].parse::<u32>().unwrap_or(1);
        (season, episode)
    })
}

#[async_trait]
impl SubtitleProvider for SubsCenterProvider {
    fn name(&self) -> &str {
        "subscenter"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let query = request
            .query
            .as_deref()
            .ok_or("subscenter requires query")?;

        let encoded = url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>();
        let search_url = format!("{SUBSCENTER_BASE}subtitle/search/?q={encoded}");

        let client = build_client()?;
        let resp = client
            .get(&search_url)
            .send()
            .await
            .map_err(|e| format!("subscenter search failed: {e}"))?;

        let (kind, url_title) = if resp.status().is_redirection() {
            // Redirected directly to a subtitle page
            let location = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            // e.g. /he/subtitle/series/show-name/ or /he/subtitle/movie/movie-name/
            parse_kind_and_title(&location)?
        } else {
            if !resp.status().is_success() {
                return Err(format!("subscenter search failed: {}", resp.status()));
            }
            let html = resp
                .text()
                .await
                .map_err(|e| format!("subscenter read error: {e}"))?;

            let document = Html::parse_document(&html);
            let link_sel = Selector::parse("#processes div.generalWindowTop a")
                .map_err(|e| format!("subscenter selector error: {e}"))?;

            let href = document
                .select(&link_sel)
                .next()
                .and_then(|a| a.value().attr("href"))
                .ok_or("subscenter: no suggestions found")?
                .to_string();

            parse_kind_and_title(&href)?
        };

        let se_opt = parse_season_episode(query);
        let is_series = kind == "series" || se_opt.is_some();

        let data_url = if is_series {
            let (season, episode) = se_opt.unwrap_or((1, 1));
            format!("{SUBSCENTER_BASE}cst/data/series/sb/{url_title}/{season}/{episode}/")
        } else {
            format!("{SUBSCENTER_BASE}cst/data/movie/sb/{url_title}/")
        };

        // Use a following-redirect client for the data request
        let data_client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .cookie_store(true)
            .build()
            .map_err(|e| format!("subscenter: failed to build data client: {e}"))?;

        let data_resp = data_client
            .get(&data_url)
            .send()
            .await
            .map_err(|e| format!("subscenter data request failed: {e}"))?;

        if !data_resp.status().is_success() {
            return Err(format!(
                "subscenter data request failed: {}",
                data_resp.status()
            ));
        }

        let json_text = data_resp
            .text()
            .await
            .map_err(|e| format!("subscenter read data error: {e}"))?;

        let json: serde_json::Value = serde_json::from_str(&json_text)
            .map_err(|e| format!("subscenter JSON parse error: {e}"))?;

        let mut results = Vec::new();

        // JSON: { "he": { "group": { "quality": { "sub_id": { id, key, h_version, downloaded, subtitle_version, hearing_impaired } } } } }
        if let Some(lang_map) = json.as_object() {
            for (lang_key, groups) in lang_map {
                if !matches_preferred_language(lang_key, request.languages.as_deref()) {
                    continue;
                }

                if let Some(groups_obj) = groups.as_object() {
                    for (_group_name, qualities) in groups_obj {
                        if let Some(qualities_obj) = qualities.as_object() {
                            for (_quality, subs) in qualities_obj {
                                if let Some(subs_obj) = subs.as_object() {
                                    for (_sub_id_str, sub_info) in subs_obj {
                                        let id = sub_info["id"]
                                            .as_u64()
                                            .map(|v| v.to_string())
                                            .unwrap_or_default();
                                        let key =
                                            sub_info["key"].as_str().unwrap_or("").to_string();
                                        let subtitle_version = sub_info["subtitle_version"]
                                            .as_str()
                                            .unwrap_or("")
                                            .to_string();
                                        let downloaded = sub_info["downloaded"].as_u64();

                                        if id.is_empty() {
                                            continue;
                                        }

                                        let composite_id = format!("{id}:{key}:{subtitle_version}");

                                        results.push(SubtitleSearchResult {
                                            id: composite_id,
                                            name: subtitle_version.clone(),
                                            language: lang_key.clone(),
                                            language_name: "Hebrew".into(),
                                            format: "srt".into(),
                                            provider: "subscenter".into(),
                                            detail_path: None,
                                            download_path: None,
                                            download_count: downloaded,
                                            rating: None,
                                            movie_name: None,
                                            release_group: None,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(results)
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        // Parse composite ID: subtitle_id:key:version
        let parts: Vec<&str> = request.subtitle_id.splitn(3, ':').collect();
        if parts.len() < 3 {
            return Err(format!(
                "subscenter: invalid subtitle_id format (expected id:key:version): {}",
                request.subtitle_id
            ));
        }
        let subtitle_id = parts[0];
        let subtitle_key = parts[1];
        let subtitle_version = parts[2];

        let download_url = format!(
            "{SUBSCENTER_BASE}subtitle/download/he/{subtitle_id}/?v={subtitle_version}&key={subtitle_key}"
        );

        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .cookie_store(true)
            .build()
            .map_err(|e| format!("subscenter: failed to build download client: {e}"))?;

        let resp = client
            .get(&download_url)
            .send()
            .await
            .map_err(|e| format!("subscenter download failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("subscenter download failed: {}", resp.status()));
        }

        let content = resp
            .bytes()
            .await
            .map_err(|e| format!("subscenter read content error: {e}"))?;

        extract_archive(
            &content,
            "subtitle.zip",
            &request.language,
            &self.staging_root,
        )
        .await
    }
}

fn parse_kind_and_title(path: &str) -> Result<(String, String), String> {
    // e.g. /he/subtitle/series/show-name/ or /he/subtitle/movie/movie-name/
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    // Find "subtitle" segment, then kind is next, title is after
    let sub_idx = parts.iter().position(|&p| p == "subtitle");
    if let Some(idx) = sub_idx {
        if parts.len() > idx + 2 {
            let kind = parts[idx + 1].to_string();
            let title = parts[idx + 2].to_string();
            return Ok((kind, title));
        }
    }
    // Fallback: last two path segments
    if parts.len() >= 2 {
        let kind = parts[parts.len() - 2].to_string();
        let title = parts[parts.len() - 1].to_string();
        Ok((kind, title))
    } else {
        Err(format!(
            "subscenter: cannot parse kind/title from path: {path}"
        ))
    }
}
