/// Titulky provider — Czech and Slovak subtitles
/// Site: https://premium.titulky.com
/// Requires TITULKY_USER / TITULKY_PASS env vars
/// Requires IMDB ID for searching
use async_trait::async_trait;
use regex::Regex;
use scraper::{Html, Selector};
use tokio::sync::Mutex;

use super::SubtitleProvider;
use crate::archive::extract_archive;
use crate::models::{
    normalize_format, DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest,
    SubtitleSearchResult,
};

const SERVER_URL: &str = "https://premium.titulky.com";
const DOWNLOAD_BASE: &str = "https://premium.titulky.com/download.php?id=";
const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) \
                  Chrome/122.0.0.0 Safari/537.36";

struct LoginState {
    logged_in: bool,
}

pub struct TitulkyProvider {
    username: String,
    password: String,
    client: reqwest::Client,
    login_state: Mutex<LoginState>,
    staging_root: std::path::PathBuf,
}

impl TitulkyProvider {
    pub fn new(
        username: impl Into<String>,
        password: impl Into<String>,
        staging_root: impl Into<std::path::PathBuf>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(UA)
            .cookie_store(true)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("Failed to build Titulky HTTP client");
        Self {
            username: username.into(),
            password: password.into(),
            client,
            login_state: Mutex::new(LoginState { logged_in: false }),
            staging_root: staging_root.into(),
        }
    }

    async fn ensure_logged_in(&self) -> Result<(), String> {
        let is_logged_in = {
            let guard = self.login_state.lock().await;
            guard.logged_in
        };
        if is_logged_in {
            return Ok(());
        }

        let params = [
            ("LoginName", self.username.as_str()),
            ("LoginPassword", self.password.as_str()),
        ];

        let resp = self
            .client
            .post(SERVER_URL)
            .header("Referer", SERVER_URL)
            .form(&params)
            .send()
            .await
            .map_err(|e| format!("Titulky: login request failed: {e}"))?;

        // Successful login returns a 302 redirect with msg_type=i in the Location header
        if resp.status().as_u16() == 302 {
            let location = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");

            if location.contains("msg_type=e") {
                return Err("Titulky: login failed — check TITULKY_USER / TITULKY_PASS".into());
            }

            if location.contains("omezen") {
                return Err("Titulky: V.I.P. account required".into());
            }

            let mut guard = self.login_state.lock().await;
            guard.logged_in = true;
            return Ok(());
        }

        if resp.status().is_success() {
            // Some configurations redirect internally; optimistically mark as logged in
            let mut guard = self.login_state.lock().await;
            guard.logged_in = true;
            return Ok(());
        }

        Err(format!(
            "Titulky: login failed with HTTP {}",
            resp.status().as_u16()
        ))
    }

    /// Build a URL for the Titulky search page.
    /// Uses action=serial&step={season}&id={imdb_id_without_tt}
    fn build_search_url(&self, imdb_id: &str, season: u32) -> String {
        let raw_id = imdb_id.trim_start_matches("tt");
        format!("{SERVER_URL}/?action=serial&step={season}&id={raw_id}")
    }

