use std::path::PathBuf;

use async_trait::async_trait;
use regex::Regex;
use scraper::{Html, Selector};

use super::SubtitleProvider;
use crate::models::{
    matches_preferred_language, DownloadedSubtitle, SubtitleDownloadRequest,
    SubtitleSearchRequest, SubtitleSearchResult,
};

const KTUVIT_BASE: &str = "https://www.ktuvit.me/";
const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

pub struct KtuvitProvider {
    #[allow(dead_code)]
    staging_root: PathBuf,
}

impl KtuvitProvider {
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
        .map_err(|e| format!("Failed to build ktuvit client: {e}"))
}

fn parse_season_episode(query: &str) -> Option<(u32, u32)> {
    let re = Regex::new(r"[Ss](\d+)[Ee](\d+)").unwrap();
    re.captures(query).map(|cap| {
        let season = cap[1].parse::<u32>().unwrap_or(1);
        let episode = cap[2].parse::<u32>().unwrap_or(1);
        (season, episode)
    })
}

/// Login to ktuvit and return a client with session cookies.
async fn ktuvit_login(email: &str, password: &str) -> Result<reqwest::Client, String> {
    let client = build_client()?;

    let login_url = format!("{KTUVIT_BASE}Services/MembershipService.svc/Login");
    let body = serde_json::json!({
        "request": {
            "Email": email,
            "Password": password
        }
    });

    let resp = client
        .post(&login_url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("ktuvit login request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("ktuvit login failed: {}", resp.status()));
    }

    let json_text = resp
        .text()
        .await
        .map_err(|e| format!("ktuvit login read error: {e}"))?;

    // Response: {"d": "{\"IsSuccess\":true,...}"}
    let outer: serde_json::Value = serde_json::from_str(&json_text)
        .map_err(|e| format!("ktuvit login outer JSON parse error: {e}"))?;

    let inner_str = outer["d"]
        .as_str()
        .ok_or("ktuvit login: missing 'd' field")?;

    let inner: serde_json::Value = serde_json::from_str(inner_str)
        .map_err(|e| format!("ktuvit login inner JSON parse error: {e}"))?;

    if !inner["IsSuccess"].as_bool().unwrap_or(false) {
        return Err("ktuvit login: IsSuccess is false".into());
    }

    Ok(client)
}

#[async_trait]
impl SubtitleProvider for KtuvitProvider {
    fn name(&self) -> &str {
        "ktuvit"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let email = std::env::var("KTUVIT_USER")
            .map_err(|_| "ktuvit: KTUVIT_USER env var not set")?;
        let password = std::env::var("KTUVIT_PASS")
            .map_err(|_| "ktuvit: KTUVIT_PASS env var not set")?;

        let query = request
            .query
            .as_deref()
            .ok_or("ktuvit requires query")?;

        let se_opt = parse_season_episode(query);
        let is_tv = se_opt.is_some();

        let title_re = Regex::new(r"\s*[Ss]\d+[Ee]\d+.*").unwrap();
        let show_title = title_re.replace(query, "").trim().to_string();

        let client = ktuvit_login(&email, &password).await?;

        // Search for films
        let search_url = format!(
            "{KTUVIT_BASE}Services/ContentProvider.svc/SearchPage_search"
        );
        let search_type = if is_tv { "1" } else { "0" };
        let search_body = serde_json::json!({
            "request": {
                "FilmName": show_title,
                "Actors": [],
                "Studios": [],
                "Directors": [],
                "Genres": [],
                "Countries": [],
                "Languages": [],
                "Year": "",
                "Rating": [],
                "Page": 1,
                "SearchType": search_type,
                "WithSubsOnly": false
            }
        });

        let search_resp = client
            .post(&search_url)
            .json(&search_body)
            .send()
            .await
            .map_err(|e| format!("ktuvit search request failed: {e}"))?;

        if !search_resp.status().is_success() {
            return Err(format!("ktuvit search failed: {}", search_resp.status()));
        }

        let search_text = search_resp
            .text()
            .await
            .map_err(|e| format!("ktuvit search read error: {e}"))?;

        let outer: serde_json::Value = serde_json::from_str(&search_text)
            .map_err(|e| format!("ktuvit search outer JSON parse error: {e}"))?;
        let inner_str = outer["d"]
            .as_str()
            .ok_or("ktuvit search: missing 'd' field")?;
        let inner: serde_json::Value = serde_json::from_str(inner_str)
            .map_err(|e| format!("ktuvit search inner JSON parse error: {e}"))?;

        let films = inner["Films"]
            .as_array()
            .ok_or("ktuvit search: no Films array")?;

        // Match by IMDB ID if provided, otherwise take first result
        let ktuvit_id = if let Some(imdb_id) = &request.imdb_id {
            films
                .iter()
                .find_map(|film| {
                    let link = film["IMDB_Link"].as_str().unwrap_or("");
                    if link.contains(imdb_id.trim_start_matches("tt")) {
                        film["ID"].as_str().map(|s| s.to_string())
                    } else {
                        None
                    }
                })
                .or_else(|| {
                    films
                        .first()
                        .and_then(|f| f["ID"].as_str().map(|s| s.to_string()))
                })
        } else {
            films
                .first()
                .and_then(|f| f["ID"].as_str().map(|s| s.to_string()))
        }
        .ok_or("ktuvit: no matching film found")?;

        if !matches_preferred_language("he", request.languages.as_deref()) {
            return Ok(Vec::new());
        }

        // Get subtitle list
        let subs_html = if is_tv {
            let (season, episode) = se_opt.unwrap();
            let subs_url = format!(
                "{KTUVIT_BASE}Services/GetModuleAjax.ashx?moduleName=SubtitlesList&SeriesID={ktuvit_id}&Season={season}&Episode={episode}"
            );
            let resp = client
                .get(&subs_url)
                .send()
                .await
                .map_err(|e| format!("ktuvit subs list request failed: {e}"))?;
            resp.text()
                .await
                .map_err(|e| format!("ktuvit subs list read error: {e}"))?
        } else {
            let movie_url = format!("{KTUVIT_BASE}MovieInfo.aspx?ID={ktuvit_id}");
            let resp = client
                .get(&movie_url)
                .send()
                .await
                .map_err(|e| format!("ktuvit movie info request failed: {e}"))?;
            resp.text()
                .await
                .map_err(|e| format!("ktuvit movie info read error: {e}"))?
        };

        let doc = Html::parse_document(&subs_html);

        let mut results = Vec::new();

        if is_tv {
            // TV: parse tr rows, column 0=release, column 5 has input[data-sub-id]
            let tr_sel = Selector::parse("tbody > tr")
                .map_err(|e| format!("ktuvit selector error: {e}"))?;
            let td_sel =
                Selector::parse("td").map_err(|e| format!("ktuvit selector error: {e}"))?;
            let input_sel = Selector::parse("input[data-sub-id]")
                .map_err(|e| format!("ktuvit selector error: {e}"))?;

            for row in doc.select(&tr_sel) {
                let tds: Vec<_> = row.select(&td_sel).collect();
                if tds.len() < 6 {
                    continue;
                }
                let release = tds[0].text().collect::<String>().trim().to_string();
                let subtitle_id = tds[5]
                    .select(&input_sel)
                    .next()
                    .and_then(|i| i.value().attr("data-sub-id"))
                    .unwrap_or("")
                    .to_string();

                if subtitle_id.is_empty() {
                    continue;
                }

                let download_path = format!("{ktuvit_id}:{subtitle_id}");
                results.push(SubtitleSearchResult {
                    id: format!("ktuvit-{subtitle_id}"),
                    name: release,
                    language: "he".into(),
                    language_name: "Hebrew".into(),
                    format: "srt".into(),
                    provider: "ktuvit".into(),
                    detail_path: None,
                    download_path: Some(download_path),
                    download_count: None,
                    rating: None,
                    movie_name: None,
                    release_group: None,
                });
            }
        } else {
            // Movie: table#subtitlesList tbody > tr, column 5 has a[data-subtitle-id]
            let tr_sel = Selector::parse("table#subtitlesList tbody > tr")
                .map_err(|e| format!("ktuvit selector error: {e}"))?;
            let td_sel =
                Selector::parse("td").map_err(|e| format!("ktuvit selector error: {e}"))?;
            let a_sel = Selector::parse("a[data-subtitle-id]")
                .map_err(|e| format!("ktuvit selector error: {e}"))?;

            for row in doc.select(&tr_sel) {
                let tds: Vec<_> = row.select(&td_sel).collect();
                if tds.len() < 6 {
                    continue;
                }
                let release = tds[0].text().collect::<String>().trim().to_string();
                let subtitle_id = tds[5]
                    .select(&a_sel)
                    .next()
                    .and_then(|a| a.value().attr("data-subtitle-id"))
                    .unwrap_or("")
                    .to_string();

                if subtitle_id.is_empty() {
                    continue;
                }

                let download_path = format!("{ktuvit_id}:{subtitle_id}");
                results.push(SubtitleSearchResult {
                    id: format!("ktuvit-{subtitle_id}"),
                    name: release,
                    language: "he".into(),
                    language_name: "Hebrew".into(),
                    format: "srt".into(),
                    provider: "ktuvit".into(),
                    detail_path: None,
                    download_path: Some(download_path),
                    download_count: None,
                    rating: None,
                    movie_name: None,
                    release_group: None,
                });
            }
        }

        Ok(results)
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        let email = std::env::var("KTUVIT_USER")
            .map_err(|_| "ktuvit: KTUVIT_USER env var not set")?;
        let password = std::env::var("KTUVIT_PASS")
            .map_err(|_| "ktuvit: KTUVIT_PASS env var not set")?;

        let download_path = request
            .download_path
            .as_deref()
            .ok_or("ktuvit: download_path required")?;

        let parts: Vec<&str> = download_path.splitn(2, ':').collect();
        if parts.len() < 2 {
            return Err(format!(
                "ktuvit: invalid download_path (expected ktuvit_id:subtitle_id): {download_path}"
            ));
        }
        let ktuvit_id = parts[0];
        let subtitle_id = parts[1];

        let client = ktuvit_login(&email, &password).await?;

        // Request download identifier
        let req_url = format!(
            "{KTUVIT_BASE}Services/ContentProvider.svc/RequestSubtitleDownload"
        );
        let body = serde_json::json!({
            "request": {
                "FilmID": ktuvit_id,
                "SubtitleID": subtitle_id,
                "FontSize": 0,
                "FontColor": "",
                "PredefinedLayout": -1
            }
        });

        let req_resp = client
            .post(&req_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("ktuvit download request failed: {e}"))?;

        if !req_resp.status().is_success() {
            return Err(format!("ktuvit download request failed: {}", req_resp.status()));
        }

        let req_text = req_resp
            .text()
            .await
            .map_err(|e| format!("ktuvit download read error: {e}"))?;

        let outer: serde_json::Value = serde_json::from_str(&req_text)
            .map_err(|e| format!("ktuvit download outer JSON parse error: {e}"))?;
        let inner_str = outer["d"]
            .as_str()
            .ok_or("ktuvit download: missing 'd' field")?;
        let inner: serde_json::Value = serde_json::from_str(inner_str)
            .map_err(|e| format!("ktuvit download inner JSON parse error: {e}"))?;

        let identifier = inner["DownloadIdentifier"]
            .as_str()
            .ok_or("ktuvit download: missing DownloadIdentifier")?
            .to_string();

        let file_url = format!(
            "{KTUVIT_BASE}Services/DownloadFile.ashx?DownloadIdentifier={identifier}"
        );

        let file_resp = client
            .get(&file_url)
            .send()
            .await
            .map_err(|e| format!("ktuvit file download failed: {e}"))?;

        if !file_resp.status().is_success() {
            return Err(format!("ktuvit file download failed: {}", file_resp.status()));
        }

        let content = file_resp
            .bytes()
            .await
            .map_err(|e| format!("ktuvit read file content error: {e}"))?;

        let name = request
            .name
            .clone()
            .unwrap_or_else(|| "subtitle.srt".to_string());

        Ok(DownloadedSubtitle {
            name,
            format: "srt".into(),
            content: content.to_vec(),
        })
    }
}