    /// Follow redirects manually (Titulky heavily uses meta-refresh / Location headers).
    async fn fetch_following_redirects(&self, url: &str) -> Result<String, String> {
        let mut current_url = url.to_string();
        let mut depth = 0usize;

        loop {
            if depth > 10 {
                return Err("Titulky: too many redirects".into());
            }

            let resp = self
                .client
                .get(&current_url)
                .header("Referer", SERVER_URL)
                .send()
                .await
                .map_err(|e| format!("Titulky: fetch failed: {e}"))?;

            if resp.status().as_u16() == 302 || resp.status().as_u16() == 301 {
                let location = resp
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();

                if location.is_empty() {
                    return Err(format!(
                        "Titulky: redirect with empty Location from {current_url}"
                    ));
                }

                // Check for error redirects
                if location.contains("msg_type=e") {
                    // Session expired — try re-login once
                    if location.contains("přihlašte") || location.contains("prihlaste") {
                        let mut guard = self.login_state.lock().await;
                        guard.logged_in = false;
                        drop(guard);
                        self.ensure_logged_in().await?;
                        depth += 1;
                        continue;
                    }
                    return Err(format!("Titulky: server error redirect: {location}"));
                }

                current_url = if location.starts_with("http") {
                    location
                } else {
                    format!("{SERVER_URL}{location}")
                };
                depth += 1;
                continue;
            }

            if !resp.status().is_success() {
                return Err(format!(
                    "Titulky: fetch failed with HTTP {} for {current_url}",
                    resp.status().as_u16()
                ));
            }

            return resp
                .text()
                .await
                .map_err(|e| format!("Titulky: failed to read response: {e}"));
        }
    }
}

#[async_trait]
impl SubtitleProvider for TitulkyProvider {
    fn name(&self) -> &str {
        "titulky"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let imdb_id = request
            .imdb_id
            .as_deref()
            .ok_or("Titulky: search requires an IMDB ID")?;

        if imdb_id.is_empty() {
            return Err("Titulky: invalid IMDB ID".into());
        }

        self.ensure_logged_in().await?;

        // Filter desired languages
        let want_cs = match &request.languages {
            None => true,
            Some(l) if l.is_empty() => true,
            Some(l) => l.iter().any(|x| x == "cs"),
        };
        let want_sk = match &request.languages {
            None => true,
            Some(l) if l.is_empty() => true,
            Some(l) => l.iter().any(|x| x == "sk"),
        };

        // Use season=0 to list all (movie-style) or season 1 for episodes
        // Since SearchRequest has no season field, default to season 0 (movie/all)
        let search_url = self.build_search_url(imdb_id, 0);
        let html = self.fetch_following_redirects(&search_url).await?;

        parse_titulky_results(&html, imdb_id, want_cs, want_sk)
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        self.ensure_logged_in().await?;

        let url = request
            .download_path
            .as_deref()
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("{DOWNLOAD_BASE}{}", request.subtitle_id));

        let resp = self
            .client
            .get(&url)
            .header("Referer", SERVER_URL)
            .send()
            .await
            .map_err(|e| format!("Titulky: download request failed: {e}"))?;

        // Follow redirect if needed
        let (resp, _url) = if resp.status().as_u16() == 302 {
            let location = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            let redirect_url = if location.starts_with("http") {
                location.clone()
            } else {
                format!("{SERVER_URL}{location}")
            };
            let r = self
                .client
                .get(&redirect_url)
                .header("Referer", &url)
                .send()
                .await
                .map_err(|e| format!("Titulky: download redirect failed: {e}"))?;
            (r, redirect_url)
        } else {
            (resp, url)
        };

        if !resp.status().is_success() {
            return Err(format!(
                "Titulky: download failed with HTTP {}",
                resp.status().as_u16()
            ));
        }

        let file_name = resp
            .headers()
            .get("content-disposition")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| {
                let re = regex::Regex::new(r#"filename[^;=\n]*=(?:"([^"]+)"|'([^']+)'|([^;\n]*))"#)
                    .ok()?;
                re.captures(v)
                    .and_then(|c| c.get(1).or_else(|| c.get(2)).or_else(|| c.get(3)))
                    .map(|m| m.as_str().trim().to_string())
            })
            .unwrap_or_else(|| format!("titulky_{}.zip", request.subtitle_id));

        let content = resp
            .bytes()
            .await
            .map_err(|e| format!("Titulky: failed to read download content: {e}"))?;

        if let Some(format) = normalize_format(&file_name) {
            return Ok(DownloadedSubtitle {
                name: file_name,
                format,
                content: content.to_vec(),
            });
        }

        extract_archive(&content, &file_name, &request.language, &self.staging_root).await
    }
}

fn parse_titulky_results(
    html: &str,
    imdb_id: &str,
    want_cs: bool,
    want_sk: bool,
) -> Result<Vec<SubtitleSearchResult>, String> {
    let document = Html::parse_document(html);
    let mut results = Vec::new();

    // Container with subtitle rows
    let form_sel =
        Selector::parse("form.cloudForm").map_err(|e| format!("Titulky: selector error: {e}"))?;
    let row_sel =
        Selector::parse("div.row").map_err(|e| format!("Titulky: selector error: {e}"))?;
    let h5_sel = Selector::parse("h5").map_err(|e| format!("Titulky: selector error: {e}"))?;
    let anchor_sel = Selector::parse("a").map_err(|e| format!("Titulky: selector error: {e}"))?;

    let id_re = Regex::new(r"id=(\d+)").map_err(|e| format!("Titulky: regex error: {e}"))?;

    let container = match document.select(&form_sel).next() {
        Some(c) => c,
        None => {
            tracing::debug!("Titulky: no cloudForm container found");
            return Ok(Vec::new());
        }
    };

    let mut last_ep_num: Option<u32> = None;

    for row in container.select(&row_sel) {
        let classes: Vec<&str> = row
            .value()
            .attr("class")
            .unwrap_or("")
            .split_whitespace()
            .collect();

        // Episode number row
        if let Some(h5) = row.select(&h5_sel).next() {
            let num_str = h5.text().collect::<String>();
            let num_str = num_str.trim().trim_end_matches('.');
            if let Ok(n) = num_str.parse::<u32>() {
                last_ep_num = Some(n);
            }
            continue;
        }

        // Subtitle row — must have pbl1 or pbl0 class and contain a link
        let is_sub_row = classes.contains(&"pbl1") || classes.contains(&"pbl0");
        if !is_sub_row {
            continue;
        }

        let anchor = match row.select(&anchor_sel).next() {
            Some(a) => a,
            None => continue,
        };

        // Determine language from flag images in the row
        let row_html = row.html();
        let language = if row_html.contains("flag-CZ") && !row_html.contains("flag-SK") {
            if !want_cs {
                continue;
            }
            "cs"
        } else if row_html.contains("flag-SK") && !row_html.contains("flag-CZ") {
            if !want_sk {
                continue;
            }
            "sk"
        } else {
            // Ambiguous or unknown — skip
            continue;
        };

        let language_name = if language == "cs" { "Czech" } else { "Slovak" };

        let href = anchor.value().attr("href").unwrap_or("");
        let details_link = if href.starts_with("http") {
            href.to_string()
        } else {
            format!("{SERVER_URL}{}", href.trim_start_matches('.'))
        };

        let sub_id = id_re
            .captures(&details_link)
            .map(|c| c[1].to_string())
            .unwrap_or_else(|| format!("titulky_{}", results.len()));

        let download_link = format!("{DOWNLOAD_BASE}{sub_id}");
        let release_info = anchor.text().collect::<String>().trim().to_string();
        let release_info = if release_info == "???" {
            String::new()
        } else {
            release_info
        };

        let approved = classes.contains(&"pbl1");
        let episode = last_ep_num.unwrap_or(0);

        let name = if release_info.is_empty() {
            format!("{imdb_id} E{episode:02}")
        } else {
            release_info.clone()
        };

        results.push(SubtitleSearchResult {
            id: sub_id,
            name,
            language: language.into(),
            language_name: language_name.into(),
            format: "srt".into(),
            provider: "titulky".into(),
            detail_path: Some(details_link),
            download_path: Some(download_link),
            download_count: None,
            rating: if approved { Some(1.0) } else { Some(0.0) },
            movie_name: Some(imdb_id.to_string()),
            release_group: if release_info.is_empty() {
                None
            } else {
                Some(release_info)
            },
        });
    }

    tracing::info!("Titulky: found {} results", results.len());
    Ok(results)
}
